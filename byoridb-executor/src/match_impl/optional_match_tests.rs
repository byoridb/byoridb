// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! OPTIONAL MATCH may turn an empty match into NULLs, but must not turn a
//! storage/execution failure into a successful empty or partial result.

use super::MatchExecutor;
use crate::context::ExecutionContext;
use crate::error::ExecutionError;
use crate::plan::{ExecutionPlan, ExecutionPlanBuilder, MatchPlan};
use async_trait::async_trait;
use byoridb_codec::{EdgeData, TagData, VertexCodec, VertexData};
use byoridb_common::Value;
use byoridb_kvstore::{KVStore, KVStoreError, MemoryKVStore, Result as KvResult, VersionRecord};
use byoridb_parser::ast::Pattern;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Fail one exact scan prefix so setup, the required MATCH, and unrelated
/// optional work succeed. Default scan helpers all pass through scan_prefix.
struct FailingScanStore {
    inner: MemoryKVStore,
    fail_prefix: Option<Vec<u8>>,
    failures: AtomicUsize,
}

#[async_trait]
impl KVStore for FailingScanStore {
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
        if self.fail_prefix.as_deref() == Some(prefix) {
            self.failures.fetch_add(1, Ordering::SeqCst);
            return Err(KVStoreError::StoreClosed);
        }
        self.inner.scan_prefix(prefix).await
    }

    async fn batch_get(&self, keys: &[Vec<u8>]) -> KvResult<Vec<Option<Vec<u8>>>> {
        self.inner.batch_get(keys).await
    }
}

fn context(fail_prefix: Option<&[u8]>) -> (Arc<FailingScanStore>, Arc<ExecutionContext>) {
    let store = Arc::new(FailingScanStore {
        inner: MemoryKVStore::new(),
        fail_prefix: fail_prefix.map(<[u8]>::to_vec),
        failures: AtomicUsize::new(0),
    });
    let ctx = Arc::new(
        ExecutionContext::new(store.clone())
            .with_space("default".to_string())
            .with_space_id(1),
    );
    (store, ctx)
}

fn match_plan(query: &str) -> MatchPlan {
    let statement = byoridb_parser::parse(query).unwrap();
    match ExecutionPlanBuilder::build(statement).unwrap() {
        ExecutionPlan::Match(plan) => plan,
        _ => panic!("expected MATCH plan"),
    }
}

async fn put_vertex(ctx: &ExecutionContext, vid: i64, label: &str) {
    let blob = VertexCodec::encode_vertex(&VertexData {
        vid,
        tags: vec![TagData {
            name: label.to_string(),
            properties: Default::default(),
        }],
    })
    .unwrap();
    ctx.kvstore
        .put(&crate::key::SchemaKey::vertex("default", vid), &blob)
        .await
        .unwrap();
    ctx.kvstore
        .put(&crate::key::SchemaKey::tagvid("default", label, vid), &[])
        .await
        .unwrap();
}

async fn put_edge(ctx: &ExecutionContext, src: i64, dst: i64, edge_type: &str) {
    let blob = VertexCodec::encode_edge(&EdgeData {
        src_vid: src,
        dst_vid: dst,
        edge_type: edge_type.to_string(),
        ranking: 0,
        properties: Default::default(),
    })
    .unwrap();
    ctx.kvstore
        .put(
            &crate::key::SchemaKey::edge_data("default", src, edge_type, dst, 0),
            &blob,
        )
        .await
        .unwrap();
}

fn assert_store_closed(result: crate::error::Result<crate::executor::ExecutorResult>) {
    match result {
        Err(ExecutionError::Storage(byoridb_storage::StorageError::KVStore(
            KVStoreError::StoreClosed,
        ))) => {}
        other => panic!("expected original StoreClosed error, got {other:?}"),
    }
}

#[tokio::test]
async fn optional_match_bound_start_propagates_storage_error() {
    let (store, ctx) = context(Some(b"default:edge:1:"));
    put_vertex(&ctx, 1, "anchor").await;
    let result = MatchExecutor::new(ctx)
        .execute_match(match_plan(
            "MATCH (a:anchor) OPTIONAL MATCH (a)-[:link]->(b) RETURN id(a), id(b)",
        ))
        .await;
    assert_eq!(store.failures.load(Ordering::SeqCst), 1);
    assert_store_closed(result);
}

#[tokio::test]
async fn optional_match_unbound_start_propagates_candidate_scan_error() {
    let (store, ctx) = context(Some(b"default:tagvid:candidate:"));
    put_vertex(&ctx, 1, "anchor").await;
    let result = MatchExecutor::new(ctx)
        .execute_match(match_plan(
            "MATCH (a:anchor) OPTIONAL MATCH (b:candidate) RETURN id(a), id(b)",
        ))
        .await;
    assert_eq!(store.failures.load(Ordering::SeqCst), 1);
    assert_store_closed(result);
}

// The first branch produces (b=2, c=4) before the second branch's scan fails.
// Both paths must reject the whole query rather than publish that partial row.
const BOUND_BRANCHES: &str = "MATCH (a:anchor) \
    OPTIONAL MATCH (a)-[:link]->(b:candidate)-[:next]->(c:target) \
    RETURN id(a), id(b), id(c) ORDER BY id(b)";
const UNBOUND_BRANCHES: &str = "MATCH (a:anchor) \
    OPTIONAL MATCH (b:candidate)-[:next]->(c:target) \
    RETURN id(a), id(b), id(c) ORDER BY id(b)";

async fn seed_branches(ctx: &ExecutionContext) {
    put_vertex(ctx, 1, "anchor").await;
    put_vertex(ctx, 2, "candidate").await;
    put_vertex(ctx, 3, "candidate").await;
    put_vertex(ctx, 4, "target").await;
    put_vertex(ctx, 5, "target").await;
    put_edge(ctx, 1, 2, "link").await;
    put_edge(ctx, 1, 3, "link").await;
    put_edge(ctx, 2, 4, "next").await;
    put_edge(ctx, 3, 5, "next").await;
}

async fn assert_late_failure(query: &str) {
    let (store, ctx) = context(Some(b"default:edge:3:"));
    seed_branches(&ctx).await;
    let result = MatchExecutor::new(ctx)
        .execute_match(match_plan(query))
        .await;
    assert_eq!(store.failures.load(Ordering::SeqCst), 1);
    assert_store_closed(result);
}

#[tokio::test]
async fn optional_match_bound_start_discards_rows_after_late_error() {
    assert_late_failure(BOUND_BRANCHES).await;
}

#[tokio::test]
async fn optional_match_unbound_start_discards_rows_after_late_error() {
    assert_late_failure(UNBOUND_BRANCHES).await;
}

#[tokio::test]
async fn optional_match_chained_failure_discards_prior_optional_results() {
    for query in [
        "MATCH (a:anchor) OPTIONAL MATCH (a)-[:link]->(b:candidate) \
         OPTIONAL MATCH (b)-[:next]->(c:target) RETURN id(a), id(b), id(c)",
        "MATCH (a:anchor) OPTIONAL MATCH (a)-[:link]->(b:candidate) \
         OPTIONAL MATCH (d:candidate)-[:next]->(c:target) RETURN id(a), id(b), id(c)",
    ] {
        let (store, ctx) = context(Some(b"default:edge:3:"));
        seed_branches(&ctx).await;
        let executor = MatchExecutor::new(ctx);

        // The first OPTIONAL succeeds with this fault active. Only the
        // second OPTIONAL reaches the failing scan, after matching vertex 2.
        let first_optional = executor
            .execute_match(match_plan(
                "MATCH (a:anchor) OPTIONAL MATCH (a)-[:link]->(b:candidate) \
                 RETURN id(a), id(b)",
            ))
            .await
            .unwrap();
        assert_eq!(
            first_optional.rows,
            vec![
                vec![Value::Int(1), Value::Int(2)],
                vec![Value::Int(1), Value::Int(3)],
            ]
        );
        assert_eq!(store.failures.load(Ordering::SeqCst), 0);

        let plan = match_plan(query);
        assert_eq!(plan.optional_patterns.len(), 2);
        let result = executor.execute_match(plan).await;
        assert_eq!(store.failures.load(Ordering::SeqCst), 1);
        assert_store_closed(result);
    }
}

#[tokio::test]
async fn optional_match_success_keeps_all_rows() {
    let (_, ctx) = context(None);
    seed_branches(&ctx).await;
    let executor = MatchExecutor::new(ctx);
    for query in [BOUND_BRANCHES, UNBOUND_BRANCHES] {
        let mut rows = executor
            .execute_match(match_plan(query))
            .await
            .unwrap()
            .rows;
        rows.sort_by_key(|row| match row[1] {
            Value::Int(vid) => vid,
            _ => panic!("expected candidate VID"),
        });
        assert_eq!(
            rows,
            vec![
                vec![Value::Int(1), Value::Int(2), Value::Int(4)],
                vec![Value::Int(1), Value::Int(3), Value::Int(5)],
            ],
            "query: {query}"
        );
    }
}

#[tokio::test]
async fn optional_match_no_match_preserves_base_row_with_null() {
    let (_, ctx) = context(None);
    put_vertex(&ctx, 1, "anchor").await;
    let executor = MatchExecutor::new(ctx);
    for query in [
        "MATCH (a:anchor) OPTIONAL MATCH (a)-[e:missing]->(b) RETURN id(a), b, e",
        "MATCH (a:anchor) OPTIONAL MATCH (b:missing)-[e:link]->(c) RETURN id(a), b, e",
    ] {
        let result = executor.execute_match(match_plan(query)).await.unwrap();
        assert_eq!(
            result.rows,
            vec![vec![Value::Int(1), Value::null(), Value::null()]],
            "query: {query}"
        );
    }
}

#[tokio::test]
async fn optional_match_invalid_pattern_is_an_error() {
    let (_, ctx) = context(None);
    put_vertex(&ctx, 1, "anchor").await;
    let mut plan = match_plan("MATCH (a:anchor) OPTIONAL MATCH (b) RETURN id(a), b");
    plan.optional_patterns = vec![Pattern::Multiple(vec![])];
    let result = MatchExecutor::new(ctx).execute_match(plan).await;
    assert!(
        matches!(result, Err(ExecutionError::InvalidOperation(ref message)) if message == "Empty pattern"),
        "expected invalid optional plan to fail, got {result:?}"
    );
}
