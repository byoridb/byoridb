// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! gRPC and HTTP server implementation for ByoriDB

use super::service::GraphService;
use axum::{
    extract::{ConnectInfo, Json, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Router,
};
use byoridb_kvstore::KVStore;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;
use tracing::{error, info};

/// Graph gRPC server
pub struct GraphServer {
    service: Arc<GraphService>,
    addr: SocketAddr,
}

impl GraphServer {
    pub fn new(addr: SocketAddr, kvstore: Arc<dyn KVStore>) -> Self {
        GraphServer {
            service: Arc::new(GraphService::new(kvstore)),
            addr,
        }
    }

    /// Like [`GraphServer::new`] but sharing the binary-wide readiness/drain
    /// state, so SIGTERM rejects new gRPC queries and the drain counter sees
    /// gRPC in-flight queries too.
    pub fn new_with_shutdown(
        addr: SocketAddr,
        kvstore: Arc<dyn KVStore>,
        shutdown: Arc<crate::shutdown::ShutdownState>,
    ) -> Self {
        GraphServer {
            service: Arc::new(GraphService::new(kvstore).with_shutdown_state(shutdown)),
            addr,
        }
    }

    /// Build a protocol server around an existing service. The standalone
    /// launcher uses this for HTTP and gRPC so credentials, sessions, user
    /// updates, active-query state, and shutdown state are all shared.
    pub fn with_service(addr: SocketAddr, service: Arc<GraphService>) -> Self {
        Self { service, addr }
    }

    /// Start the gRPC server with compression support
    pub async fn start(self) -> Result<(), Box<dyn std::error::Error>> {
        use std::time::Duration;

        info!(
            "Starting gRPC server on {} with compression enabled (gzip, zstd)",
            self.addr
        );

        // Spawn background session cleanup before moving the service into the
        // gRPC wrapper. The task uses Weak refs and exits when the service drops.
        let _cleanup_handle = self.service.spawn_session_cleanup(Duration::from_secs(
            crate::service::DEFAULT_SESSION_CLEANUP_INTERVAL_SECS,
        ));

        let grpc_service = crate::grpc::GrpcService::new(self.service);

        // Enable compression for both receiving and sending
        // This can reduce network I/O by 30-50% for large responses
        let service_server =
            crate::grpc::graph_proto::graph_service_server::GraphServiceServer::new(grpc_service)
                .accept_compressed(CompressionEncoding::Gzip)
                .accept_compressed(CompressionEncoding::Zstd)
                .send_compressed(CompressionEncoding::Gzip)
                .send_compressed(CompressionEncoding::Zstd)
                // Limit incoming message size to 64 MiB to prevent zip-bomb / OOM DoS
                .max_decoding_message_size(64 * 1024 * 1024);

        Server::builder()
            .add_service(service_server)
            .serve(self.addr)
            .await?;

        Ok(())
    }

    /// Get the graph service
    pub fn service(&self) -> &GraphService {
        self.service.as_ref()
    }
}

/// Shared application state for HTTP handlers
#[derive(Clone)]
pub struct AppState {
    service: Arc<GraphService>,
}

/// HTTP server for REST API
pub struct HttpServer {
    service: Arc<GraphService>,
    addr: SocketAddr,
}

impl HttpServer {
    pub fn new(addr: SocketAddr, kvstore: Arc<dyn KVStore>) -> Self {
        HttpServer {
            service: Arc::new(GraphService::new(kvstore)),
            addr,
        }
    }

    /// Like [`HttpServer::new`] but sharing the binary-wide readiness/drain
    /// state (see [`GraphServer::new_with_shutdown`]). `/ready` reports it.
    pub fn new_with_shutdown(
        addr: SocketAddr,
        kvstore: Arc<dyn KVStore>,
        shutdown: Arc<crate::shutdown::ShutdownState>,
    ) -> Self {
        HttpServer {
            service: Arc::new(GraphService::new(kvstore).with_shutdown_state(shutdown)),
            addr,
        }
    }

    /// Build an HTTP server around the same service used by other protocols.
    pub fn with_service(addr: SocketAddr, service: Arc<GraphService>) -> Self {
        Self { service, addr }
    }

    /// Start the HTTP server
    pub async fn start(self) -> Result<(), Box<dyn std::error::Error>> {
        use std::time::Duration;

        info!("Starting HTTP server on {}", self.addr);

        // Spawn background session cleanup. The task uses Weak refs and exits
        // when all strong references to the service are dropped.
        let _cleanup_handle = self.service.spawn_session_cleanup(Duration::from_secs(
            crate::service::DEFAULT_SESSION_CLEANUP_INTERVAL_SECS,
        ));

        let state = AppState {
            service: self.service,
        };

        // Build REST API with state
        let app = Router::new()
            .route("/health", get(health_check))
            .route("/ready", get(readiness_check))
            .route("/metrics", get(metrics_endpoint))
            .route("/api/v1/metrics", get(metrics_json))
            .route("/api/v1/diagnostics/queries", get(list_active_queries))
            .route(
                "/api/v1/session",
                post(create_session).delete(delete_session),
            )
            .route("/api/v1/query", post(execute_query))
            .route("/api/v1/query/json", post(execute_query_json))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(self.addr).await?;
        info!("HTTP server listening on {}", self.addr);

        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;

        Ok(())
    }
}

/// Health check endpoint
async fn health_check() -> &'static str {
    "OK"
}

/// Readiness endpoint: 200 while accepting queries, 503 once a graceful
/// shutdown has begun. Kubernetes uses this to pull the pod out of Service
/// endpoints *before* the process exits, so clients stop seeing
/// connection-refused during rollouts and node drains.
async fn readiness_check(State(state): State<AppState>) -> (StatusCode, &'static str) {
    if state.service.shutdown_state().is_accepting() {
        (StatusCode::OK, "READY")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "SHUTTING DOWN")
    }
}

/// Prometheus metrics endpoint
async fn metrics_endpoint() -> (
    StatusCode,
    [(axum::http::header::HeaderName, &'static str); 1],
    String,
) {
    let metrics = crate::metrics::render_metrics();
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        metrics,
    )
}

/// JSON metrics endpoint for API consumption
async fn metrics_json() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "metrics": {
            "prometheus_url": "/metrics"
        }
    }))
}

/// Bearer session header used for HTTP operations that cannot safely put a
/// session ID in the URL. Header names are case-insensitive.
const SESSION_ID_HEADER: &str = "x-byoridb-session-id";

type HttpApiError = (StatusCode, Json<ErrorResponse>);

fn sign_out_http_error(error: crate::error::GraphError) -> HttpApiError {
    match error {
        crate::error::GraphError::SessionNotFound(_) => (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "Invalid or expired session".to_string(),
                code: "SESSION_EXPIRED".to_string(),
            }),
        ),
        crate::error::GraphError::InvalidOperation(_) => (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "Sign out is not allowed".to_string(),
                code: "FORBIDDEN".to_string(),
            }),
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Sign out failed".to_string(),
                code: "INTERNAL_ERROR".to_string(),
            }),
        ),
    }
}

fn session_id_from_headers(headers: &HeaderMap) -> Result<i64, HttpApiError> {
    let session_id = headers
        .get(SESSION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|id| *id > 0)
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "A valid session header is required".to_string(),
                    code: "AUTH_REQUIRED".to_string(),
                }),
            )
        })?;
    Ok(session_id)
}

/// Diagnostics: list safe metadata for queries currently executing. This is an
/// authenticated administrative endpoint; it never returns bearer session IDs
/// or raw query text.
async fn list_active_queries(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, HttpApiError> {
    let session_id = session_id_from_headers(&headers)?;
    match state.service.is_admin_session(session_id).await {
        Ok(true) => {}
        Ok(false) => {
            return Err((
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: "GOD or ADMIN role required".to_string(),
                    code: "FORBIDDEN".to_string(),
                }),
            ));
        }
        Err(_) => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "Invalid or expired session".to_string(),
                    code: "AUTH_REQUIRED".to_string(),
                }),
            ));
        }
    }
    let queries = state.service.list_active_queries();
    Ok(Json(serde_json::json!({
        "count": queries.len(),
        "queries": queries,
    })))
}

/// Create a new session (authenticate)
async fn create_session(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state
        .service
        .authenticate_from(payload.username, payload.password, Some(peer.ip()))
        .await
    {
        Ok(session_id) => {
            info!("Session created");
            Ok(Json(SessionResponse {
                session_id,
                time_zone: Some("UTC".to_string()),
            }))
        }
        Err(e) => {
            error!(
                error_type = GraphService::error_kind(&e),
                "Authentication failed"
            );
            Err((
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    // Never disclose whether the account exists, is disabled,
                    // locked, or merely received a wrong password.
                    error: "Invalid credentials".to_string(),
                    code: "AUTH_FAILED".to_string(),
                }),
            ))
        }
    }
}

/// Delete a session (sign out)
async fn delete_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, HttpApiError> {
    // Session IDs are bearer credentials, so logout authenticates through a
    // header and never places the token in a URL or response body.
    let session_id = session_id_from_headers(&headers)?;
    state
        .service
        .sign_out(session_id, session_id)
        .await
        .map_err(sign_out_http_error)?;
    info!("Session deleted");
    Ok(Json(serde_json::json!({"deleted": true})))
}

/// Maximum query string length accepted by the HTTP API (1 MiB).
const MAX_QUERY_LEN: usize = 1024 * 1024;

/// Execute a query
async fn execute_query(
    State(state): State<AppState>,
    Json(payload): Json<QueryRequest>,
) -> Result<Json<QueryResponse>, (StatusCode, Json<ErrorResponse>)> {
    if payload.query.len() > MAX_QUERY_LEN {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(ErrorResponse {
                error: format!(
                    "Query string exceeds maximum length of {} bytes",
                    MAX_QUERY_LEN
                ),
                code: "QUERY_TOO_LARGE".to_string(),
            }),
        ));
    }

    let start = Instant::now();

    match state
        .service
        .execute(payload.session_id, payload.query.clone())
        .await
    {
        Ok(dataset) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            info!(
                query_type = crate::logging::safe_statement_type(&payload.query),
                query_length_bytes = payload.query.len(),
                latency_ms = latency_ms,
                row_count = dataset.row_count(),
                "Query executed"
            );

            // Convert DataSet to JSON results
            let results = dataset_to_json(&dataset);

            Ok(Json(QueryResponse {
                results,
                latency_ms,
                row_count: dataset.row_count(),
                column_names: dataset.column_names.clone(),
            }))
        }
        Err(e) => {
            error!(
                error_type = GraphService::error_kind(&e),
                query_length_bytes = payload.query.len(),
                "Query execution failed"
            );
            let (status, code) = if matches!(e, crate::error::GraphError::SessionNotFound(_)) {
                (StatusCode::UNAUTHORIZED, "SESSION_EXPIRED")
            } else {
                (StatusCode::BAD_REQUEST, "QUERY_ERROR")
            };
            Err((
                status,
                Json(ErrorResponse {
                    error: format!("Query execution failed: {}", e),
                    code: code.to_string(),
                }),
            ))
        }
    }
}

/// Execute a query and return raw JSON string
async fn execute_query_json(
    State(state): State<AppState>,
    Json(payload): Json<QueryRequest>,
) -> Result<String, (StatusCode, String)> {
    if payload.query.len() > MAX_QUERY_LEN {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Query string exceeds maximum length of {} bytes",
                MAX_QUERY_LEN
            ),
        ));
    }

    let start = Instant::now();

    match state
        .service
        .execute(payload.session_id, payload.query.clone())
        .await
    {
        Ok(dataset) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            let results = dataset_to_json(&dataset);

            let response = serde_json::json!({
                "results": results,
                "latency_ms": latency_ms,
                "row_count": dataset.row_count(),
                "column_names": dataset.column_names,
            });

            Ok(response.to_string())
        }
        Err(e) => {
            let (status, code) = if matches!(e, crate::error::GraphError::SessionNotFound(_)) {
                (StatusCode::UNAUTHORIZED, "SESSION_EXPIRED")
            } else {
                (StatusCode::BAD_REQUEST, "QUERY_ERROR")
            };
            let error_response = serde_json::json!({
                "error": format!("Query execution failed: {}", e),
                "code": code
            });
            Err((status, error_response.to_string()))
        }
    }
}

/// Convert DataSet to JSON array
fn dataset_to_json(dataset: &byoridb_common::DataSet) -> Vec<serde_json::Value> {
    let columns = &dataset.column_names;
    let mut results = Vec::new();

    for row in &dataset.rows {
        let mut row_obj = serde_json::Map::new();
        for (i, value) in row.iter().enumerate() {
            if i < columns.len() {
                row_obj.insert(columns[i].clone(), value_to_json(value));
            }
        }
        results.push(serde_json::Value::Object(row_obj));
    }

    results
}

/// Convert byoridb_common::Value to serde_json::Value
fn value_to_json(value: &byoridb_common::Value) -> serde_json::Value {
    match value {
        byoridb_common::Value::Null(_) => serde_json::Value::Null,
        byoridb_common::Value::Bool(b) => serde_json::Value::Bool(*b),
        byoridb_common::Value::Int(i) => serde_json::Value::Number((*i).into()),
        byoridb_common::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        byoridb_common::Value::String(s) => serde_json::Value::String(s.clone()),
        byoridb_common::Value::List(list) => {
            serde_json::Value::Array(list.values.iter().map(value_to_json).collect())
        }
        byoridb_common::Value::Map(map) => {
            let obj: serde_json::Map<String, serde_json::Value> = map
                .data
                .iter()
                .map(|(k, v)| (k.clone(), value_to_json(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
        byoridb_common::Value::Vertex(v) => {
            serde_json::json!({
                "vid": value_to_json(&v.vid),
                "tags": v.tags.iter().map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "props": t.props.iter()
                            .map(|(k, v)| (k.clone(), value_to_json(v)))
                            .collect::<serde_json::Map<String, serde_json::Value>>()
                    })
                }).collect::<Vec<_>>()
            })
        }
        byoridb_common::Value::Edge(e) => {
            serde_json::json!({
                "src": value_to_json(&e.src),
                "dst": value_to_json(&e.dst),
                "type": e.edge_type,
                "name": e.name,
                "ranking": e.ranking,
                "props": e.props.iter()
                    .map(|(k, v)| (k.clone(), value_to_json(v)))
                    .collect::<serde_json::Map<String, serde_json::Value>>()
            })
        }
        byoridb_common::Value::Path(p) => {
            // Path contains vertices only
            serde_json::json!({
                "vertices": p.vertices.iter().map(|v| {
                    serde_json::json!({
                        "vid": value_to_json(&v.vid),
                        "tags": v.tags.iter().map(|t| {
                            serde_json::json!({
                                "name": t.name,
                                "props": t.props.iter()
                                    .map(|(k, val)| (k.clone(), value_to_json(val)))
                                    .collect::<serde_json::Map<String, serde_json::Value>>()
                            })
                        }).collect::<Vec<_>>()
                    })
                }).collect::<Vec<_>>()
            })
        }
        _ => serde_json::Value::String(format!("{:?}", value)),
    }
}

/// Request types for HTTP API
#[derive(serde::Deserialize)]
struct CreateSessionRequest {
    username: String,
    password: String,
}

/// Serialize an i64 session id as a decimal **string**, and accept it as either
/// a string or a JSON number on input. Session ids are random 63-bit integers,
/// ~99.4% of which are not exactly representable by a JavaScript `Number`
/// (IEEE-754 double). Emitting a number let browser/Electron/Tauri clients round
/// the id on `JSON.parse`, so their very next request failed with SESSION_EXPIRED.
mod id_str {
    use serde::{de, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(id: &i64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&id.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = i64;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a session id as a decimal string or integer")
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<i64, E> {
                Ok(v)
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<i64, E> {
                Ok(v as i64)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<i64, E> {
                v.trim().parse().map_err(de::Error::custom)
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(serde::Serialize)]
struct SessionResponse {
    #[serde(serialize_with = "id_str::serialize")]
    session_id: i64,
    time_zone: Option<String>,
}

#[derive(serde::Deserialize)]
struct QueryRequest {
    #[serde(deserialize_with = "id_str::deserialize")]
    session_id: i64,
    query: String,
}

#[derive(serde::Serialize)]
struct QueryResponse {
    results: Vec<serde_json::Value>,
    latency_ms: u64,
    row_count: usize,
    column_names: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
struct ErrorResponse {
    error: String,
    code: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthManager;
    use byoridb_kvstore::MemoryKVStore;
    use std::time::Duration;

    #[test]
    fn session_id_is_json_string_and_accepts_string_or_number() {
        // A 63-bit id that a JavaScript Number cannot represent exactly.
        let big = 9_223_372_036_854_775_806_i64;

        // Response must emit the id as a JSON *string* (JS-safe).
        let resp = SessionResponse {
            session_id: big,
            time_zone: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(
            json.contains(&format!("\"session_id\":\"{big}\"")),
            "session_id must serialize as a string, got: {json}"
        );

        // Request accepts the id as a string (what a JS client round-trips)…
        let from_str: QueryRequest =
            serde_json::from_str(&format!(r#"{{"session_id":"{big}","query":"Q"}}"#)).unwrap();
        assert_eq!(from_str.session_id, big);

        // …and still as a number (back-compat with existing clients).
        let from_num: QueryRequest =
            serde_json::from_str(r#"{"session_id":123,"query":"Q"}"#).unwrap();
        assert_eq!(from_num.session_id, 123);
    }

    fn test_state(auth: AuthManager) -> AppState {
        AppState {
            service: Arc::new(GraphService::with_auth(
                Arc::new(MemoryKVStore::new()),
                auth,
            )),
        }
    }

    fn session_headers(session_id: i64) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(SESSION_ID_HEADER, session_id.to_string().parse().unwrap());
        headers
    }

    async fn failed_http_auth(
        state: AppState,
        username: &str,
        password: &str,
    ) -> (StatusCode, ErrorResponse) {
        match create_session(
            ConnectInfo("127.0.0.1:12345".parse().unwrap()),
            State(state),
            Json(CreateSessionRequest {
                username: username.to_string(),
                password: password.to_string(),
            }),
        )
        .await
        {
            Ok(_) => panic!("invalid credentials unexpectedly authenticated"),
            Err((status, Json(error))) => (status, error),
        }
    }

    #[tokio::test]
    async fn diagnostics_requires_live_admin_session() {
        let auth = AuthManager::with_config("root-password", Duration::from_secs(3600));
        auth.create_user("reader", "reader-password", vec!["USER".to_string()])
            .await
            .unwrap();
        let state = test_state(auth);

        let missing = list_active_queries(State(state.clone()), HeaderMap::new())
            .await
            .unwrap_err();
        assert_eq!(missing.0, StatusCode::UNAUTHORIZED);

        let reader = state
            .service
            .authenticate("reader".to_string(), "reader-password".to_string())
            .await
            .unwrap();
        let forbidden = list_active_queries(State(state.clone()), session_headers(reader))
            .await
            .unwrap_err();
        assert_eq!(forbidden.0, StatusCode::FORBIDDEN);

        let root = state
            .service
            .authenticate("root".to_string(), "root-password".to_string())
            .await
            .unwrap();
        let Json(response) = list_active_queries(State(state), session_headers(root))
            .await
            .unwrap();
        assert_eq!(response["count"], 0);
    }

    #[tokio::test]
    async fn authentication_failures_do_not_enumerate_accounts() {
        let auth = AuthManager::with_config("root-password", Duration::from_secs(3600));
        auth.create_user(
            "disabled-user",
            "disabled-password",
            vec!["USER".to_string()],
        )
        .await
        .unwrap();
        auth.set_user_enabled("disabled-user", false).await.unwrap();
        auth.create_user("locked-user", "locked-password", vec!["USER".to_string()])
            .await
            .unwrap();
        for _ in 0..crate::auth::MAX_FAILED_ATTEMPTS {
            let _ = auth.authenticate("locked-user", "wrong-password").await;
        }
        let state = test_state(auth);

        for (username, password) in [
            ("missing-user", "wrong-password"),
            ("root", "wrong-password"),
            ("disabled-user", "disabled-password"),
            ("locked-user", "locked-password"),
        ] {
            let (status, error) = failed_http_auth(state.clone(), username, password).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(error.error, "Invalid credentials");
            assert_eq!(error.code, "AUTH_FAILED");
        }
    }

    #[tokio::test]
    async fn expired_query_sessions_return_401_session_expired_on_both_http_surfaces() {
        const ROOT_PASSWORD: &str = "root-password";
        let state = test_state(AuthManager::with_config(
            ROOT_PASSWORD,
            Duration::from_millis(20),
        ));

        let json_session = state
            .service
            .authenticate("root".to_string(), ROOT_PASSWORD.to_string())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let (status, Json(error)) = match execute_query(
            State(state.clone()),
            Json(QueryRequest {
                session_id: json_session,
                query: "SHOW SPACES".to_string(),
            }),
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("expired session unexpectedly executed a JSON query"),
        };
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error.code, "SESSION_EXPIRED");

        let raw_session = state
            .service
            .authenticate("root".to_string(), ROOT_PASSWORD.to_string())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let (status, body) = execute_query_json(
            State(state),
            Json(QueryRequest {
                session_id: raw_session,
                query: "SHOW SPACES".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let error: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(error["code"], "SESSION_EXPIRED");
    }

    #[tokio::test]
    async fn logout_uses_header_and_does_not_echo_bearer_token() {
        let state = test_state(AuthManager::with_config(
            "root-password",
            Duration::from_secs(3600),
        ));
        let root = state
            .service
            .authenticate("root".to_string(), "root-password".to_string())
            .await
            .unwrap();
        let Json(response) = delete_session(State(state.clone()), session_headers(root))
            .await
            .unwrap();
        assert_eq!(response, serde_json::json!({"deleted": true}));
        assert!(!response.to_string().contains(&root.to_string()));
        assert!(state.service.validate_session(root).await.is_err());

        let (status, Json(error)) = delete_session(State(state), session_headers(root))
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error.code, "SESSION_EXPIRED");
        assert!(!error.error.contains(&root.to_string()));
    }

    #[tokio::test]
    async fn expired_logout_returns_the_same_session_expired_contract_as_absent_logout() {
        const ROOT_PASSWORD: &str = "root-password";
        let state = test_state(AuthManager::with_config(
            ROOT_PASSWORD,
            Duration::from_millis(20),
        ));
        let session = state
            .service
            .authenticate("root".to_string(), ROOT_PASSWORD.to_string())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;

        let (status, Json(error)) = delete_session(State(state), session_headers(session))
            .await
            .unwrap_err();

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error.code, "SESSION_EXPIRED");
        assert!(!error.error.contains(&session.to_string()));
    }

    #[tokio::test]
    async fn concurrent_logout_race_has_one_winner_and_a_stable_rest_error() {
        let state = test_state(AuthManager::with_config(
            "root-password",
            Duration::from_secs(3600),
        ));
        let session = state
            .service
            .authenticate("root".to_string(), "root-password".to_string())
            .await
            .unwrap();
        let service = state.service.clone();

        let (direct_result, http_result) = tokio::join!(
            service.sign_out(session, session),
            delete_session(State(state), session_headers(session)),
        );

        assert_eq!(
            usize::from(direct_result.is_ok()) + usize::from(http_result.is_ok()),
            1,
            "the auth-store removal must select exactly one sign-out winner"
        );
        if let Err(error) = direct_result {
            assert!(matches!(
                error,
                crate::error::GraphError::SessionNotFound(_)
            ));
        }
        if let Err((status, Json(error))) = http_result {
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(error.code, "SESSION_EXPIRED");
            assert!(!error.error.contains(&session.to_string()));
        }
    }

    #[test]
    fn sign_out_race_error_mapping_does_not_expose_the_bearer() {
        let session = 99_999_999;

        let (status, Json(error)) =
            sign_out_http_error(crate::error::GraphError::SessionNotFound(session));

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error.code, "SESSION_EXPIRED");
        assert!(!error.error.contains(&session.to_string()));
    }

    #[test]
    fn shared_protocol_servers_reference_the_same_service() {
        let service = Arc::new(GraphService::with_auth(
            Arc::new(MemoryKVStore::new()),
            AuthManager::with_config("root-password", Duration::from_secs(3600)),
        ));
        let grpc = GraphServer::with_service("127.0.0.1:9669".parse().unwrap(), service.clone());
        let http = HttpServer::with_service("127.0.0.1:19669".parse().unwrap(), service.clone());
        assert!(Arc::ptr_eq(&grpc.service, &http.service));
    }
}
