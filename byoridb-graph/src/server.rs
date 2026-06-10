// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! gRPC and HTTP server implementation for ByoriDB

use super::service::GraphService;
use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
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
    service: GraphService,
    addr: SocketAddr,
}

impl GraphServer {
    pub fn new(addr: SocketAddr, kvstore: Arc<dyn KVStore>) -> Self {
        GraphServer {
            service: GraphService::new(kvstore),
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
            service: GraphService::new(kvstore).with_shutdown_state(shutdown),
            addr,
        }
    }

    /// Start the gRPC server with compression support
    pub async fn start(self) -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::Arc;
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

        let grpc_service = crate::grpc::GrpcService::new(Arc::new(self.service));

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
async fn list_active_queries(State(state): State<AppState>) -> Json<serde_json::Value> {
    let queries = state.service.list_active_queries();
    Json(serde_json::json!({
        "count": queries.len(),
        "queries": queries,
    }))
}

/// Create a new session (authenticate)
async fn create_session(
    State(state): State<AppState>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state
        .service
        .authenticate(payload.username, payload.password)
        .await
    {
        Ok(session_id) => {
            info!("Session created: {}", session_id);
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
    info!("Session deleted: {}", id);
    Ok(Json(serde_json::json!({"session_id": id, "deleted": true})))
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
                "Query executed in {}ms: {}",
                latency_ms,
                truncate_query(&payload.query)
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
            error!("Query execution failed: {}", e);
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

/// Truncate query for logging
fn truncate_query(query: &str) -> &str {
    if query.len() > 100 {
        &query[..100]
    } else {
        query
    }
}

/// Request types for HTTP API
#[derive(serde::Deserialize)]
struct CreateSessionRequest {
    username: String,
    password: String,
}

#[derive(serde::Serialize)]
struct SessionResponse {
    session_id: i64,
    time_zone: Option<String>,
}

#[derive(serde::Deserialize)]
struct QueryRequest {
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
