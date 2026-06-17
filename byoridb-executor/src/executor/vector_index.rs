// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Persisted HNSW vector index for `RECOMMEND ... BY EMBEDDING` (PLAN.md R-2b).
//!
//! R-2a scans the dense f32 side-store and computes cosine for every vector —
//! exact, but O(N) per query. R-2b builds an approximate-nearest-neighbour
//! index (HNSW, via the pure-Rust `instant-distance`) over the same dense
//! store and **persists it to redb** so subsequent queries load + search in
//! O(log N) without a full scan.
//!
//! **Lifecycle (D9 = persisted index).**
//! - `{space}:vecidx:{prop}` → `bincode(HnswMap<Emb, vid>)`. The map carries the
//!   point→vid mapping, so search yields vids directly.
//! - `{space}:vecidx-dirty:{prop}` (empty value) marks the index stale. INSERT
//!   and UPDATE of a numeric-list property set it; a rebuild clears it.
//! - A clean, present index is loaded and searched with **no full scan**. A
//!   dirty/missing index triggers one rebuild from the dense store.
//!
//! **Threshold.** `instant-distance` builds the index from the full point set
//! (no cheap incremental insert), so a rebuild is O(N). Below
//! `config.vector_index_min` vectors we skip the index entirely and use exact
//! flat KNN — no index cost, exact results. The index only earns its keep on
//! large catalogs (bulk-ingest then many queries).
//!
//! **Deletes / staleness.** DELETE VERTEX cannot mark the index dirty (it does
//! not decode props), so the index may hold tombstone-less stale vids; the
//! caller's emit-time vertex-existence check drops them. Approximate recall is
//! acceptable for recommendation; exact flat remains the fallback.

use super::Executor;
use crate::error::{ExecutionError, Result};
use crate::key::SchemaKey;
use instant_distance::{Builder, HnswMap, Point, Search};
use serde::{Deserialize, Serialize};

/// An embedding point under cosine distance. `distance` returns
/// `1 - cosine_similarity` so that nearer = smaller, as HNSW expects.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Emb(pub Vec<f32>);

impl Point for Emb {
    fn distance(&self, other: &Self) -> f32 {
        if self.0.len() != other.0.len() {
            return 1.0; // incomparable dimensions → maximally distant
        }
        let mut dot = 0f32;
        let mut na = 0f32;
        let mut nb = 0f32;
        for i in 0..self.0.len() {
            dot += self.0[i] * other.0[i];
            na += self.0[i] * self.0[i];
            nb += other.0[i] * other.0[i];
        }
        if na == 0.0 || nb == 0.0 {
            return 1.0;
        }
        1.0 - dot / (na.sqrt() * nb.sqrt())
    }
}

/// How many candidates an ANN search returns. Generous so the caller's
/// existence + WHERE filtering still has enough to fill `LIMIT`. Heavy
/// filtering on huge catalogs may under-return — raise LIMIT or use flat.
const ANN_SEARCH_BUDGET: usize = 256;

impl Executor {
    /// Mark a property's persisted vector index stale (rebuilt on next query).
    pub(super) async fn mark_vector_index_dirty(&self, space: &str, prop: &str) -> Result<()> {
        self.ctx
            .kvstore
            .put(&SchemaKey::vec_index_dirty(space, prop), &[])
            .await?;
        Ok(())
    }

    /// Return candidate `(vid, cosine_score)` pairs for the seed, sorted by
    /// score descending. Chooses the persisted HNSW index for large catalogs
    /// and exact flat KNN below `vector_index_min`. The seed itself may be
    /// included (it has a dense entry) — the caller excludes it.
    pub(super) async fn scored_embedding_candidates(
        &self,
        space: &str,
        prop: &str,
        seed: &[f32],
    ) -> Result<Vec<(i64, f32)>> {
        let idx_key = SchemaKey::vec_index(space, prop);
        let dirty = self
            .ctx
            .kvstore
            .get(&SchemaKey::vec_index_dirty(space, prop))
            .await?
            .is_some();
        let has_index = self.ctx.kvstore.get(&idx_key).await?.is_some();

        // Fast path: a fresh persisted index → load + ANN search, no full scan.
        if has_index && !dirty {
            return self.ann_query(&idx_key, seed).await;
        }

        // Otherwise scan the dense store once to decide: (re)build the index for
        // large catalogs, or exact flat for small ones.
        let prefix = SchemaKey::vec_data_prop_prefix(space, prop);
        let entries = self.ctx.kvstore.scan_prefix(&prefix).await?;
        let points: Vec<(i64, Vec<f32>)> = entries
            .into_iter()
            .filter_map(|(key, bytes)| {
                SchemaKey::vec_data_vid_from_key(&key)
                    .map(|vid| (vid, super::recommend::unpack_embedding(&bytes)))
            })
            .collect();

        if points.len() > self.ctx.config.vector_index_min {
            self.persist_index(space, prop, &points).await?;
            self.ann_query(&idx_key, seed).await
        } else {
            // Small catalog: drop any stale index, exact flat KNN.
            self.ctx.kvstore.delete(&idx_key).await?;
            self.ctx
                .kvstore
                .delete(&SchemaKey::vec_index_dirty(space, prop))
                .await?;
            Ok(flat_cosine(seed, &points))
        }
    }

    /// Build an HNSW over `points`, serialize it, persist to redb, clear dirty.
    async fn persist_index(
        &self,
        space: &str,
        prop: &str,
        points: &[(i64, Vec<f32>)],
    ) -> Result<()> {
        let embs: Vec<Emb> = points.iter().map(|(_, v)| Emb(v.clone())).collect();
        let vids: Vec<i64> = points.iter().map(|(vid, _)| *vid).collect();
        let map: HnswMap<Emb, i64> = Builder::default().build(embs, vids);
        let bytes = bincode::serialize(&map)
            .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?;
        self.ctx
            .kvstore
            .put(&SchemaKey::vec_index(space, prop), &bytes)
            .await?;
        self.ctx
            .kvstore
            .delete(&SchemaKey::vec_index_dirty(space, prop))
            .await?;
        Ok(())
    }

    /// Load the persisted index and ANN-search for the seed. Returns
    /// `(vid, cosine_score)` sorted by score descending.
    async fn ann_query(&self, idx_key: &[u8], seed: &[f32]) -> Result<Vec<(i64, f32)>> {
        let bytes = match self.ctx.kvstore.get(idx_key).await? {
            Some(b) => b,
            None => return Ok(Vec::new()),
        };
        let map: HnswMap<Emb, i64> = bincode::deserialize(&bytes)
            .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?;
        let query = Emb(seed.to_vec());
        let mut search = Search::default();
        let mut out: Vec<(i64, f32)> = map
            .search(&query, &mut search)
            .take(ANN_SEARCH_BUDGET)
            .map(|item| (*item.value, 1.0 - item.distance)) // distance = 1 - cosine
            .collect();
        // search yields by increasing distance == decreasing score already, but
        // make the ordering explicit and deterministic on ties.
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        Ok(out)
    }
}

/// Exact cosine KNN over `points`, sorted by score descending (vid asc on ties).
/// Zero / dimension-mismatched vectors are skipped.
fn flat_cosine(seed: &[f32], points: &[(i64, Vec<f32>)]) -> Vec<(i64, f32)> {
    let seed_norm = dot(seed, seed).sqrt();
    if seed_norm == 0.0 {
        return Vec::new();
    }
    let mut scored: Vec<(i64, f32)> = points
        .iter()
        .filter_map(|(vid, v)| {
            if v.len() != seed.len() {
                return None;
            }
            let n = dot(v, v).sqrt();
            if n == 0.0 {
                return None;
            }
            Some((*vid, dot(seed, v) / (seed_norm * n)))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored
}

#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ExecutionConfig, ExecutionContext};
    use byoridb_kvstore::store::MemoryKVStore;
    use std::sync::Arc;

    /// Executor whose vector-index threshold is 1, so even a handful of vectors
    /// exercise the persisted HNSW path instead of flat.
    fn ann_executor() -> Executor {
        let kv = Arc::new(MemoryKVStore::new());
        let cfg = ExecutionConfig {
            vector_index_min: 1,
            ..Default::default()
        };
        let ctx = Arc::new(
            ExecutionContext::new(kv)
                .with_space("default".to_string())
                .with_config(cfg),
        );
        Executor::new(ctx)
    }

    /// Default-config executor (threshold 1000 → flat for small inputs).
    fn flat_executor() -> Executor {
        let kv = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kv).with_space("default".to_string()));
        Executor::new(ctx)
    }

    async fn put_dense(exec: &Executor, vid: i64, vec: &[f32]) {
        let bytes: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
        exec.ctx
            .kvstore
            .put(&SchemaKey::vec_data("default", "emb", vid), &bytes)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ann_path_builds_persists_and_ranks() {
        let exec = ann_executor();
        put_dense(&exec, 1, &[1.0, 0.0]).await;
        put_dense(&exec, 2, &[0.9, 0.1]).await;
        put_dense(&exec, 3, &[0.0, 1.0]).await;

        let scored = exec
            .scored_embedding_candidates("default", "emb", &[1.0, 0.0])
            .await
            .unwrap();

        // Index was built and persisted; dirty marker cleared.
        assert!(exec
            .ctx
            .kvstore
            .get(&SchemaKey::vec_index("default", "emb"))
            .await
            .unwrap()
            .is_some());
        assert!(exec
            .ctx
            .kvstore
            .get(&SchemaKey::vec_index_dirty("default", "emb"))
            .await
            .unwrap()
            .is_none());

        // Ranking matches cosine: vid1 (1.0) > vid2 (~0.996) > vid3 (0.0).
        let vids: Vec<i64> = scored.iter().map(|(v, _)| *v).collect();
        assert_eq!(vids, vec![1, 2, 3]);
        assert!((scored[0].1 - 1.0).abs() < 1e-5);
    }

    #[tokio::test]
    async fn dirty_marker_triggers_rebuild() {
        let exec = ann_executor();
        put_dense(&exec, 1, &[1.0, 0.0]).await;
        put_dense(&exec, 2, &[0.0, 1.0]).await;
        // First query builds the index over {1,2}.
        let _ = exec
            .scored_embedding_candidates("default", "emb", &[1.0, 0.0])
            .await
            .unwrap();

        // Simulate an INSERT: new vector + dirty marker.
        put_dense(&exec, 3, &[0.95, 0.05]).await;
        exec.mark_vector_index_dirty("default", "emb")
            .await
            .unwrap();

        let scored = exec
            .scored_embedding_candidates("default", "emb", &[1.0, 0.0])
            .await
            .unwrap();
        let vids: std::collections::HashSet<i64> = scored.iter().map(|(v, _)| *v).collect();
        assert!(
            vids.contains(&3),
            "rebuilt index must include newly added vid 3"
        );
    }

    #[tokio::test]
    async fn below_threshold_uses_flat_and_persists_no_index() {
        let exec = flat_executor(); // threshold 1000
        put_dense(&exec, 1, &[1.0, 0.0]).await;
        put_dense(&exec, 2, &[0.9, 0.1]).await;

        let scored = exec
            .scored_embedding_candidates("default", "emb", &[1.0, 0.0])
            .await
            .unwrap();
        assert_eq!(
            scored.iter().map(|(v, _)| *v).collect::<Vec<_>>(),
            vec![1, 2]
        );
        // No index persisted below the threshold (exact flat used).
        assert!(exec
            .ctx
            .kvstore
            .get(&SchemaKey::vec_index("default", "emb"))
            .await
            .unwrap()
            .is_none());
    }
}
