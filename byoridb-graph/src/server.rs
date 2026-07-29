// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! gRPC and HTTP server implementation for ByoriDB

use super::service::GraphService;
use axum::{
    extract::{Json, Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::{delete, get, post},
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

    /// Construct a gRPC server around an existing graph service. The standalone
    /// binary uses this to share one authentication/session authority with HTTP.
    pub fn with_service(addr: SocketAddr, service: Arc<GraphService>) -> Self {
        GraphServer { service, addr }
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
        &self.service
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

    /// Construct an HTTP server around an existing graph service. Sharing this
    /// instance with gRPC keeps users, roles, and sessions protocol-consistent.
    pub fn with_service(addr: SocketAddr, service: Arc<GraphService>) -> Self {
        HttpServer { service, addr }
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
            .route("/api/v1/session", post(create_session))
            .route("/api/v1/session/:id", delete(delete_session))
            .route("/api/v1/query", post(execute_query))
            .route("/api/v1/query/json", post(execute_query_json))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(self.addr).await?;
        info!("HTTP server listening on {}", self.addr);

        axum::serve(listener, app).await?;

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

/// Diagnostics: list queries currently executing on the server. Useful for
/// seeing what a long-running load is doing and whether work continues after a
/// client-side HTTP timeout.
async fn list_active_queries(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let session_id = bearer_session_id(&headers).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "A valid Bearer session is required".to_string(),
                code: "AUTH_REQUIRED".to_string(),
            }),
        )
    })?;

    if !state.service.is_admin_session(session_id).await {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "GOD or ADMIN role is required".to_string(),
                code: "ADMIN_REQUIRED".to_string(),
            }),
        ));
    }

    let queries: Vec<serde_json::Value> = state
        .service
        .list_active_queries()
        .iter()
        .map(active_query_to_json)
        .collect();
    Ok(Json(serde_json::json!({
        "count": queries.len(),
        "queries": queries,
    })))
}

fn bearer_session_id(headers: &HeaderMap) -> Option<i64> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    token.trim().parse().ok()
}

fn active_query_to_json(query: &crate::service::RunningQuery) -> serde_json::Value {
    serde_json::json!({
        "id": query.id,
        "query_type": query.query_type,
        "query": crate::service::redact_sensitive_query(&query.query),
        "space": query.space,
        "started_at_ms": query.started_at_ms,
    })
}

/// Create a new session (authenticate)
async fn create_session(
    State(state): State<AppState>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let username = payload.username;
    match state
        .service
        .authenticate(username.clone(), payload.password)
        .await
    {
        Ok(session_id) => {
            info!(user = %username, "Session created");
            Ok(Json(SessionResponse {
                session_id,
                time_zone: Some("UTC".to_string()),
            }))
        }
        Err(e) => {
            error!("Authentication failed: {}", e);
            Err((
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: format!("Authentication failed: {}", e),
                    code: "AUTH_FAILED".to_string(),
                }),
            ))
        }
    }
}

/// Delete a session (sign out)
async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    // HTTP endpoint: caller is identified by the session ID in the path.
    // A session can only sign out itself via this endpoint.
    state.service.sign_out(id, id).await;
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
            let redacted_query = crate::service::redact_sensitive_query(&payload.query);
            info!(
                latency_ms,
                query = %truncate_query(&redacted_query),
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
            let (status, code, message) =
                if matches!(&e, crate::error::GraphError::SessionNotFound(_)) {
                    error!("Query rejected: invalid or expired session");
                    (
                        StatusCode::UNAUTHORIZED,
                        "SESSION_EXPIRED",
                        "Query execution failed: session is invalid or expired".to_string(),
                    )
                } else {
                    error!(err = %e, "Query execution failed");
                    (
                        StatusCode::BAD_REQUEST,
                        "QUERY_ERROR",
                        format!("Query execution failed: {e}"),
                    )
                };
            Err((
                status,
                Json(ErrorResponse {
                    error: message,
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
            let (status, code, message) =
                if matches!(&e, crate::error::GraphError::SessionNotFound(_)) {
                    (
                        StatusCode::UNAUTHORIZED,
                        "SESSION_EXPIRED",
                        "Query execution failed: session is invalid or expired".to_string(),
                    )
                } else {
                    (
                        StatusCode::BAD_REQUEST,
                        "QUERY_ERROR",
                        format!("Query execution failed: {e}"),
                    )
                };
            let error_response = serde_json::json!({
                "error": message,
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

/// Truncate query for logging — to the first 100 *characters*, not bytes.
///
/// Slicing at a fixed byte offset (`&query[..100]`) panics when byte 100 lands
/// inside a multi-byte UTF-8 sequence (e.g. Korean text in a long INSERT). That
/// panic happened in the request handler *after* the query succeeded, so the
/// connection was dropped before the response was sent — surfacing to clients
/// as a connection reset. Char-based truncation is always on a valid boundary.
fn truncate_query(query: &str) -> &str {
    match query.char_indices().nth(100) {
        Some((idx, _)) => &query[..idx],
        None => query,
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

#[derive(serde::Serialize)]
struct ErrorResponse {
    error: String,
    code: String,
}

#[cfg(test)]
mod tests {
    use super::{
        active_query_to_json, bearer_session_id, execute_query, execute_query_json,
        list_active_queries, truncate_query, AppState, GraphServer, HttpServer, QueryRequest,
        SessionResponse,
    };
    use crate::auth::AuthManager;
    use crate::service::{GraphService, RunningQuery};
    use axum::extract::{Json, State};
    use axum::http::{header::AUTHORIZATION, HeaderMap, HeaderValue, StatusCode};
    use byoridb_kvstore::{KVStore, MemoryKVStore};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn bearer_session_parser_requires_bearer_scheme_and_integer_token() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer 12345"));
        assert_eq!(bearer_session_id(&headers), Some(12345));

        headers.insert(AUTHORIZATION, HeaderValue::from_static("Basic 12345"));
        assert_eq!(bearer_session_id(&headers), None);

        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer not-a-number"),
        );
        assert_eq!(bearer_session_id(&headers), None);
    }

    #[test]
    fn grpc_and_http_can_share_one_graph_service() {
        let service = Arc::new(GraphService::new(Arc::new(MemoryKVStore::new())));
        let grpc = GraphServer::with_service("127.0.0.1:0".parse().unwrap(), service.clone());
        let http = HttpServer::with_service("127.0.0.1:0".parse().unwrap(), service.clone());

        assert!(Arc::ptr_eq(&grpc.service, &service));
        assert!(Arc::ptr_eq(&http.service, &service));
        assert!(Arc::ptr_eq(&grpc.service, &http.service));
    }

    #[test]
    fn active_query_json_excludes_session_credentials_and_redacts_passwords() {
        let query = RunningQuery {
            id: 7,
            session_id: 9_876_543_210,
            query_type: "create",
            query: "CREATE USER alice WITH PASSWORD 'top-secret' ROLE USER".to_string(),
            space: "default".to_string(),
            started_at_ms: 123,
        };

        let json = active_query_to_json(&query);
        let encoded = json.to_string();
        assert!(json.get("session_id").is_none());
        assert!(!encoded.contains("9876543210"));
        assert!(!encoded.contains("top-secret"));
    }

    #[tokio::test]
    async fn diagnostics_requires_an_admin_bearer_session() {
        let auth = AuthManager::with_config("root-password", Duration::from_secs(60));
        let kvstore = Arc::new(MemoryKVStore::new());
        let service = Arc::new(GraphService::with_auth(kvstore.clone(), auth));
        let root_session = service
            .authenticate("root".to_string(), "root-password".to_string())
            .await
            .unwrap();
        service
            .execute(
                root_session,
                r#"CREATE USER diagnostics_guest WITH PASSWORD "guest-password" ROLE GUEST"#
                    .to_string(),
            )
            .await
            .unwrap();
        let stored_user: crate::auth::User = serde_json::from_slice(
            &kvstore
                .get(b"__user_diagnostics_guest")
                .await
                .unwrap()
                .expect("CREATE USER must persist the durable user"),
        )
        .unwrap();
        assert!(byoridb_common::crypto::verify_password(
            "guest-password",
            &stored_user.password_hash
        ));
        let guest_session = service
            .authenticate(
                "diagnostics_guest".to_string(),
                "guest-password".to_string(),
            )
            .await
            .unwrap();
        let state = AppState { service };

        let missing = list_active_queries(State(state.clone()), HeaderMap::new()).await;
        match missing {
            Err((status, _)) => assert_eq!(status, StatusCode::UNAUTHORIZED),
            Ok(_) => panic!("missing bearer token must be rejected"),
        }

        let mut guest_headers = HeaderMap::new();
        guest_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {guest_session}")).unwrap(),
        );
        let guest = list_active_queries(State(state.clone()), guest_headers).await;
        match guest {
            Err((status, _)) => assert_eq!(status, StatusCode::FORBIDDEN),
            Ok(_) => panic!("GUEST bearer token must be rejected"),
        }

        let mut root_headers = HeaderMap::new();
        root_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {root_session}")).unwrap(),
        );
        assert!(list_active_queries(State(state), root_headers)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn invalid_session_errors_do_not_reflect_bearer_ids() {
        let state = AppState {
            service: Arc::new(GraphService::new(Arc::new(MemoryKVStore::new()))),
        };
        let bearer_id = 8_765_432_109_876_543_210_i64;

        let regular = execute_query(
            State(state.clone()),
            Json(QueryRequest {
                session_id: bearer_id,
                query: "SHOW SPACES".to_string(),
            }),
        )
        .await;
        let regular_body = match regular {
            Err((status, Json(body))) => {
                assert_eq!(status, StatusCode::UNAUTHORIZED);
                serde_json::to_string(&body).unwrap()
            }
            Ok(_) => panic!("unknown bearer token must be rejected"),
        };
        assert!(!regular_body.contains(&bearer_id.to_string()));

        let raw_json = execute_query_json(
            State(state),
            Json(QueryRequest {
                session_id: bearer_id,
                query: "SHOW SPACES".to_string(),
            }),
        )
        .await;
        let raw_json_body = match raw_json {
            Err((status, body)) => {
                assert_eq!(status, StatusCode::UNAUTHORIZED);
                body
            }
            Ok(_) => panic!("unknown bearer token must be rejected"),
        };
        assert!(!raw_json_body.contains(&bearer_id.to_string()));
    }

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

    #[test]
    fn truncate_query_short_is_unchanged() {
        assert_eq!(
            truncate_query("INSERT VERTEX t() VALUES 1:()"),
            "INSERT VERTEX t() VALUES 1:()"
        );
    }

    #[test]
    fn truncate_query_does_not_panic_on_multibyte_boundary() {
        // Long Korean query: each '한' is 3 bytes, so byte offset 100 lands inside
        // a character — `&query[..100]` used to panic here (dogfooding: nexprice
        // product INSERT with long Korean prod_name → server reset).
        let q = format!(
            "INSERT VERTEX product() VALUES 1:(\"{}\")",
            "한".repeat(200)
        );
        let t = truncate_query(&q); // must not panic
        assert!(q.starts_with(t), "truncation is a prefix");
        assert!(t.chars().count() <= 100, "at most 100 chars");
        assert!(q.is_char_boundary(t.len()), "ends on a char boundary");
    }

    #[test]
    fn truncate_query_keeps_first_100_chars() {
        let q = "a".repeat(250);
        assert_eq!(truncate_query(&q).len(), 100);
    }
}
