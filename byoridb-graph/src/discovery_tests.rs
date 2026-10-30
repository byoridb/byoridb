//! Discovery authorization, failure and bound regressions.
use super::*;
use crate::discovery::*;
use async_trait::async_trait;
use byoridb_executor::key::SchemaKey;
use byoridb_kvstore::{KVStore, KVStoreError, MemoryKVStore, Result as KvResult, VersionRecord};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Default)]
struct ProbeStore {
    inner: MemoryKVStore,
    fail: AtomicUsize,
    reads: AtomicUsize,
    scans: Mutex<Vec<(Vec<u8>, Option<usize>)>>,
}
#[async_trait]
impl KVStore for ProbeStore {
    async fn get(&self, key: &[u8]) -> KvResult<Option<Vec<u8>>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) == 1 {
            return Err(KVStoreError::StoreClosed);
        }
        self.inner.get(key).await
    }

    async fn put(&self, key: &[u8], value: &[u8]) -> KvResult<()> {
        self.inner.put(key, value).await
    }

    async fn put_if_absent(&self, key: &[u8], value: &[u8]) -> KvResult<Option<Vec<u8>>> {
        self.inner.put_if_absent(key, value).await
    }

    async fn delete(&self, key: &[u8]) -> KvResult<()> {
        self.inner.delete(key).await
    }

    async fn batch_put(&self, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> KvResult<()> {
        self.inner.batch_put(pairs).await
    }

    async fn batch_delete(&self, keys: Vec<Vec<u8>>) -> KvResult<()> {
        self.inner.batch_delete(keys).await
    }

    async fn put_version(
        &self,
        entity_key: &[u8],
        valid_from: i64,
        valid_to: i64,
        tx: i64,
        value: &[u8],
    ) -> KvResult<()> {
        self.inner
            .put_version(entity_key, valid_from, valid_to, tx, value)
            .await
    }

    async fn scan_history(&self, entity_key: &[u8]) -> KvResult<Vec<VersionRecord>> {
        self.inner.scan_history(entity_key).await
    }

    async fn scan_history_entity_keys(&self, entity_prefix: &[u8]) -> KvResult<Vec<Vec<u8>>> {
        self.inner.scan_history_entity_keys(entity_prefix).await
    }

    async fn batch_put_version(
        &self,
        versions: Vec<(Vec<u8>, i64, i64, i64, Vec<u8>)>,
    ) -> KvResult<()> {
        self.inner.batch_put_version(versions).await
    }

    async fn scan_prefix(&self, prefix: &[u8]) -> KvResult<Vec<(Vec<u8>, Vec<u8>)>> {
        self.inner.scan_prefix(prefix).await
    }

    async fn scan_prefix_limited(
        &self,
        prefix: &[u8],
        limit: Option<usize>,
    ) -> KvResult<Vec<(Vec<u8>, Vec<u8>)>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.scans.lock().unwrap().push((prefix.to_vec(), limit));
        let fail = self.fail.load(Ordering::SeqCst);
        if (fail == 2 && prefix.ends_with(b":tag:")) || (fail == 3 && prefix.ends_with(b":edge:")) {
            return Err(KVStoreError::StoreClosed);
        }
        self.inner.scan_prefix_limited(prefix, limit).await
    }

    async fn batch_get(&self, keys: &[Vec<u8>]) -> KvResult<Vec<Option<Vec<u8>>>> {
        self.inner.batch_get(keys).await
    }
}

const PASSWORD: &str = "discovery-test-password";
fn fixture() -> (Arc<ProbeStore>, GraphService) {
    let store = Arc::new(ProbeStore::default());
    let service = GraphService::with_auth(
        store.clone(),
        AuthManager::with_config(PASSWORD, Duration::from_secs(60)),
    );
    (store, service)
}
async fn login(service: &GraphService, name: &str) -> i64 {
    service
        .authenticate(name.into(), PASSWORD.into())
        .await
        .unwrap()
}
fn request(space: Option<&str>) -> DiscoveryRequest {
    DiscoveryRequest {
        contract_version: "1".into(),
        space: space.map(str::to_string),
    }
}
async fn space(store: &ProbeStore, name: &str) {
    store
        .put(
            &SchemaKey::space(name),
            serde_json::to_vec(&serde_json::json!({"name":name,"vid_type":"INT64"}))
                .unwrap()
                .as_slice(),
        )
        .await
        .unwrap();
}
async fn tag(store: &ProbeStore, space: &str, name: &str, properties: serde_json::Value) {
    store
        .put(
            &SchemaKey::tag(space, name),
            serde_json::to_vec(&serde_json::json!({"name":name,"properties":properties}))
                .unwrap()
                .as_slice(),
        )
        .await
        .unwrap();
}
fn property(name: &str) -> serde_json::Value {
    serde_json::json!({"name":name,"data_type":"String","nullable":true,"default_value":null})
}

#[tokio::test]
async fn discovery_authentication_reconciles_both_session_stores_before_metadata() {
    let (store, service) = fixture();
    assert_eq!(
        service
            .discover(0, request(Some("secret")))
            .await
            .unwrap_err(),
        DiscoveryError::AuthRequired
    );
    assert_eq!(
        service
            .discover(1234567, request(Some("secret")))
            .await
            .unwrap_err(),
        DiscoveryError::SessionExpired
    );
    let graph_only = login(&service, "root").await;
    service.auth_manager.sign_out(graph_only).await.unwrap();
    assert_eq!(
        service
            .discover(graph_only, request(Some("secret")))
            .await
            .unwrap_err(),
        DiscoveryError::SessionExpired
    );
    assert!(service
        .session_manager
        .get_session(graph_only)
        .await
        .is_none());
    let bearer_only = login(&service, "root").await;
    service.session_manager.remove_session(bearer_only).await;
    assert_eq!(
        service
            .discover(bearer_only, request(None))
            .await
            .unwrap_err(),
        DiscoveryError::SessionExpired
    );
    assert!(
        !service
            .auth_manager
            .has_live_session_without_touch(bearer_only)
            .await
    );
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    let expired = GraphService::with_auth(
        store.clone(),
        AuthManager::with_config(PASSWORD, Duration::from_secs(60)),
    )
    .with_session_ttl(Duration::ZERO);
    let session = login(&expired, "root").await;
    assert_eq!(
        expired.discover(session, request(None)).await.unwrap_err(),
        DiscoveryError::SessionExpired
    );
    assert!(
        !expired
            .auth_manager
            .has_live_session_without_touch(session)
            .await
    );
}

#[tokio::test]
async fn discovery_permission_denial_does_not_reveal_space_existence() {
    let (store, service) = fixture();
    space(&store, "secret").await;
    service
        .auth_manager
        .create_user("none", PASSWORD, vec![])
        .await
        .unwrap();
    let session = login(&service, "none").await;
    for name in [Some("secret"), Some("missing"), None] {
        let error = service.discover(session, request(name)).await.unwrap_err();
        assert_eq!(error, DiscoveryError::PermissionDenied);
        let serialized = serde_json::to_string(&error).unwrap();
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains(&session.to_string()));
    }
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn discovery_reports_effective_guest_and_root_authority_and_truthful_capabilities() {
    let (store, service) = fixture();
    space(&store, "alpha").await;
    service
        .auth_manager
        .create_user("guest", PASSWORD, vec!["GUEST".into()])
        .await
        .unwrap();
    let guest = login(&service, "guest").await;
    let response = service
        .discover(guest, request(Some("alpha")))
        .await
        .unwrap();
    assert_eq!(response.permissions, vec!["read"]);
    assert_eq!(response.authorization_model, "built_in_global_roles");
    assert_eq!(response.consistency, "live_reads_no_snapshot");
    assert!(!response.schema_revision && !response.schema_snapshot);
    assert_eq!(response.server_version, env!("CARGO_PKG_VERSION"));
    for capability in response.capabilities {
        let expected = if ["schema_discovery", "text_query", "request_read_only"]
            .contains(&capability.name.as_str())
        {
            "available"
        } else {
            "unsupported"
        };
        assert_eq!(capability.status, expected);
    }
    let root = login(&service, "root").await;
    assert_eq!(
        service
            .discover(root, request(None))
            .await
            .unwrap()
            .permissions,
        vec!["alter", "create", "delete", "drop", "read", "write"]
    );
}

#[tokio::test]
async fn discovery_explicit_selected_and_no_space_never_change_selection_or_leak_other_schema() {
    let (store, service) = fixture();
    for name in ["alpha", "beta"] {
        space(&store, name).await;
        tag(&store, name, name, serde_json::json!([])).await;
    }
    let root = login(&service, "root").await;
    let empty = service.discover(root, request(None)).await.unwrap();
    assert_eq!(empty.schema_state, "not_selected");
    assert!(empty.schema.is_none());
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    service
        .session_manager
        .set_space(root, "alpha".into())
        .await
        .unwrap();
    let explicit = service.discover(root, request(Some("beta"))).await.unwrap();
    assert_eq!(explicit.schema.unwrap().tags[0].name, "beta");
    assert_eq!(
        service
            .session_manager
            .get_session(root)
            .await
            .unwrap()
            .space
            .as_deref(),
        Some("alpha")
    );
    let selected = service
        .discover(root, request(None))
        .await
        .unwrap()
        .schema
        .unwrap();
    assert_eq!(selected.space, "alpha");
    assert_eq!(selected.tags[0].name, "alpha");
    let before = store.reads.load(Ordering::SeqCst);
    for bad in ["alpha:tag:beta", "", "bad\nspace"] {
        assert_eq!(
            service
                .discover(root, request(Some(bad)))
                .await
                .unwrap_err(),
            DiscoveryError::InvalidRequest
        );
    }
    assert_eq!(store.reads.load(Ordering::SeqCst), before);
    assert_eq!(
        service
            .discover(root, request(Some("missing")))
            .await
            .unwrap_err(),
        DiscoveryError::SpaceNotFound
    );
}

#[tokio::test]
async fn discovery_version_and_shutdown_fail_without_metadata_reads() {
    let (store, service) = fixture();
    let root = login(&service, "root").await;
    let mut req = request(Some("alpha"));
    req.contract_version = "future-secret".into();
    assert_eq!(
        service.discover(root, req).await.unwrap_err(),
        DiscoveryError::UnsupportedContract
    );
    service.shutdown_state().stop_accepting();
    assert_eq!(
        service
            .discover(root, request(Some("alpha")))
            .await
            .unwrap_err(),
        DiscoveryError::ServiceUnavailable
    );
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    assert_eq!(service.shutdown_state().active_queries(), 0);
}

#[tokio::test]
async fn discovery_storage_failures_never_become_empty_or_partial_schemas() {
    let (store, service) = fixture();
    space(&store, "alpha").await;
    tag(&store, "alpha", "safe", serde_json::json!([])).await;
    let root = login(&service, "root").await;
    for failure in [1, 2, 3] {
        store.fail.store(failure, Ordering::SeqCst);
        assert_eq!(
            service
                .discover(root, request(Some("alpha")))
                .await
                .unwrap_err(),
            DiscoveryError::StorageError
        );
        assert_eq!(service.shutdown_state().active_queries(), 0);
    }
}

#[tokio::test]
async fn discovery_malformed_metadata_fails_closed() {
    let (store, service) = fixture();
    space(&store, "alpha").await;
    let root = login(&service, "root").await;
    let key = SchemaKey::tag("alpha", "broken");
    for value in [
        "not-json".to_string(),
        serde_json::json!({"name":"wrong","properties":[]}).to_string(),
        serde_json::json!({"name":"broken","properties":[{"name":"x","data_type":"Unknown","nullable":true,"default_value":null}]}).to_string(),
        serde_json::json!({"name":"broken","properties":[property("duplicate"),property("duplicate")]}).to_string(),
        serde_json::json!({"name":"broken","properties":[{"name":"x","data_type":"String","nullable":"true","default_value":null}]}).to_string(),
    ] {
        store.put(&key,value.as_bytes()).await.unwrap();
        assert_eq!(service.discover(root,request(Some("alpha"))).await.unwrap_err(),DiscoveryError::InvalidMetadata);
    }
}

#[tokio::test]
async fn discovery_count_limits_are_inclusive_and_scans_use_cap_plus_one() {
    let (store, _) = fixture();
    space(&store, "alpha").await;
    let limits = DiscoveryLimits {
        max_tags: 2,
        max_edges: 2,
        max_properties_per_type: 2,
        max_metadata_bytes: 8192,
    };
    assert!(
        crate::discovery::read_schema(store.as_ref(), "alpha", &limits)
            .await
            .unwrap()
            .tags
            .is_empty()
    );
    tag(
        &store,
        "alpha",
        "z",
        serde_json::json!([property("z"), property("a")]),
    )
    .await;
    assert_eq!(
        crate::discovery::read_schema(store.as_ref(), "alpha", &limits)
            .await
            .unwrap()
            .tags
            .len(),
        1
    );
    tag(&store, "alpha", "a", serde_json::json!([])).await;
    let schema = crate::discovery::read_schema(store.as_ref(), "alpha", &limits)
        .await
        .unwrap();
    assert_eq!(schema.tags.len(), 2);
    assert_eq!(schema.tags[0].name, "a");
    assert_eq!(schema.tags[1].properties[0].name, "a");
    tag(&store, "alpha", "b", serde_json::json!([])).await;
    assert_eq!(
        crate::discovery::read_schema(store.as_ref(), "alpha", &limits)
            .await
            .unwrap_err(),
        DiscoveryError::ResourceExhausted
    );
    assert!(store
        .scans
        .lock()
        .unwrap()
        .iter()
        .all(|(_, limit)| *limit == Some(3)));
    store.delete(&SchemaKey::tag("alpha", "b")).await.unwrap();
    tag(
        &store,
        "alpha",
        "z",
        serde_json::json!([property("a"), property("b"), property("c")]),
    )
    .await;
    assert_eq!(
        crate::discovery::read_schema(store.as_ref(), "alpha", &limits)
            .await
            .unwrap_err(),
        DiscoveryError::ResourceExhausted
    );
}

#[tokio::test]
async fn discovery_byte_budget_is_inclusive_and_counts_raw_hidden_defaults() {
    let (store, _) = fixture();
    space(&store, "alpha").await;
    let key = SchemaKey::space("alpha");
    let bytes = store.inner.get(&key).await.unwrap().unwrap();
    let size = (key.len() + bytes.len()) as u32;
    for (budget, success) in [(size + 1, true), (size, true), (size - 1, false)] {
        let limits = DiscoveryLimits {
            max_metadata_bytes: budget,
            ..Default::default()
        };
        assert_eq!(
            crate::discovery::read_schema(store.as_ref(), "alpha", &limits)
                .await
                .is_ok(),
            success
        );
    }
    tag(&store,"alpha","hidden",serde_json::json!([{"name":"x","data_type":"String","nullable":true,"default_value":{"Literal":{"String":"secret".repeat(1000)}}}])).await;
    let limits = DiscoveryLimits {
        max_metadata_bytes: 1024,
        ..Default::default()
    };
    assert_eq!(
        crate::discovery::read_schema(store.as_ref(), "alpha", &limits)
            .await
            .unwrap_err(),
        DiscoveryError::ResourceExhausted
    );
}

#[tokio::test]
async fn discovery_normalizes_real_create_and_alter_types_without_exposing_defaults() {
    let (store, service) = fixture();
    let root = login(&service, "root").await;
    for query in [
        "CREATE SPACE example (vid_type=FIXED_STRING(64))",
        "USE example",
        "CREATE EDGE knows (since int)",
    ] {
        service
            .execute(root, query.into())
            .await
            .unwrap_or_else(|e| panic!("{query}: {e}"));
    }
    // The public parser currently supports FIXED_STRING for VIDs only. Build
    // the typed executor plans to exercise both actual metadata writers.
    use byoridb_executor::plan::{AlterPlan, CreatePlan, ExecutionPlan, ExecutionPlanBuilder};
    let ctx = Arc::new(
        byoridb_executor::context::ExecutionContext::new(store.clone())
            .with_space("example".into())
            .with_space_id(1),
    );
    let executor = byoridb_executor::executor::Executor::new(ctx);
    for query in [
        "CREATE TAG person (created string, secret string DEFAULT \"hidden-default\")",
        "ALTER TAG person ADD (altered string)",
    ] {
        let mut plan = ExecutionPlanBuilder::build(byoridb_parser::parse(query).unwrap()).unwrap();
        match &mut plan {
            ExecutionPlan::Create(CreatePlan::Tag { props, .. }) => {
                props[0].data_type = byoridb_parser::ast::DataType::FixedString(32)
            }
            ExecutionPlan::Alter(AlterPlan::Tag { operations, .. }) => {
                operations[0].prop.data_type = byoridb_parser::ast::DataType::FixedString(48)
            }
            _ => unreachable!(),
        }
        executor.execute(plan).await.unwrap();
    }
    let response = service.discover(root, request(None)).await.unwrap();
    let encoded = serde_json::to_string(&response).unwrap();
    assert!(!encoded.contains("hidden-default"));
    assert!(!encoded.contains(&root.to_string()));
    let schema = response.schema.unwrap();
    assert_eq!(schema.vid.kind, "fixed_string");
    assert_eq!(schema.vid.max_utf8_bytes, Some(64));
    assert_eq!(schema.edges[0].name, "knows");
    let props = &schema.tags[0].properties;
    assert_eq!(props[0].name, "altered");
    assert_eq!(props[0].data_type, "fixed_string");
    assert_eq!(props[0].max_utf8_bytes, Some(48));
    assert_eq!(props[1].name, "created");
    assert_eq!(props[1].max_utf8_bytes, Some(32));
    assert!(props[2].has_default);
}

#[tokio::test]
async fn discovery_edge_limits_are_independent_and_inclusive() {
    let (store, _) = fixture();
    space(&store, "alpha").await;
    let limits = DiscoveryLimits {
        max_tags: 0,
        max_edges: 2,
        ..Default::default()
    };
    for (i, name) in ["z", "a", "over"].into_iter().enumerate() {
        let key = SchemaKey::edge("alpha", name);
        let bytes = serde_json::to_vec(&serde_json::json!({"name":name,"properties":[]})).unwrap();
        store.put(&key, &bytes).await.unwrap();
        let result = crate::discovery::read_schema(store.as_ref(), "alpha", &limits).await;
        if i < 2 {
            let schema = result.unwrap();
            assert_eq!(schema.edges.len(), i + 1);
            assert!(schema.tags.is_empty());
            if i == 1 {
                assert_eq!(schema.edges[0].name, "a");
            }
        } else {
            assert_eq!(result.unwrap_err(), DiscoveryError::ResourceExhausted);
        }
    }
    let scans = store.scans.lock().unwrap();
    for (prefix, limit) in scans.iter() {
        assert_eq!(
            *limit,
            if prefix.ends_with(b":tag:") {
                Some(1)
            } else {
                Some(3)
            }
        );
    }
}

#[tokio::test]
async fn discovery_rejects_corrupt_vid_types_fixed_string_sizes_and_defaults() {
    let (store, _) = fixture();
    for bad in [
        "unknown",
        "FIXED_STRING(0)",
        "FIXED_STRING(-1)",
        "FIXED_STRING(4294967296)",
    ] {
        store
            .put(
                &SchemaKey::space("alpha"),
                serde_json::json!({"name":"alpha","vid_type":bad})
                    .to_string()
                    .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(
            crate::discovery::read_schema(store.as_ref(), "alpha", &DiscoveryLimits::default())
                .await
                .unwrap_err(),
            DiscoveryError::InvalidMetadata
        );
    }
    space(&store, "alpha").await;
    for bad_type in [
        serde_json::json!({"FixedString":0}),
        serde_json::json!({"FixedString":4294967296_u64}),
        serde_json::json!({"FixedString":32,"extra":1}),
        serde_json::json!("FixedString(-1)"),
    ] {
        let mut prop = property("value");
        prop["data_type"] = bad_type;
        tag(&store, "alpha", "invalid", serde_json::json!([prop])).await;
        assert_eq!(
            crate::discovery::read_schema(store.as_ref(), "alpha", &DiscoveryLimits::default())
                .await
                .unwrap_err(),
            DiscoveryError::InvalidMetadata
        );
    }
    let mut prop = property("value");
    prop["default_value"] = serde_json::json!("not-an-expression");
    tag(&store, "alpha", "invalid", serde_json::json!([prop])).await;
    assert_eq!(
        crate::discovery::read_schema(store.as_ref(), "alpha", &DiscoveryLimits::default())
            .await
            .unwrap_err(),
        DiscoveryError::InvalidMetadata
    );
}
