use super::{Executor, ExecutorResult};
use crate::{context::ExecutionContext, error::Result, key::SchemaKey, ExecutionPlanBuilder};
use async_trait::async_trait;
use byoridb_common::Value;
use byoridb_kvstore::{KVStore, KVStoreError, MemoryKVStore, Result as KvResult, VersionRecord};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::sync::Notify;

#[derive(Default)]
struct Faults {
    batch_key: Mutex<Option<Vec<u8>>>,
    delete_key: Mutex<Option<Vec<u8>>>,
    fail_assertion: AtomicBool,
    pause_assertion: AtomicBool,
    committed: Notify,
    resume: Notify,
}
struct FaultStore {
    inner: MemoryKVStore,
    faults: Faults,
}
impl FaultStore {
    fn new() -> Self {
        Self {
            inner: MemoryKVStore::new(),
            faults: Faults::default(),
        }
    }
}
#[async_trait]
impl KVStore for FaultStore {
    async fn get(&self, key: &[u8]) -> KvResult<Option<Vec<u8>>> {
        self.inner.get(key).await
    }
    async fn put(&self, key: &[u8], value: &[u8]) -> KvResult<()> {
        self.inner.put(key, value).await
    }
    async fn put_if_absent(&self, key: &[u8], value: &[u8]) -> KvResult<Option<Vec<u8>>> {
        self.inner.put_if_absent(key, value).await
    }
    async fn delete(&self, key: &[u8]) -> KvResult<()> {
        if self.faults.delete_key.lock().unwrap().as_deref() == Some(key) {
            return Err(KVStoreError::StoreClosed);
        }
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
        key: &[u8],
        from: i64,
        to: i64,
        tx: i64,
        value: &[u8],
    ) -> KvResult<()> {
        self.inner.put_version(key, from, to, tx, value).await
    }
    async fn scan_history(&self, key: &[u8]) -> KvResult<Vec<VersionRecord>> {
        self.inner.scan_history(key).await
    }
    async fn scan_history_entity_keys(&self, prefix: &[u8]) -> KvResult<Vec<Vec<u8>>> {
        self.inner.scan_history_entity_keys(prefix).await
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
    async fn batch_get(&self, keys: &[Vec<u8>]) -> KvResult<Vec<Option<Vec<u8>>>> {
        self.inner.batch_get(keys).await
    }
    async fn batch_apply(
        &self,
        puts: Vec<(Vec<u8>, Vec<u8>)>,
        deletes: Vec<Vec<u8>>,
        versions: Vec<(Vec<u8>, i64, i64, i64, Vec<u8>)>,
    ) -> KvResult<()> {
        let assertion = !versions.is_empty();
        if (assertion && self.faults.fail_assertion.load(Ordering::SeqCst))
            || self
                .faults
                .batch_key
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|key| puts.iter().any(|(k, _)| k == key))
        {
            return Err(KVStoreError::StoreClosed);
        }
        self.inner.batch_apply(puts, deletes, versions).await?;
        if assertion && self.faults.pause_assertion.swap(false, Ordering::SeqCst) {
            self.faults.committed.notify_one();
            self.faults.resume.notified().await;
        }
        Ok(())
    }
}
fn executor(store: Arc<dyn KVStore>, cap: usize, admin: bool) -> Executor {
    let mut ctx = ExecutionContext::new(store).with_space("default".into());
    ctx.config.max_traversal_nodes = cap;
    if admin {
        ctx.caller_roles = vec!["ADMIN".into()];
    }
    Executor::new(Arc::new(ctx))
}
async fn query(e: &Executor, q: &str) -> Result<ExecutorResult> {
    e.execute(ExecutionPlanBuilder::build(byoridb_parser::parse(q).unwrap()).unwrap())
        .await
}
async fn ok(e: &Executor, q: &str) {
    query(e, q).await.unwrap_or_else(|err| panic!("{q}: {err}"));
}
async fn status(e: &Executor) -> String {
    let r = query(e, "CHECK INFERENCE").await.unwrap();
    let Value::String(value) = &r.rows[0][1] else {
        panic!("status")
    };
    value.clone()
}
async fn setup(e: &Executor) {
    ok(e, "CREATE EDGE link() SYMMETRIC").await;
}

#[tokio::test]
async fn zero_budget_is_unbounded_and_exact_budget_is_complete() {
    for cap in [0, 1] {
        let e = executor(Arc::new(MemoryKVStore::new()), cap, true);
        setup(&e).await;
        ok(&e, "INSERT EDGE link() VALUES 1->2:()").await;
        assert_eq!(status(&e).await, "complete");
        assert!(e.triple_exists("default", 2, "link", 1).await.unwrap());
        assert_eq!(
            e.ctx
                .kvstore
                .scan_prefix(b"default:edge:")
                .await
                .unwrap()
                .len(),
            2
        );
    }
}

#[tokio::test]
async fn over_budget_commits_assertions_blocks_reads_and_repairs_without_replay() {
    let store = Arc::new(MemoryKVStore::new());
    let e = executor(store.clone(), 1, true);
    setup(&e).await;
    let err = query(&e, "INSERT EDGE link() VALUES 1->2:(), 3->4:()")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        crate::error::ExecutionError::ResourceExhausted(_)
    ));
    assert!(store
        .get(b"default:edge:1:link:2:0")
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        store
            .scan_history(b"default:edge:1:link:2:0")
            .await
            .unwrap()
            .len(),
        1
    );
    let fresh = executor(store.clone(), 0, true);
    assert_eq!(status(&fresh).await, "repair_required");
    for q in [
        "MATCH (n) RETURN n",
        "GO FROM 1 OVER link",
        "LOOKUP ON link",
        "WHY 1 -> 2 OVER link",
        "FETCH PROP ON link 1->2",
        "FIND PATH FROM 1 TO 2 OVER link",
        "CHECK CONSISTENCY",
        "INSERT EDGE link() VALUES 8->9:()",
    ] {
        let err = query(&fresh, q).await.unwrap_err().to_string();
        assert!(err.contains("Inference incomplete"), "{q}: {err}");
    }
    ok(&fresh, "REPAIR INFERENCE").await;
    assert_eq!(status(&fresh).await, "complete");
    assert_eq!(store.scan_prefix(b"default:edge:").await.unwrap().len(), 4);
    assert_eq!(
        store
            .scan_history(b"default:edge:1:link:2:0")
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .get(b"default:edge:8:link:9:0")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn assertion_and_dirty_marker_commit_atomically() {
    let store = Arc::new(FaultStore::new());
    let e = executor(store.clone(), 0, true);
    setup(&e).await;
    store.faults.fail_assertion.store(true, Ordering::SeqCst);
    assert!(query(&e, "INSERT EDGE link() VALUES 1->2:()")
        .await
        .is_err());
    assert_eq!(status(&e).await, "complete");
    assert!(store
        .scan_prefix(b"default:edge:")
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .scan_history(b"default:edge:1:link:2:0")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn derived_failure_keeps_fence_and_fact_indexes_atomic() {
    let store = Arc::new(FaultStore::new());
    let e = executor(store.clone(), 0, true);
    setup(&e).await;
    *store.faults.batch_key.lock().unwrap() = Some(b"default:edge:2:link:1:0".to_vec());
    assert!(query(&e, "INSERT EDGE link() VALUES 1->2:()")
        .await
        .is_err());
    assert_eq!(status(&e).await, "repair_required");
    for key in [
        SchemaKey::edge_data("default", 2, "link", 1, 0),
        SchemaKey::in_edge_data("default", 1, "link", 2, 0),
        SchemaKey::prov_edge("default", 2, "link", 1),
        SchemaKey::prov_rev_edge("default", 1, "link", 2),
    ] {
        assert!(
            store.get(&key).await.unwrap().is_none(),
            "partial derived batch: {key:?}"
        );
    }
    *store.faults.batch_key.lock().unwrap() = None;
    ok(&e, "REPAIR INFERENCE").await;
    assert_eq!(status(&e).await, "complete");
}

#[tokio::test]
async fn failure_to_clear_marker_requires_repair_even_after_closure() {
    let store = Arc::new(FaultStore::new());
    let e = executor(store.clone(), 0, true);
    setup(&e).await;
    *store.faults.delete_key.lock().unwrap() = Some(Executor::inference_dirty_key("default"));
    assert!(query(&e, "INSERT EDGE link() VALUES 1->2:()")
        .await
        .is_err());
    assert_eq!(status(&e).await, "repair_required");
    assert!(e.triple_exists("default", 2, "link", 1).await.unwrap());
    *store.faults.delete_key.lock().unwrap() = None;
    ok(&e, "REPAIR INFERENCE").await;
}

#[tokio::test]
async fn repair_is_authorized_and_repeatable_without_history_changes() {
    let store = Arc::new(MemoryKVStore::new());
    let e = executor(store.clone(), 0, true);
    setup(&e).await;
    ok(&e, "INSERT EDGE link() VALUES 1->2:()").await;
    let guest = executor(store.clone(), 0, false);
    assert!(query(&guest, "REPAIR INFERENCE")
        .await
        .unwrap_err()
        .to_string()
        .contains("GOD or ADMIN"));
    let before = store.scan_prefix(b"default:").await.unwrap();
    for _ in 0..3 {
        ok(&e, "REPAIR INFERENCE").await;
        assert_eq!(store.scan_prefix(b"default:").await.unwrap(), before);
    }
    assert_eq!(
        store
            .scan_history(b"default:edge:1:link:2:0")
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn corrupt_schema_and_provenance_are_not_silently_ignored() {
    let store = Arc::new(MemoryKVStore::new());
    let e = executor(store.clone(), 0, true);
    setup(&e).await;
    ok(&e, "INSERT EDGE link() VALUES 1->2:()").await;
    let key = SchemaKey::prov_edge("default", 2, "link", 1);
    store.put(&key, b"invalid").await.unwrap();
    assert!(query(&e, "WHY 2 -> 1 OVER link").await.is_err());
    ok(&e, "REPAIR INFERENCE").await;
    ok(&e, "WHY 2 -> 1 OVER link").await;
    let key = SchemaKey::edge("default", "link");
    let original = store.get(&key).await.unwrap().unwrap();
    for bad in [
        b"invalid".as_slice(),
        br#"{"name":"link","semantics":{"transitive":"yes"}}"#,
        br#"{"name":"other","semantics":{}}"#,
        br#"{"name":"link","semantics":{"property_chain":["link"]}}"#,
    ] {
        store.put(&key, bad).await.unwrap();
        assert!(query(&e, "REPAIR INFERENCE").await.is_err());
        assert_eq!(status(&e).await, "repair_required");
        store.put(&key, &original).await.unwrap();
        ok(&e, "REPAIR INFERENCE").await;
    }
}

#[tokio::test]
async fn repair_removes_orphan_inferred_reverse_type_and_provenance() {
    let store = Arc::new(MemoryKVStore::new());
    let e = executor(store.clone(), 0, true);
    setup(&e).await;
    ok(&e, "INSERT EDGE link() VALUES 1->2:()").await;
    let blob = store
        .get(b"default:edge:2:link:1:0")
        .await
        .unwrap()
        .unwrap();
    store.delete(b"default:edge:2:link:1:0").await.unwrap();
    store
        .put(&SchemaKey::vtype("default", 9, "stale"), b"")
        .await
        .unwrap();
    store
        .put(&SchemaKey::prov_edge("default", 9, "stale", 10), b"invalid")
        .await
        .unwrap();
    store
        .put(b"default:in-edge:1:link:2:0", &blob)
        .await
        .unwrap();
    ok(&e, "DROP EDGE link").await;
    assert!(store
        .get(b"default:in-edge:1:link:2:0")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .scan_prefix(b"default:vtype:")
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .scan_prefix(&SchemaKey::prov_prefix("default"))
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .get(b"default:edge:1:link:2:0")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn corruption_during_rebuild_keeps_dirty_until_data_is_fixed() {
    let store = Arc::new(MemoryKVStore::new());
    let e = executor(store.clone(), 0, true);
    setup(&e).await;
    store
        .put(b"default:edge:1:link:2:0", b"invalid")
        .await
        .unwrap();
    assert!(query(&e, "REPAIR INFERENCE").await.is_err());
    assert_eq!(status(&e).await, "repair_required");
    store.delete(b"default:edge:1:link:2:0").await.unwrap();
    ok(&e, "REPAIR INFERENCE").await;
}

#[tokio::test]
async fn concurrent_repair_waits_for_mutation_and_recovers_its_failure() {
    let store = Arc::new(FaultStore::new());
    let e = executor(store.clone(), 0, true);
    setup(&e).await;
    store.faults.pause_assertion.store(true, Ordering::SeqCst);
    *store.faults.batch_key.lock().unwrap() = Some(b"default:edge:2:link:1:0".to_vec());
    let s = store.clone();
    let writer = tokio::spawn(async move {
        query(&executor(s, 0, true), "INSERT EDGE link() VALUES 1->2:()").await
    });
    store.faults.committed.notified().await;
    let s = store.clone();
    let mut repair =
        tokio::spawn(async move { query(&executor(s, 0, true), "REPAIR INFERENCE").await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), &mut repair)
            .await
            .is_err(),
        "repair must wait for the mutation write lock"
    );
    store.faults.resume.notify_one();
    assert!(writer.await.unwrap().is_err());
    // The queued repair may already have failed on the intentionally injected
    // derived-write fault; either way no operation can clear incomplete closure.
    let _ = repair.await.unwrap();
    assert_eq!(status(&e).await, "repair_required");
    *store.faults.batch_key.lock().unwrap() = None;
    ok(&e, "REPAIR INFERENCE").await;
    assert!(e.triple_exists("default", 2, "link", 1).await.unwrap());
}

#[tokio::test]
async fn cancellation_after_asserted_commit_leaves_recoverable_fence() {
    let store = Arc::new(FaultStore::new());
    let e = executor(store.clone(), 0, true);
    setup(&e).await;
    store.faults.pause_assertion.store(true, Ordering::SeqCst);
    let s = store.clone();
    let writer = tokio::spawn(async move {
        query(&executor(s, 0, true), "INSERT EDGE link() VALUES 1->2:()").await
    });
    store.faults.committed.notified().await;
    writer.abort();
    assert!(writer.await.unwrap_err().is_cancelled());
    assert_eq!(status(&e).await, "repair_required");
    ok(&e, "REPAIR INFERENCE").await;
    assert_eq!(
        store
            .scan_history(b"default:edge:1:link:2:0")
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn dirty_marker_survives_redb_close_and_reopen() {
    use byoridb_kvstore::{KVStoreOptions, RedbKVStore};
    let path = std::env::temp_dir().join(format!(
        "byoridb-inference-recovery-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let store = Arc::new(RedbKVStore::open(&path, KVStoreOptions::default()).unwrap());
        let e = executor(store, 1, true);
        setup(&e).await;
        assert!(query(&e, "INSERT EDGE link() VALUES 1->2:(),3->4:()")
            .await
            .is_err());
    }
    {
        let store = Arc::new(RedbKVStore::open(&path, KVStoreOptions::default()).unwrap());
        let e = executor(store.clone(), 0, true);
        assert_eq!(status(&e).await, "repair_required");
        assert!(query(&e, "WHY 1 -> 2 OVER link").await.is_err());
        ok(&e, "REPAIR INFERENCE").await;
        assert_eq!(status(&e).await, "complete");
        assert_eq!(store.scan_prefix(b"default:edge:").await.unwrap().len(), 4);
    }
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn update_of_derived_edge_promotes_assertion_and_survives_repair() {
    let store = Arc::new(MemoryKVStore::new());
    let e = executor(store.clone(), 0, true);
    ok(&e, "CREATE EDGE link(weight int) SYMMETRIC").await;
    ok(&e, "INSERT EDGE link(weight) VALUES 1->2:(1)").await;
    ok(&e, "UPDATE EDGE ON link 2->1 SET weight = 9").await;
    ok(&e, "DELETE EDGE link 1->2").await;
    ok(&e, "REPAIR INFERENCE").await;
    let blob = store
        .get(b"default:edge:2:link:1:0")
        .await
        .unwrap()
        .unwrap();
    let edge = byoridb_codec::VertexCodec::decode_edge(&blob).unwrap();
    assert_eq!(edge.properties.get("weight"), Some(&Value::Int(9)));
    assert!(!edge
        .properties
        .contains_key(super::inference::INFERRED_MARKER));
    assert_eq!(
        store
            .scan_history(b"default:edge:2:link:1:0")
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn reserved_inference_property_cannot_forge_derived_assertions() {
    let store = Arc::new(MemoryKVStore::new());
    let e = executor(store, 0, true);
    ok(&e, "CREATE EDGE link(__inferred__ bool)").await;
    assert!(
        query(&e, "INSERT EDGE link(__inferred__) VALUES 1->2:(true)")
            .await
            .unwrap_err()
            .to_string()
            .contains("reserved")
    );
    assert_eq!(status(&e).await, "complete");
}

#[tokio::test]
async fn vertex_type_facts_share_exact_inference_budget() {
    let store = Arc::new(MemoryKVStore::new());
    let e = executor(store.clone(), 1, true);
    ok(&e, "CREATE CLASS person()").await;
    ok(&e, "CREATE CLASS city()").await;
    ok(&e, "CREATE EDGE lives() DOMAIN person RANGE city").await;
    assert!(matches!(
        query(&e, "INSERT EDGE lives() VALUES 1->2:()").await,
        Err(crate::error::ExecutionError::ResourceExhausted(_))
    ));
    assert_eq!(status(&e).await, "repair_required");
    let e = executor(store.clone(), 2, true);
    ok(&e, "REPAIR INFERENCE").await;
    assert_eq!(store.scan_prefix(b"default:vtype:").await.unwrap().len(), 2);
}

#[tokio::test]
async fn corrupt_reverse_edge_cannot_be_ignored_during_rebuild() {
    let store = Arc::new(MemoryKVStore::new());
    let e = executor(store.clone(), 0, true);
    setup(&e).await;
    ok(&e, "INSERT EDGE link() VALUES 1->2:()").await;
    store
        .put(b"default:in-edge:2:link:1:0", b"broken")
        .await
        .unwrap();
    assert!(query(&e, "REPAIR INFERENCE").await.is_err());
    assert_eq!(status(&e).await, "repair_required");
    store.delete(b"default:in-edge:2:link:1:0").await.unwrap();
    ok(&e, "REPAIR INFERENCE").await;
    assert!(store
        .get(b"default:in-edge:2:link:1:0")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn read_waits_for_semantic_mutation_then_rejects_its_dirty_result() {
    let store = Arc::new(FaultStore::new());
    let e = executor(store.clone(), 0, true);
    setup(&e).await;
    store.faults.pause_assertion.store(true, Ordering::SeqCst);
    let s = store.clone();
    let writer = tokio::spawn(async move {
        query(&executor(s, 0, true), "INSERT EDGE link() VALUES 1->2:()").await
    });
    store.faults.committed.notified().await;
    let s = store.clone();
    let mut reader =
        tokio::spawn(async move { query(&executor(s, 0, true), "WHY 1 -> 2 OVER link").await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), &mut reader)
            .await
            .is_err()
    );
    writer.abort();
    assert!(writer.await.unwrap_err().is_cancelled());
    assert!(reader
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("Inference incomplete"));
}

#[tokio::test]
async fn deleting_inferred_only_edge_does_not_append_asserted_history() {
    let store = Arc::new(MemoryKVStore::new());
    let e = executor(store.clone(), 0, true);
    setup(&e).await;
    ok(&e, "INSERT EDGE link() VALUES 1->2:()").await;
    assert!(query(&e, "DELETE EDGE link 2->1")
        .await
        .unwrap_err()
        .to_string()
        .contains("supporting assertions"));
    assert_eq!(status(&e).await, "complete");
    assert!(store
        .scan_history(b"default:edge:2:link:1:0")
        .await
        .unwrap()
        .is_empty());
}
