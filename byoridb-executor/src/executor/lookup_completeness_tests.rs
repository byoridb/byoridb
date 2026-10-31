// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! LOOKUP must not present budget exhaustion, index failures or corrupt data
//! as a complete query result. Query LIMIT/OFFSET is separate from the budget.

use super::{Executor, ExecutorResult};
use crate::context::{ExecutionConfig, ExecutionContext};
use crate::error::ExecutionError;
use crate::plan::{ExecutionPlan, ExecutionPlanBuilder};
use async_trait::async_trait;
use byoridb_codec::{EdgeData, TagData, VertexCodec, VertexData};
use byoridb_common::Value;
use byoridb_kvstore::{KVStore, KVStoreError, MemoryKVStore, Result as KvResult, VersionRecord};
use byoridb_storage::key::{IndexValue, KeyUtils};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct FailingScanStore {
    inner: MemoryKVStore,
    fail_prefix: Mutex<Option<Vec<u8>>>,
    fail_get: Mutex<Option<(Vec<u8>, usize)>>,
    matching_gets: AtomicUsize,
    failures: AtomicUsize,
}

#[async_trait]
impl KVStore for FailingScanStore {
    async fn get(&self, key: &[u8]) -> KvResult<Option<Vec<u8>>> {
        if let Some((failed_key, fail_on_call)) = self.fail_get.lock().unwrap().as_ref() {
            if failed_key.as_slice() == key
                && self.matching_gets.fetch_add(1, Ordering::SeqCst) + 1 == *fail_on_call
            {
                self.failures.fetch_add(1, Ordering::SeqCst);
                return Err(KVStoreError::StoreClosed);
            }
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
        if self
            .fail_prefix
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|failed| prefix.starts_with(failed))
        {
            self.failures.fetch_add(1, Ordering::SeqCst);
            return Err(KVStoreError::StoreClosed);
        }
        self.inner.scan_prefix(prefix).await
    }
    async fn scan_range(
        &self,
        start: &[u8],
        end: &[u8],
        limit: Option<usize>,
    ) -> KvResult<Vec<(Vec<u8>, Vec<u8>)>> {
        if self
            .fail_prefix
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|failed| start.starts_with(failed))
        {
            self.failures.fetch_add(1, Ordering::SeqCst);
            return Err(KVStoreError::StoreClosed);
        }
        self.inner.scan_range(start, end, limit).await
    }
    async fn batch_get(&self, keys: &[Vec<u8>]) -> KvResult<Vec<Option<Vec<u8>>>> {
        self.inner.batch_get(keys).await
    }
}

#[derive(Clone, Copy, Debug)]
enum Route {
    TagScan,
    TagEquality,
    TagRange,
    EdgeScan,
    EdgeEquality,
    EdgeRange,
}

const ROUTES: [Route; 6] = [
    Route::TagScan,
    Route::TagEquality,
    Route::TagRange,
    Route::EdgeScan,
    Route::EdgeEquality,
    Route::EdgeRange,
];
const INDEX_ROUTES: [Route; 4] = [
    Route::TagEquality,
    Route::TagRange,
    Route::EdgeEquality,
    Route::EdgeRange,
];

impl Route {
    fn edge(self) -> bool {
        matches!(self, Self::EdgeScan | Self::EdgeEquality | Self::EdgeRange)
    }
    fn indexed(self) -> bool {
        !matches!(self, Self::TagScan | Self::EdgeScan)
    }
    fn name(self) -> &'static str {
        if self.edge() {
            "link"
        } else {
            "item"
        }
    }
    fn query(self) -> String {
        let predicate = match self {
            Self::TagScan | Self::EdgeScan => "",
            Self::TagEquality | Self::EdgeEquality => " WHERE score == 10",
            Self::TagRange | Self::EdgeRange => " WHERE score > 0",
        };
        format!("LOOKUP ON {}{predicate}", self.name())
    }
    fn data_key(self, id: i64) -> Vec<u8> {
        if self.edge() {
            crate::key::SchemaKey::edge_data("default", id, "link", id + 100, 0)
        } else {
            crate::key::SchemaKey::vertex("default", id)
        }
    }
    fn index_prefix(self, partition: u32, index: u32) -> Vec<u8> {
        if self.edge() {
            KeyUtils::edge_index_prefix(partition, index)
        } else {
            KeyUtils::tag_index_prefix(partition, index)
        }
    }
}

struct Fixture {
    store: Arc<FailingScanStore>,
    ctx: Arc<ExecutionContext>,
    route: Route,
    index_id: Option<u32>,
}

impl Fixture {
    async fn new(route: Route, cap: usize, partitions: u32) -> Self {
        let store = Arc::new(FailingScanStore {
            inner: MemoryKVStore::new(),
            fail_prefix: Mutex::new(None),
            fail_get: Mutex::new(None),
            matching_gets: AtomicUsize::new(0),
            failures: AtomicUsize::new(0),
        });
        let mut ctx = ExecutionContext::new(store.clone())
            .with_space("default".to_string())
            .with_space_id(1)
            .with_config(ExecutionConfig {
                max_scan_limit: cap,
                ..ExecutionConfig::default()
            });
        ctx.partition_num = Some(partitions);
        let ctx = Arc::new(ctx);
        let schema_key = if route.edge() {
            crate::key::SchemaKey::edge("default", route.name())
        } else {
            crate::key::SchemaKey::tag("default", route.name())
        };
        store.put(&schema_key, &serde_json::to_vec(&serde_json::json!({ "name": route.name(), "properties": [{"name": "score", "data_type": "Int64"}] })).unwrap()).await.unwrap();
        let manager = ctx.index_manager.as_ref().unwrap();
        let index_id = if !route.indexed() {
            None
        } else if route.edge() {
            Some(
                manager
                    .create_edge_index(
                        1,
                        "score_idx".to_string(),
                        1,
                        route.name().to_string(),
                        vec!["score".to_string()],
                        vec![0],
                    )
                    .await
                    .unwrap(),
            )
        } else {
            Some(
                manager
                    .create_tag_index(
                        1,
                        "score_idx".to_string(),
                        1,
                        route.name().to_string(),
                        vec!["score".to_string()],
                        vec![0],
                    )
                    .await
                    .unwrap(),
            )
        };
        Self {
            store,
            ctx,
            route,
            index_id,
        }
    }

    async fn seed(&self, id: i64, partition: u32) {
        let properties = HashMap::from([("score".to_string(), Value::Int(10))]);
        let blob = if self.route.edge() {
            VertexCodec::encode_edge(&EdgeData {
                src_vid: id,
                dst_vid: id + 100,
                edge_type: "link".to_string(),
                ranking: 0,
                properties,
            })
            .unwrap()
        } else {
            VertexCodec::encode_vertex(&VertexData {
                vid: id,
                tags: vec![TagData {
                    name: "item".to_string(),
                    properties,
                }],
            })
            .unwrap()
        };
        self.store
            .put(&self.route.data_key(id), &blob)
            .await
            .unwrap();
        if let Some(index) = self.index_id {
            let manager = self.ctx.index_manager.as_ref().unwrap();
            if self.route.edge() {
                manager
                    .insert_edge_index(partition, index, &[IndexValue::Int(10)], id, 0, id + 100)
                    .await
                    .unwrap();
            } else {
                manager
                    .insert_tag_index(partition, index, &[IndexValue::Int(10)], id)
                    .await
                    .unwrap();
            }
        }
    }

    async fn run(&self, suffix: &str) -> crate::error::Result<ExecutorResult> {
        let statement = byoridb_parser::parse(&format!("{}{suffix}", self.route.query())).unwrap();
        let ExecutionPlan::Lookup(plan) = ExecutionPlanBuilder::build(statement).unwrap() else {
            panic!("LOOKUP plan required")
        };
        Executor::new(self.ctx.clone()).execute_lookup(plan).await
    }
}

fn assert_exhausted(result: crate::error::Result<ExecutorResult>, route: Route) {
    assert!(
        matches!(result, Err(ExecutionError::ResourceExhausted(_))),
        "{route:?} must fail instead of returning partial success: {result:?}"
    );
}

async fn cap_boundaries(route: Route) {
    for size in [1, 2, 3] {
        let fixture = Fixture::new(route, 2, 1).await;
        for id in 1..=size {
            fixture.seed(id, 1).await;
        }
        let result = fixture.run("").await;
        if size <= 2 {
            assert_eq!(result.unwrap().rows.len(), size as usize, "{route:?}");
        } else {
            assert_exhausted(result, route);
        }
    }
}

#[tokio::test]
async fn tag_scan_cap_boundary() {
    cap_boundaries(Route::TagScan).await;
}
#[tokio::test]
async fn tag_equality_cap_boundary() {
    cap_boundaries(Route::TagEquality).await;
}
#[tokio::test]
async fn tag_range_cap_boundary() {
    cap_boundaries(Route::TagRange).await;
}
#[tokio::test]
async fn edge_scan_cap_boundary() {
    cap_boundaries(Route::EdgeScan).await;
}
#[tokio::test]
async fn edge_equality_cap_boundary() {
    cap_boundaries(Route::EdgeEquality).await;
}
#[tokio::test]
async fn edge_range_cap_boundary() {
    cap_boundaries(Route::EdgeRange).await;
}

#[tokio::test]
async fn zero_scan_cap_and_offset_only_keep_all_remaining_rows() {
    for route in ROUTES {
        let fixture = Fixture::new(route, 0, 1).await;
        for id in 1..=3 {
            fixture.seed(id, 1).await;
        }
        let rows = fixture.run(" OFFSET 1").await.unwrap().rows;
        assert_eq!(
            rows.iter().map(|row| row[0].clone()).collect::<Vec<_>>(),
            vec![Value::Int(2), Value::Int(3)],
            "{route:?}"
        );
    }
}

#[tokio::test]
async fn explicit_query_window_is_preserved_under_the_budget() {
    for route in ROUTES {
        let fixture = Fixture::new(route, 3, 1).await;
        for id in 1..=3 {
            fixture.seed(id, 1).await;
        }
        let rows = fixture.run(" LIMIT 1 OFFSET 1").await.unwrap().rows;
        assert_eq!(rows.len(), 1, "{route:?}");
        assert_eq!(rows[0][0], Value::Int(2), "{route:?}");
        assert!(fixture.run(" LIMIT 0").await.unwrap().rows.is_empty());
    }
}

#[tokio::test]
async fn explicit_limits_do_not_hide_or_raise_the_scan_budget() {
    for route in ROUTES {
        let fixture = Fixture::new(route, 2, 1).await;
        for id in 1..=3 {
            fixture.seed(id, 1).await;
        }
        for suffix in [" LIMIT 1", " LIMIT 1000000000 OFFSET 1", " OFFSET 1"] {
            assert_exhausted(fixture.run(suffix).await, route);
        }
    }
}

#[tokio::test]
async fn unrelated_prefix_rows_consume_the_full_scan_budget() {
    for route in [Route::TagScan, Route::EdgeScan] {
        let fixture = Fixture::new(route, 2, 1).await;
        fixture.seed(1, 1).await;
        for id in [2, 3] {
            let blob = if route.edge() {
                VertexCodec::encode_edge(&EdgeData {
                    src_vid: id,
                    dst_vid: id + 100,
                    edge_type: "other".to_string(),
                    ranking: 0,
                    properties: HashMap::new(),
                })
                .unwrap()
            } else {
                VertexCodec::encode_vertex(&VertexData {
                    vid: id,
                    tags: vec![TagData {
                        name: "other".to_string(),
                        properties: HashMap::new(),
                    }],
                })
                .unwrap()
            };
            fixture.store.put(&route.data_key(id), &blob).await.unwrap();
        }
        assert_exhausted(fixture.run(" LIMIT 1").await, route);
    }
}

#[tokio::test]
async fn index_partition_failures_are_errors_even_when_full_scan_could_succeed() {
    for route in INDEX_ROUTES {
        for failed_partition in [1, 2] {
            let fixture = Fixture::new(route, 0, 2).await;
            fixture.seed(1, 1).await;
            fixture.seed(2, 2).await;
            *fixture.store.fail_prefix.lock().unwrap() =
                Some(route.index_prefix(failed_partition, fixture.index_id.unwrap()));
            let result = fixture.run("").await;
            assert_eq!(
                fixture.store.failures.load(Ordering::SeqCst),
                1,
                "{route:?}"
            );
            assert!(
                matches!(&result, Err(ExecutionError::Storage(byoridb_storage::StorageError::Index(message))) if message.contains(&KVStoreError::StoreClosed.to_string())),
                "{route:?}: {result:?}"
            );
        }
    }
}

#[tokio::test]
async fn index_partitions_are_complete_with_independent_scan_budgets() {
    for route in INDEX_ROUTES {
        let fixture = Fixture::new(route, 2, 2).await;
        for (id, part) in [(1, 1), (2, 1), (3, 2), (4, 2)] {
            fixture.seed(id, part).await;
        }
        assert_eq!(fixture.run("").await.unwrap().rows.len(), 4, "{route:?}");
    }
}

#[tokio::test]
async fn malformed_blobs_do_not_become_no_match() {
    for route in ROUTES {
        for corrupt in [b"not-json".as_slice(), &[0xca, 0xff]] {
            let fixture = Fixture::new(route, 0, 1).await;
            fixture.seed(1, 1).await;
            fixture.seed(2, 1).await;
            fixture
                .store
                .put(&route.data_key(2), corrupt)
                .await
                .unwrap();
            let result = fixture.run("").await;
            assert!(
                result.is_err(),
                "{route:?} dropped malformed blob after a valid row: {result:?}"
            );
        }
    }
}

#[tokio::test]
async fn malformed_edge_index_key_cannot_hide_cap_exhaustion() {
    let fixture = Fixture::new(Route::EdgeEquality, 2, 1).await;
    for id in 1..=3 {
        fixture.seed(id, 1).await;
    }
    let malformed = KeyUtils::edge_index_prefix_with_values(
        1,
        fixture.index_id.unwrap(),
        &[IndexValue::Int(10)],
    );
    fixture.store.put(&malformed, &[]).await.unwrap();
    let result = fixture.run("").await;
    assert!(
        matches!(&result, Err(ExecutionError::Storage(byoridb_storage::StorageError::Index(message))) if message.contains("malformed edge-index key")),
        "malformed index key hid cap: {result:?}"
    );
}

#[tokio::test]
async fn malformed_vertex_key_does_not_invent_vid_zero() {
    let fixture = Fixture::new(Route::TagScan, 0, 1).await;
    fixture
        .store
        .put(
            b"default:vertex:not-an-id",
            br#"{"tags":[{"name":"item","props":{"score":10}}]}"#,
        )
        .await
        .unwrap();
    let result = fixture.run("").await;
    assert!(
        result.is_err(),
        "malformed vertex key became a row: {result:?}"
    );
}

#[tokio::test]
async fn zero_partition_count_is_rejected() {
    for route in INDEX_ROUTES {
        let fixture = Fixture::new(route, 0, 0).await;
        fixture.seed(1, 1).await;
        let result = fixture.run("").await;
        assert!(
            matches!(result, Err(ExecutionError::InvalidOperation(_))),
            "zero partitions must not silently become one: {route:?} {result:?}"
        );
    }
}

#[tokio::test]
async fn range_schema_read_failure_is_not_an_unindexed_fallback() {
    let fixture = Fixture::new(Route::TagRange, 0, 1).await;
    fixture.seed(1, 1).await;
    // The first read selects the tag route; the second determines the range
    // index domain. Only failing the second read distinguishes propagation
    // from the old metadata `.ok()??` fallback to a successful full scan.
    *fixture.store.fail_get.lock().unwrap() =
        Some((crate::key::SchemaKey::tag("default", "item"), 2));
    let result = fixture.run("").await;
    assert_eq!(fixture.store.failures.load(Ordering::SeqCst), 1);
    assert!(
        matches!(
            result,
            Err(ExecutionError::Storage(
                byoridb_storage::StorageError::KVStore(KVStoreError::StoreClosed)
            ))
        ),
        "schema failure became a scan result: {result:?}"
    );
}

#[tokio::test]
async fn stale_index_entries_and_true_no_match_remain_empty() {
    for route in ROUTES {
        let fixture = Fixture::new(route, 2, 1).await;
        fixture.seed(1, 1).await;
        fixture.store.delete(&route.data_key(1)).await.unwrap();
        assert!(fixture.run("").await.unwrap().rows.is_empty(), "{route:?}");
    }
}

#[tokio::test]
async fn structured_edge_json_cannot_default_missing_or_invalid_metadata() {
    for route in [Route::EdgeScan, Route::EdgeEquality, Route::EdgeRange] {
        for malformed in [
            serde_json::json!({"edge_type":"link", "props":{"score":10}}),
            serde_json::json!({}),
            serde_json::json!({"src":1, "edge_type":"link", "props":{"score":10}}),
            serde_json::json!({"src":"1", "dst":101, "edge_type":"link", "props":{"score":10}}),
            serde_json::json!({"src":1, "dst":101, "edge_type":"link", "rank":"0", "props":{"score":10}}),
            serde_json::json!({"src":1, "dst":101, "edge_type":"link", "props":7}),
        ] {
            let fixture = Fixture::new(route, 0, 1).await;
            fixture.seed(1, 1).await;
            fixture
                .store
                .put(&route.data_key(1), &serde_json::to_vec(&malformed).unwrap())
                .await
                .unwrap();
            let result = fixture.run("").await;
            assert!(
                result.is_err(),
                "{route:?} accepted malformed edge {malformed}: {result:?}"
            );
        }
    }
}

#[tokio::test]
async fn edge_payload_identity_must_match_the_stored_key() {
    for route in [Route::EdgeScan, Route::EdgeEquality, Route::EdgeRange] {
        for (src, dst, rank, edge_type) in [
            (2, 101, 0, "link"),
            (1, 102, 0, "link"),
            (1, 101, 1, "link"),
            (1, 101, 0, "other"),
        ] {
            for proto in [false, true] {
                let fixture = Fixture::new(route, 0, 1).await;
                fixture.seed(1, 1).await;
                let blob = if proto {
                    VertexCodec::encode_edge(&EdgeData {
                        src_vid: src,
                        dst_vid: dst,
                        ranking: rank,
                        edge_type: edge_type.to_string(),
                        properties: HashMap::from([("score".to_string(), Value::Int(10))]),
                    })
                    .unwrap()
                } else {
                    serde_json::to_vec(&serde_json::json!({"src":src, "dst":dst, "rank":rank, "type":edge_type, "props":{"score":10}})).unwrap()
                };
                fixture.store.put(&route.data_key(1), &blob).await.unwrap();
                let result = fixture.run("").await;
                assert!(
                    result.is_err(),
                    "{route:?} accepted mismatched edge identity (proto={proto}): {result:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn structured_vertex_tag_metadata_is_validated_before_filtering() {
    for route in [Route::TagScan, Route::TagEquality, Route::TagRange] {
        for tags in [
            serde_json::json!([{"name":"item", "props":7}]),
            serde_json::json!([{"name":7, "props":{"score":10}}]),
            serde_json::json!([{"props":{"score":10}}]),
            serde_json::json!([null]),
            serde_json::json!([{"name":"item", "props":{"score":10}}, {"name":"other", "props":null}]),
        ] {
            let fixture = Fixture::new(route, 0, 1).await;
            fixture.seed(1, 1).await;
            let blob = serde_json::to_vec(&serde_json::json!({"tags":tags})).unwrap();
            fixture.store.put(&route.data_key(1), &blob).await.unwrap();
            let result = fixture.run("").await;
            assert!(
                result.is_err(),
                "{route:?} accepted malformed tag metadata {tags}: {result:?}"
            );
        }
    }
}

#[tokio::test]
async fn supplied_vertex_identity_must_match_the_stored_key() {
    for route in [Route::TagScan, Route::TagEquality, Route::TagRange] {
        for vid in [
            serde_json::json!(2),
            serde_json::json!("1"),
            serde_json::Value::Null,
        ] {
            let fixture = Fixture::new(route, 0, 1).await;
            fixture.seed(1, 1).await;
            let blob = serde_json::to_vec(
                &serde_json::json!({"vid":vid, "tags":[{"name":"item", "props":{"score":10}}]}),
            )
            .unwrap();
            fixture.store.put(&route.data_key(1), &blob).await.unwrap();
            let result = fixture.run("").await;
            assert!(
                result.is_err(),
                "{route:?} accepted invalid JSON VID {vid}: {result:?}"
            );
        }
        let fixture = Fixture::new(route, 0, 1).await;
        fixture.seed(1, 1).await;
        let blob = VertexCodec::encode_vertex(&VertexData {
            vid: 2,
            tags: vec![TagData {
                name: "item".to_string(),
                properties: HashMap::from([("score".to_string(), Value::Int(10))]),
            }],
        })
        .unwrap();
        fixture.store.put(&route.data_key(1), &blob).await.unwrap();
        let result = fixture.run("").await;
        assert!(
            result.is_err(),
            "{route:?} accepted mismatched protobuf VID: {result:?}"
        );
    }
}

#[tokio::test]
async fn valid_legacy_optional_vid_rank_and_props_remain_supported() {
    for route in ROUTES {
        let fixture = Fixture::new(route, 0, 1).await;
        fixture.seed(1, 1).await;
        let with_props = if route.edge() {
            serde_json::json!({"src":1, "dst":101, "type":"link", "props":{"score":10}})
        } else {
            serde_json::json!({"tags":[{"name":"item", "props":{"score":10}}]})
        };
        fixture
            .store
            .put(
                &route.data_key(1),
                &serde_json::to_vec(&with_props).unwrap(),
            )
            .await
            .unwrap();
        let rows = fixture.run("").await.unwrap().rows;
        assert_eq!(rows.len(), 1, "{route:?}");
        assert_eq!(rows[0][0], Value::Int(1));
        if route.edge() {
            assert_eq!(rows[0], vec![Value::Int(1), Value::Int(101), Value::Int(0)]);
        }

        let without_props = if route.edge() {
            serde_json::json!({"src":1, "dst":101, "type":"link"})
        } else {
            serde_json::json!({"tags":[{"name":"item"}]})
        };
        fixture
            .store
            .put(
                &route.data_key(1),
                &serde_json::to_vec(&without_props).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            fixture.run("").await.unwrap().rows.len(),
            usize::from(!route.indexed()),
            "{route:?}"
        );
    }
}

#[tokio::test]
async fn local_index_projection_enforces_result_memory_budget() {
    for route in [Route::TagEquality, Route::TagRange] {
        let mut fixture = Fixture::new(route, 0, 1).await;
        Arc::get_mut(&mut fixture.ctx).unwrap().config.max_memory_mb = 1;
        fixture.seed(1, 1).await;
        let data = VertexCodec::encode_vertex(&VertexData {
            vid: 1,
            tags: vec![TagData {
                name: "item".to_string(),
                properties: HashMap::from([
                    ("score".to_string(), Value::Int(10)),
                    ("payload".to_string(), Value::String("x".repeat(1_100_000))),
                ]),
            }],
        })
        .unwrap();
        fixture.store.put(&route.data_key(1), &data).await.unwrap();
        assert_exhausted(fixture.run("").await, route);
    }
}

#[tokio::test]
async fn local_float_equality_preserves_epsilon_signed_zero_and_type_strictness() {
    let samples = [
        Value::Float(0.0),
        Value::Float(-0.0),
        Value::Float(1e-10),
        Value::Float(1.0),
        Value::Float(1.0 + 1e-10),
        Value::Float(1.0 + 1e-6),
        Value::Int(0),
        Value::Int(1),
    ];
    for route in [Route::TagEquality, Route::EdgeEquality] {
        let mut fixture = Fixture::new(route, 0, 1).await;
        for (vid, value) in (0..).zip(samples.iter()) {
            let manager = fixture.ctx.index_manager.as_ref().unwrap();
            let id = fixture.index_id.unwrap();
            let indexed = match value {
                Value::Float(v) => IndexValue::Float(*v),
                Value::Int(v) => IndexValue::Int(*v),
                _ => unreachable!(),
            };
            let props = HashMap::from([("score".into(), value.clone())]);
            let data = if route.edge() {
                manager
                    .insert_edge_index(1, id, &[indexed], vid, 0, vid + 100)
                    .await
                    .unwrap();
                VertexCodec::encode_edge(&EdgeData {
                    src_vid: vid,
                    dst_vid: vid + 100,
                    edge_type: "link".into(),
                    ranking: 0,
                    properties: props,
                })
                .unwrap()
            } else {
                manager
                    .insert_tag_index(1, id, &[indexed], vid)
                    .await
                    .unwrap();
                VertexCodec::encode_vertex(&VertexData {
                    vid,
                    tags: vec![TagData {
                        name: "item".into(),
                        properties: props,
                    }],
                })
                .unwrap()
            };
            fixture
                .store
                .put(&route.data_key(vid), &data)
                .await
                .unwrap();
        }
        for (literal, expected) in [("0.0", 3), ("1.0", 2), ("1", 1)] {
            let plan = ExecutionPlanBuilder::build(
                byoridb_parser::parse(&format!(
                    "LOOKUP ON {} WHERE score == {literal}",
                    route.name()
                ))
                .unwrap(),
            )
            .unwrap();
            assert_eq!(
                Executor::new(fixture.ctx.clone())
                    .execute(plan)
                    .await
                    .unwrap()
                    .rows
                    .len(),
                expected,
                "{route:?} {literal}"
            );
        }
        Arc::get_mut(&mut fixture.ctx)
            .unwrap()
            .config
            .max_scan_limit = 1;
        let plan = ExecutionPlanBuilder::build(
            byoridb_parser::parse(&format!(
                "LOOKUP ON {} WHERE score == 0.0 LIMIT 1",
                route.name()
            ))
            .unwrap(),
        )
        .unwrap();
        assert_exhausted(
            Executor::new(fixture.ctx.clone()).execute(plan).await,
            route,
        );
    }
}
