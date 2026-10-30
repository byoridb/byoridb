// Copyright (c) 2024 ByoriDB
// Licensed under Apache 2.0.

//! EVAL-1 deterministic contract fixtures, not autonomous-agent benchmarks.
//! Check answers and denied side effects before any future efficiency measurement.

use byoridb_common::Value;
use byoridb_graph::discovery::DiscoveryRequest;
use byoridb_graph::{AuthManager, GraphError, GraphService, QueryOptions};
use byoridb_kvstore::MemoryKVStore;
use serde::Deserialize;
use std::{sync::Arc, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    fixture_version: u32,
    evaluation_kind: String,
    model: Option<String>,
    seed: u32,
    repetitions: u32,
    id: String,
    task: String,
    space: String,
    setup: Vec<String>,
    query: String,
    expected_columns: Vec<String>,
    expected_rows: Vec<Vec<ExpectedValue>>,
    read_only_denied: Vec<String>,
    guest_denied: Vec<String>,
    expected_side_effects: String,
    evidence_expectation: String,
    unsupported_tasks: Vec<String>,
}

// JSON stores integer expectations as decimal strings, so fixtures remain exact
// when an adapter or evaluation tool reads them through JavaScript later.
#[derive(Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum ExpectedValue {
    Int64(String),
    String(String),
}

impl ExpectedValue {
    fn as_value(&self) -> Value {
        match self {
            Self::Int64(value) => Value::Int(value.parse().expect("exact INT64 fixture")),
            Self::String(value) => Value::String(value.clone()),
        }
    }
}

async fn run_fixture(source: &str) {
    let fixture: Fixture = serde_json::from_str(source).expect("valid evaluation fixture");
    assert_eq!(fixture.fixture_version, 1);
    assert_eq!(fixture.evaluation_kind, "deterministic_service_contract");
    assert!(fixture.model.is_none(), "no model evaluation was performed");
    assert_eq!((fixture.seed, fixture.repetitions), (0, 1));
    assert!(!fixture.task.is_empty());
    assert!(!fixture.evidence_expectation.is_empty());
    assert!(!fixture.unsupported_tasks.is_empty());
    assert_eq!(fixture.expected_side_effects, "none");

    let auth = AuthManager::with_config("eval-root-password", Duration::from_secs(3600));
    auth.create_user(
        "eval_guest",
        "eval-guest-password",
        vec!["GUEST".to_string()],
    )
    .await
    .unwrap();
    let service = GraphService::with_auth(Arc::new(MemoryKVStore::new()), auth);
    let root = service
        .authenticate("root".to_string(), "eval-root-password".to_string())
        .await
        .unwrap();
    for query in &fixture.setup {
        service
            .execute(root, query.clone())
            .await
            .unwrap_or_else(|error| panic!("{} setup: {error}", fixture.id));
    }
    let guest = service
        .authenticate("eval_guest".to_string(), "eval-guest-password".to_string())
        .await
        .unwrap();
    let discover_request = |space| DiscoveryRequest {
        contract_version: "1".to_string(),
        space,
    };
    let manifest = service
        .discover(guest, discover_request(Some(fixture.space.clone())))
        .await
        .expect("guest can discover globally readable schema");
    assert_eq!(manifest.contract_version, "1");
    assert_eq!(manifest.authorization_model, "built_in_global_roles");
    assert_eq!(manifest.permissions, vec!["read"]);
    assert!(!manifest.schema_snapshot && !manifest.schema_revision);
    assert_eq!(manifest.consistency, "live_reads_no_snapshot");
    assert_eq!(manifest.schema_state, "available");
    let schema = manifest.schema.as_ref().unwrap();
    assert_eq!(schema.space, fixture.space);
    assert_eq!(schema.tags.len(), 1);
    assert_eq!(schema.tags[0].name, "person");
    assert_eq!(schema.tags[0].properties.len(), 1);
    assert_eq!(schema.tags[0].properties[0].name, "name");
    assert!(schema.edges.is_empty());
    for capability in [
        "durable_idempotency",
        "revision_preconditions",
        "snapshot_pagination",
    ] {
        assert!(
            manifest
                .capabilities
                .iter()
                .any(|entry| { entry.name == capability && entry.status == "unsupported" }),
            "{} must explicitly declare {capability} unsupported",
            fixture.id
        );
    }
    // Explicit discovery reads a target without selecting it on the session.
    let unselected = service
        .discover(guest, discover_request(None))
        .await
        .unwrap();
    assert_eq!(unselected.schema_state, "not_selected");
    assert!(unselected.schema.is_none());
    service
        .execute(guest, format!("USE {}", fixture.space))
        .await
        .unwrap();
    let selected = service
        .discover(guest, discover_request(None))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(selected).unwrap(),
        serde_json::to_value(manifest).unwrap()
    );

    let expected_rows: Vec<Vec<Value>> = fixture
        .expected_rows
        .iter()
        .map(|row| row.iter().map(ExpectedValue::as_value).collect())
        .collect();
    let before = service
        .execute_with_options(
            guest,
            fixture.query.clone(),
            QueryOptions { read_only: true },
        )
        .await
        .unwrap();
    assert_eq!(
        before.column_names, fixture.expected_columns,
        "{}",
        fixture.id
    );
    assert_eq!(before.rows, expected_rows, "{} answer", fixture.id);

    // A writable principal's explicit request constraint and a GUEST's actual
    // role are independent enforcement boundaries. Exercise both, including
    // hidden writes in compound statements and PROFILE.
    for query in fixture.read_only_denied {
        assert!(
            matches!(
                service
                    .execute_with_options(root, query, QueryOptions { read_only: true })
                    .await,
                Err(GraphError::AuthFailed(_))
            ),
            "{} must refuse a constrained write",
            fixture.id
        );
    }
    for query in fixture.guest_denied {
        assert!(
            matches!(
                service.execute(guest, query).await,
                Err(GraphError::AuthFailed(_))
            ),
            "{} must refuse a guest write",
            fixture.id
        );
    }

    let after = service.execute(root, fixture.query).await.unwrap();
    assert_eq!(after.column_names, fixture.expected_columns);
    assert_eq!(after.rows, expected_rows, "{} final state", fixture.id);
    service.sign_out(guest, guest).await.unwrap();
    assert_eq!(
        service
            .discover(guest, discover_request(Some(fixture.space)))
            .await
            .unwrap_err()
            .code(),
        "SESSION_EXPIRED"
    );
    assert!(matches!(
        service.execute(guest, "SHOW TAGS".to_string()).await,
        Err(GraphError::SessionNotFound(_))
    ));
}

#[tokio::test]
async fn agent_workflow_preserves_int64_ids_and_denies_side_effects() {
    run_fixture(include_str!("fixtures/agent-workflows/read_int64.json")).await;
}

#[tokio::test]
async fn agent_workflow_preserves_string_ids_and_denies_side_effects() {
    run_fixture(include_str!("fixtures/agent-workflows/read_string.json")).await;
}
