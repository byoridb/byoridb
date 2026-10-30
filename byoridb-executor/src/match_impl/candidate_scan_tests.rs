// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Candidate scans must either complete or fail, never publish a capped or
//! partially failed index result as a successful MATCH.

use super::MatchExecutor;
use crate::context::{ExecutionConfig, ExecutionContext};
use crate::error::ExecutionError;
use crate::plan::{ExecutionPlan, ExecutionPlanBuilder, MatchPlan};
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
        if self.fail_prefix.lock().unwrap().as_deref() == Some(prefix) {
            self.failures.fetch_add(1, Ordering::SeqCst);
            return Err(KVStoreError::StoreClosed);
        }
        self.inner.scan_prefix(prefix).await
    }

    async fn batch_get(&self, keys: &[Vec<u8>]) -> KvResult<Vec<Option<Vec<u8>>>> {
        self.inner.batch_get(keys).await
    }
}

fn context(scan_cap: usize, partitions: u32) -> (Arc<FailingScanStore>, Arc<ExecutionContext>) {
    let store = Arc::new(FailingScanStore {
        inner: MemoryKVStore::new(),
        fail_prefix: Mutex::new(None),
        failures: AtomicUsize::new(0),
    });
    let mut ctx = ExecutionContext::new(store.clone())
        .with_space("default".to_string())
        .with_space_id(1)
        .with_config(ExecutionConfig {
            max_scan_limit: scan_cap,
            ..ExecutionConfig::default()
        });
    ctx.partition_num = Some(partitions);
    (store, Arc::new(ctx))
}

fn match_plan(query: &str) -> MatchPlan {
    let statement = byoridb_parser::parse(query).unwrap();
    match ExecutionPlanBuilder::build(statement).unwrap() {
        ExecutionPlan::Match(plan) => plan,
        _ => panic!("expected MATCH plan"),
    }
}

async fn put_vertex(ctx: &ExecutionContext, vid: i64) {
    put_vertex_with_age(ctx, vid, Value::Int(10)).await;
}

async fn put_vertex_with_age(ctx: &ExecutionContext, vid: i64, age: Value) {
    let blob = VertexCodec::encode_vertex(&VertexData {
        vid,
        tags: vec![TagData {
            name: "item".to_string(),
            properties: HashMap::from([("age".to_string(), age)]),
        }],
    })
    .unwrap();
    ctx.kvstore
        .put(&crate::key::SchemaKey::vertex("default", vid), &blob)
        .await
        .unwrap();
    ctx.kvstore
        .put(&crate::key::SchemaKey::tagvid("default", "item", vid), &[])
        .await
        .unwrap();
}

async fn create_index(ctx: &ExecutionContext) -> u32 {
    ctx.index_manager
        .as_ref()
        .unwrap()
        .create_tag_index(
            1,
            "item_age_idx".to_string(),
            1,
            "item".to_string(),
            vec!["age".to_string()],
            vec![0],
        )
        .await
        .unwrap()
}

async fn index_vertex(ctx: &ExecutionContext, index_id: u32, part_id: u32, vid: i64) {
    put_vertex(ctx, vid).await;
    ctx.index_manager
        .as_ref()
        .unwrap()
        .insert_tag_index(part_id, index_id, &[IndexValue::Int(10)], vid)
        .await
        .unwrap();
}

const SCAN_QUERIES: [&str; 3] = [
    "MATCH (n) RETURN id(n)",
    "MATCH (n:item) RETURN id(n)",
    "MATCH (n:item {age: 10}) RETURN id(n)",
];

async fn assert_scan_cap_boundary(query: &str) {
    for size in [1, 2, 3] {
        let (_, ctx) = context(2, 1);
        let index_id = create_index(&ctx).await;
        for vid in 1..=size {
            index_vertex(&ctx, index_id, 1, vid).await;
        }
        let result = MatchExecutor::new(ctx)
            .execute_match(match_plan(query))
            .await;
        if size <= 2 {
            let mut rows = result.unwrap().rows;
            rows.sort_by_key(|row| match row[0] {
                Value::Int(vid) => vid,
                _ => panic!("expected VID"),
            });
            assert_eq!(
                rows,
                (1..=size)
                    .map(|vid| vec![Value::Int(vid)])
                    .collect::<Vec<_>>()
            );
        } else {
            assert!(
                matches!(result, Err(ExecutionError::ResourceExhausted(_))),
                "scan above cap must fail, query={query}, result={result:?}"
            );
        }
    }
}

#[tokio::test]
async fn full_vertex_scan_reports_cap_instead_of_truncating() {
    assert_scan_cap_boundary(SCAN_QUERIES[0]).await;
}

#[tokio::test]
async fn tagvid_scan_reports_cap_instead_of_truncating() {
    assert_scan_cap_boundary(SCAN_QUERIES[1]).await;
}

#[tokio::test]
async fn property_index_scan_reports_cap_instead_of_truncating() {
    assert_scan_cap_boundary(SCAN_QUERIES[2]).await;
}

#[tokio::test]
async fn malformed_index_key_cannot_hide_a_scan_cap_overflow() {
    let (_, ctx) = context(2, 1);
    let index_id = create_index(&ctx).await;
    for vid in 1..=3 {
        put_vertex_with_age(&ctx, vid, Value::Bool(true)).await;
        ctx.index_manager
            .as_ref()
            .unwrap()
            .insert_tag_index(1, index_id, &[IndexValue::Bool(true)], vid)
            .await
            .unwrap();
    }
    let executor = MatchExecutor::new(ctx.clone());
    let query = "MATCH (n:item {age: true}) RETURN id(n)";
    assert!(matches!(
        executor.execute_match(match_plan(query)).await,
        Err(ExecutionError::ResourceExhausted(_))
    ));

    // A prefix without the trailing VID sorts before all valid entries. With
    // cap+1 = 3, the raw scan now sees this key and only two of three vertices.
    // Dropping it during decoding would incorrectly make the scan look complete.
    let malformed = KeyUtils::tag_index_prefix_with_values(1, index_id, &[IndexValue::Bool(true)]);
    ctx.kvstore.put(&malformed, &[]).await.unwrap();
    let result = executor.execute_match(match_plan(query)).await;
    assert!(
        matches!(&result, Err(ExecutionError::Storage(byoridb_storage::StorageError::Index(message)))
            if message.contains("malformed tag-index key")),
        "malformed index entry must fail instead of hiding the cap overflow: {result:?}"
    );
}

#[tokio::test]
async fn disabled_scan_cap_keeps_all_candidates() {
    for query in SCAN_QUERIES {
        let (_, ctx) = context(0, 1);
        let index_id = create_index(&ctx).await;
        for vid in 1..=3 {
            index_vertex(&ctx, index_id, 1, vid).await;
        }
        assert_eq!(
            MatchExecutor::new(ctx)
                .execute_match(match_plan(query))
                .await
                .unwrap()
                .rows
                .len(),
            3
        );
    }
}

#[tokio::test]
async fn explicit_limit_and_offset_preserve_query_window() {
    for query in SCAN_QUERIES {
        let (_, ctx) = context(3, 1);
        let index_id = create_index(&ctx).await;
        for vid in 1..=3 {
            index_vertex(&ctx, index_id, 1, vid).await;
        }
        let result = MatchExecutor::new(ctx)
            .execute_match(match_plan(&format!(
                "{query} ORDER BY id(n) LIMIT 1 OFFSET 1"
            )))
            .await
            .unwrap();
        assert_eq!(result.rows, vec![vec![Value::Int(2)]]);
    }
}

#[tokio::test]
async fn explicit_limit_does_not_hide_an_incomplete_candidate_scan() {
    for query in SCAN_QUERIES {
        let (_, ctx) = context(2, 1);
        let index_id = create_index(&ctx).await;
        for vid in 1..=3 {
            index_vertex(&ctx, index_id, 1, vid).await;
        }
        // Candidate collection precedes projection and query-defined LIMIT.
        // Until it can stream a provably sufficient query window, it must
        // report that its scan budget was exceeded, even with a small LIMIT.
        let result = MatchExecutor::new(ctx)
            .execute_match(match_plan(&format!("{query} LIMIT 1")))
            .await;
        assert!(
            matches!(result, Err(ExecutionError::ResourceExhausted(_))),
            "LIMIT must not disguise an incomplete scan, query={query}, result={result:?}"
        );
    }
}

fn assert_index_error(result: crate::error::Result<crate::executor::ExecutorResult>) {
    assert!(
        matches!(&result, Err(ExecutionError::Storage(byoridb_storage::StorageError::Index(message)))
            if message.contains(&KVStoreError::StoreClosed.to_string())),
        "expected the index storage failure, got {result:?}"
    );
}

#[tokio::test]
async fn failed_property_index_partition_never_becomes_partial_success() {
    for failed_partition in [1, 2] {
        let (store, ctx) = context(0, 2);
        let index_id = create_index(&ctx).await;
        index_vertex(&ctx, index_id, 1, 1).await;
        index_vertex(&ctx, index_id, 2, 2).await;
        *store.fail_prefix.lock().unwrap() = Some(KeyUtils::tag_index_prefix_with_values(
            failed_partition,
            index_id,
            &[IndexValue::Int(10)],
        ));
        let result = MatchExecutor::new(ctx)
            .execute_match(match_plan(SCAN_QUERIES[2]))
            .await;
        assert_eq!(store.failures.load(Ordering::SeqCst), 1);
        assert_index_error(result);
    }
}

#[tokio::test]
async fn failed_only_property_index_partition_never_falls_back_to_success() {
    let (store, ctx) = context(0, 1);
    let index_id = create_index(&ctx).await;
    index_vertex(&ctx, index_id, 1, 1).await;
    *store.fail_prefix.lock().unwrap() = Some(KeyUtils::tag_index_prefix_with_values(
        1,
        index_id,
        &[IndexValue::Int(10)],
    ));
    let result = MatchExecutor::new(ctx)
        .execute_match(match_plan(SCAN_QUERIES[2]))
        .await;
    assert_eq!(store.failures.load(Ordering::SeqCst), 1);
    assert_index_error(result);
}

#[tokio::test]
async fn successful_property_index_partitions_keep_all_rows() {
    let (_, ctx) = context(2, 2);
    let index_id = create_index(&ctx).await;
    index_vertex(&ctx, index_id, 1, 1).await;
    index_vertex(&ctx, index_id, 2, 2).await;
    let result = MatchExecutor::new(ctx)
        .execute_match(match_plan(&format!("{} ORDER BY id(n)", SCAN_QUERIES[2])))
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(1)], vec![Value::Int(2)]]);
}

#[tokio::test]
async fn failed_indexed_group_count_never_falls_back_to_success() {
    let (store, ctx) = context(0, 1);
    let index_id = create_index(&ctx).await;
    index_vertex(&ctx, index_id, 1, 1).await;
    *store.fail_prefix.lock().unwrap() = Some(KeyUtils::tag_index_prefix(1, index_id));
    let result = MatchExecutor::new(ctx)
        .execute_match(match_plan(
            "MATCH (n:item) RETURN n.item.age AS age, count(*) AS total GROUP BY n.item.age",
        ))
        .await;
    assert_eq!(store.failures.load(Ordering::SeqCst), 1);
    assert_index_error(result);
}

#[tokio::test]
async fn empty_property_index_is_a_true_no_match() {
    let (_, ctx) = context(2, 2);
    create_index(&ctx).await;
    let result = MatchExecutor::new(ctx)
        .execute_match(match_plan(SCAN_QUERIES[2]))
        .await
        .unwrap();
    assert!(result.rows.is_empty());
}

async fn seed_chain(ctx: &ExecutionContext, size: i64) {
    for vid in 1..=size {
        put_vertex(ctx, vid).await;
        if vid < size {
            let blob = VertexCodec::encode_edge(&EdgeData {
                src_vid: vid,
                dst_vid: vid + 1,
                edge_type: "link".to_string(),
                ranking: 0,
                properties: HashMap::new(),
            })
            .unwrap();
            ctx.kvstore
                .put(
                    &crate::key::SchemaKey::edge_data("default", vid, "link", vid + 1, 0),
                    &blob,
                )
                .await
                .unwrap();
        }
    }
}

const RANGED_MATCH: &str = "MATCH (n)-[:link*1..3]->(m) WHERE id(n) == 1 \
    RETURN id(m) ORDER BY id(m)";

#[tokio::test]
async fn variable_length_expansion_reports_cap_instead_of_partial_terminals() {
    for size in [1, 2, 3] {
        let (_, mut ctx) = context(0, 1);
        Arc::get_mut(&mut ctx).unwrap().config.max_traversal_nodes = 2;
        seed_chain(&ctx, size).await;
        let result = MatchExecutor::new(ctx)
            .execute_match(match_plan(RANGED_MATCH))
            .await;
        if size <= 2 {
            assert_eq!(
                result.unwrap().rows,
                (2..=size)
                    .map(|vid| vec![Value::Int(vid)])
                    .collect::<Vec<_>>()
            );
        } else {
            assert!(
                matches!(result, Err(ExecutionError::ResourceExhausted(_))),
                "traversal above cap must fail, got {result:?}"
            );
        }
    }
}

#[tokio::test]
async fn disabled_variable_length_cap_keeps_all_terminals() {
    let (_, mut ctx) = context(0, 1);
    Arc::get_mut(&mut ctx).unwrap().config.max_traversal_nodes = 0;
    seed_chain(&ctx, 3).await;
    let result = MatchExecutor::new(ctx)
        .execute_match(match_plan(RANGED_MATCH))
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(2)], vec![Value::Int(3)]]);
}

#[tokio::test]
async fn optional_variable_length_cap_never_becomes_null_or_partial_success() {
    let (_, mut ctx) = context(0, 1);
    Arc::get_mut(&mut ctx).unwrap().config.max_traversal_nodes = 2;
    seed_chain(&ctx, 3).await;
    let result = MatchExecutor::new(ctx)
        .execute_match(match_plan(
            "MATCH (n:item) OPTIONAL MATCH (n)-[:link*1..3]->(m) RETURN id(n), id(m)",
        ))
        .await;
    assert!(
        matches!(result, Err(ExecutionError::ResourceExhausted(_))),
        "optional traversal cap must remain an error, got {result:?}"
    );
}
