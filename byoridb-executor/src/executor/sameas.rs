// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! owl:sameAs node-equivalence via write-time canonical merge (PLAN.md O-8).
//!
//! Deep-research (2026-06-19) found that fully materializing the sameAs
//! congruence closure blows up combinatorially (a size-n equivalence class
//! derives n² triples via 2n³ derivations). Every production RDF store
//! (GraphDB, RDFox, Stardog, Oracle) instead **rewrites** to a single canonical
//! representative per class. ByoriDB follows the same design, fitted to its
//! redb KV + insertion-only forward-chaining engine.
//!
//! **Strategy.** sameAs is asserted as a reserved edge type:
//! `INSERT EDGE sameAs() VALUES a->b:()`. After the asserted edges commit (and
//! *before* O-5 forward-chaining), [`Executor::merge_sameas_triples`] union-finds
//! the two equivalence classes and **collapses** the larger-id representative
//! into the smaller-id one (D2: min-id representative). The loser's edges
//! (forward + reverse index), inferred types, tagvid entries, dense embedding
//! vectors, and vertex blob are all rewritten onto the winner; the loser blob is
//! deleted. The loser — and any members previously collapsed into it — are then
//! repointed at the winner in the `{space}:sameas:` union-find map.
//!
//! Read paths (GO/FETCH/MATCH) normalize input vids to their representative via
//! [`crate::ontology::representative_of`], so the merged-away facts are seen
//! through the single surviving node (D5). No query-time expansion needed.
//!
//! **Scope (D4): irreversible, insertion-only.** Like O-5, deletes do not
//! retract a merge. DELETE of a merged node / a sameAs edge is rejected
//! (`executor::dml` guards). A future B/F retraction algorithm is out of scope.

use super::Executor;
use crate::error::Result;
use crate::key::SchemaKey;
use crate::ontology;
use byoridb_codec::{EdgeData as CodecEdgeData, TagData as CodecTagData, VertexCodec, VertexData};
use std::collections::HashSet;

/// Reserved edge type asserting owl:sameAs node equivalence (D1). Do not declare
/// O-4 semantic flags on it — equivalence is handled here, not by O-5 rules.
pub(super) const SAMEAS_EDGE: &str = "sameAs";

impl Executor {
    /// Apply owl:sameAs merges for the `sameAs`-typed triples in `triples`
    /// (PLAN.md O-8). No-op for spaces/inserts without sameAs edges. Called by
    /// INSERT EDGE before O-5 materialization so forward-chaining runs over the
    /// canonicalized graph (D10).
    pub(super) async fn merge_sameas_triples(
        &self,
        space: &str,
        triples: &[(i64, String, i64)],
    ) -> Result<()> {
        let cap = self.ctx.config.max_traversal_nodes;
        let mut merged = 0usize;
        for (a, p, b) in triples {
            if p != SAMEAS_EDGE || a == b {
                continue;
            }
            let rep_a = ontology::representative_of(&self.ctx, space, *a).await?;
            let rep_b = ontology::representative_of(&self.ctx, space, *b).await?;
            if rep_a == rep_b {
                continue; // already the same equivalence class
            }
            let (winner, loser) = (rep_a.min(rep_b), rep_a.max(rep_b));
            self.collapse_node(space, winner, loser).await?;
            merged += 1;
            if merged >= cap {
                tracing::warn!(
                    cap,
                    "O-8 sameAs merge hit max_traversal_nodes; some equivalences left unmerged"
                );
                return Ok(());
            }
        }
        Ok(())
    }

    /// Collapse representative `loser` into representative `winner`
    /// (`winner < loser`). Rewrites all of loser's vid-addressed facts onto
    /// winner, then repoints loser and its former members in the union-find map.
    /// Immediate puts/deletes (not batched) so a later step in the same merge
    /// run observes earlier rewrites, mirroring O-5's convention.
    async fn collapse_node(&self, space: &str, winner: i64, loser: i64) -> Result<()> {
        self.rewrite_out_edges(space, winner, loser).await?;
        self.rewrite_in_edges(space, winner, loser).await?;
        self.rewrite_vtype(space, winner, loser).await?;
        self.merge_vertex_blob(space, winner, loser).await?;

        // Repoint loser → winner, plus every member previously collapsed into
        // loser (their facts already live on loser, now moving to winner).
        let repr = ontology::encode_repr(winner);
        for m in ontology::members_of(&self.ctx, space, loser).await? {
            self.ctx
                .kvstore
                .put(&SchemaKey::sameas(space, m), &repr)
                .await?;
            self.ctx
                .kvstore
                .put(&SchemaKey::sameas_member(space, winner, m), &[])
                .await?;
            self.ctx
                .kvstore
                .delete(&SchemaKey::sameas_member(space, loser, m))
                .await?;
        }
        self.ctx
            .kvstore
            .put(&SchemaKey::sameas(space, loser), &repr)
            .await?;
        self.ctx
            .kvstore
            .put(&SchemaKey::sameas_member(space, winner, loser), &[])
            .await?;
        Ok(())
    }

    /// Rewrite loser's outgoing edges onto winner (forward `{space}:edge:{loser}:`
    /// scan). Both indexes (forward + in-edge) are deleted and re-written; a
    /// self-loop `loser→loser` becomes `winner→winner`.
    async fn rewrite_out_edges(&self, space: &str, winner: i64, loser: i64) -> Result<()> {
        let prefix = SchemaKey::edge_data_src_prefix(space, loser);
        for (key, value) in self.ctx.kvstore.scan_prefix(&prefix).await? {
            let Ok(mut edge) = VertexCodec::decode_edge(&value) else {
                continue;
            };
            let p = edge.edge_type.clone();
            let old_dst = edge.dst_vid;
            let r = edge.ranking;
            self.ctx.kvstore.delete(&key).await?;
            self.ctx
                .kvstore
                .delete(&SchemaKey::in_edge_data(space, old_dst, &p, loser, r))
                .await?;
            edge.src_vid = winner;
            edge.dst_vid = if old_dst == loser { winner } else { old_dst };
            self.write_edge(space, &edge).await?;
        }
        Ok(())
    }

    /// Rewrite loser's incoming edges onto winner (reverse `{space}:in-edge:{loser}:`
    /// scan). Self-loops were already handled by [`Self::rewrite_out_edges`]
    /// (their in-edge entry is gone), so a residual `src == loser` is skipped.
    async fn rewrite_in_edges(&self, space: &str, winner: i64, loser: i64) -> Result<()> {
        let prefix = SchemaKey::in_edge_data_dst_prefix(space, loser);
        for (key, value) in self.ctx.kvstore.scan_prefix(&prefix).await? {
            let Ok(mut edge) = VertexCodec::decode_edge(&value) else {
                continue;
            };
            let old_src = edge.src_vid;
            if old_src == loser {
                self.ctx.kvstore.delete(&key).await?; // stale self-loop in-edge
                continue;
            }
            let p = edge.edge_type.clone();
            let r = edge.ranking;
            self.ctx.kvstore.delete(&key).await?;
            self.ctx
                .kvstore
                .delete(&SchemaKey::edge_data(space, old_src, &p, loser, r))
                .await?;
            edge.dst_vid = winner;
            self.write_edge(space, &edge).await?;
        }
        Ok(())
    }

    /// Persist an edge to both the forward and reverse-edge indexes.
    async fn write_edge(&self, space: &str, edge: &CodecEdgeData) -> Result<()> {
        let data = VertexCodec::encode_edge(edge)
            .map_err(|e| crate::error::ExecutionError::Io(std::io::Error::other(e.to_string())))?;
        let fwd = SchemaKey::edge_data(
            space,
            edge.src_vid,
            &edge.edge_type,
            edge.dst_vid,
            edge.ranking,
        );
        self.ctx.kvstore.put(&fwd, &data).await?;
        let rev = SchemaKey::in_edge_data(
            space,
            edge.dst_vid,
            &edge.edge_type,
            edge.src_vid,
            edge.ranking,
        );
        self.ctx.kvstore.put(&rev, &data).await?;
        Ok(())
    }

    /// Move loser's inferred class memberships (O-5 domain/range) onto winner.
    async fn rewrite_vtype(&self, space: &str, winner: i64, loser: i64) -> Result<()> {
        let prefix = SchemaKey::vtype_prefix(space, loser);
        for (key, _) in self.ctx.kvstore.scan_prefix(&prefix).await? {
            if let Some(class) = SchemaKey::vtype_class_from_key(&key) {
                self.ctx
                    .kvstore
                    .put(&SchemaKey::vtype(space, winner, &class), &[])
                    .await?;
            }
            self.ctx.kvstore.delete(&key).await?;
        }
        Ok(())
    }

    /// Merge loser's vertex blob into winner's (D6: winner wins on property
    /// conflict), maintaining the tagvid index and dense embedding store, then
    /// delete the loser blob. No-op if loser has no vertex record (edge-only id).
    async fn merge_vertex_blob(&self, space: &str, winner: i64, loser: i64) -> Result<()> {
        let loser_key = SchemaKey::vertex(space, loser);
        let Some(loser_blob) = self.ctx.kvstore.get(&loser_key).await? else {
            return Ok(());
        };
        let loser_v = VertexCodec::decode_vertex(&loser_blob)
            .map_err(|e| crate::error::ExecutionError::Io(std::io::Error::other(e.to_string())))?;

        let winner_key = SchemaKey::vertex(space, winner);
        let mut winner_v = match self.ctx.kvstore.get(&winner_key).await? {
            Some(b) => VertexCodec::decode_vertex(&b).map_err(|e| {
                crate::error::ExecutionError::Io(std::io::Error::other(e.to_string()))
            })?,
            None => VertexData {
                vid: winner,
                tags: Vec::new(),
            },
        };

        let mut dirty_vec_props: HashSet<String> = HashSet::new();

        // Tear down loser's tagvid + dense-vector entries and fold its tags in.
        for ltag in &loser_v.tags {
            self.ctx
                .kvstore
                .delete(&SchemaKey::tagvid(space, &ltag.name, loser))
                .await?;
            for (prop, value) in &ltag.properties {
                if crate::executor::recommend::pack_embedding(value).is_some() {
                    self.ctx
                        .kvstore
                        .delete(&SchemaKey::vec_data(space, prop, loser))
                        .await?;
                    dirty_vec_props.insert(prop.clone());
                }
            }
            match winner_v.tags.iter_mut().find(|t| t.name == ltag.name) {
                Some(wtag) => {
                    for (k, v) in &ltag.properties {
                        // winner wins: only fill props it lacks.
                        wtag.properties
                            .entry(k.clone())
                            .or_insert_with(|| v.clone());
                    }
                }
                None => winner_v.tags.push(CodecTagData {
                    name: ltag.name.clone(),
                    properties: ltag.properties.clone(),
                }),
            }
        }

        self.ctx.kvstore.delete(&loser_key).await?;

        let wdata = VertexCodec::encode_vertex(&winner_v)
            .map_err(|e| crate::error::ExecutionError::Io(std::io::Error::other(e.to_string())))?;
        self.ctx.kvstore.put(&winner_key, &wdata).await?;

        // Re-assert winner's tagvid + dense vectors for the (possibly extended)
        // tag/prop set.
        for wtag in &winner_v.tags {
            self.ctx
                .kvstore
                .put(&SchemaKey::tagvid(space, &wtag.name, winner), &[])
                .await?;
            for (prop, value) in &wtag.properties {
                if let Some(bytes) = crate::executor::recommend::pack_embedding(value) {
                    self.ctx
                        .kvstore
                        .put(&SchemaKey::vec_data(space, prop, winner), &bytes)
                        .await?;
                    dirty_vec_props.insert(prop.clone());
                }
            }
        }

        for prop in &dirty_vec_props {
            self.mark_vector_index_dirty(space, prop).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ExecutionContext;
    use byoridb_kvstore::store::MemoryKVStore;
    use std::sync::Arc;

    fn create_executor() -> Executor {
        let kv = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kv).with_space("default".to_string()));
        Executor::new(ctx)
    }

    async fn run(executor: &Executor, q: &str) -> Result<crate::executor::ExecutorResult> {
        let stmt = byoridb_parser::parse(q).expect("parse");
        let plan = crate::ExecutionPlanBuilder::build(stmt).expect("plan build");
        executor.execute(plan).await
    }

    async fn ok(executor: &Executor, q: &str) {
        run(executor, q)
            .await
            .unwrap_or_else(|e| panic!("query failed: {q}\n{e:?}"));
    }

    async fn has(executor: &Executor, s: i64, p: &str, d: i64) -> bool {
        executor.triple_exists("default", s, p, d).await.unwrap()
    }

    async fn repr(executor: &Executor, vid: i64) -> i64 {
        ontology::representative_of(&executor.ctx, "default", vid)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn merge_picks_min_id_representative() {
        let e = create_executor();
        ok(&e, "CREATE EDGE sameAs()").await;
        ok(&e, "INSERT EDGE sameAs() VALUES 5->2:()").await;
        // min-id (2) is the representative; 5 points to it.
        assert_eq!(repr(&e, 5).await, 2, "5 → 2");
        assert_eq!(repr(&e, 2).await, 2, "2 is its own representative");
    }

    #[tokio::test]
    async fn merge_rewrites_loser_out_edges_onto_winner() {
        let e = create_executor();
        ok(&e, "CREATE EDGE sameAs()").await;
        ok(&e, "CREATE EDGE knows()").await;
        // 5 knows 9; then 5 sameAs 2 ⟹ winner=2 should own the knows edge.
        ok(&e, "INSERT EDGE knows() VALUES 5->9:()").await;
        ok(&e, "INSERT EDGE sameAs() VALUES 5->2:()").await;
        assert!(
            has(&e, 2, "knows", 9).await,
            "loser out-edge moved to winner"
        );
        assert!(!has(&e, 5, "knows", 9).await, "old loser edge removed");
    }

    #[tokio::test]
    async fn merge_rewrites_loser_in_edges_onto_winner() {
        let e = create_executor();
        ok(&e, "CREATE EDGE sameAs()").await;
        ok(&e, "CREATE EDGE knows()").await;
        // 9 knows 5; 5 sameAs 2 ⟹ 9 knows 2 (reverse index follows).
        ok(&e, "INSERT EDGE knows() VALUES 9->5:()").await;
        ok(&e, "INSERT EDGE sameAs() VALUES 5->2:()").await;
        assert!(
            has(&e, 9, "knows", 2).await,
            "loser in-edge moved to winner"
        );
        assert!(!has(&e, 9, "knows", 5).await, "old loser in-edge removed");
        // Reverse-index visibility: get_incoming_neighbors of 2 sees 9.
        let inc = crate::algo::get_incoming_neighbors(&e.ctx, "default", 2, &["knows".to_string()])
            .await
            .unwrap();
        assert!(inc.iter().any(|n| n.dst == 9), "reverse index rewritten");
    }

    #[tokio::test]
    async fn merge_folds_vertex_props_winner_wins() {
        let e = create_executor();
        ok(&e, "CREATE EDGE sameAs()").await;
        ok(&e, "CREATE TAG product(name string, sku string)").await;
        ok(
            &e,
            "INSERT VERTEX product(name, sku) VALUES 2:(\"A\", \"win\")",
        )
        .await;
        ok(
            &e,
            "INSERT VERTEX product(name, sku) VALUES 5:(\"B\", \"lose\")",
        )
        .await;
        ok(&e, "INSERT EDGE sameAs() VALUES 5->2:()").await;
        // Winner 2's vertex survives; loser 5's blob is gone.
        assert!(
            e.ctx
                .kvstore
                .get(&SchemaKey::vertex("default", 5))
                .await
                .unwrap()
                .is_none(),
            "loser blob deleted"
        );
        let blob = e
            .ctx
            .kvstore
            .get(&SchemaKey::vertex("default", 2))
            .await
            .unwrap()
            .unwrap();
        let v = VertexCodec::decode_vertex(&blob).unwrap();
        let props = &v
            .tags
            .iter()
            .find(|t| t.name == "product")
            .unwrap()
            .properties;
        // sku conflict → winner (2) keeps "win".
        assert_eq!(
            props.get("sku"),
            Some(&byoridb_common::Value::String("win".to_string())),
            "winner wins on property conflict"
        );
    }

    #[tokio::test]
    async fn merge_is_idempotent_and_unions_classes() {
        let e = create_executor();
        ok(&e, "CREATE EDGE sameAs()").await;
        // Chain: 7≡4, 4≡1 ⟹ all collapse to min-id 1.
        ok(&e, "INSERT EDGE sameAs() VALUES 7->4:()").await;
        ok(&e, "INSERT EDGE sameAs() VALUES 4->1:()").await;
        assert_eq!(repr(&e, 7).await, 1, "7 → 1 (transitive union)");
        assert_eq!(repr(&e, 4).await, 1, "4 → 1");
        // Re-asserting an existing equivalence is a no-op (no panic, stable rep).
        ok(&e, "INSERT EDGE sameAs() VALUES 7->1:()").await;
        assert_eq!(repr(&e, 7).await, 1, "still 1 after redundant sameAs");
    }

    #[tokio::test]
    async fn self_sameas_is_noop() {
        let e = create_executor();
        ok(&e, "CREATE EDGE sameAs()").await;
        ok(&e, "INSERT EDGE sameAs() VALUES 3->3:()").await;
        assert_eq!(repr(&e, 3).await, 3, "self sameAs leaves rep unchanged");
    }
}
