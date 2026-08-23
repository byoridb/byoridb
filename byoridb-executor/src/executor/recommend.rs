// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! RECOMMEND — similar-vertex recommendation (PLAN.md R track).
//!
//! `RECOMMEND SIMILAR TO <vid> ( OVER <edges>|* | BY EMBEDDING <prop> ) [LIMIT k]`
//! returns the top-k vertices most similar to the seed. Two modes:
//!
//! - **Neighbors (R-1, structural).** Jaccard overlap of out-neighbor sets:
//!   `sim(a,b) = |N(a)∩N(b)| / |N(a)∪N(b)|`. Candidate generation walks each of
//!   the seed's neighbors back through the O-1 reverse-edge index, so only
//!   vertices sharing ≥1 neighbor are scored (no full scan).
//!
//! - **Embedding (R-2a, semantic).** Cosine over a stored embedding property
//!   (a numeric list). INSERT VERTEX mirrors every numeric-list property as
//!   packed little-endian f32 under `{space}:vec:{prop}:{vid}` (see
//!   [`pack_embedding`]), so KNN scans only the packed floats — it never decodes
//!   a full vertex on the hot path. This catches cross-channel matches that the
//!   structural mode can't (different titles, same meaning).
//!
//! **Deletes.** DELETE VERTEX decodes the vertex it removes, deletes the dense
//! entries for its numeric-list properties, and marks the affected vector
//! indexes dirty — so a deleted vid leaves no stale dense entry or index point.
//! As a belt-and-suspenders guard the embedding path still verifies that each
//! emitted top-k vid has a live vertex (a bounded `k` lookups, off the scan hot
//! path), covering any window before an index rebuild.

use super::{Executor, ExecutorResult};
use crate::algo;
use crate::error::{ExecutionError, Result};
use crate::evaluator::{EvalContext, Evaluator};
use crate::key::SchemaKey;
use byoridb_common::Value;
use byoridb_parser::ast::{Expression, RecommendBy};
use std::collections::{HashMap, HashSet};

impl Executor {
    pub(super) async fn execute_recommend(
        &self,
        plan: crate::plan::RecommendPlan,
    ) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();
        if matches!(
            crate::vid::space_vid_type(&self.ctx, &space).await?,
            crate::vid::SpaceVidType::FixedString(_)
        ) {
            return Err(ExecutionError::InvalidOperation(format!(
                "RECOMMEND is INT64-only and is unsupported in FIXED_STRING space '{space}'"
            )));
        }
        // O-8 D5: recommend from the owl:sameAs representative. If the seed was
        // merged away, its edges/embedding now live on the representative, so a
        // RECOMMEND on the merged-away vid normalizes to the surviving node —
        // consistent with GO/FETCH/MATCH. Candidates need no normalization: the
        // merge already collapsed non-representative edges/vectors onto the rep.
        let src_vid = crate::ontology::representative_of(&self.ctx, &space, plan.src_vid).await?;
        let filter = plan.filter.as_ref();
        match &plan.by {
            RecommendBy::Neighbors { over_edges, .. } => {
                self.recommend_neighbors(&space, src_vid, over_edges, filter, plan.limit)
                    .await
            }
            RecommendBy::Embedding { prop } => {
                self.recommend_embedding(&space, src_vid, prop, filter, plan.limit)
                    .await
            }
            RecommendBy::Blend {
                embedding_prop,
                embedding_weight,
                over_edges,
                structural_weight,
            } => {
                self.recommend_blend(
                    &space,
                    src_vid,
                    embedding_prop,
                    *embedding_weight,
                    over_edges,
                    *structural_weight,
                    filter,
                    plan.limit,
                )
                .await
            }
        }
    }

    /// Shared-neighbor Jaccard candidate scores `(vid, jaccard, shared_count)`,
    /// unsorted; empty if the seed has no neighbors. Candidates are vertices
    /// sharing ≥1 neighbor with the seed (found via the O-1 reverse-edge index),
    /// excluding the seed, capped at `max_traversal_nodes`.
    async fn structural_scores(
        &self,
        space: &str,
        src_vid: i64,
        over_edges: &[String],
    ) -> Result<Vec<(i64, f64, usize)>> {
        let cap = self.ctx.config.max_traversal_nodes;
        let seed_neighbors = algo::get_neighbors(&self.ctx, space, src_vid, over_edges).await?;
        let seed_set: HashSet<i64> = seed_neighbors.iter().map(|n| n.dst).collect();
        if seed_set.is_empty() {
            return Ok(Vec::new());
        }

        let mut candidates: HashSet<i64> = HashSet::new();
        let mut cap_reached = false;
        'gather: for feature in &seed_set {
            let incoming =
                algo::get_incoming_neighbors(&self.ctx, space, *feature, over_edges).await?;
            for n in incoming {
                if n.dst == src_vid {
                    continue;
                }
                candidates.insert(n.dst);
                if candidates.len() >= cap {
                    cap_reached = true;
                    break 'gather;
                }
            }
        }

        let mut scored: Vec<(i64, f64, usize)> = Vec::with_capacity(candidates.len());
        for cand in candidates {
            let cand_neighbors = algo::get_neighbors(&self.ctx, space, cand, over_edges).await?;
            let cand_set: HashSet<i64> = cand_neighbors.iter().map(|n| n.dst).collect();
            let inter = seed_set.intersection(&cand_set).count();
            if inter == 0 {
                continue;
            }
            let union = seed_set.len() + cand_set.len() - inter;
            scored.push((cand, inter as f64 / union as f64, inter));
        }
        if cap_reached {
            tracing::warn!(
                cap,
                "RECOMMEND candidate set hit max_traversal_nodes; results may be partial"
            );
        }
        Ok(scored)
    }

    /// R-1 structural Jaccard over shared neighbors.
    async fn recommend_neighbors(
        &self,
        space: &str,
        src_vid: i64,
        over_edges: &[String],
        filter: Option<&Expression>,
        limit: usize,
    ) -> Result<ExecutorResult> {
        let mut scored = self.structural_scores(space, src_vid, over_edges).await?;

        // Rank: score desc, then shared-count desc, then vid asc (deterministic).
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));

        // No filter → fast path (no vertex decode), preserving R-1 behavior.
        if filter.is_none() {
            scored.truncate(limit);
            return Ok(neighbors_result(scored));
        }

        // R-3a/R-3b filter: keep only candidates whose vertex satisfies the
        // predicate. The seed's own properties are bound as `seed` so the
        // predicate can compare against the source (e.g. `channel != seed.channel`).
        // Class sets (`is_a`) are computed per candidate only when the filter uses it.
        let needs_isa = filter.is_some_and(filter_uses_isa);
        let seed_props = self
            .load_candidate_props(space, src_vid, false)
            .await?
            .unwrap_or_default();
        let mut out = Vec::new();
        for (vid, score, shared) in scored {
            if out.len() >= limit {
                break;
            }
            if let Some(props) = self.load_candidate_props(space, vid, needs_isa).await? {
                if passes_filter(filter, &props, &seed_props) {
                    out.push((vid, score, shared));
                }
            }
        }
        Ok(neighbors_result(out))
    }

    /// R-2a embedding cosine KNN over the dense side-store.
    async fn recommend_embedding(
        &self,
        space: &str,
        src_vid: i64,
        prop: &str,
        filter: Option<&Expression>,
        limit: usize,
    ) -> Result<ExecutorResult> {
        // Seed embedding from the dense store; absent → empty result.
        let seed_key = SchemaKey::vec_data(space, prop, src_vid);
        let seed = match self.ctx.kvstore.get(&seed_key).await? {
            Some(bytes) => unpack_embedding(&bytes),
            None => return Ok(embedding_result(Vec::new())),
        };
        if seed.is_empty() {
            return Ok(embedding_result(Vec::new()));
        }

        // Candidate scoring: persisted HNSW index for large catalogs (R-2b),
        // exact flat KNN below the threshold (R-2a). Both return (vid, cosine)
        // sorted by score desc; the seed itself is excluded here.
        //
        // ANN over-fetch budget scales with LIMIT, with extra headroom when a
        // WHERE filter may discard candidates (flat ignores it — returns all).
        let budget = if filter.is_some() {
            limit.saturating_mul(16).max(256)
        } else {
            limit.saturating_add(32)
        };
        let scored: Vec<(i64, f32)> = self
            .scored_embedding_candidates(space, prop, &seed, budget)
            .await?
            .into_iter()
            .filter(|(vid, _)| *vid != src_vid)
            .collect();

        // Seed properties for seed-relative predicates (e.g. `channel != seed.channel`),
        // loaded once. Only needed when a filter is present. Class sets (`is_a`)
        // are computed per candidate only when the filter uses it.
        let needs_isa = filter.is_some_and(filter_uses_isa);
        let seed_props = if filter.is_some() {
            self.load_candidate_props(space, src_vid, false)
                .await?
                .unwrap_or_default()
        } else {
            HashMap::new()
        };

        // Emit top-k. Bounded by `limit` live+matching results, off the scan
        // hot path. No filter → lightweight existence check (a get, no decode),
        // preserving R-2a's hot path; with a filter → decode once for the
        // predicate (the decode doubles as the stale-entry guard).
        let mut rows = Vec::new();
        for (vid, score) in scored {
            if rows.len() >= limit {
                break;
            }
            let keep = if filter.is_none() {
                self.ctx
                    .kvstore
                    .get(&SchemaKey::vertex(space, vid))
                    .await?
                    .is_some()
            } else {
                match self.load_candidate_props(space, vid, needs_isa).await? {
                    Some(props) => passes_filter(filter, &props, &seed_props),
                    None => false, // stale (deleted) vid
                }
            };
            if keep {
                rows.push(vec![Value::Int(vid), Value::Float(score as f64)]);
            }
        }

        Ok(ExecutorResult {
            columns: vec!["vid".to_string(), "score".to_string()],
            rows,
            latency_ms: 0,
        })
    }

    /// R-3b blended re-ranking: `score = w_emb·max(0,cosine) + w_struct·jaccard`.
    /// Candidates are the union of the embedding and structural signals (a
    /// missing signal contributes 0). Cosine is clamped to `[0,1]` (negative =
    /// dissimilar) so both 0..1 signals combine on one scale; weights are caller-
    /// supplied. Result columns: vid / score / emb (cosine) / struct (jaccard).
    #[allow(clippy::too_many_arguments)]
    async fn recommend_blend(
        &self,
        space: &str,
        src_vid: i64,
        prop: &str,
        w_emb: f64,
        over_edges: &[String],
        w_struct: f64,
        filter: Option<&Expression>,
        limit: usize,
    ) -> Result<ExecutorResult> {
        // Embedding cosine per candidate (vid → cosine), seed excluded.
        let emb_scores: HashMap<i64, f32> = match self
            .ctx
            .kvstore
            .get(&SchemaKey::vec_data(space, prop, src_vid))
            .await?
        {
            Some(bytes) => {
                let seed = unpack_embedding(&bytes);
                if seed.is_empty() {
                    HashMap::new()
                } else {
                    let budget = limit.saturating_mul(16).max(256);
                    self.scored_embedding_candidates(space, prop, &seed, budget)
                        .await?
                        .into_iter()
                        .filter(|(vid, _)| *vid != src_vid)
                        .collect()
                }
            }
            None => HashMap::new(),
        };

        // Structural Jaccard per candidate (vid → jaccard).
        let struct_scores: HashMap<i64, f64> = self
            .structural_scores(space, src_vid, over_edges)
            .await?
            .into_iter()
            .map(|(vid, jac, _)| (vid, jac))
            .collect();

        // Union of both signals, blended.
        let mut candidates: HashSet<i64> = HashSet::new();
        candidates.extend(emb_scores.keys().copied());
        candidates.extend(struct_scores.keys().copied());
        let mut scored: Vec<(i64, f64, f64, f64)> = candidates
            .into_iter()
            .map(|vid| {
                let cos = (emb_scores.get(&vid).copied().unwrap_or(0.0) as f64).max(0.0);
                let jac = struct_scores.get(&vid).copied().unwrap_or(0.0);
                (vid, w_emb * cos + w_struct * jac, cos, jac)
            })
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));

        // Filter (WHERE + is_a) + existence + top-k.
        let needs_isa = filter.is_some_and(filter_uses_isa);
        let seed_props = if filter.is_some() {
            self.load_candidate_props(space, src_vid, false)
                .await?
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        let mut rows = Vec::new();
        for (vid, blended, cos, jac) in scored {
            if rows.len() >= limit {
                break;
            }
            let keep = if filter.is_none() {
                self.ctx
                    .kvstore
                    .get(&SchemaKey::vertex(space, vid))
                    .await?
                    .is_some()
            } else {
                match self.load_candidate_props(space, vid, needs_isa).await? {
                    Some(props) => passes_filter(filter, &props, &seed_props),
                    None => false,
                }
            };
            if keep {
                rows.push(vec![
                    Value::Int(vid),
                    Value::Float(blended),
                    Value::Float(cos),
                    Value::Float(jac),
                ]);
            }
        }
        Ok(ExecutorResult {
            columns: vec![
                "vid".to_string(),
                "score".to_string(),
                "emb".to_string(),
                "struct".to_string(),
            ],
            rows,
            latency_ms: 0,
        })
    }

    /// Load a candidate vertex's flattened properties for R-3a/R-3b filtering.
    /// Returns `None` if the vertex no longer exists (stale dense entry from a
    /// deleted vid). Each property is inserted under both its bare name and
    /// `{tag}.{prop}` so either form resolves in the predicate.
    ///
    /// When `compute_isa` is set, also injects `__isa__` — the vertex's class
    /// set (tags ∪ inferred O-5 types, each expanded with O-3 superclasses) via
    /// the shared [`crate::ontology::vertex_class_set`] — so `is_a("...")`
    /// predicates match subclasses and inferred types. Only computed when the
    /// filter uses `is_a`, so the class walk is skipped otherwise.
    async fn load_candidate_props(
        &self,
        space: &str,
        vid: i64,
        compute_isa: bool,
    ) -> Result<Option<HashMap<String, Value>>> {
        let data = match self.ctx.kvstore.get(&SchemaKey::vertex(space, vid)).await? {
            Some(d) => d,
            None => return Ok(None),
        };
        let vertex = byoridb_codec::VertexCodec::decode_vertex(&data)
            .map_err(|e| crate::error::ExecutionError::Io(std::io::Error::other(e.to_string())))?;
        let mut props = HashMap::new();
        for tag in vertex.tags {
            for (k, v) in tag.properties {
                props.insert(format!("{}.{}", tag.name, k), v.clone());
                props.insert(k, v);
            }
        }
        if compute_isa {
            let set = crate::ontology::vertex_class_set(&self.ctx, space, vid)
                .await?
                .unwrap_or_default();
            let list: Vec<Value> = set.into_iter().map(Value::String).collect();
            props.insert(
                "__isa__".to_string(),
                Value::List(byoridb_common::datatypes::list::List::from(list)),
            );
        }
        Ok(Some(props))
    }
}

/// True if the filter expression references the `is_a(...)` ontology predicate,
/// so the caller knows whether to compute each candidate's class set.
fn filter_uses_isa(expr: &Expression) -> bool {
    match expr {
        Expression::FunctionCall { name, args } => {
            let n = name.to_uppercase();
            n == "IS_A" || n == "ISA" || args.iter().any(filter_uses_isa)
        }
        Expression::BinaryOp { left, right, .. } => filter_uses_isa(left) || filter_uses_isa(right),
        Expression::UnaryOp { operand, .. } => filter_uses_isa(operand),
        Expression::List(items) => items.iter().any(filter_uses_isa),
        Expression::Map(m) => m.values().any(filter_uses_isa),
        _ => false,
    }
}

/// Evaluate the optional filter against a candidate's properties. Bare property
/// names resolve to the candidate; `seed.<prop>` resolves to the source vertex
/// (R-3b seed-relative comparison, e.g. `channel != seed.channel`). No filter →
/// always passes; an evaluation error is treated as "does not pass" (the
/// candidate is dropped rather than failing the whole query).
fn passes_filter(
    filter: Option<&Expression>,
    props: &HashMap<String, Value>,
    seed_props: &HashMap<String, Value>,
) -> bool {
    match filter {
        None => true,
        Some(expr) => {
            let ctx = EvalContext::new()
                .with_current(props.clone())
                .with_variable("seed", seed_props.clone());
            Evaluator::evaluate_condition(expr, &ctx).unwrap_or(false)
        }
    }
}

/// Pack a numeric-list `Value` into little-endian f32 bytes for the dense
/// embedding store. Returns `None` for anything that isn't a non-empty list of
/// numbers (so non-vector properties are not mirrored). `pub(crate)` so INSERT
/// VERTEX (dml) can write the side-store.
pub(crate) fn pack_embedding(value: &Value) -> Option<Vec<u8>> {
    let Value::List(list) = value else {
        return None;
    };
    if list.values.is_empty() {
        return None;
    }
    let mut bytes = Vec::with_capacity(list.values.len() * 4);
    for v in &list.values {
        let f = match v {
            Value::Float(f) => *f as f32,
            Value::Int(i) => *i as f32,
            _ => return None, // a non-numeric element → not an embedding
        };
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    Some(bytes)
}

pub(super) fn unpack_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Neighbors-mode result table (vid/score/shared), stable schema even when empty.
fn neighbors_result(scored: Vec<(i64, f64, usize)>) -> ExecutorResult {
    let rows = scored
        .into_iter()
        .map(|(vid, score, shared)| {
            vec![
                Value::Int(vid),
                Value::Float(score),
                Value::Int(shared as i64),
            ]
        })
        .collect();
    ExecutorResult {
        columns: vec!["vid".to_string(), "score".to_string(), "shared".to_string()],
        rows,
        latency_ms: 0,
    }
}

/// Embedding-mode result table (vid/score), stable schema even when empty.
fn embedding_result(rows: Vec<Vec<Value>>) -> ExecutorResult {
    ExecutorResult {
        columns: vec!["vid".to_string(), "score".to_string()],
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
    use byoridb_parser::ast::{RecommendBy, SimilarityMetric};
    use std::sync::Arc;

    fn create_executor() -> Executor {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kvstore).with_space("default".to_string()));
        Executor::new(ctx)
    }

    #[tokio::test]
    async fn fixed_string_space_rejects_recommend_before_internal_vid_leaks() {
        let executor = create_executor();
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::space("default"),
                &serde_json::to_vec(&serde_json::json!({
                    "id": 1,
                    "name": "default",
                    "vid_type": "FIXED_STRING(32)"
                }))
                .unwrap(),
            )
            .await
            .unwrap();

        let error = executor
            .execute_recommend(neighbors_plan(1, &["has"], 10))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("RECOMMEND is INT64-only"));
    }

    // ---- neighbors (R-1) ----

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

    fn neighbors_plan(src: i64, edges: &[&str], limit: usize) -> crate::plan::RecommendPlan {
        crate::plan::RecommendPlan {
            src_vid: src,
            by: RecommendBy::Neighbors {
                over_edges: edges.iter().map(|e| e.to_string()).collect(),
                metric: SimilarityMetric::Jaccard,
            },
            filter: None,
            limit,
        }
    }

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
            .execute_recommend(neighbors_plan(1, &["has"], 10))
            .await
            .unwrap();

        assert_eq!(result.columns, vec!["vid", "score", "shared"]);
        let vids: Vec<i64> = result
            .rows
            .iter()
            .map(|r| match r[0] {
                Value::Int(v) => v,
                _ => panic!("vid column not int"),
            })
            .collect();
        assert_eq!(vids, vec![2, 3]);
        let top_score = match result.rows[0][1] {
            Value::Float(f) => f,
            _ => panic!("score column not float"),
        };
        assert!((top_score - 1.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn limit_truncates_results() {
        let executor = create_executor();
        seed_catalog(&executor).await;
        let result = executor
            .execute_recommend(neighbors_plan(1, &["has"], 1))
            .await
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(matches!(result.rows[0][0], Value::Int(2)));
    }

    #[tokio::test]
    async fn seed_without_neighbors_returns_empty_with_schema() {
        let executor = create_executor();
        seed_catalog(&executor).await;
        let result = executor
            .execute_recommend(neighbors_plan(50, &["has"], 10))
            .await
            .unwrap();
        assert!(result.rows.is_empty());
        assert_eq!(result.columns, vec!["vid", "score", "shared"]);
    }

    // ---- embedding (R-2a) ----

    /// Write the dense embedding entry the way INSERT VERTEX would, and a
    /// minimal vertex blob so the emitted-result existence check passes.
    async fn put_vec(executor: &Executor, vid: i64, prop: &str, vec: &[f32]) {
        let value = Value::List(byoridb_common::datatypes::list::List::from(
            vec.iter()
                .map(|f| Value::Float(*f as f64))
                .collect::<Vec<_>>(),
        ));
        let bytes = pack_embedding(&value).unwrap();
        let key = SchemaKey::vec_data("default", prop, vid);
        executor.ctx.kvstore.put(&key, &bytes).await.unwrap();

        let vertex = byoridb_codec::VertexData {
            vid,
            tags: vec![byoridb_codec::TagData {
                name: "product".to_string(),
                properties: std::collections::HashMap::new(),
            }],
        };
        let data = VertexCodec::encode_vertex(&vertex).unwrap();
        executor
            .ctx
            .kvstore
            .put(&SchemaKey::vertex("default", vid), &data)
            .await
            .unwrap();
    }

    fn embedding_plan(src: i64, prop: &str, limit: usize) -> crate::plan::RecommendPlan {
        crate::plan::RecommendPlan {
            src_vid: src,
            by: RecommendBy::Embedding {
                prop: prop.to_string(),
            },
            filter: None,
            limit,
        }
    }

    #[tokio::test]
    async fn embedding_cosine_ranks_nearest_first() {
        let executor = create_executor();
        // Seed direction (1,0). vid 2 identical → cos 1; vid 3 ~ (1,1) → cos ~0.707;
        // vid 4 orthogonal (0,1) → cos 0.
        put_vec(&executor, 1, "emb", &[1.0, 0.0]).await;
        put_vec(&executor, 2, "emb", &[2.0, 0.0]).await; // same direction, diff magnitude
        put_vec(&executor, 3, "emb", &[1.0, 1.0]).await;
        put_vec(&executor, 4, "emb", &[0.0, 1.0]).await;

        let result = executor
            .execute_recommend(embedding_plan(1, "emb", 10))
            .await
            .unwrap();

        assert_eq!(result.columns, vec!["vid", "score"]);
        let vids: Vec<i64> = result
            .rows
            .iter()
            .map(|r| match r[0] {
                Value::Int(v) => v,
                _ => panic!(),
            })
            .collect();
        // vid 2 (cos 1) first, then 3 (~0.707), then 4 (cos 0). Seed excluded.
        assert_eq!(vids, vec![2, 3, 4]);
        let top = match result.rows[0][1] {
            Value::Float(f) => f,
            _ => panic!(),
        };
        assert!((top - 1.0).abs() < 1e-6, "cosine of same direction = 1");
    }

    #[tokio::test]
    async fn embedding_limit_and_dim_mismatch() {
        let executor = create_executor();
        put_vec(&executor, 1, "emb", &[1.0, 0.0, 0.0]).await;
        put_vec(&executor, 2, "emb", &[1.0, 0.0, 0.0]).await;
        put_vec(&executor, 3, "emb", &[0.5, 0.5, 0.5]).await;
        put_vec(&executor, 9, "emb", &[1.0, 0.0]).await; // wrong dim → skipped

        let result = executor
            .execute_recommend(embedding_plan(1, "emb", 1))
            .await
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(matches!(result.rows[0][0], Value::Int(2)));
    }

    #[tokio::test]
    async fn embedding_skips_stale_deleted_vertex() {
        let executor = create_executor();
        put_vec(&executor, 1, "emb", &[1.0, 0.0]).await;
        put_vec(&executor, 2, "emb", &[1.0, 0.0]).await;
        // Simulate a deleted vertex: dense entry remains, vertex blob removed.
        executor
            .ctx
            .kvstore
            .delete(&SchemaKey::vertex("default", 2))
            .await
            .unwrap();

        let result = executor
            .execute_recommend(embedding_plan(1, "emb", 10))
            .await
            .unwrap();
        assert!(
            result.rows.is_empty(),
            "stale (deleted) vid must not appear"
        );
    }

    #[tokio::test]
    async fn embedding_missing_seed_returns_empty_with_schema() {
        let executor = create_executor();
        put_vec(&executor, 2, "emb", &[1.0, 0.0]).await;
        let result = executor
            .execute_recommend(embedding_plan(1, "emb", 10))
            .await
            .unwrap();
        assert!(result.rows.is_empty());
        assert_eq!(result.columns, vec!["vid", "score"]);
    }

    /// Dense embedding entry + a vertex blob carrying a `channel` property,
    /// for R-3a hybrid-filter tests.
    async fn put_product(executor: &Executor, vid: i64, vec: &[f32], channel: &str) {
        let value = Value::List(byoridb_common::datatypes::list::List::from(
            vec.iter()
                .map(|f| Value::Float(*f as f64))
                .collect::<Vec<_>>(),
        ));
        let bytes = pack_embedding(&value).unwrap();
        executor
            .ctx
            .kvstore
            .put(&SchemaKey::vec_data("default", "emb", vid), &bytes)
            .await
            .unwrap();
        let mut props = std::collections::HashMap::new();
        props.insert("channel".to_string(), Value::String(channel.to_string()));
        let vertex = byoridb_codec::VertexData {
            vid,
            tags: vec![byoridb_codec::TagData {
                name: "product".to_string(),
                properties: props,
            }],
        };
        let data = VertexCodec::encode_vertex(&vertex).unwrap();
        executor
            .ctx
            .kvstore
            .put(&SchemaKey::vertex("default", vid), &data)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn embedding_with_channel_filter_via_full_pipeline() {
        // The original use case: "items similar to naver:1, but on coupang".
        let executor = create_executor();
        put_product(&executor, 1, &[1.0, 0.0], "naver").await; // seed
        put_product(&executor, 2, &[1.0, 0.0], "naver").await; // closest, but same channel → filtered
        put_product(&executor, 3, &[0.9, 0.1], "coupang").await; // similar, coupang
        put_product(&executor, 4, &[0.0, 1.0], "coupang").await; // less similar, coupang

        let stmt = byoridb_parser::parse(
            "RECOMMEND SIMILAR TO 1 BY EMBEDDING emb WHERE channel = \"coupang\"",
        )
        .unwrap();
        let plan = crate::ExecutionPlanBuilder::build(stmt).unwrap();
        let result = executor.execute(plan).await.unwrap();

        let vids: Vec<i64> = result
            .rows
            .iter()
            .map(|r| match r[0] {
                Value::Int(v) => v,
                _ => panic!(),
            })
            .collect();
        // naver:2 (closest) excluded by filter; coupang 3 (closer) before 4.
        assert_eq!(vids, vec![3, 4]);
    }

    #[tokio::test]
    async fn embedding_seed_relative_filter_different_channel() {
        // "items similar to seed but in a DIFFERENT channel" — no hardcoded value.
        let executor = create_executor();
        put_product(&executor, 1, &[1.0, 0.0], "naver").await; // seed, naver
        put_product(&executor, 2, &[1.0, 0.0], "naver").await; // closest, same channel → excluded
        put_product(&executor, 3, &[0.9, 0.1], "coupang").await;
        put_product(&executor, 4, &[0.5, 0.5], "coupang").await;

        let stmt = byoridb_parser::parse(
            "RECOMMEND SIMILAR TO 1 BY EMBEDDING emb WHERE channel != seed.channel",
        )
        .unwrap();
        let plan = crate::ExecutionPlanBuilder::build(stmt).unwrap();
        let result = executor.execute(plan).await.unwrap();
        let vids: Vec<i64> = result
            .rows
            .iter()
            .map(|r| match r[0] {
                Value::Int(v) => v,
                _ => panic!(),
            })
            .collect();
        // naver:2 excluded (same channel as seed); coupang 3 (closer) before 4.
        assert_eq!(vids, vec![3, 4]);
    }

    /// Write a class def `{name, superclasses}` the way CREATE CLASS (O-3) would.
    async fn put_class(executor: &Executor, name: &str, supers: &[&str]) {
        let json = serde_json::json!({ "name": name, "superclasses": supers });
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::class("default", name),
                &serde_json::to_vec(&json).unwrap(),
            )
            .await
            .unwrap();
    }

    /// A vertex with a single `tag` label and an `emb` embedding (blob + dense).
    async fn put_classified(executor: &Executor, vid: i64, tag: &str, vec: &[f32]) {
        let emb = Value::List(byoridb_common::datatypes::list::List::from(
            vec.iter()
                .map(|f| Value::Float(*f as f64))
                .collect::<Vec<_>>(),
        ));
        let mut props = std::collections::HashMap::new();
        props.insert("emb".to_string(), emb.clone());
        let vertex = byoridb_codec::VertexData {
            vid,
            tags: vec![byoridb_codec::TagData {
                name: tag.to_string(),
                properties: props,
            }],
        };
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::vertex("default", vid),
                &VertexCodec::encode_vertex(&vertex).unwrap(),
            )
            .await
            .unwrap();
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::vec_data("default", "emb", vid),
                &pack_embedding(&emb).unwrap(),
            )
            .await
            .unwrap();
    }

    async fn run_blend(executor: &Executor, w_emb: f64, w_struct: f64) -> Vec<i64> {
        let q = format!(
            "RECOMMEND SIMILAR TO 1 BLEND EMBEDDING emb {} OVER has {}",
            w_emb, w_struct
        );
        let stmt = byoridb_parser::parse(&q).unwrap();
        let plan = crate::ExecutionPlanBuilder::build(stmt).unwrap();
        let result = executor.execute(plan).await.unwrap();
        assert_eq!(result.columns, vec!["vid", "score", "emb", "struct"]);
        result
            .rows
            .iter()
            .map(|r| match r[0] {
                Value::Int(v) => v,
                _ => panic!(),
            })
            .collect()
    }

    #[tokio::test]
    async fn blend_weights_shift_ranking() {
        let executor = create_executor();
        put_vec(&executor, 1, "emb", &[1.0, 0.0]).await; // seed
        put_vec(&executor, 2, "emb", &[1.0, 0.0]).await; // embedding match, no structure
        put_vec(&executor, 3, "emb", &[0.0, 1.0]).await; // embedding orthogonal
        link(&executor, 1, 100, "has").await;
        link(&executor, 3, 100, "has").await; // 1 & 3 share neighbor 100

        // Embedding-dominant weight → vid 2 (cosine 1) tops.
        let emb_first = run_blend(&executor, 1.0, 0.0).await;
        assert_eq!(emb_first[0], 2, "embedding-weighted: vid 2 first");

        // Structure-dominant weight → vid 3 (shares a neighbor) tops.
        let struct_first = run_blend(&executor, 0.0, 1.0).await;
        assert_eq!(struct_first[0], 3, "structure-weighted: vid 3 first");
    }

    #[tokio::test]
    async fn embedding_isa_filter_matches_subclasses() {
        // O-3 hierarchy: dog SUBCLASS OF animal; cat is unrelated (plain tag).
        let executor = create_executor();
        put_class(&executor, "animal", &[]).await;
        put_class(&executor, "dog", &["animal"]).await;
        put_classified(&executor, 1, "dog", &[1.0, 0.0]).await; // seed
        put_classified(&executor, 2, "dog", &[0.9, 0.1]).await; // is_a animal (subclass)
        put_classified(&executor, 3, "cat", &[0.95, 0.05]).await; // NOT animal
        put_classified(&executor, 4, "animal", &[0.8, 0.2]).await; // is_a animal (self)

        let stmt =
            byoridb_parser::parse("RECOMMEND SIMILAR TO 1 BY EMBEDDING emb WHERE is_a(\"animal\")")
                .unwrap();
        let plan = crate::ExecutionPlanBuilder::build(stmt).unwrap();
        let result = executor.execute(plan).await.unwrap();

        let vids: Vec<i64> = result
            .rows
            .iter()
            .map(|r| match r[0] {
                Value::Int(v) => v,
                _ => panic!(),
            })
            .collect();
        // dog(2, subclass) and animal(4, self) match; cat(3) excluded. Cosine order.
        assert_eq!(vids, vec![2, 4]);
    }

    #[tokio::test]
    async fn neighbors_filter_excludes_nonmatching() {
        let executor = create_executor();
        seed_catalog(&executor).await; // 1,2,3 share neighbors; vids are products
                                       // Give 2 and 3 a channel property; 1 is the seed.
        put_product(&executor, 2, &[1.0], "naver").await;
        put_product(&executor, 3, &[1.0], "coupang").await;

        let stmt =
            byoridb_parser::parse("RECOMMEND SIMILAR TO 1 OVER has WHERE channel = \"coupang\"")
                .unwrap();
        let plan = crate::ExecutionPlanBuilder::build(stmt).unwrap();
        let result = executor.execute(plan).await.unwrap();
        let vids: Vec<i64> = result
            .rows
            .iter()
            .map(|r| match r[0] {
                Value::Int(v) => v,
                _ => panic!(),
            })
            .collect();
        // Of structural matches {2,3}, only coupang:3 survives the filter.
        assert_eq!(vids, vec![3]);
    }

    #[tokio::test]
    async fn delete_vertex_cleans_dense_and_marks_index_dirty() {
        // DELETE must remove the dense entry and invalidate the vector index so
        // the deleted vid is not re-indexed on rebuild (recall correctness).
        let executor = create_executor();
        // Vertex blob carries `emb` as a numeric list (as a real INSERT would),
        // plus the dense side-store entry.
        let emb = Value::List(byoridb_common::datatypes::list::List::from(vec![
            Value::Float(1.0),
            Value::Float(0.0),
        ]));
        let mut props = std::collections::HashMap::new();
        props.insert("emb".to_string(), emb.clone());
        let vertex = byoridb_codec::VertexData {
            vid: 1,
            tags: vec![byoridb_codec::TagData {
                name: "product".to_string(),
                properties: props,
            }],
        };
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::vertex("default", 1),
                &VertexCodec::encode_vertex(&vertex).unwrap(),
            )
            .await
            .unwrap();
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::vec_data("default", "emb", 1),
                &pack_embedding(&emb).unwrap(),
            )
            .await
            .unwrap();

        executor
            .execute_delete(crate::plan::DeletePlan {
                space: "default".to_string(),
                vids: vec![1.into()],
                conditions: None,
            })
            .await
            .unwrap();

        // Dense entry removed; vector index marked dirty for rebuild.
        assert!(executor
            .ctx
            .kvstore
            .get(&SchemaKey::vec_data("default", "emb", 1))
            .await
            .unwrap()
            .is_none());
        assert!(executor
            .ctx
            .kvstore
            .get(&SchemaKey::vec_index_dirty("default", "emb"))
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn recommend_seed_normalized_to_sameas_representative() {
        // O-8 connection: a RECOMMEND seeded with a merged-away vid normalizes
        // to its sameAs representative, so it recommends from the surviving
        // node's embedding (the entity-resolution loop's reverse direction).
        let executor = create_executor();
        let exec = |q: &str| {
            let stmt = byoridb_parser::parse(q).expect("parse");
            crate::ExecutionPlanBuilder::build(stmt).expect("build")
        };
        executor
            .execute(exec("CREATE EDGE sameAs()"))
            .await
            .unwrap();

        // Embedding catalog around representative 2.
        put_vec(&executor, 2, "emb", &[1.0, 0.0]).await; // representative seed
        put_vec(&executor, 3, "emb", &[1.0, 0.0]).await; // cos 1 with rep
        put_vec(&executor, 4, "emb", &[0.0, 1.0]).await; // cos 0 with rep

        // Merge 5 → representative 2 (5 has no facts of its own).
        executor
            .execute(exec("INSERT EDGE sameAs() VALUES 5->2:()"))
            .await
            .unwrap();
        assert_eq!(
            crate::ontology::representative_of(&executor.ctx, "default", 5)
                .await
                .unwrap(),
            2
        );

        // RECOMMEND from the merged-away vid 5 → resolves to 2's embedding.
        let result = executor
            .execute(exec("RECOMMEND SIMILAR TO 5 BY EMBEDDING emb"))
            .await
            .unwrap();
        let vids: Vec<i64> = result
            .rows
            .iter()
            .map(|r| match r[0] {
                Value::Int(v) => v,
                _ => panic!(),
            })
            .collect();
        // 3 (cos 1) before 4 (cos 0); representative 2 excluded as the seed.
        assert_eq!(vids, vec![3, 4], "seed 5 normalized to representative 2");
    }

    #[test]
    fn pack_unpack_round_trip() {
        let value = Value::List(byoridb_common::datatypes::list::List::from(vec![
            Value::Float(0.5),
            Value::Float(-0.25),
            Value::Int(2), // numeric ints coerce to f32
        ]));
        let bytes = pack_embedding(&value).expect("numeric list packs");
        assert_eq!(bytes.len(), 12); // 3 × f32
        assert_eq!(unpack_embedding(&bytes), vec![0.5_f32, -0.25, 2.0]);

        // Non-numeric / empty lists are not embeddings.
        assert!(
            pack_embedding(&Value::List(byoridb_common::datatypes::list::List::from(
                vec![Value::String("x".into())]
            )))
            .is_none()
        );
        assert!(
            pack_embedding(&Value::List(byoridb_common::datatypes::list::List::new())).is_none()
        );
        assert!(pack_embedding(&Value::Int(7)).is_none());
    }

    #[tokio::test]
    async fn update_vertex_remirrors_dense_store() {
        // Regression for the UPDATE-stale-vector bug: re-running UPDATE with a
        // new embedding must refresh the dense store, not leave the old vector.
        let executor = create_executor();
        // Tag schema so validate_tag_props accepts the `emb` field.
        let schema = serde_json::json!({
            "name": "product",
            "properties": [{"name": "emb", "data_type": "String", "nullable": true}],
            "version": 1
        });
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::tag("default", "product"),
                &serde_json::to_vec(&schema).unwrap(),
            )
            .await
            .unwrap();

        let update = |vec: Vec<f64>| crate::plan::UpdatePlan {
            space: "default".to_string(),
            vid: 1.into(),
            tag_name: Some("product".to_string()),
            updates: std::collections::HashMap::from([(
                "emb".to_string(),
                Value::List(byoridb_common::datatypes::list::List::from(
                    vec.into_iter().map(Value::Float).collect::<Vec<_>>(),
                )),
            )]),
            conditions: None,
            yield_clause: None,
            edge: None,
        };

        executor
            .execute_update(update(vec![1.0, 0.0, 0.0]))
            .await
            .unwrap();
        let key = SchemaKey::vec_data("default", "emb", 1);
        let first = executor.ctx.kvstore.get(&key).await.unwrap().unwrap();
        assert_eq!(unpack_embedding(&first), vec![1.0_f32, 0.0, 0.0]);

        // Second update must overwrite, not leave the stale vector.
        executor
            .execute_update(update(vec![0.0, 1.0, 0.0]))
            .await
            .unwrap();
        let second = executor.ctx.kvstore.get(&key).await.unwrap().unwrap();
        assert_eq!(unpack_embedding(&second), vec![0.0_f32, 1.0, 0.0]);
    }
}
