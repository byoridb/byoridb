use crate::service::GraphService as InternalGraphService;
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub mod graph_proto {
    tonic::include_proto!("graph");
}

use graph_proto::graph_service_server::GraphService;
use graph_proto::{
    AuthenticateRequest, AuthenticateResponse, ExecuteJsonResponse, ExecuteRequest,
    ExecuteResponse, SignOutRequest, SignOutResponse,
};

/// Convert a `byoridb_common::Value` into the proto `Value` defined in
/// `graph.proto`.
///
/// Common scalar types map directly; everything else (Vertex, Edge,
/// Date/Time, List, Map, ...) is JSON-encoded into `json_value` so the
/// client can still render it. Promoting complex types to first-class
/// proto representations is tracked as a follow-up to PR 9.
fn value_to_proto(v: &byoridb_common::Value) -> graph_proto::Value {
    use byoridb_common::Value as V;
    use graph_proto::value::Value as PV;
    let pv = match v {
        V::Empty | V::Null(_) => PV::NullValue(graph_proto::NullValue::NullValue as i32),
        V::Bool(b) => PV::BoolValue(*b),
        V::Int(i) => PV::IntValue(*i),
        V::Float(f) => PV::FloatValue(*f),
        V::String(s) => PV::StringValue(s.clone()),
        other => {
            // Fallback path for complex types: serialize as JSON. Lossy if
            // serde fails, but matches what `data` (the deprecated bytes
            // field) carried before this PR.
            let json = serde_json::to_string(other).unwrap_or_else(|_| "null".to_string());
            PV::JsonValue(json)
        }
    };
    graph_proto::Value { value: Some(pv) }
}

/// Convert a `byoridb_common::DataSet` into the proto `DataSet`.
fn dataset_to_proto(ds: &byoridb_common::DataSet) -> graph_proto::DataSet {
    let rows = ds
        .rows
        .iter()
        .map(|row| graph_proto::Row {
            values: row.iter().map(value_to_proto).collect(),
        })
        .collect();
    graph_proto::DataSet {
        column_names: ds.column_names.clone(),
        rows,
    }
}

pub struct GrpcService {
    internal_service: Arc<InternalGraphService>,
}

impl GrpcService {
    pub fn new(internal_service: Arc<InternalGraphService>) -> Self {
        Self { internal_service }
    }
}

#[tonic::async_trait]
impl GraphService for GrpcService {
    async fn authenticate(
        &self,
        request: Request<AuthenticateRequest>,
    ) -> Result<Response<AuthenticateResponse>, Status> {
        let req = request.into_inner();

        match self
            .internal_service
            .authenticate(req.username, req.password)
            .await
        {
            Ok(session_id) => Ok(Response::new(AuthenticateResponse {
                session_id,
                error_code: 0,
                error_msg: "".to_string(),
            })),
            Err(_) => Ok(Response::new(AuthenticateResponse {
                session_id: 0,
                error_code: 1,
                // Authentication failures are intentionally indistinguishable
                // to remote callers (unknown/disabled/locked/wrong password).
                error_msg: "Invalid credentials".to_string(),
            })),
        }
    }

    async fn sign_out(
        &self,
        request: Request<SignOutRequest>,
    ) -> Result<Response<SignOutResponse>, Status> {
        let req = request.into_inner();
        // Caller signs out their own session (session_id == caller_session_id)
        match self
            .internal_service
            .sign_out(req.session_id, req.session_id)
            .await
        {
            Ok(()) => Ok(Response::new(SignOutResponse {
                error_code: 0,
                error_msg: "".to_string(),
            })),
            Err(error) => Ok(Response::new(SignOutResponse {
                error_code: if matches!(error, crate::error::GraphError::SessionNotFound(_)) {
                    2
                } else {
                    1
                },
                error_msg: error.to_string(),
            })),
        }
    }

    #[allow(deprecated)] // intentional: populate legacy `data` field for backward compat
    async fn execute(
        &self,
        request: Request<ExecuteRequest>,
    ) -> Result<Response<ExecuteResponse>, Status> {
        let req = request.into_inner();
        let start = std::time::Instant::now();

        match self
            .internal_service
            .execute(req.session_id, req.statement)
            .await
        {
            Ok(dataset) => {
                // Populate both the legacy JSON `data` blob and the new
                // structured `result` so clients on either side of the
                // PR 9 transition see a valid response.
                let data = serde_json::to_vec(&dataset).unwrap_or_default();
                let result = Some(dataset_to_proto(&dataset));
                let latency_us = elapsed_us(start);

                Ok(Response::new(ExecuteResponse {
                    error_code: 0,
                    error_msg: "".to_string(),
                    latency_us,
                    data,
                    result,
                }))
            }
            Err(e) => {
                let latency_us = elapsed_us(start);
                let error_code = if matches!(e, crate::error::GraphError::SessionNotFound(_)) {
                    2
                } else {
                    1
                };
                Ok(Response::new(ExecuteResponse {
                    error_code,
                    error_msg: e.to_string(),
                    latency_us,
                    data: vec![],
                    result: None,
                }))
            }
        }
    }

    async fn execute_json(
        &self,
        request: Request<ExecuteRequest>,
    ) -> Result<Response<ExecuteJsonResponse>, Status> {
        let req = request.into_inner();
        let start = std::time::Instant::now();

        match self
            .internal_service
            .execute_json(req.session_id, req.statement)
            .await
        {
            Ok(json_data) => {
                let latency_us = elapsed_us(start);
                Ok(Response::new(ExecuteJsonResponse {
                    error_code: 0,
                    error_msg: "".to_string(),
                    latency_us,
                    json_data,
                }))
            }
            Err(e) => {
                let latency_us = elapsed_us(start);
                let error_code = if matches!(e, crate::error::GraphError::SessionNotFound(_)) {
                    2
                } else {
                    1
                };
                Ok(Response::new(ExecuteJsonResponse {
                    error_code,
                    error_msg: e.to_string(),
                    latency_us,
                    json_data: "{}".to_string(),
                }))
            }
        }
    }
}

/// Compute the elapsed time in microseconds as an `i64`, saturating at
/// [`i64::MAX`] for extremely long-running queries.
fn elapsed_us(start: std::time::Instant) -> i64 {
    let micros = start.elapsed().as_micros();
    if micros > i64::MAX as u128 {
        i64::MAX
    } else {
        micros as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthManager;
    use byoridb_kvstore::MemoryKVStore;
    use std::time::Duration;

    #[test]
    fn elapsed_us_returns_at_least_the_sleep_duration() {
        let start = std::time::Instant::now();
        std::thread::sleep(Duration::from_millis(2));
        let elapsed = elapsed_us(start);
        // Sleep granularity varies, but we expect well above 1000 us
        // and a sane upper bound to catch accidental i64::MAX returns.
        assert!(elapsed >= 1_000, "elapsed was {}", elapsed);
        assert!(
            elapsed < 1_000_000_000,
            "elapsed suspiciously large: {}",
            elapsed
        );
    }

    async fn failed_grpc_auth(
        service: &GrpcService,
        username: &str,
        password: &str,
    ) -> AuthenticateResponse {
        GraphService::authenticate(
            service,
            Request::new(AuthenticateRequest {
                username: username.to_string(),
                password: password.to_string(),
            }),
        )
        .await
        .unwrap()
        .into_inner()
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
        let internal = Arc::new(InternalGraphService::with_auth(
            Arc::new(MemoryKVStore::new()),
            auth,
        ));
        let service = GrpcService::new(internal);

        for (username, password) in [
            ("missing-user", "wrong-password"),
            ("root", "wrong-password"),
            ("disabled-user", "disabled-password"),
            ("locked-user", "locked-password"),
        ] {
            let response = failed_grpc_auth(&service, username, password).await;
            assert_eq!(response.session_id, 0);
            assert_eq!(response.error_code, 1);
            assert_eq!(response.error_msg, "Invalid credentials");
        }
    }

    #[tokio::test]
    async fn sign_out_unknown_session_returns_session_error() {
        let internal = Arc::new(InternalGraphService::with_auth(
            Arc::new(MemoryKVStore::new()),
            AuthManager::with_config("root-password", Duration::from_secs(3600)),
        ));
        let service = GrpcService::new(internal);

        let response = GraphService::sign_out(
            &service,
            Request::new(SignOutRequest {
                session_id: 99_999_999,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.error_code, 2);
        assert_eq!(response.error_msg, "Session not found");
        assert!(!response.error_msg.contains("99999999"));
    }

    #[tokio::test]
    async fn sign_out_expired_session_returns_session_error() {
        const ROOT_PASSWORD: &str = "root-password";
        let internal = Arc::new(InternalGraphService::with_auth(
            Arc::new(MemoryKVStore::new()),
            AuthManager::with_config(ROOT_PASSWORD, Duration::from_millis(20)),
        ));
        let session = internal
            .authenticate("root".to_string(), ROOT_PASSWORD.to_string())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let service = GrpcService::new(internal);

        let response = GraphService::sign_out(
            &service,
            Request::new(SignOutRequest {
                session_id: session,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.error_code, 2);
        assert_eq!(response.error_msg, "Session not found");
        assert!(!response.error_msg.contains(&session.to_string()));
    }

    #[test]
    fn elapsed_us_never_negative() {
        let start = std::time::Instant::now();
        let elapsed = elapsed_us(start);
        assert!(elapsed >= 0);
    }

    // ===== PR 9 — structured DataSet response =====

    #[test]
    fn value_to_proto_handles_primitives() {
        use graph_proto::value::Value as PV;
        let cases: Vec<(byoridb_common::Value, PV)> = vec![
            (byoridb_common::Value::Bool(true), PV::BoolValue(true)),
            (byoridb_common::Value::Int(42), PV::IntValue(42)),
            (byoridb_common::Value::Float(1.5), PV::FloatValue(1.5)),
            (
                byoridb_common::Value::String("hi".into()),
                PV::StringValue("hi".into()),
            ),
        ];
        for (input, expected) in cases {
            let got = value_to_proto(&input).value.expect("value set");
            assert_eq!(got, expected, "case {:?}", input);
        }
    }

    #[test]
    fn value_to_proto_null_variants_collapse_to_null_value() {
        use byoridb_common::types::NullType;
        let from_empty = value_to_proto(&byoridb_common::Value::Empty);
        let from_null = value_to_proto(&byoridb_common::Value::Null(NullType::Null));
        match (from_empty.value, from_null.value) {
            (
                Some(graph_proto::value::Value::NullValue(_)),
                Some(graph_proto::value::Value::NullValue(_)),
            ) => {}
            other => panic!("expected NullValue for both, got {:?}", other),
        }
    }

    #[test]
    fn value_to_proto_complex_types_fall_back_to_json() {
        use byoridb_common::datatypes::list::List as CommonList;
        let list = byoridb_common::Value::List(CommonList::with_values(vec![
            byoridb_common::Value::Int(1),
            byoridb_common::Value::Int(2),
        ]));
        let pv = value_to_proto(&list).value.expect("value set");
        match pv {
            graph_proto::value::Value::JsonValue(s) => {
                assert!(s.contains("1") && s.contains("2"));
            }
            other => panic!("expected JsonValue fallback, got {:?}", other),
        }
    }

    #[test]
    fn dataset_to_proto_preserves_shape() {
        let ds = byoridb_common::DataSet::with_rows(
            vec!["src".into(), "dst".into()],
            vec![
                vec![byoridb_common::Value::Int(1), byoridb_common::Value::Int(2)],
                vec![byoridb_common::Value::Int(2), byoridb_common::Value::Int(3)],
            ],
        );
        let pds = dataset_to_proto(&ds);
        assert_eq!(pds.column_names, vec!["src", "dst"]);
        assert_eq!(pds.rows.len(), 2);
        assert_eq!(pds.rows[0].values.len(), 2);
    }
}
