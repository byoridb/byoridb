// Copyright (c) 2024 ByoriDB
// This source code is licensed under Apache 2.0 License.

//! MATCH materialization and aggregation must not hide failed reads as NULL.
use super::MatchExecutor;
use crate::context::ExecutionContext;
use crate::error::ExecutionError;
use crate::plan::{ExecutionPlan, ExecutionPlanBuilder, MatchPlan};
use async_trait::async_trait;
use byoridb_codec::{TagData, VertexCodec, VertexData};
use byoridb_common::{NullType, Value};
use byoridb_kvstore::{KVStore, KVStoreError, MemoryKVStore, Result as KvResult, VersionRecord};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct ProjectionStore {
    inner: MemoryKVStore,
    mode: AtomicUsize,
}
#[async_trait]
impl KVStore for ProjectionStore {
    async fn get(&self, key: &[u8]) -> KvResult<Option<Vec<u8>>> {
        if key == b"default:vertex:1" {
            match self.mode.load(Ordering::SeqCst) {
                1 => return Err(KVStoreError::StoreClosed),
                2 => return Ok(Some(b"not-json".to_vec())),
                3 => return Ok(None),
                _ => {}
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

    async fn batch_get(&self, keys: &[Vec<u8>]) -> KvResult<Vec<Option<Vec<u8>>>> {
        self.inner.batch_get(keys).await
    }
}

fn context() -> (Arc<ProjectionStore>, Arc<ExecutionContext>) {
    let store = Arc::new(ProjectionStore {
        inner: MemoryKVStore::new(),
        mode: AtomicUsize::new(0),
    });
    let ctx = Arc::new(
        ExecutionContext::new(store.clone())
            .with_space("default".to_string())
            .with_space_id(1),
    );
    (store, ctx)
}
fn plan(query: &str) -> MatchPlan {
    match ExecutionPlanBuilder::build(byoridb_parser::parse(query).unwrap()).unwrap() {
        ExecutionPlan::Match(plan) => plan,
        _ => panic!("expected MATCH"),
    }
}
async fn put(ctx: &ExecutionContext, vid: i64, score: Option<i64>) {
    let mut properties = HashMap::from([("bucket".to_string(), Value::String("same".to_string()))]);
    if let Some(score) = score {
        properties.insert("score".to_string(), Value::Int(score));
    }
    let blob = VertexCodec::encode_vertex(&VertexData {
        vid,
        tags: vec![TagData {
            name: "item".to_string(),
            properties,
        }],
    })
    .unwrap();
    ctx.kvstore
        .put(format!("default:vertex:{vid}").as_bytes(), &blob)
        .await
        .unwrap();
}
async fn rows(ctx: Arc<ExecutionContext>, query: &str) -> Vec<Vec<Value>> {
    MatchExecutor::new(ctx)
        .execute_match(plan(query))
        .await
        .unwrap()
        .rows
}

#[tokio::test]
async fn projection_get_errors_are_not_null_or_empty_objects() {
    for expr in [
        "n",
        "n.score",
        "properties(n)",
        "tags(n)",
        "labels(n)",
        "is_a(n, 'item')",
        "lower(n.bucket)",
        "[n.score]",
    ] {
        let (store, ctx) = context();
        put(&ctx, 1, Some(5)).await;
        store.mode.store(1, Ordering::SeqCst);
        let result = MatchExecutor::new(ctx)
            .execute_match(plan(&format!("MATCH (n) RETURN {expr}")))
            .await;
        assert!(
            matches!(result, Err(ExecutionError::Storage(_))),
            "{expr}: {result:?}"
        );
    }
}

#[tokio::test]
async fn projection_only_corrupt_reads_are_errors() {
    for expr in [
        "n",
        "n.score",
        "properties(n)",
        "tags(n)",
        "is_a(n, 'item')",
    ] {
        let (store, ctx) = context();
        put(&ctx, 1, Some(5)).await;
        store.mode.store(2, Ordering::SeqCst);
        let result = MatchExecutor::new(ctx)
            .execute_match(plan(&format!("MATCH (n) RETURN {expr}")))
            .await;
        assert!(result.is_err(), "{expr}: {result:?}");
    }
}

#[tokio::test]
async fn where_group_and_aggregate_read_failures_propagate() {
    for query in [
        "MATCH (n) WHERE n.score == 5 RETURN id(n)",
        "MATCH (n) RETURN n.bucket, COUNT(*)",
        "MATCH (n) RETURN COUNT(n.score)",
        "MATCH (n) RETURN SUM(n.score)",
        "MATCH (n) RETURN AVG(n.score)",
        "MATCH (n) RETURN MIN(n.score)",
        "MATCH (n) RETURN MAX(n.score)",
    ] {
        let (store, ctx) = context();
        put(&ctx, 1, Some(5)).await;
        store.mode.store(1, Ordering::SeqCst);
        let result = MatchExecutor::new(ctx).execute_match(plan(query)).await;
        assert!(result.is_err(), "{query}: {result:?}");
    }
}

#[tokio::test]
async fn count_expression_and_extrema_skip_nulls() {
    let (_, ctx) = context();
    put(&ctx, 1, None).await;
    put(&ctx, 2, Some(3)).await;
    put(&ctx, 3, Some(7)).await;
    assert_eq!(
        rows(
            ctx,
            "MATCH (n) RETURN COUNT(*), COUNT(n.score), MIN(n.score), MAX(n.score)"
        )
        .await,
        vec![vec![
            Value::Int(3),
            Value::Int(2),
            Value::Int(3),
            Value::Int(7)
        ]]
    );
}

#[tokio::test]
async fn empty_global_aggregates_emit_one_row() {
    let (_, ctx) = context();
    assert_eq!(rows(ctx.clone(), "MATCH (n) RETURN COUNT(n.score), SUM(n.score), AVG(n.score), MIN(n.score), MAX(n.score)").await,
        vec![vec![Value::Int(0), Value::Int(0), Value::Null(NullType::Null), Value::Null(NullType::Null), Value::Null(NullType::Null)]]);
    assert!(rows(ctx, "MATCH (n) RETURN n.bucket, COUNT(*)")
        .await
        .is_empty());
}

#[tokio::test]
async fn aggregate_limit_and_offset_apply_after_reduction() {
    let (_, ctx) = context();
    for vid in 1..=3 {
        put(&ctx, vid, Some(vid)).await;
    }
    assert_eq!(
        rows(ctx.clone(), "MATCH (n) RETURN SUM(n.score) LIMIT 1").await,
        vec![vec![Value::Int(6)]]
    );
    assert_eq!(
        rows(ctx.clone(), "MATCH (n) RETURN n.bucket, COUNT(*) LIMIT 1").await,
        vec![vec![Value::String("same".into()), Value::Int(3)]]
    );
    assert!(rows(ctx.clone(), "MATCH (n) RETURN SUM(n.score) LIMIT 0")
        .await
        .is_empty());
    assert!(rows(ctx, "MATCH (n) RETURN SUM(n.score) LIMIT 1 OFFSET 1")
        .await
        .is_empty());
}

#[tokio::test]
async fn overflowed_match_window_is_rejected() {
    let (_, ctx) = context();
    let mut match_plan = plan("MATCH (n) RETURN id(n)");
    match_plan.limit = Some(1);
    match_plan.offset = Some(usize::MAX);
    let result = MatchExecutor::new(ctx).execute_match(match_plan).await;
    assert!(
        matches!(result, Err(ExecutionError::InvalidOperation(_))),
        "{result:?}"
    );
}

#[test]
fn unsupported_match_forms_fail_during_planning() {
    for query in [
        "MATCH (n) RETURN COLLECT(n.score)",
        "MATCH (n) RETURN n.score + 1",
        "MATCH (n) RETURN -n.score",
        "MATCH (n) RETURN [COUNT(*)]",
        "MATCH (n) RETURN SUM(COUNT(*))",
        "MATCH (n) WHERE COUNT(*) > 1 RETURN id(n)",
        "MATCH (n) RETURN COUNT(*) GROUP BY mystery(n.score)",
        "MATCH (n) RETURN COUNT(*) GROUP BY SUM(n.score)",
        "MATCH (n) RETURN COUNT(n.score, n.bucket)",
        "MATCH (n) RETURN SUM(*)",
    ] {
        let statement = byoridb_parser::parse(query).unwrap();
        assert!(ExecutionPlanBuilder::build(statement).is_err(), "{query}");
    }
}

#[tokio::test]
async fn missing_properties_and_bad_accessor_types_remain_typed_nulls() {
    let (_, ctx) = context();
    put(&ctx, 1, None).await;
    assert_eq!(
        rows(ctx, "MATCH (n) RETURN n.missing, COUNT(n.missing), src(n)").await,
        vec![vec![
            Value::Null(NullType::Null),
            Value::Int(0),
            Value::Null(NullType::BadType)
        ]]
    );
}

#[tokio::test]
async fn isa_alias_matches_is_a() {
    let (_, ctx) = context();
    put(&ctx, 1, Some(5)).await;
    assert_eq!(
        rows(ctx, "MATCH (n) RETURN is_a(n, 'item'), isa(n, 'item')").await,
        vec![vec![Value::Bool(true), Value::Bool(true)]]
    );
}

#[tokio::test]
async fn missing_projected_vertex_preserves_empty_object_control() {
    let (store, ctx) = context();
    put(&ctx, 1, Some(5)).await;
    store.mode.store(3, Ordering::SeqCst);
    let result = rows(ctx, "MATCH (n) RETURN n, properties(n), tags(n), n.score").await;
    assert!(
        matches!(&result[0][0], Value::Vertex(v) if v.vid == Value::Int(1) && v.tags.is_empty())
    );
    assert!(matches!(&result[0][1], Value::Map(m) if m.data.is_empty()));
    assert_eq!(
        result[0][2..],
        [Value::Null(NullType::Null), Value::Null(NullType::Null)]
    );
}

#[tokio::test]
async fn integer_sum_keeps_precision_and_reports_overflow() {
    let (_, ctx) = context();
    put(&ctx, 1, Some(9_007_199_254_740_993)).await;
    put(&ctx, 2, Some(2)).await;
    assert_eq!(
        rows(ctx.clone(), "MATCH (n) RETURN SUM(n.score)").await,
        vec![vec![Value::Int(9_007_199_254_740_995)]]
    );
    put(&ctx, 3, Some(i64::MAX)).await;
    assert!(matches!(
        MatchExecutor::new(ctx)
            .execute_match(plan("MATCH (n) RETURN SUM(n.score)"))
            .await,
        Err(ExecutionError::InvalidOperation(_))
    ));
}

#[tokio::test]
async fn invalid_regex_is_not_a_false_predicate() {
    let (_, ctx) = context();
    put(&ctx, 1, Some(5)).await;
    assert!(matches!(
        MatchExecutor::new(ctx)
            .execute_match(plan("MATCH (n) WHERE n.bucket =~ '[' RETURN id(n)"))
            .await,
        Err(ExecutionError::InvalidOperation(_))
    ));
}

#[tokio::test]
async fn numeric_aggregates_keep_float_types_and_reject_nonfinite_results() {
    let (_, ctx) = context();
    put(&ctx, 1, Some(2)).await;
    for score in [0.5, f64::INFINITY] {
        let blob = VertexCodec::encode_vertex(&VertexData {
            vid: 2,
            tags: vec![TagData {
                name: "item".into(),
                properties: HashMap::from([("score".into(), Value::Float(score))]),
            }],
        })
        .unwrap();
        ctx.kvstore.put(b"default:vertex:2", &blob).await.unwrap();
        if score.is_finite() {
            assert_eq!(
                rows(ctx.clone(), "MATCH (n) RETURN SUM(n.score), AVG(n.score)").await,
                vec![vec![Value::Float(2.5), Value::Float(1.25)]]
            );
        } else {
            for expr in ["SUM(n.score)", "AVG(n.score)"] {
                assert!(matches!(
                    MatchExecutor::new(ctx.clone())
                        .execute_match(plan(&format!("MATCH (n) RETURN {expr}")))
                        .await,
                    Err(ExecutionError::InvalidOperation(_))
                ));
            }
        }
    }
}

#[tokio::test]
async fn review_identical_property_maps_form_one_group() {
    let (_, ctx) = context();
    for vid in 1..=32 {
        let properties = (0..8)
            .map(|n| (format!("key_{n}"), Value::Int(n)))
            .collect();
        let blob = VertexCodec::encode_vertex(&VertexData {
            vid,
            tags: vec![TagData {
                name: "item".into(),
                properties,
            }],
        })
        .unwrap();
        ctx.kvstore
            .put(format!("default:vertex:{vid}").as_bytes(), &blob)
            .await
            .unwrap();
    }
    let result = rows(ctx, "MATCH (n) RETURN properties(n), COUNT(*)").await;
    assert_eq!(result.len(), 1, "identical maps must have one group");
    assert_eq!(result[0][1], Value::Int(32));
}

#[tokio::test]
async fn review_same_vertex_fanout_forms_one_group() {
    let (_, ctx) = context();
    for vid in 1..=33 {
        put(&ctx, vid, Some(1)).await;
    }
    for dst_vid in 2..=33 {
        let edge = byoridb_codec::EdgeData {
            src_vid: 1,
            dst_vid,
            edge_type: "link".into(),
            ranking: 0,
            properties: HashMap::new(),
        };
        let blob = VertexCodec::encode_edge(&edge).unwrap();
        ctx.kvstore
            .put(format!("default:edge:1:link:{dst_vid}:0").as_bytes(), &blob)
            .await
            .unwrap();
    }
    let result = rows(ctx, "MATCH (n)-[:link]->(m) RETURN n, COUNT(*)").await;
    assert_eq!(
        result.len(),
        1,
        "one vertex must have one group across fanout"
    );
    assert_eq!(result[0][1], Value::Int(32));
}

#[tokio::test]
async fn review_group_float_zero_normalization_preserves_numeric_types() {
    let (_, ctx) = context();
    for (vid, score) in [
        (1, Value::Float(0.0)),
        (2, Value::Float(-0.0)),
        (3, Value::Int(0)),
    ] {
        let blob = VertexCodec::encode_vertex(&VertexData {
            vid,
            tags: vec![TagData {
                name: "item".into(),
                properties: HashMap::from([("score".into(), score)]),
            }],
        })
        .unwrap();
        ctx.kvstore
            .put(format!("default:vertex:{vid}").as_bytes(), &blob)
            .await
            .unwrap();
    }
    let result = rows(ctx, "MATCH (n) RETURN n.score, COUNT(*)").await;
    assert_eq!(result.len(), 2);
    assert!(result.contains(&vec![Value::Float(0.0), Value::Int(2)]));
    assert!(result.contains(&vec![Value::Int(0), Value::Int(1)]));
}

#[test]
fn review_unknown_or_unsupported_order_keys_fail_before_empty_scan() {
    for query in [
        "MATCH (n) RETURN id(n) ORDER BY missing",
        "MATCH (n) RETURN id(n) ORDER BY mystery(n)",
        "MATCH (n) RETURN n.score ORDER BY n.score + 1",
        "MATCH (n) RETURN n.score ORDER BY n.bucket",
        "MATCH (n) RETURN id(n), COUNT(*) ORDER BY value",
    ] {
        let stmt = byoridb_parser::parse(query).unwrap();
        assert!(ExecutionPlanBuilder::build(stmt).is_err(), "{query}");
    }
}

#[tokio::test]
async fn review_order_alias_and_exact_projected_expressions_sort() {
    let (_, ctx) = context();
    put(&ctx, 1, Some(1)).await;
    put(&ctx, 2, Some(2)).await;
    for query in [
        "MATCH (n) RETURN id(n) AS vid ORDER BY vid DESC",
        "MATCH (n) RETURN id(n) AS vid ORDER BY id(n) DESC",
        "MATCH (n) RETURN id(n) ORDER BY id(n) DESC",
        "MATCH (n) RETURN n.score AS score ORDER BY n.score DESC",
        "MATCH (n) RETURN n.score ORDER BY n.score DESC",
    ] {
        assert_eq!(
            rows(ctx.clone(), query).await,
            vec![vec![Value::Int(2)], vec![Value::Int(1)]],
            "{query}"
        );
    }
    let (_, empty) = context();
    assert!(rows(empty, "MATCH (n) RETURN id(n) AS vid ORDER BY vid")
        .await
        .is_empty());
}

#[tokio::test]
async fn review_aggregate_materialized_rows_enforce_result_budget() {
    for bytes in [20, 1024 * 1024 + 1] {
        let (_, base) = context();
        let config = crate::context::ExecutionConfig {
            max_memory_mb: 1,
            ..crate::context::ExecutionConfig::default()
        };
        let ctx = Arc::new(
            ExecutionContext::new(base.kvstore.clone())
                .with_space("default".into())
                .with_space_id(1)
                .with_config(config),
        );
        let blob = VertexCodec::encode_vertex(&VertexData {
            vid: 1,
            tags: vec![TagData {
                name: "item".into(),
                properties: HashMap::from([("bucket".into(), Value::String("x".repeat(bytes)))]),
            }],
        })
        .unwrap();
        ctx.kvstore.put(b"default:vertex:1", &blob).await.unwrap();
        for query in [
            "MATCH (n) RETURN n.bucket, COUNT(*)",
            "MATCH (n) RETURN MAX(n.bucket)",
        ] {
            let result = MatchExecutor::new(ctx.clone())
                .execute_match(plan(query))
                .await;
            if bytes > 1024 * 1024 {
                assert!(
                    matches!(result, Err(ExecutionError::ResourceExhausted(_))),
                    "{query}"
                );
            } else {
                assert_eq!(result.unwrap().rows.len(), 1);
            }
        }
    }
}
