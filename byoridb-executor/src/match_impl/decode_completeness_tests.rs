// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! MATCH must validate records it consumes, including unfiltered and shortcut paths.

use super::MatchExecutor;
use crate::context::ExecutionContext;
use crate::error::{ExecutionError, Result};
use crate::executor::ExecutorResult;
use crate::plan::{ExecutionPlan, ExecutionPlanBuilder};
use async_trait::async_trait;
use byoridb_codec::{EdgeData, TagData, VertexCodec, VertexData};
use byoridb_common::Value;
use byoridb_kvstore::store::ScanStreamItem;
use byoridb_kvstore::{KVStore, KVStoreError, MemoryKVStore, Result as KvResult, VersionRecord};
use byoridb_storage::key::IndexValue;
use futures::{stream, stream::BoxStream};
use std::sync::{Arc, Mutex};

type GetOverride = (Vec<u8>, Option<Vec<u8>>);

struct FaultStore {
    inner: MemoryKVStore,
    failed_get: Mutex<Option<Vec<u8>>>,
    overridden_get: Mutex<Option<GetOverride>>,
    failed_scan: Mutex<Option<Vec<u8>>>,
}

#[async_trait]
impl KVStore for FaultStore {
    async fn get(&self, key: &[u8]) -> KvResult<Option<Vec<u8>>> {
        if self.failed_get.lock().unwrap().as_deref() == Some(key) {
            return Err(KVStoreError::StoreClosed);
        }
        if let Some((overridden_key, value)) = self.overridden_get.lock().unwrap().as_ref() {
            if overridden_key.as_slice() == key {
                return Ok(value.clone());
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
        if self.failed_scan.lock().unwrap().as_deref() == Some(prefix) {
            return Err(KVStoreError::StoreClosed);
        }
        self.inner.scan_prefix(prefix).await
    }
    async fn scan_stream(&self, prefix: &[u8]) -> KvResult<BoxStream<'static, ScanStreamItem>> {
        let mut rows: Vec<ScanStreamItem> = self
            .inner
            .scan_prefix(prefix)
            .await?
            .into_iter()
            .map(Ok)
            .collect();
        if self.failed_scan.lock().unwrap().as_deref() == Some(prefix) {
            rows.push(Err(KVStoreError::StoreClosed));
        }
        Ok(Box::pin(stream::iter(rows)))
    }
    async fn batch_get(&self, keys: &[Vec<u8>]) -> KvResult<Vec<Option<Vec<u8>>>> {
        let mut result = Vec::with_capacity(keys.len());
        for key in keys {
            result.push(self.get(key).await?);
        }
        Ok(result)
    }
}

fn context() -> (Arc<FaultStore>, Arc<ExecutionContext>) {
    let store = Arc::new(FaultStore {
        inner: MemoryKVStore::new(),
        failed_get: Mutex::new(None),
        overridden_get: Mutex::new(None),
        failed_scan: Mutex::new(None),
    });
    let ctx = Arc::new(
        ExecutionContext::new(store.clone())
            .with_space("default".into())
            .with_space_id(1),
    );
    (store, ctx)
}

async fn query(ctx: Arc<ExecutionContext>, query: &str) -> Result<ExecutorResult> {
    let statement = byoridb_parser::parse(query).unwrap();
    let ExecutionPlan::Match(plan) = ExecutionPlanBuilder::build(statement).unwrap() else {
        panic!("expected MATCH")
    };
    MatchExecutor::new(ctx).execute_match(plan).await
}

fn vertex(vid: i64) -> Vec<u8> {
    VertexCodec::encode_vertex(&VertexData {
        vid,
        tags: vec![TagData {
            name: "item".into(),
            properties: [("age".into(), Value::Int(10))].into_iter().collect(),
        }],
    })
    .unwrap()
}

fn edge(src: i64, dst: i64) -> Vec<u8> {
    VertexCodec::encode_edge(&EdgeData {
        src_vid: src,
        dst_vid: dst,
        edge_type: "link".into(),
        ranking: 0,
        properties: Default::default(),
    })
    .unwrap()
}

async fn seed(ctx: &ExecutionContext) {
    for vid in [0, 1, 2] {
        ctx.kvstore
            .put(format!("default:vertex:{vid}").as_bytes(), &vertex(vid))
            .await
            .unwrap();
    }
    let blob = edge(1, 2);
    ctx.kvstore
        .put(b"default:edge:1:link:2:0", &blob)
        .await
        .unwrap();
    ctx.kvstore
        .put(b"default:in-edge:2:link:1:0", &blob)
        .await
        .unwrap();
}

fn assert_invalid(result: Result<ExecutorResult>) {
    assert!(
        matches!(
            result,
            Err(ExecutionError::InvalidOperation(_)
                | ExecutionError::Serialization(_)
                | ExecutionError::Storage(byoridb_storage::StorageError::DecodingError(_)))
        ),
        "invalid consumed metadata must fail: {result:?}"
    );
}

#[tokio::test]
async fn direct_id_requires_a_present_vertex() {
    let (_, ctx) = context();
    assert!(query(ctx, "MATCH (n) WHERE id(n) == 123 RETURN id(n)")
        .await
        .unwrap()
        .rows
        .is_empty());
}

#[tokio::test]
async fn direct_id_propagates_get_failure() {
    let (store, ctx) = context();
    *store.failed_get.lock().unwrap() = Some(b"default:vertex:1".to_vec());
    assert!(query(ctx, "MATCH (n) WHERE id(n) == 1 RETURN id(n)")
        .await
        .is_err());
}

#[tokio::test]
async fn direct_id_validates_unfiltered_vertex() {
    for blob in [
        b"not json".to_vec(),
        vertex(99),
        br#"{"vid":1,"tags":[{}]}"#.to_vec(),
    ] {
        let (_, ctx) = context();
        ctx.kvstore.put(b"default:vertex:1", &blob).await.unwrap();
        assert_invalid(query(ctx, "MATCH (n) WHERE id(n) == 1 RETURN id(n)").await);
    }
}

#[tokio::test]
async fn full_scan_validates_unfiltered_and_filtered_vertices() {
    for suffix in ["", ":item"] {
        for blob in [
            b"not json".to_vec(),
            vertex(99),
            br#"{"vid":1,"tags":[{"name":"item","props":[]}]}"#.to_vec(),
        ] {
            let (_, ctx) = context();
            ctx.kvstore.put(b"default:vertex:1", &blob).await.unwrap();
            assert_invalid(query(ctx, &format!("MATCH (n{suffix}) RETURN id(n)")).await);
        }
    }
}

#[tokio::test]
async fn malformed_full_scan_vertex_keys_fail() {
    for key in [
        b"default:vertex:bad".as_slice(),
        b"default:vertex:1:extra",
        b"default:vertex:+1",
        b"default:vertex:01",
        b"default:vertex:\xff",
    ] {
        let (_, ctx) = context();
        ctx.kvstore.put(key, &vertex(1)).await.unwrap();
        assert_invalid(query(ctx, "MATCH (n) RETURN id(n)").await);
    }
}

#[tokio::test]
async fn malformed_tagvid_keys_fail() {
    for key in [
        b"default:tagvid:item:bad".as_slice(),
        b"default:tagvid:item:extra:1",
        b"default:tagvid:item:01",
        b"default:tagvid:item:\xff",
    ] {
        let (_, ctx) = context();
        ctx.kvstore
            .put(b"default:vertex:1", &vertex(1))
            .await
            .unwrap();
        ctx.kvstore.put(key, &[]).await.unwrap();
        assert_invalid(query(ctx, "MATCH (n:item) RETURN id(n)").await);
    }
}

#[tokio::test]
async fn tagvid_reference_validates_vertex_identity() {
    let (_, ctx) = context();
    ctx.kvstore
        .put(b"default:vertex:1", &vertex(2))
        .await
        .unwrap();
    ctx.kvstore
        .put(b"default:tagvid:item:1", &[])
        .await
        .unwrap();
    assert_invalid(query(ctx, "MATCH (n:item) RETURN id(n)").await);
}

async fn create_index(ctx: &ExecutionContext) -> u32 {
    ctx.index_manager
        .as_ref()
        .unwrap()
        .create_tag_index(
            1,
            "item_age_idx".into(),
            1,
            "item".into(),
            vec!["age".into()],
            vec![0],
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn property_index_reference_validates_vertex_identity() {
    let (_, ctx) = context();
    let index = create_index(&ctx).await;
    ctx.kvstore
        .put(b"default:vertex:1", &vertex(2))
        .await
        .unwrap();
    ctx.index_manager
        .as_ref()
        .unwrap()
        .insert_tag_index(1, index, &[IndexValue::Int(10)], 1)
        .await
        .unwrap();
    assert_invalid(query(ctx, "MATCH (n:item {age:10}) RETURN id(n)").await);
}

#[tokio::test]
async fn property_index_rejects_zero_partitions() {
    let (_, mut ctx) = context();
    Arc::get_mut(&mut ctx).unwrap().partition_num = Some(0);
    create_index(&ctx).await;
    assert_invalid(query(ctx, "MATCH (n:item {age:10}) RETURN id(n)").await);
}

#[tokio::test]
async fn legacy_omitted_vid_and_props_and_valid_zero_vid_remain_queryable() {
    let (_, ctx) = context();
    ctx.kvstore
        .put(b"default:vertex:1", br#"{"tags":[{"name":"item"}]}"#)
        .await
        .unwrap();
    ctx.kvstore
        .put(b"default:vertex:0", &vertex(0))
        .await
        .unwrap();
    assert_eq!(
        query(ctx, "MATCH (n:item) RETURN id(n) ORDER BY id(n)")
            .await
            .unwrap()
            .rows,
        vec![vec![Value::Int(0)], vec![Value::Int(1)]]
    );
}

const EDGE_QUERIES: [&str; 5] = [
    "MATCH (n)-[:link]->(m) WHERE id(n) == 1 RETURN id(m)",
    "MATCH (n)<-[:link]-(m) WHERE id(n) == 2 RETURN id(m)",
    "MATCH (n)-[:link]-(m) WHERE id(n) == 1 RETURN id(m)",
    "MATCH (n)-[:link*1..2]->(m) WHERE id(n) == 1 RETURN id(m)",
    "MATCH (n)-[:link]->(m) WHERE id(m) == 2 RETURN id(n)",
];

#[tokio::test]
async fn malformed_edges_fail_all_match_directions_and_range() {
    for query_text in EDGE_QUERIES {
        for blob in [
            b"not json".to_vec(),
            edge(1, 0),
            br#"{"type":"link","dst":2}"#.to_vec(),
        ] {
            let (_, ctx) = context();
            seed(&ctx).await;
            ctx.kvstore
                .put(b"default:edge:1:link:2:0", &blob)
                .await
                .unwrap();
            ctx.kvstore
                .put(b"default:in-edge:2:link:1:0", &blob)
                .await
                .unwrap();
            assert_invalid(query(ctx, query_text).await);
        }
    }
}

#[tokio::test]
async fn malformed_destination_vertex_fails_fixed_ranged_and_reverse() {
    for query_text in EDGE_QUERIES {
        let (_, ctx) = context();
        seed(&ctx).await;
        let key = if query_text.contains("id(n) == 2") || query_text.contains("id(m) == 2") {
            b"default:vertex:1"
        } else {
            b"default:vertex:2"
        };
        ctx.kvstore.put(key, b"not json").await.unwrap();
        assert_invalid(query(ctx, query_text).await);
    }
}

#[tokio::test]
async fn partial_edge_stream_failure_never_publishes_rows() {
    for query_text in EDGE_QUERIES {
        let (store, ctx) = context();
        seed(&ctx).await;
        let prefix = if query_text.contains("id(n) == 2") || query_text.contains("id(m) == 2") {
            b"default:in-edge:2:".to_vec()
        } else {
            b"default:edge:1:".to_vec()
        };
        *store.failed_scan.lock().unwrap() = Some(prefix);
        assert!(query(ctx, query_text).await.is_err());
    }
}

#[tokio::test]
async fn optional_edge_corruption_is_not_a_null_extension() {
    let (_, ctx) = context();
    seed(&ctx).await;
    ctx.kvstore
        .put(b"default:edge:1:link:2:0", b"not json")
        .await
        .unwrap();
    assert_invalid(
        query(
            ctx,
            "MATCH (n:item) OPTIONAL MATCH (n)-[:link]->(m) RETURN id(n), id(m)",
        )
        .await,
    );
}

#[tokio::test]
async fn reverse_edge_property_expression_errors_propagate() {
    let (_, ctx) = context();
    seed(&ctx).await;
    let statement =
        byoridb_parser::parse("MATCH (n)-[:link]->(m) WHERE id(m) == 2 RETURN id(n)").unwrap();
    let ExecutionPlan::Match(mut plan) = ExecutionPlanBuilder::build(statement).unwrap() else {
        panic!("expected MATCH")
    };
    let byoridb_parser::ast::Pattern::Path(path) = &mut plan.pattern else {
        panic!("expected path")
    };
    path.edges[0].props.insert(
        "weight".into(),
        byoridb_parser::ast::Expression::Identifier("unsupported".into()),
    );
    assert_invalid(MatchExecutor::new(ctx).execute_match(plan).await);
}

#[tokio::test]
async fn stale_references_and_missing_destinations_remain_true_no_matches() {
    let (_, ctx) = context();
    ctx.kvstore
        .put(b"default:tagvid:item:99", &[])
        .await
        .unwrap();
    assert!(query(ctx.clone(), "MATCH (n:item) RETURN id(n)")
        .await
        .unwrap()
        .rows
        .is_empty());
    ctx.kvstore
        .put(b"default:vertex:1", &vertex(1))
        .await
        .unwrap();
    ctx.kvstore
        .put(b"default:edge:1:link:2:0", &edge(1, 2))
        .await
        .unwrap();
    assert!(query(ctx, EDGE_QUERIES[0]).await.unwrap().rows.is_empty());
}

#[tokio::test]
async fn unrelated_corrupt_edge_type_does_not_poison_requested_traversal() {
    let (_, ctx) = context();
    seed(&ctx).await;
    ctx.kvstore
        .put(b"default:edge:1:unrelated:2:0", b"not json")
        .await
        .unwrap();
    assert_eq!(
        query(ctx, EDGE_QUERIES[0]).await.unwrap().rows,
        vec![vec![Value::Int(2)]]
    );
}

#[tokio::test]
async fn ranged_path_validates_vertices_before_the_minimum_terminal_depth() {
    for corrupt in [false, true] {
        let (_, ctx) = context();
        seed(&ctx).await;
        ctx.kvstore
            .put(b"default:vertex:3", &vertex(3))
            .await
            .unwrap();
        ctx.kvstore
            .put(b"default:edge:2:link:3:0", &edge(2, 3))
            .await
            .unwrap();
        if corrupt {
            ctx.kvstore
                .put(b"default:vertex:2", b"not json")
                .await
                .unwrap();
        } else {
            ctx.kvstore.delete(b"default:vertex:2").await.unwrap();
        }
        let result = query(
            ctx,
            "MATCH (n)-[:link*2..2]->(m) WHERE id(n) == 1 RETURN id(m)",
        )
        .await;
        if corrupt {
            assert_invalid(result);
        } else {
            assert!(
                result.unwrap().rows.is_empty(),
                "a missing intermediate vertex must not bridge a path"
            );
        }
    }
}

#[tokio::test]
async fn simple_count_partial_candidate_streams_propagate_failure() {
    for (query_text, prefix) in [
        ("MATCH (n) RETURN count(*)", "default:vertex:"),
        ("MATCH (n:item) RETURN count(*)", "default:tagvid:item:"),
    ] {
        let (store, ctx) = context();
        ctx.kvstore
            .put(b"default:vertex:1", &vertex(1))
            .await
            .unwrap();
        ctx.kvstore
            .put(b"default:tagvid:item:1", &[])
            .await
            .unwrap();
        *store.failed_scan.lock().unwrap() = Some(prefix.as_bytes().to_vec());
        assert!(query(ctx, query_text).await.is_err(), "{query_text}");
    }
}

#[tokio::test]
async fn simple_count_batch_get_propagates_failure() {
    let (store, ctx) = context();
    ctx.kvstore
        .put(b"default:tagvid:item:1", &[])
        .await
        .unwrap();
    *store.failed_get.lock().unwrap() = Some(b"default:vertex:1".to_vec());
    assert!(query(ctx, "MATCH (n:item) RETURN count(*)").await.is_err());
}

#[tokio::test]
async fn bound_start_required_join_rechecks_labels_and_properties() {
    for pattern in [
        "(n:other)",
        "(n {age:99})",
        "(n:other)-[:link]->(m)",
        "(n {age:99})-[:link]->(m)",
    ] {
        let (_, ctx) = context();
        seed(&ctx).await;
        let result = query(ctx, &format!("MATCH (n:item), {pattern} RETURN count(*)"))
            .await
            .unwrap();
        assert_eq!(result.rows, vec![vec![Value::Int(0)]], "{pattern}");
    }
}

#[tokio::test]
async fn bound_start_optional_join_rechecks_labels_and_properties() {
    for pattern in ["(n:other)-[:link]->(m)", "(n {age:99})-[:link]->(m)"] {
        let (_, ctx) = context();
        seed(&ctx).await;
        let result = query(
            ctx,
            &format!("MATCH (n:item) OPTIONAL MATCH {pattern} RETURN id(n), id(m) ORDER BY id(n)"),
        )
        .await
        .unwrap();
        assert_eq!(
            result.rows,
            (0..=2)
                .map(|vid| vec![Value::Int(vid), Value::Null(byoridb_common::NullType::Null)])
                .collect::<Vec<_>>(),
            "{pattern}"
        );
    }
}

#[tokio::test]
async fn bound_start_joins_preserve_matching_labels_and_properties() {
    for join in [",", "OPTIONAL MATCH"] {
        let (_, ctx) = context();
        seed(&ctx).await;
        let result = query(ctx, &format!("MATCH (n:item) {join} (n:item {{age:10}})-[:link]->(m) RETURN id(n), id(m) ORDER BY id(n)")).await.unwrap();
        let expected = if join == "," {
            vec![vec![Value::Int(1), Value::Int(2)]]
        } else {
            vec![
                vec![Value::Int(0), Value::Null(byoridb_common::NullType::Null)],
                vec![Value::Int(1), Value::Int(2)],
                vec![Value::Int(2), Value::Null(byoridb_common::NullType::Null)],
            ]
        };
        assert_eq!(result.rows, expected, "{join}");
    }
}

#[tokio::test]
async fn bound_start_joins_propagate_get_and_decode_failures() {
    for join in [",", "OPTIONAL MATCH"] {
        for corrupt in [false, true] {
            let (store, ctx) = context();
            ctx.kvstore
                .put(b"default:vertex:1", &vertex(1))
                .await
                .unwrap();
            if corrupt {
                *store.overridden_get.lock().unwrap() =
                    Some((b"default:vertex:1".to_vec(), Some(vertex(9))));
            } else {
                *store.failed_get.lock().unwrap() = Some(b"default:vertex:1".to_vec());
            }
            let result = query(ctx, &format!("MATCH (n:item) {join} (n) RETURN id(n)")).await;
            if corrupt {
                assert_invalid(result);
            } else {
                assert!(
                    matches!(
                        result,
                        Err(ExecutionError::Storage(
                            byoridb_storage::StorageError::KVStore(KVStoreError::StoreClosed)
                        ))
                    ),
                    "{join}: {result:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn bound_start_missing_record_drops_required_but_retains_optional_row() {
    for join in [",", "OPTIONAL MATCH"] {
        let (store, ctx) = context();
        ctx.kvstore
            .put(b"default:vertex:1", &vertex(1))
            .await
            .unwrap();
        ctx.kvstore
            .put(
                b"default:vertex:2",
                br#"{"vid":2,"tags":[{"name":"other"}]}"#,
            )
            .await
            .unwrap();
        ctx.kvstore
            .put(b"default:edge:1:link:2:0", &edge(1, 2))
            .await
            .unwrap();
        *store.overridden_get.lock().unwrap() = Some((b"default:vertex:1".to_vec(), None));
        let result = query(
            ctx,
            &format!("MATCH (n:item) {join} (n)-[:link]->(m) RETURN id(n), id(m)"),
        )
        .await
        .unwrap();
        if join == "," {
            assert!(result.rows.is_empty());
        } else {
            assert_eq!(
                result.rows,
                vec![vec![
                    Value::Int(1),
                    Value::Null(byoridb_common::NullType::Null)
                ]]
            );
        }
    }
}

async fn put_edge_both_directions(ctx: &ExecutionContext, src: i64, dst: i64, rank: i64) {
    let blob = VertexCodec::encode_edge(&EdgeData {
        src_vid: src,
        dst_vid: dst,
        edge_type: "link".into(),
        ranking: rank,
        properties: Default::default(),
    })
    .unwrap();
    ctx.kvstore
        .put(
            format!("default:edge:{src}:link:{dst}:{rank}").as_bytes(),
            &blob,
        )
        .await
        .unwrap();
    ctx.kvstore
        .put(
            format!("default:in-edge:{dst}:link:{src}:{rank}").as_bytes(),
            &blob,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn undirected_self_loop_is_one_edge_binding_and_one_count() {
    let (_, ctx) = context();
    ctx.kvstore
        .put(b"default:vertex:1", &vertex(1))
        .await
        .unwrap();
    put_edge_both_directions(&ctx, 1, 1, 0).await;
    assert_eq!(
        query(
            ctx.clone(),
            "MATCH (n)-[e:link]-(m) WHERE id(n) == 1 RETURN src(e), dst(e), rank(e)"
        )
        .await
        .unwrap()
        .rows,
        vec![vec![Value::Int(1), Value::Int(1), Value::Int(0)]]
    );
    assert_eq!(
        query(
            ctx,
            "MATCH (n)-[e:link]-(m) WHERE id(n) == 1 RETURN count(*)"
        )
        .await
        .unwrap()
        .rows,
        vec![vec![Value::Int(1)]]
    );
}

#[tokio::test]
async fn undirected_parallel_ranks_and_opposite_edges_remain_distinct() {
    let (_, ctx) = context();
    ctx.kvstore
        .put(b"default:vertex:1", &vertex(1))
        .await
        .unwrap();
    ctx.kvstore
        .put(b"default:vertex:2", &vertex(2))
        .await
        .unwrap();
    for (src, dst, rank) in [(1, 1, 0), (1, 1, 1), (1, 2, 0), (1, 2, 1), (2, 1, 0)] {
        put_edge_both_directions(&ctx, src, dst, rank).await;
    }
    let result = query(ctx.clone(), "MATCH (n)-[e:link]-(m) WHERE id(n) == 1 RETURN src(e) AS source, dst(e) AS target, rank(e) AS ranking ORDER BY source, target, ranking").await.unwrap();
    assert_eq!(
        result.rows,
        [(1, 1, 0), (1, 1, 1), (1, 2, 0), (1, 2, 1), (2, 1, 0)]
            .into_iter()
            .map(|(src, dst, rank)| vec![Value::Int(src), Value::Int(dst), Value::Int(rank)])
            .collect::<Vec<_>>()
    );
    assert_eq!(
        query(
            ctx,
            "MATCH (n)-[e:link]-(m) WHERE id(n) == 1 RETURN count(*)"
        )
        .await
        .unwrap()
        .rows,
        vec![vec![Value::Int(5)]]
    );
}

#[tokio::test]
async fn binding_identity_repeated_node_requires_the_same_vertex_including_reverse() {
    for filter in ["", " WHERE id(n) == 1", " WHERE id(n) == 2"] {
        let (_, ctx) = context();
        seed(&ctx).await;
        let result = query(
            ctx,
            &format!("MATCH (n)-[:link]->(n){filter} RETURN count(*)"),
        )
        .await
        .unwrap();
        assert_eq!(result.rows, vec![vec![Value::Int(0)]], "{filter}");
    }
}

#[tokio::test]
async fn binding_identity_repeated_node_keeps_valid_self_loop() {
    for filter in ["", " WHERE id(n) == 2"] {
        let (_, ctx) = context();
        seed(&ctx).await;
        put_edge_both_directions(&ctx, 2, 2, 0).await;
        assert_eq!(
            query(ctx, &format!("MATCH (n)-[:link]->(n){filter} RETURN id(n)"))
                .await
                .unwrap()
                .rows,
            vec![vec![Value::Int(2)]]
        );
    }
}

#[tokio::test]
async fn binding_identity_required_endpoint_join_filters_mismatched_vertices() {
    let (_, ctx) = context();
    seed(&ctx).await;
    assert_eq!(
        query(
            ctx,
            "MATCH (target:item), (n)-[:link]->(target) RETURN id(n), id(target)"
        )
        .await
        .unwrap()
        .rows,
        vec![vec![Value::Int(1), Value::Int(2)]]
    );
}

#[tokio::test]
async fn binding_identity_optional_endpoint_join_preserves_base_identity() {
    let (_, ctx) = context();
    seed(&ctx).await;
    assert_eq!(query(ctx, "MATCH (target:item) OPTIONAL MATCH (n)-[:link]->(target) RETURN id(target), id(n) ORDER BY id(target)").await.unwrap().rows,
        vec![vec![Value::Int(0), Value::Null(byoridb_common::NullType::Null)], vec![Value::Int(1), Value::Null(byoridb_common::NullType::Null)], vec![Value::Int(2), Value::Int(1)]]);
}

#[tokio::test]
async fn binding_identity_ranged_endpoint_join_preserves_existing_vertex() {
    let (_, ctx) = context();
    seed(&ctx).await;
    put_edge_both_directions(&ctx, 2, 0, 0).await;
    assert_eq!(query(ctx, "MATCH (target:item), (n)-[:link*1..2]->(target) RETURN id(n) AS source, id(target) AS target ORDER BY source, target").await.unwrap().rows,
        vec![vec![Value::Int(1), Value::Int(0)], vec![Value::Int(1), Value::Int(2)], vec![Value::Int(2), Value::Int(0)]]);
}

#[tokio::test]
async fn binding_identity_reused_edge_matches_full_identity_and_preserves_parallel_ranks() {
    let (_, ctx) = context();
    seed(&ctx).await;
    put_edge_both_directions(&ctx, 1, 2, 1).await;
    let result = query(ctx, "MATCH (n)-[e:link]->(m), (n)-[e:link]->(x) RETURN rank(e) AS rank, id(m), id(x) ORDER BY rank").await.unwrap();
    assert_eq!(
        result.rows,
        vec![
            vec![Value::Int(0), Value::Int(2), Value::Int(2)],
            vec![Value::Int(1), Value::Int(2), Value::Int(2)]
        ]
    );
}

#[tokio::test]
async fn binding_identity_reused_edge_mismatch_drops_required_and_null_extends_optional() {
    for join in [",", "OPTIONAL MATCH"] {
        let (_, ctx) = context();
        seed(&ctx).await;
        ctx.kvstore
            .put(
                b"default:vertex:3",
                br#"{"vid":3,"tags":[{"name":"other"}]}"#,
            )
            .await
            .unwrap();
        put_edge_both_directions(&ctx, 1, 3, 0).await;
        let result = query(ctx, &format!("MATCH (n)-[e:link]->(m:item) {join} (n)-[e:link]->(x:other) RETURN dst(e), id(m), id(x)")).await.unwrap();
        if join == "," {
            assert!(result.rows.is_empty());
        } else {
            assert_eq!(
                result.rows,
                vec![vec![
                    Value::Int(2),
                    Value::Int(2),
                    Value::Null(byoridb_common::NullType::Null)
                ]]
            );
        }
    }
}

#[tokio::test]
async fn binding_identity_optional_null_cannot_be_rebound_by_a_later_pattern() {
    let (_, ctx) = context();
    seed(&ctx).await;
    let result = query(ctx, "MATCH (n:item) OPTIONAL MATCH (n)-[:missing]->(m) OPTIONAL MATCH (m)-[:link]->(x) RETURN id(n), id(m), id(x) ORDER BY id(n)").await.unwrap();
    assert_eq!(
        result.rows,
        (0..=2)
            .map(|vid| vec![
                Value::Int(vid),
                Value::Null(byoridb_common::NullType::Null),
                Value::Null(byoridb_common::NullType::Null)
            ])
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn binding_identity_vertex_and_edge_names_cannot_overwrite_each_other() {
    for query_text in [
        "MATCH (n)-[n:link]->(m) RETURN id(n)",
        "MATCH (n)-[e:link]->(e) RETURN id(n)",
        "MATCH (n)-[e:link]->(m), (e) RETURN id(n)",
    ] {
        let (_, ctx) = context();
        seed(&ctx).await;
        assert_invalid(query(ctx, query_text).await);
    }
}

#[tokio::test]
async fn binding_identity_reverse_shortcut_does_not_change_direction_or_range() {
    for (query_text, expected) in [
        (
            "MATCH (n)<-[:link]-(m) WHERE id(m) == 1 RETURN id(n)",
            vec![vec![Value::Int(2)]],
        ),
        (
            "MATCH (n)-[:link]-(m) WHERE id(m) == 1 RETURN id(n)",
            vec![vec![Value::Int(2)]],
        ),
        (
            "MATCH (n)-[:link*2..2]->(m) WHERE id(m) == 0 RETURN id(n)",
            vec![vec![Value::Int(1)]],
        ),
    ] {
        let (_, ctx) = context();
        seed(&ctx).await;
        put_edge_both_directions(&ctx, 2, 0, 0).await;
        assert_eq!(
            query(ctx, query_text).await.unwrap().rows,
            expected,
            "{query_text}"
        );
    }
}
