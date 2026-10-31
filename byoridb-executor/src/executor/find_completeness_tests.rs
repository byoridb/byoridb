//! FIND must distinguish an exhaustive no-path answer from incomplete execution.
use super::*;
use crate::context::ExecutionConfig;
use crate::error::ExecutionError;
use crate::plan::{FindPlan, FindType};
use byoridb_codec::{EdgeData, VertexCodec};
use byoridb_common::Value;
use byoridb_kvstore::{KVStore, KVStoreError, MemoryKVStore, Result as KvResult, VersionRecord};
use byoridb_parser::ast::{Expression, Literal};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

struct FaultStore {
    inner: MemoryKVStore,
    fault: AtomicUsize,
}
#[async_trait::async_trait]
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

    async fn scan_stream(
        &self,
        prefix: &[u8],
    ) -> KvResult<futures::stream::BoxStream<'static, byoridb_kvstore::store::ScanStreamItem>> {
        if self.fault.load(Ordering::SeqCst) == 1 {
            return Err(KVStoreError::StoreClosed);
        }
        let rows = self.inner.scan_prefix(prefix).await?;
        let mut items: Vec<_> = rows.into_iter().map(Ok).collect();
        if self.fault.load(Ordering::SeqCst) == 2 {
            items.push(Err(KVStoreError::StoreClosed));
        }
        Ok(futures::stream::iter(items).boxed())
    }
    async fn batch_get(&self, keys: &[Vec<u8>]) -> KvResult<Vec<Option<Vec<u8>>>> {
        self.inner.batch_get(keys).await
    }
}

fn executor(nodes: usize, paths: usize) -> (Executor, Arc<FaultStore>) {
    let store = Arc::new(FaultStore {
        inner: MemoryKVStore::new(),
        fault: AtomicUsize::new(0),
    });
    let mut ctx = ExecutionContext::new(store.clone()).with_space("default".into());
    ctx.config = ExecutionConfig {
        max_traversal_nodes: nodes,
        max_find_paths: paths,
        ..Default::default()
    };
    (Executor::new(Arc::new(ctx)), store)
}
async fn edge(ex: &Executor, src: i64, dst: i64, cost: f64) {
    let edge = EdgeData {
        src_vid: src,
        dst_vid: dst,
        edge_type: "road".into(),
        ranking: 0,
        properties: HashMap::from([("cost".into(), Value::Float(cost))]),
    };
    let bytes = VertexCodec::encode_edge(&edge).unwrap();
    ex.ctx
        .kvstore
        .put(
            &crate::key::SchemaKey::edge_data("default", src, "road", dst, 0),
            &bytes,
        )
        .await
        .unwrap();
    ex.ctx
        .kvstore
        .put(
            &crate::key::SchemaKey::in_edge_data("default", dst, "road", src, 0),
            &bytes,
        )
        .await
        .unwrap();
}
fn plan(all: bool, weighted: bool, upto: Option<u32>, target: i64) -> FindPlan {
    FindPlan {
        find_type: if all {
            FindType::AllShortestPaths
        } else {
            FindType::ShortestPath
        },
        from_vid: Expression::Literal(Literal::Int(1)),
        to_vid: Expression::Literal(Literal::Int(target)),
        over_edge: "road".into(),
        weight_prop: weighted.then(|| "cost".into()),
        bidirect: false,
        upto_steps: upto,
        where_clause: None,
        yield_clause: None,
    }
}
fn paths(result: ExecutorResult) -> Vec<Vec<i64>> {
    result
        .rows
        .into_iter()
        .map(|r| match &r[0] {
            Value::List(l) => l
                .values
                .iter()
                .map(|v| match v {
                    Value::Int(n) => *n,
                    _ => panic!("nonintegerVID"),
                })
                .collect(),
            _ => panic!("nonpath"),
        })
        .collect()
}
#[tokio::test]
async fn find_completeness_traversal_cap_is_error_below_and_success_at_boundary() {
    for all in [false, true] {
        for cap in [1, 2, 3, 0] {
            let (ex, _) = executor(cap, 0);
            edge(&ex, 1, 2, 1.0).await;
            edge(&ex, 2, 3, 1.0).await;
            let result = ex.execute_find(plan(all, false, Some(3), 3)).await;
            if cap == 1 {
                assert!(
                    matches!(result, Err(ExecutionError::ResourceExhausted(_))),
                    "cap{cap},all{all}: {result:?}"
                );
            } else {
                assert_eq!(paths(result.unwrap()), vec![vec![1, 2, 3]]);
            }
        }
    }
}
#[tokio::test]
async fn find_completeness_hop_boundary_and_exhaustive_no_path_are_success() {
    for all in [false, true] {
        let (ex, _) = executor(1, 0);
        edge(&ex, 1, 2, 1.0).await;
        assert!(ex
            .execute_find(plan(all, false, Some(1), 9))
            .await
            .unwrap()
            .rows
            .is_empty());
        let (ex, _) = executor(2, 0);
        edge(&ex, 1, 2, 1.0).await;
        assert!(ex
            .execute_find(plan(all, false, Some(10), 9))
            .await
            .unwrap()
            .rows
            .is_empty());
        assert!(ex
            .execute_find(plan(all, false, Some(0), 9))
            .await
            .unwrap()
            .rows
            .is_empty());
    }
}
#[tokio::test]
async fn find_completeness_all_paths_exact_cap_and_zero_succeed_overflow_fails() {
    for cap in [0, 1, 2, 3] {
        let (ex, _) = executor(0, cap);
        for mid in [2, 3] {
            edge(&ex, 1, mid, 1.0).await;
            edge(&ex, mid, 4, 1.0).await;
        }
        let result = ex.execute_find(plan(true, false, Some(2), 4)).await;
        if cap == 1 {
            assert!(
                matches!(result, Err(ExecutionError::ResourceExhausted(_))),
                "{result:?}"
            );
        } else {
            let mut got = paths(result.unwrap());
            got.sort();
            assert_eq!(got, vec![vec![1, 2, 4], vec![1, 3, 4]]);
        }
    }
}
#[tokio::test]
async fn find_completeness_all_paths_midlevel_cap_does_not_return_first_path() {
    let (ex, _) = executor(2, 0);
    for mid in [2, 3] {
        edge(&ex, 1, mid, 1.0).await;
        edge(&ex, mid, 4, 1.0).await;
    }
    assert!(matches!(
        ex.execute_find(plan(true, false, Some(2), 4)).await,
        Err(ExecutionError::ResourceExhausted(_))
    ));
}
#[tokio::test]
async fn find_completeness_weighted_upto_requires_layered_distance_state() {
    let (ex, _) = executor(0, 0);
    // The cheapest prefix to 2 uses both permitted hops; 1->2 must survive
    // as a distinct state so the feasible optimum 1->2->4 is not lost.
    for (s, d, w) in [
        (1, 2, 5.0),
        (1, 3, 1.0),
        (3, 2, 1.0),
        (2, 4, 1.0),
        (1, 4, 10.0),
    ] {
        edge(&ex, s, d, w).await;
    }
    assert_eq!(
        paths(
            ex.execute_find(plan(false, true, Some(1), 4))
                .await
                .unwrap()
        ),
        vec![vec![1, 4]]
    );
    assert_eq!(
        paths(
            ex.execute_find(plan(false, true, Some(2), 4))
                .await
                .unwrap()
        ),
        vec![vec![1, 2, 4]]
    );
    assert_eq!(
        paths(
            ex.execute_find(plan(false, true, Some(3), 4))
                .await
                .unwrap()
        ),
        vec![vec![1, 3, 2, 4]]
    );
    assert!(ex
        .execute_find(plan(false, true, Some(0), 4))
        .await
        .unwrap()
        .rows
        .is_empty());
}
#[tokio::test]
async fn find_completeness_weighted_cap_and_boundary_are_distinct() {
    for cap in [1, 2, 3, 0] {
        let (ex, _) = executor(cap, 0);
        edge(&ex, 1, 2, 1.0).await;
        edge(&ex, 2, 3, 1.0).await;
        let result = ex.execute_find(plan(false, true, Some(2), 3)).await;
        if cap == 1 {
            assert!(
                matches!(result, Err(ExecutionError::ResourceExhausted(_))),
                "{result:?}"
            );
        } else {
            assert_eq!(paths(result.unwrap()), vec![vec![1, 2, 3]]);
        }
    }
    let (ex, _) = executor(1, 0);
    edge(&ex, 1, 2, 1.0).await;
    assert!(ex
        .execute_find(plan(false, true, Some(1), 9))
        .await
        .unwrap()
        .rows
        .is_empty());
}
#[tokio::test]
async fn find_completeness_weighted_omitted_upto_preserves_unbounded_search() {
    let (ex, _) = executor(0, 0);
    for src in 1..=11 {
        edge(&ex, src, src + 1, 1.0).await;
    }
    assert_eq!(
        paths(ex.execute_find(plan(false, true, None, 12)).await.unwrap())[0].len(),
        12
    );
}
#[tokio::test]
async fn find_completeness_relevant_corruption_is_error_but_other_types_are_ignored() {
    for payload in [
        b"not-an-edge".as_slice(),
        b"{}",
        br#"{"src":1,"dst":"zero","type":"road"}"#,
    ] {
        for (all, weighted, reverse) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let (ex, _) = executor(0, 0);
            let key = if reverse {
                crate::key::SchemaKey::in_edge_data("default", 1, "road", 2, 0)
            } else {
                crate::key::SchemaKey::edge_data("default", 1, "road", 2, 0)
            };
            ex.ctx.kvstore.put(&key, payload).await.unwrap();
            let mut request = plan(all, weighted, Some(2), 9);
            request.bidirect = reverse;
            assert!(
                ex.execute_find(request).await.is_err(),
                "all{all},weighted{weighted},reverse{reverse}"
            );
        }
    }
    let (ex, _) = executor(0, 0);
    ex.ctx
        .kvstore
        .put(
            &crate::key::SchemaKey::edge_data("default", 1, "other", 2, 0),
            b"broken",
        )
        .await
        .unwrap();
    assert!(ex
        .execute_find(plan(false, false, Some(2), 9))
        .await
        .unwrap()
        .rows
        .is_empty());
}
#[tokio::test]
async fn find_completeness_scan_init_and_stream_errors_propagate() {
    for fault in [1, 2] {
        for (all, weighted) in [(false, false), (true, false), (false, true)] {
            let (ex, store) = executor(0, 0);
            edge(&ex, 1, 2, 1.0).await;
            store.fault.store(fault, Ordering::SeqCst);
            assert!(matches!(
                ex.execute_find(plan(all, weighted, Some(2), 9)).await,
                Err(ExecutionError::Storage(_))
            ));
        }
    }
}
#[tokio::test]
async fn find_completeness_weighted_invalid_costs_and_overflow_fail() {
    for weight in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let (ex, _) = executor(0, 0);
        edge(&ex, 1, 2, weight).await;
        assert!(matches!(
            ex.execute_find(plan(false, true, Some(2), 2)).await,
            Err(ExecutionError::InvalidOperation(_))
        ));
    }
    let (ex, _) = executor(0, 0);
    edge(&ex, 1, 2, f64::MAX).await;
    edge(&ex, 2, 3, f64::MAX).await;
    assert!(matches!(
        ex.execute_find(plan(false, true, Some(2), 3)).await,
        Err(ExecutionError::InvalidOperation(_))
    ));
}
#[tokio::test]
async fn find_completeness_weighted_early_target_does_not_hide_later_negative_weight() {
    let (ex, _) = executor(0, 0);
    edge(&ex, 1, 2, 1.0).await;
    edge(&ex, 1, 3, 2.0).await;
    edge(&ex, 3, 2, -5.0).await;
    assert!(matches!(
        ex.execute_find(plan(false, true, Some(2), 2)).await,
        Err(ExecutionError::InvalidOperation(_))
    ));
}
#[tokio::test]
async fn find_completeness_start_equals_target_at_zero_hops() {
    let (ex, _) = executor(1, 1);
    for (all, weighted) in [(false, false), (true, false), (false, true)] {
        assert_eq!(
            paths(
                ex.execute_find(plan(all, weighted, Some(0), 1))
                    .await
                    .unwrap()
            ),
            vec![vec![1]]
        );
    }
}

#[tokio::test]
async fn find_completeness_unsupported_where_and_yield_are_rejected() {
    let (ex, _) = executor(0, 0);
    for field in ["where", "yield"] {
        let mut request = plan(false, false, Some(2), 9);
        if field == "where" {
            request.where_clause = Some(Expression::Literal(Literal::Bool(true)));
        } else {
            request.yield_clause = Some("path".into());
        }
        assert!(
            matches!(
                ex.execute_find(request).await,
                Err(ExecutionError::InvalidOperation(_))
            ),
            "{field} was ignored"
        );
    }
}

#[tokio::test]
async fn find_completeness_invalid_options_fail_before_missing_string_vids() {
    let (ex, _) = executor(0, 0);
    ex.ctx
        .kvstore
        .put(
            &crate::key::SchemaKey::space("default"),
            br#"{"vid_type":"FIXED_STRING(32)"}"#,
        )
        .await
        .unwrap();
    for invalid in 0..4 {
        let mut request = plan(false, invalid == 1, None, 9);
        request.from_vid = Expression::Literal(Literal::String("absent".into()));
        request.to_vid = Expression::Literal(Literal::String("missing".into()));
        match invalid {
            0 => request.upto_steps = Some(u32::MAX),
            1 => request.bidirect = true,
            2 => request.where_clause = Some(Expression::Literal(Literal::Bool(true))),
            _ => request.yield_clause = Some("path".into()),
        }
        assert!(
            matches!(
                ex.execute_find(request).await,
                Err(ExecutionError::InvalidOperation(_))
            ),
            "option {invalid} bypassed validation"
        );
    }
}

#[tokio::test]
async fn find_completeness_weighted_stale_heap_entries_do_not_spend_budget() {
    let (ex, _) = executor(3, 0);
    for (src, dst, cost) in [(1, 2, 10.0), (1, 3, 1.0), (3, 2, 1.0), (2, 4, 1.0)] {
        edge(&ex, src, dst, cost).await;
    }
    assert_eq!(
        paths(ex.execute_find(plan(false, true, None, 4)).await.unwrap()),
        vec![vec![1, 3, 2, 4]]
    );
}

#[tokio::test]
async fn find_completeness_weight_validation_respects_hop_and_type_scope() {
    let (ex, _) = executor(0, 0);
    edge(&ex, 1, 2, 1.0).await;
    edge(&ex, 2, 3, -1.0).await;
    ex.ctx
        .kvstore
        .put(
            &crate::key::SchemaKey::edge_data("default", 1, "other", 4, 0),
            b"broken",
        )
        .await
        .unwrap();
    assert!(ex
        .execute_find(plan(false, true, Some(1), 9))
        .await
        .unwrap()
        .rows
        .is_empty());
    assert!(matches!(
        ex.execute_find(plan(false, true, Some(2), 9)).await,
        Err(ExecutionError::InvalidOperation(_))
    ));
}

#[tokio::test]
async fn find_completeness_default_weight_and_zero_weight_cycles_remain_supported() {
    let (ex, _) = executor(0, 0);
    edge(&ex, 1, 2, 0.0).await;
    edge(&ex, 2, 1, 0.0).await;
    edge(&ex, 1, 3, 4.0).await;
    let data = EdgeData {
        src_vid: 2,
        dst_vid: 3,
        edge_type: "road".into(),
        ranking: 0,
        properties: HashMap::new(),
    };
    ex.ctx
        .kvstore
        .put(
            &crate::key::SchemaKey::edge_data("default", 2, "road", 3, 0),
            &VertexCodec::encode_edge(&data).unwrap(),
        )
        .await
        .unwrap();
    for steps in [None, Some(3)] {
        assert_eq!(
            paths(ex.execute_find(plan(false, true, steps, 3)).await.unwrap()),
            vec![vec![1, 2, 3]]
        );
    }
}

#[tokio::test]
async fn find_completeness_payload_identity_and_proto_tail_corruption_fail() {
    let (ex, _) = executor(0, 0);
    let key = crate::key::SchemaKey::edge_data("default", 1, "road", 2, 0);
    let data = EdgeData {
        src_vid: 1,
        dst_vid: 2,
        edge_type: "road".into(),
        ranking: 0,
        properties: HashMap::new(),
    };
    let mut broken = VertexCodec::encode_edge(&data).unwrap();
    broken.push(0x80);
    for bytes in [broken, br#"{"src":1,"dst":3,"type":"road"}"#.to_vec()] {
        ex.ctx.kvstore.put(&key, &bytes).await.unwrap();
        assert!(matches!(
            ex.execute_find(plan(false, false, Some(1), 2)).await,
            Err(ExecutionError::InvalidOperation(_))
        ));
    }
    ex.ctx.kvstore.delete(&key).await.unwrap();
    ex.ctx
        .kvstore
        .put(
            &crate::key::SchemaKey::edge_data("default", 1, "road", 0, 0),
            br#"{"src":1,"dst":0,"type":"road"}"#,
        )
        .await
        .unwrap();
    assert_eq!(
        paths(
            ex.execute_find(plan(false, false, Some(1), 0))
                .await
                .unwrap()
        ),
        vec![vec![1, 0]]
    );
}
