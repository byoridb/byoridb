// Copyright (c) 2024 ByoriDB
// Licensed under Apache 2.0 License.

//! Count optimizations must preserve the matched rows and scan budget.

use super::MatchExecutor;
use crate::context::{ExecutionConfig, ExecutionContext};
use crate::error::ExecutionError;
use crate::key::SchemaKey;
use crate::plan::{ExecutionPlan, ExecutionPlanBuilder, MatchPlan};
use byoridb_codec::{EdgeData, TagData, VertexCodec, VertexData};
use byoridb_common::Value;
use byoridb_kvstore::MemoryKVStore;
use byoridb_storage::key::IndexValue;
use std::collections::HashMap;
use std::sync::Arc;

fn context(cap: usize) -> Arc<ExecutionContext> {
    Arc::new(
        ExecutionContext::new(Arc::new(MemoryKVStore::new()))
            .with_space("default".into())
            .with_space_id(1)
            .with_config(ExecutionConfig {
                max_scan_limit: cap,
                ..ExecutionConfig::default()
            }),
    )
}

fn plan(query: &str) -> MatchPlan {
    match ExecutionPlanBuilder::build(byoridb_parser::parse(query).unwrap()).unwrap() {
        ExecutionPlan::Match(plan) => plan,
        _ => panic!("expected MATCH"),
    }
}

#[tokio::test]
async fn public_match_entry_point_rejects_an_incomplete_inference_store() {
    let ctx = context(100);
    ctx.kvstore
        .put(b"default:inference:dirty", b"1")
        .await
        .unwrap();
    let error = MatchExecutor::new(ctx)
        .execute_match(plan("MATCH (n) RETURN COUNT(*)"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("REPAIR INFERENCE"));
}

async fn vertex(ctx: &ExecutionContext, vid: i64, labels: &[&str], group: i64, indexed: bool) {
    let bytes = VertexCodec::encode_vertex(&VertexData {
        vid,
        tags: labels
            .iter()
            .map(|label| TagData {
                name: (*label).into(),
                properties: HashMap::from([("age".into(), Value::Int(group))]),
            })
            .collect(),
    })
    .unwrap();
    ctx.kvstore
        .put(&SchemaKey::vertex("default", vid), &bytes)
        .await
        .unwrap();
    if indexed {
        for label in labels {
            ctx.kvstore
                .put(&SchemaKey::tagvid("default", label, vid), &[])
                .await
                .unwrap();
        }
    }
}

async fn edge(ctx: &ExecutionContext, src: i64, dst: i64) {
    let bytes = VertexCodec::encode_edge(&EdgeData {
        src_vid: src,
        dst_vid: dst,
        edge_type: "link".into(),
        ranking: 0,
        properties: HashMap::new(),
    })
    .unwrap();
    ctx.kvstore
        .put(
            format!("default:edge:{src}:link:{dst}:0").as_bytes(),
            &bytes,
        )
        .await
        .unwrap();
}

async fn group_index(ctx: &ExecutionContext) -> u32 {
    ctx.index_manager
        .as_ref()
        .unwrap()
        .create_tag_index(
            1,
            "item_group".into(),
            1,
            "item".into(),
            vec!["age".into()],
            vec![0],
        )
        .await
        .unwrap()
}

async fn count(ctx: Arc<ExecutionContext>, query: &str) -> i64 {
    let result = MatchExecutor::new(ctx)
        .execute_match(plan(query))
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 1);
    match result.rows[0][0] {
        Value::Int(value) => value,
        ref other => panic!("not count: {other:?}"),
    }
}

#[tokio::test]
async fn simple_count_or_labels_includes_later_labels_once() {
    let ctx = context(0);
    vertex(&ctx, 1, &["item"], 10, true).await;
    vertex(&ctx, 2, &["other"], 10, true).await;
    vertex(&ctx, 3, &["item", "other"], 10, true).await;
    assert_eq!(
        count(ctx.clone(), "MATCH (n:item|other) RETURN count(*)").await,
        3
    );
    assert_eq!(count(ctx, "MATCH (n:item:other) RETURN count(*)").await, 1);
}

#[tokio::test]
async fn simple_count_honors_limit_zero_and_offset() {
    let ctx = context(0);
    vertex(&ctx, 1, &["item"], 10, true).await;
    for query in [
        "MATCH (n:item) RETURN count(*) LIMIT 0",
        "MATCH (n:item) RETURN count(*) OFFSET 1",
        "MATCH (n:item) RETURN count(*) LIMIT 1 OFFSET 1",
    ] {
        let result = MatchExecutor::new(ctx.clone())
            .execute_match(plan(query))
            .await
            .unwrap();
        assert!(result.rows.is_empty(), "{query}: {:?}", result.rows);
    }
    assert_eq!(
        count(ctx, "MATCH (n:item) RETURN count(*) LIMIT 1 OFFSET 0").await,
        1
    );
}

#[tokio::test]
async fn simple_count_tagvid_budget_counts_stale_entries() {
    let ctx = context(2);
    vertex(&ctx, 1, &["item"], 10, true).await;
    for vid in [2, 3] {
        ctx.kvstore
            .put(&SchemaKey::tagvid("default", "item", vid), &[])
            .await
            .unwrap();
    }
    let result = MatchExecutor::new(ctx)
        .execute_match(plan("MATCH (n:item) RETURN count(*) LIMIT 1"))
        .await;
    assert!(
        matches!(result, Err(ExecutionError::ResourceExhausted(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn simple_count_full_scan_budget_counts_unmatched_vertices() {
    let ctx = context(2);
    for vid in 1..=3 {
        vertex(&ctx, vid, &["other"], 10, false).await;
    }
    let result = MatchExecutor::new(ctx)
        .execute_match(plan("MATCH (n:item) RETURN count(*)"))
        .await;
    assert!(
        matches!(result, Err(ExecutionError::ResourceExhausted(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn simple_count_exact_scan_cap_and_zero_are_complete() {
    for indexed in [false, true] {
        for cap in [0, 2] {
            let ctx = context(cap);
            vertex(&ctx, 1, &["item"], 10, indexed).await;
            vertex(&ctx, 2, &["item"], 10, indexed).await;
            assert_eq!(
                count(ctx, "MATCH (n:item) RETURN count(*) LIMIT 1").await,
                2
            );
        }
    }
}

#[tokio::test]
async fn simple_count_stale_only_index_falls_back_with_separate_prefix_budget() {
    let ctx = context(1);
    vertex(&ctx, 1, &["item"], 10, false).await;
    ctx.kvstore
        .put(&SchemaKey::tagvid("default", "item", 999), &[])
        .await
        .unwrap();
    assert_eq!(count(ctx, "MATCH (n:item) RETURN count(*)").await, 1);
}

#[tokio::test]
async fn simple_count_revalidates_multiple_batches_at_the_exact_raw_cap() {
    let ctx = context(258);
    for vid in 1..=256 {
        vertex(&ctx, vid, &["item"], 10, true).await;
    }
    for vid in [257, 258] {
        ctx.kvstore
            .put(&SchemaKey::tagvid("default", "item", vid), &[])
            .await
            .unwrap();
    }
    assert_eq!(count(ctx, "MATCH (n:item) RETURN count(*)").await, 256);
}

#[tokio::test]
async fn simple_count_keeps_required_multiple_pattern_cardinality_before_limit() {
    let ctx = context(0);
    vertex(&ctx, 1, &["item"], 10, true).await;
    for vid in [2, 3] {
        vertex(&ctx, vid, &["other"], 20, true).await;
    }
    assert_eq!(
        count(ctx, "MATCH (n:item), (m:other) RETURN count(*) LIMIT 1").await,
        2
    );
}

#[tokio::test]
async fn indexed_group_count_revalidates_live_vertex_properties_and_stale_entries() {
    let ctx = context(0);
    vertex(&ctx, 1, &["item"], 20, true).await;
    let index = group_index(&ctx).await;
    for vid in [1, 999] {
        ctx.index_manager
            .as_ref()
            .unwrap()
            .insert_tag_index(1, index, &[IndexValue::Int(10)], vid)
            .await
            .unwrap();
    }
    let result = MatchExecutor::new(ctx)
        .execute_match(plan(
            "MATCH (n:item) RETURN n.item.age, count(*) GROUP BY n.item.age",
        ))
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(20), Value::Int(1)]]);
}

#[tokio::test]
async fn grouped_count_respects_start_property_filters() {
    let ctx = context(0);
    let index = group_index(&ctx).await;
    for (vid, group) in [(1, 10), (2, 20)] {
        vertex(&ctx, vid, &["item"], group, true).await;
        ctx.index_manager
            .as_ref()
            .unwrap()
            .insert_tag_index(1, index, &[IndexValue::Int(group)], vid)
            .await
            .unwrap();
    }
    let result = MatchExecutor::new(ctx)
        .execute_match(plan(
            "MATCH (n:item {age: 20}) RETURN n.item.age, count(*) GROUP BY n.item.age",
        ))
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(20), Value::Int(1)]]);
}

#[tokio::test]
async fn grouped_count_respects_required_additional_patterns() {
    let ctx = context(0);
    let index = group_index(&ctx).await;
    vertex(&ctx, 1, &["item"], 10, true).await;
    for vid in [2, 3] {
        vertex(&ctx, vid, &["other"], 20, true).await;
    }
    ctx.index_manager
        .as_ref()
        .unwrap()
        .insert_tag_index(1, index, &[IndexValue::Int(10)], 1)
        .await
        .unwrap();
    let result = MatchExecutor::new(ctx)
        .execute_match(plan(
            "MATCH (n:item), (m:other) RETURN n.item.age, count(*) GROUP BY n.item.age",
        ))
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(10), Value::Int(2)]]);
}

#[tokio::test]
async fn edge_group_count_merges_duplicate_group_values_and_ignores_counters() {
    let ctx = context(0);
    for vid in [1, 2] {
        vertex(&ctx, vid, &["item"], 10, true).await;
        vertex(&ctx, vid + 10, &["other"], 20, true).await;
        edge(&ctx, vid, vid + 10).await;
    }
    ctx.kvstore
        .put(&SchemaKey::outdeg_counter("default", "link", 1), b"corrupt")
        .await
        .unwrap();
    let result = MatchExecutor::new(ctx)
        .execute_match(plan(
            "MATCH (n:item)-[:link]->() RETURN n.item.age, count(*)",
        ))
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(10), Value::Int(2)]]);
}

#[tokio::test]
async fn edge_group_count_does_not_count_missing_endpoints() {
    let ctx = context(0);
    vertex(&ctx, 1, &["item"], 10, true).await;
    edge(&ctx, 1, 999).await;
    let result = MatchExecutor::new(ctx)
        .execute_match(plan(
            "MATCH (n:item)-[:link]->() RETURN n.item.age, count(*)",
        ))
        .await
        .unwrap();
    assert!(result.rows.is_empty(), "{:?}", result.rows);
}

#[tokio::test]
async fn edge_group_count_rejects_corrupt_edge_records() {
    let ctx = context(0);
    vertex(&ctx, 1, &["item"], 10, true).await;
    vertex(&ctx, 2, &["other"], 20, true).await;
    ctx.kvstore
        .put(b"default:edge:1:link:2:0", b"corrupt")
        .await
        .unwrap();
    let result = MatchExecutor::new(ctx)
        .execute_match(plan(
            "MATCH (n:item)-[:link]->() RETURN n.item.age, count(*)",
        ))
        .await;
    assert!(result.is_err(), "{result:?}");
}
