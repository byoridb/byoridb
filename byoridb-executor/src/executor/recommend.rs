// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! RECOMMEND — neighbor-overlap similarity (PLAN.md R-1, structural phase).
//!
//! `RECOMMEND SIMILAR TO <vid> OVER <edges>|* [LIMIT k]` returns the top-k
//! vertices whose neighborhood most overlaps the seed's, ranked by Jaccard
//! similarity over the chosen edge types:
//!
//! ```text
//!   sim(a, b) = |N(a) ∩ N(b)| / |N(a) ∪ N(b)|
//! ```
//!
//! where `N(v)` is the set of distinct out-neighbors of `v` over `over_edges`
//! (empty = all edge types). This is the graph-structural recommendation
//! layer: two products are "similar" when they point to many shared attribute
//! nodes (brand, category, spec...). It needs no embeddings and reuses the
//! forward edge prefix scan plus the O-1 reverse-edge index.
//!
//! **Candidate generation.** Only vertices sharing ≥1 neighbor with the seed
//! can have nonzero similarity, so candidates are gathered by walking each of
//! the seed's neighbors *back* through the reverse-edge index
//! ([`algo::get_incoming_neighbors`]) instead of scanning every vertex —
//! O(Σ in-degree of the seed's neighbors), bounded by `max_traversal_nodes`.
//!
//! **Caveat (O-1).** Candidate generation reads the reverse-edge index, so a
//! space loaded before O-1 was introduced has no in-edge entries and yields no
//! candidates until reloaded (same caveat as reverse GO / BIDIRECT paths).

use super::{Executor, ExecutorResult};
use crate::algo;
use crate::error::Result;
use byoridb_parser::ast::SimilarityMetric;
use std::collections::HashSet;

impl Executor {
    pub(super) async fn execute_recommend(
        &self,
        plan: crate::plan::RecommendPlan,
    ) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();
        let cap = self.ctx.config.max_traversal_nodes;

        // N(seed): distinct out-neighbors over the chosen edge types.
        let seed_neighbors =
            algo::get_neighbors(&self.ctx, &space, plan.src_vid, &plan.over_edges).await?;
        let seed_set: HashSet<i64> = seed_neighbors.iter().map(|n| n.dst).collect();

        // No neighborhood → nothing to compare against.
        if seed_set.is_empty() {
            return Ok(recommend_result(Vec::new()));
        }

        // Candidates = vertices sharing ≥1 neighbor with the seed, discovered by
        // walking each shared neighbor back through the reverse-edge index.
        // Excludes the seed itself. Capped at `max_traversal_nodes`.
        let mut candidates: HashSet<i64> = HashSet::new();
        let mut cap_reached = false;
        'gather: for feature in &seed_set {
            let incoming =
                algo::get_incoming_neighbors(&self.ctx, &space, *feature, &plan.over_edges).await?;
            for n in incoming {
                if n.dst == plan.src_vid {
                    continue;
                }
                candidates.insert(n.dst);
                if candidates.len() >= cap {
                    cap_reached = true;
                    break 'gather;
                }
            }
        }

        // Score each candidate by Jaccard over its own out-neighborhood.
        let mut scored: Vec<(i64, f64, usize)> = Vec::with_capacity(candidates.len());
        for cand in candidates {
            let cand_neighbors =
                algo::get_neighbors(&self.ctx, &space, cand, &plan.over_edges).await?;
            let cand_set: HashSet<i64> = cand_neighbors.iter().map(|n| n.dst).collect();
            let inter = seed_set.intersection(&cand_set).count();
            if inter == 0 {
                continue;
            }
            let union = seed_set.len() + cand_set.len() - inter;
            let score = match plan.metric {
                SimilarityMetric::Jaccard => inter as f64 / union as f64,
            };
            scored.push((cand, score, inter));
        }

        // Rank: score desc, then shared-count desc, then vid asc for a stable,
        // deterministic ordering.
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
        scored.truncate(plan.limit);

        if cap_reached {
            tracing::warn!(
                cap,
                "RECOMMEND candidate set hit max_traversal_nodes; results may be partial"
            );
        }

        Ok(recommend_result(scored))
    }
}

/// Build the result table. Columns are stable even for an empty result so the
/// client always sees the schema.
fn recommend_result(scored: Vec<(i64, f64, usize)>) -> ExecutorResult {
    let rows = scored
        .into_iter()
        .map(|(vid, score, shared)| {
            vec![
                byoridb_common::Value::Int(vid),
                byoridb_common::Value::Float(score),
                byoridb_common::Value::Int(shared as i64),
            ]
        })
        .collect();
    ExecutorResult {
        columns: vec!["vid".to_string(), "score".to_string(), "shared".to_string()],
        rows,
        latency_ms: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ExecutionContext;
    use byoridb_codec::{EdgeData as CodecEdgeData, VertexCodec};
    use byoridb_kvstore::store::MemoryKVStore;
    use byoridb_parser::ast::SimilarityMetric;
    use std::sync::Arc;

    fn create_executor() -> Executor {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kvstore).with_space("default".to_string()));
        Executor::new(ctx)
    }

    /// Writes both the forward edge and the O-1 reverse-edge index, mirroring
    /// production INSERT EDGE so candidate generation can find the edge.
    async fn link(executor: &Executor, src: i64, dst: i64, edge_type: &str) {
        let key = format!("default:edge:{}:{}:{}:0", src, edge_type, dst);
        let edge = CodecEdgeData {
            src_vid: src,
            dst_vid: dst,
            edge_type: edge_type.to_string(),
            ranking: 0,
            properties: std::collections::HashMap::new(),
        };
        let data = VertexCodec::encode_edge(&edge).unwrap();
        executor
            .ctx
            .kvstore
            .put(key.as_bytes(), &data)
            .await
            .unwrap();
        let in_key = crate::key::SchemaKey::in_edge_data("default", dst, edge_type, src, 0);
        executor.ctx.kvstore.put(&in_key, &data).await.unwrap();
    }

    fn plan(src: i64, edges: &[&str], limit: usize) -> crate::plan::RecommendPlan {
        crate::plan::RecommendPlan {
            src_vid: src,
            over_edges: edges.iter().map(|e| e.to_string()).collect(),
            metric: SimilarityMetric::Jaccard,
            limit,
        }
    }

    /// Three products linked to shared attribute nodes:
    ///   1 -> {100 brand, 200 category}
    ///   2 -> {100 brand, 200 category}   (identical → Jaccard 1.0)
    ///   3 -> {100 brand, 300 category}   (one shared → Jaccard 1/3)
    ///   4 -> {999}                       (no overlap → excluded)
    async fn seed_catalog(executor: &Executor) {
        link(executor, 1, 100, "has").await;
        link(executor, 1, 200, "has").await;
        link(executor, 2, 100, "has").await;
        link(executor, 2, 200, "has").await;
        link(executor, 3, 100, "has").await;
        link(executor, 3, 300, "has").await;
        link(executor, 4, 999, "has").await;
    }

    #[tokio::test]
    async fn ranks_by_jaccard_and_excludes_zero_overlap() {
        let executor = create_executor();
        seed_catalog(&executor).await;

        let result = executor
            .execute_recommend(plan(1, &["has"], 10))
            .await
            .unwrap();

        assert_eq!(result.columns, vec!["vid", "score", "shared"]);
        // vid 2 (1.0) ranks above vid 3 (0.333); vid 4 (no overlap) and the
        // seed itself are absent.
        let vids: Vec<i64> = result
            .rows
            .iter()
            .map(|r| match r[0] {
                byoridb_common::Value::Int(v) => v,
                _ => panic!("vid column not int"),
            })
            .collect();
        assert_eq!(vids, vec![2, 3]);

        let top_score = match result.rows[0][1] {
            byoridb_common::Value::Float(f) => f,
            _ => panic!("score column not float"),
        };
        assert!(
            (top_score - 1.0).abs() < 1e-9,
            "expected exact match score 1.0"
        );
    }

    #[tokio::test]
    async fn limit_truncates_results() {
        let executor = create_executor();
        seed_catalog(&executor).await;

        let result = executor
            .execute_recommend(plan(1, &["has"], 1))
            .await
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(matches!(result.rows[0][0], byoridb_common::Value::Int(2)));
    }

    #[tokio::test]
    async fn seed_without_neighbors_returns_empty_with_schema() {
        let executor = create_executor();
        seed_catalog(&executor).await;

        // vid 50 has no out-edges.
        let result = executor
            .execute_recommend(plan(50, &["has"], 10))
            .await
            .unwrap();
        assert!(result.rows.is_empty());
        assert_eq!(result.columns, vec!["vid", "score", "shared"]);
    }

    #[tokio::test]
    async fn edge_type_filter_scopes_the_neighborhood() {
        let executor = create_executor();
        // vid 1 and vid 2 share node 100, but only via different edge types.
        link(&executor, 1, 100, "brand").await;
        link(&executor, 2, 100, "category").await;
        link(&executor, 3, 100, "brand").await;

        // OVER brand: vid 3 shares node 100 with vid 1; vid 2 (category) is out.
        let result = executor
            .execute_recommend(plan(1, &["brand"], 10))
            .await
            .unwrap();
        let vids: Vec<i64> = result
            .rows
            .iter()
            .map(|r| match r[0] {
                byoridb_common::Value::Int(v) => v,
                _ => panic!(),
            })
            .collect();
        assert_eq!(vids, vec![3]);
    }
}
