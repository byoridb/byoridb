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
    failed_scan: Mutex<Option<Vec<u8>>>,
    pause_scan_prefix: Mutex<Option<Vec<u8>>>,
    commits: std::sync::atomic::AtomicUsize,
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
        let pause = {
            let mut key = self.faults.pause_scan_prefix.lock().unwrap();
            if key.as_deref() == Some(prefix) {
                key.take();
                true
            } else {
                false
            }
        };
        if pause {
            self.faults.committed.notify_one();
            self.faults.resume.notified().await;
        }
        if self.faults.failed_scan.lock().unwrap().as_deref() == Some(prefix) {
            return Err(KVStoreError::StoreClosed);
        }
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
        self.faults.commits.fetch_add(1, Ordering::SeqCst);
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

fn executor(store: Arc<dyn KVStore>, cap: usize) -> Executor {
    let mut ctx = ExecutionContext::new(store).with_space("default".into());
    ctx.config.max_traversal_nodes = cap;
    ctx.caller_roles = vec!["ADMIN".into()];
    Executor::new(Arc::new(ctx))
}
async fn query(e: &Executor, q: &str) -> Result<ExecutorResult> {
    e.execute(ExecutionPlanBuilder::build(byoridb_parser::parse(q).unwrap()).unwrap())
        .await
}
async fn ok(e: &Executor, q: &str) {
    query(e, q).await.unwrap_or_else(|err| panic!("{q}: {err}"));
}
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    values: Vec<(Vec<u8>, Vec<u8>)>,
    history: Vec<(Vec<u8>, Vec<VersionRecord>)>,
}
async fn snapshot(store: &dyn KVStore) -> Snapshot {
    let values = store.scan_prefix(b"").await.unwrap();
    let mut history = Vec::new();
    for key in store.scan_history_entity_keys(b"").await.unwrap() {
        let rows = store.scan_history(&key).await.unwrap();
        history.push((key, rows));
    }
    Snapshot { values, history }
}
async fn setup(store: Arc<FaultStore>) -> Executor {
    let e = executor(store, 0);
    ok(&e, "CREATE TAG item(name string, emb string)").await;
    ok(&e, "CREATE TAG INDEX item_name ON item(name)").await;
    ok(&e, "CREATE EDGE link(weight int)").await;
    ok(&e, "CREATE EDGE INDEX link_weight ON link(weight)").await;
    ok(&e, "CREATE EDGE sameAs()").await;
    ok(&e,"INSERT VERTEX item(name,emb) VALUES 1:(\"one\",[1.0,0.0]),2:(\"two\",[0.0,1.0]),3:(\"three\",[1.0,1.0])").await;
    ok(&e, "INSERT EDGE link(weight) VALUES 2->3:(4)").await;
    e
}

#[tokio::test]
async fn final_commit_failure_preserves_every_graph_index_vector_counter_and_history_byte() {
    for q in [
        "UPDATE VERTEX ON item 2 SET name=\"changed\",emb=[0.5,0.5]",
        "DELETE VERTEX 2",
        "INSERT VERTEX item(name,emb) VALUES 2:(\"changed\",[0.5,0.5])",
        "INSERT EDGE link(weight) VALUES 2->3:(7)",
        "DELETE EDGE link 2->3",
        "INSERT EDGE sameAs() VALUES 2->1:()",
    ] {
        let store = Arc::new(FaultStore::new());
        let e = setup(store.clone()).await;
        let before = snapshot(store.as_ref()).await;
        store.faults.fail_assertion.store(true, Ordering::SeqCst);
        assert!(query(&e, q).await.is_err(), "{q}");
        assert_eq!(snapshot(store.as_ref()).await, before, "{q}");
    }
}

#[tokio::test]
async fn successful_dml_uses_one_authoritative_batch_for_all_side_effects() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    for q in [
        "UPDATE VERTEX ON item 2 SET name=\"changed\",emb=[0.5,0.5]",
        "INSERT EDGE link(weight) VALUES 2->3:(7)",
        "INSERT EDGE sameAs() VALUES 2->1:()",
    ] {
        store.faults.commits.store(0, Ordering::SeqCst);
        ok(&e, q).await;
        assert_eq!(store.faults.commits.load(Ordering::SeqCst), 1, "{q}");
    }
}

#[tokio::test]
async fn late_sameas_scan_failure_rolls_back_staged_vertex_edge_and_history_changes() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    let before = snapshot(store.as_ref()).await;
    *store.faults.failed_scan.lock().unwrap() =
        Some(SchemaKey::sameas_members_prefix("default", 2));
    assert!(query(&e, "INSERT EDGE sameAs() VALUES 2->1:()")
        .await
        .is_err());
    assert_eq!(snapshot(store.as_ref()).await, before);
}

#[tokio::test]
async fn index_metadata_read_failure_cannot_commit_unindexed_data() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    let before = snapshot(store.as_ref()).await;
    *store.faults.failed_scan.lock().unwrap() = Some(b"__meta:index_def:1:".to_vec());
    for q in [
        "INSERT VERTEX item(name) VALUES 9:(\"nine\")",
        "UPDATE VERTEX ON item 2 SET name=\"changed\"",
        "DELETE VERTEX 2",
        "INSERT EDGE link(weight) VALUES 1->3:(7)",
        "DELETE EDGE link 2->3",
    ] {
        assert!(query(&e, q).await.is_err(), "{q}");
        assert_eq!(snapshot(store.as_ref()).await, before, "{q}");
    }
}

#[tokio::test]
async fn corrupt_index_definition_and_sameas_edge_fail_without_any_mutation() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    store
        .put(b"__meta:index_def:1:999", b"broken")
        .await
        .unwrap();
    let before = snapshot(store.as_ref()).await;
    assert!(query(&e, "INSERT EDGE link(weight) VALUES 1->3:(8)")
        .await
        .is_err());
    assert_eq!(snapshot(store.as_ref()).await, before);
    store.delete(b"__meta:index_def:1:999").await.unwrap();
    store
        .put(b"default:edge:2:link:3:0", b"broken")
        .await
        .unwrap();
    let before = snapshot(store.as_ref()).await;
    assert!(query(&e, "INSERT EDGE sameAs() VALUES 2->1:()")
        .await
        .is_err());
    assert_eq!(snapshot(store.as_ref()).await, before);
}

#[tokio::test]
async fn sameas_zero_exact_and_over_budget_are_atomic() {
    for cap in [0, 1, 2] {
        let store = Arc::new(FaultStore::new());
        let setup_e = setup(store.clone()).await;
        drop(setup_e);
        let e = executor(store.clone(), cap);
        let before = snapshot(store.as_ref()).await;
        let result = query(&e, "INSERT EDGE sameAs() VALUES 3->2:(),2->1:()").await;
        if cap == 1 {
            assert!(matches!(
                result,
                Err(crate::error::ExecutionError::ResourceExhausted(_))
            ));
            assert_eq!(snapshot(store.as_ref()).await, before);
        } else {
            result.unwrap();
            assert_eq!(
                crate::ontology::representative_of(&e.ctx, "default", 3)
                    .await
                    .unwrap(),
                1
            );
            assert_eq!(
                crate::ontology::representative_of(&e.ctx, "default", 2)
                    .await
                    .unwrap(),
                1
            );
        }
    }
}

#[tokio::test]
async fn sameas_moves_indexes_vectors_counters_and_asserted_history() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    ok(&e, "INSERT EDGE sameAs() VALUES 2->1:()").await;
    assert!(store.get(b"default:vertex:2").await.unwrap().is_none());
    assert!(store
        .get(&SchemaKey::vec_data("default", "emb", 2))
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get(b"default:edge:1:link:3:0")
        .await
        .unwrap()
        .is_some());
    assert!(store
        .get(b"default:edge:2:link:3:0")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get(&SchemaKey::outdeg_counter("default", "link", 2))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .get(&SchemaKey::outdeg_counter("default", "link", 1))
            .await
            .unwrap()
            .unwrap(),
        SchemaKey::encode_count(1)
    );
    assert!(store.scan_history(b"default:vertex:2").await.unwrap()[0]
        .value
        .is_empty());
    assert!(store
        .scan_history(b"default:edge:2:link:3:0")
        .await
        .unwrap()[0]
        .value
        .is_empty());
    let now = store
        .get(b"default:edge:1:link:3:0")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .scan_history(b"default:edge:1:link:3:0")
            .await
            .unwrap()[0]
            .value,
        now
    );
    assert!(query(&e, "LOOKUP ON item WHERE item.name == \"two\"")
        .await
        .unwrap()
        .rows
        .is_empty());
    let rows = query(&e, "LOOKUP ON link WHERE link.weight == 4")
        .await
        .unwrap()
        .rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int(1));
}

#[tokio::test]
async fn sameas_alias_writes_target_canonical_records() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    ok(&e, "INSERT EDGE sameAs() VALUES 2->1:()").await;
    ok(
        &e,
        "INSERT VERTEX item(name,emb) VALUES 2:(\"updated\",[0.25,0.75])",
    )
    .await;
    ok(&e, "INSERT EDGE link(weight) VALUES 2->3:(6)").await;
    assert!(store.get(b"default:vertex:2").await.unwrap().is_none());
    assert!(store
        .get(b"default:edge:2:link:3:0")
        .await
        .unwrap()
        .is_none());
    let vertex = crate::record_decode::decode_vertex(
        1,
        &store.get(b"default:vertex:1").await.unwrap().unwrap(),
    )
    .unwrap();
    assert_eq!(
        vertex.tags[0].properties.get("name"),
        Some(&Value::String("updated".into()))
    );
    let edge = crate::algo::decode_query_edge(
        b"default:edge:1:link:3:0",
        &store
            .get(b"default:edge:1:link:3:0")
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(edge.properties.get("weight"), Some(&Value::Int(6)));
}

#[tokio::test]
async fn duplicate_overwrites_remove_stale_property_indexes_and_dense_vectors() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    ok(
        &e,
        "INSERT VERTEX item(name) VALUES 2:(\"intermediate\"),2:(\"final\")",
    )
    .await;
    assert!(store
        .get(&SchemaKey::vec_data("default", "emb", 2))
        .await
        .unwrap()
        .is_none());
    let tag_index = e
        .mutation_indexes("default", byoridb_storage::IndexType::Tag)
        .await
        .unwrap()
        .remove(0);
    for name in ["two", "intermediate"] {
        let key = byoridb_storage::KeyUtils::tag_index_key(
            1,
            tag_index.id,
            &[byoridb_storage::IndexValue::String(name.into())],
            2,
        );
        assert!(store.get(&key).await.unwrap().is_none());
        assert!(
            query(&e, &format!("LOOKUP ON item WHERE item.name == \"{name}\""))
                .await
                .unwrap()
                .rows
                .is_empty()
        );
    }
    assert_eq!(
        query(&e, "LOOKUP ON item WHERE item.name == \"final\"")
            .await
            .unwrap()
            .rows
            .len(),
        1
    );
    ok(&e, "INSERT EDGE link(weight) VALUES 2->3:(5),2->3:(6)").await;
    let indexes = e
        .mutation_indexes("default", byoridb_storage::IndexType::Edge)
        .await
        .unwrap();
    for weight in [4, 5] {
        let key = byoridb_storage::KeyUtils::edge_index_key(
            1,
            indexes[0].id,
            &[byoridb_storage::IndexValue::Int(weight)],
            2,
            0,
            3,
        );
        assert!(store.get(&key).await.unwrap().is_none());
    }
}

#[tokio::test]
async fn sameas_collision_preserves_winner_assertion_properties_and_count() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    ok(&e, "INSERT EDGE link(weight) VALUES 1->3:(9)").await;
    ok(&e, "INSERT EDGE sameAs() VALUES 2->1:()").await;
    let edge = crate::algo::decode_query_edge(
        b"default:edge:1:link:3:0",
        &store
            .get(b"default:edge:1:link:3:0")
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(edge.properties.get("weight"), Some(&Value::Int(9)));
    assert_eq!(
        store
            .get(&SchemaKey::outdeg_counter("default", "link", 1))
            .await
            .unwrap()
            .unwrap(),
        SchemaKey::encode_count(1)
    );
    assert!(query(&e, "LOOKUP ON link WHERE link.weight == 4")
        .await
        .unwrap()
        .rows
        .is_empty());
    assert_eq!(
        query(&e, "LOOKUP ON link WHERE link.weight == 9")
            .await
            .unwrap()
            .rows
            .len(),
        1
    );
}

#[tokio::test]
async fn staging_memory_overflow_preserves_authoritative_state() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    drop(e);
    let mut ctx = ExecutionContext::new(store.clone()).with_space("default".into());
    ctx.config.max_memory_mb = 1;
    let e = Executor::new(Arc::new(ctx));
    let before = snapshot(store.as_ref()).await;
    let q = format!(
        "INSERT VERTEX item(name) VALUES 9:(\"{}\")",
        "x".repeat(1024 * 1024)
    );
    assert!(query(&e, &q)
        .await
        .unwrap_err()
        .to_string()
        .contains("staging exceeds"));
    assert_eq!(snapshot(store.as_ref()).await, before);
}

#[tokio::test]
async fn deleting_dense_property_invalidates_the_persisted_vector_index() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    let dirty = SchemaKey::vec_index_dirty("default", "emb");
    store.delete(&dirty).await.unwrap();
    ok(&e, "UPDATE VERTEX ON item 2 SET emb=\"removed\"").await;
    assert!(store
        .get(&SchemaKey::vec_data("default", "emb", 2))
        .await
        .unwrap()
        .is_none());
    assert!(store.get(&dirty).await.unwrap().is_some());
}

#[tokio::test]
async fn concurrent_index_creation_waits_for_staged_dml_and_indexes_final_data() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    store.faults.pause_assertion.store(true, Ordering::SeqCst);
    let s = store.clone();
    let writer = tokio::spawn(async move {
        query(
            &executor(s, 0),
            "INSERT VERTEX item(name) VALUES 9:(\"nine\")",
        )
        .await
    });
    store.faults.committed.notified().await;
    let s = store.clone();
    let mut ddl = tokio::spawn(async move {
        query(
            &executor(s, 0),
            "CREATE TAG INDEX second_name ON item(name)",
        )
        .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), &mut ddl)
            .await
            .is_err()
    );
    store.faults.resume.notify_one();
    writer.await.unwrap().unwrap();
    ddl.await.unwrap().unwrap();
    let indexes = e
        .mutation_indexes("default", byoridb_storage::IndexType::Tag)
        .await
        .unwrap();
    let index = indexes
        .into_iter()
        .find(|index| index.index_name == "second_name")
        .unwrap();
    let key = byoridb_storage::KeyUtils::tag_index_key(
        1,
        index.id,
        &[byoridb_storage::IndexValue::String("nine".into())],
        9,
    );
    assert!(store.get(&key).await.unwrap().is_some());
}

#[tokio::test]
async fn cancellation_before_final_commit_discards_all_staged_sameas_work() {
    let store = Arc::new(FaultStore::new());
    let _e = setup(store.clone()).await;
    let before = snapshot(store.as_ref()).await;
    *store.faults.pause_scan_prefix.lock().unwrap() =
        Some(SchemaKey::sameas_members_prefix("default", 2));
    store.faults.commits.store(0, Ordering::SeqCst);
    let s = store.clone();
    let writer =
        tokio::spawn(
            async move { query(&executor(s, 0), "INSERT EDGE sameAs() VALUES 2->1:()").await },
        );
    store.faults.committed.notified().await;
    writer.abort();
    assert!(writer.await.unwrap_err().is_cancelled());
    assert_eq!(store.faults.commits.load(Ordering::SeqCst), 0);
    assert_eq!(snapshot(store.as_ref()).await, before);
}

#[tokio::test]
async fn counter_fault_malformed_payload_preserves_the_complete_statement() {
    for bytes in [Vec::new(), vec![0; 7], vec![0; 9]] {
        let store = Arc::new(FaultStore::new());
        let e = setup(store.clone()).await;
        store
            .put(&SchemaKey::outdeg_counter("default", "link", 1), &bytes)
            .await
            .unwrap();
        let before = snapshot(store.as_ref()).await;
        assert!(query(&e, "INSERT EDGE link(weight) VALUES 1->3:(8)")
            .await
            .is_err());
        assert_eq!(snapshot(store.as_ref()).await, before);
    }
}

#[tokio::test]
async fn counter_fault_overflow_preserves_the_complete_statement() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    store
        .put(
            &SchemaKey::outdeg_counter("default", "link", 1),
            &i64::MAX.to_le_bytes(),
        )
        .await
        .unwrap();
    let before = snapshot(store.as_ref()).await;
    assert!(query(&e, "INSERT EDGE link(weight) VALUES 1->3:(8)")
        .await
        .is_err());
    assert_eq!(snapshot(store.as_ref()).await, before);
}

#[tokio::test]
async fn counter_fault_underflow_preserves_the_complete_statement() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    store
        .put(
            &SchemaKey::outdeg_counter("default", "link", 2),
            &i64::MIN.to_le_bytes(),
        )
        .await
        .unwrap();
    let before = snapshot(store.as_ref()).await;
    assert!(query(&e, "DELETE EDGE link 2->3").await.is_err());
    assert_eq!(snapshot(store.as_ref()).await, before);
}

#[tokio::test]
async fn nonpositive_counter_result_still_deletes_counter_key() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    let key = SchemaKey::outdeg_counter("default", "link", 2);
    store.put(&key, &0i64.to_le_bytes()).await.unwrap();
    ok(&e, "DELETE EDGE link 2->3").await;
    assert!(store.get(&key).await.unwrap().is_none());
}

#[tokio::test]
async fn sameas_source_alias_delete_removes_canonical_edge_once() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    ok(&e, "INSERT EDGE sameAs() VALUES 2->1:()").await;
    let result = query(&e, "DELETE EDGE link 2->3").await.unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(1)]]);
    assert!(store
        .get(b"default:edge:1:link:3:0")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get(b"default:in-edge:3:link:1:0")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .scan_history(b"default:edge:1:link:3:0")
        .await
        .unwrap()[0]
        .value
        .is_empty());
    assert!(store
        .get(&SchemaKey::outdeg_counter("default", "link", 1))
        .await
        .unwrap()
        .is_none());
    ok(&e, "INSERT EDGE link(weight) VALUES 1->3:(4)").await;
    let result = query(&e, "DELETE EDGE link 2->3,1->3").await.unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(1)]]);
}

#[tokio::test]
async fn sameas_destination_alias_delete_removes_canonical_edge() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    ok(&e, "INSERT EDGE link(weight) VALUES 3->2:(8)").await;
    ok(&e, "INSERT EDGE sameAs() VALUES 2->1:()").await;
    let result = query(&e, "DELETE EDGE link 3->2").await.unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(1)]]);
    assert!(store
        .get(b"default:edge:3:link:1:0")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get(b"default:in-edge:1:link:3:0")
        .await
        .unwrap()
        .is_none());
    assert!(query(&e, "LOOKUP ON link WHERE link.weight == 8")
        .await
        .unwrap()
        .rows
        .is_empty());
}

#[tokio::test]
async fn sameas_reserved_edge_deletion_is_still_rejected() {
    let store = Arc::new(FaultStore::new());
    let e = setup(store.clone()).await;
    ok(&e, "INSERT EDGE sameAs() VALUES 2->1:()").await;
    let before = snapshot(store.as_ref()).await;
    assert!(query(&e, "DELETE EDGE sameAs 2->1")
        .await
        .unwrap_err()
        .to_string()
        .contains("irreversible"));
    assert_eq!(snapshot(store.as_ref()).await, before);
}
