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
//! `INSERT EDGE sameAs() VALUES a->b:()`. Inside the asserted statement transaction (and
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

use super::{inference::INFERRED_MARKER, Executor};
use crate::error::{ExecutionError, Result};
use crate::key::SchemaKey;
use crate::ontology;
use byoridb_codec::{EdgeData, TagData, VertexCodec, VertexData};
use std::collections::{BTreeMap, HashMap, HashSet};

pub(super) const SAMEAS_EDGE: &str = "sameAs";

impl Executor {
    /// All writes target the statement-local overlay. Exceeding the merge cap
    /// fails the whole asserted statement, including its history and mappings.
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
            let a = ontology::representative_of(&self.ctx, space, *a).await?;
            let b = ontology::representative_of(&self.ctx, space, *b).await?;
            if a == b {
                continue;
            }
            if cap != 0 && merged >= cap {
                return Err(ExecutionError::ResourceExhausted(format!("sameAs merges exceed max_traversal_nodes ({cap}); no statement writes committed")));
            }
            self.collapse_node(space, a.min(b), a.max(b)).await?;
            merged += 1;
        }
        if merged > 0 {
            self.rebuild_asserted_degree_counters(space).await?;
        }
        Ok(())
    }

    async fn collapse_node(&self, space: &str, winner: i64, loser: i64) -> Result<()> {
        let mut edges = BTreeMap::new();
        for prefix in [
            SchemaKey::edge_data_src_prefix(space, loser),
            SchemaKey::in_edge_data_dst_prefix(space, loser),
        ] {
            for (key, value) in self.ctx.kvstore.scan_prefix(&prefix).await? {
                let edge = crate::algo::decode_query_edge(&key, &value)?;
                let fwd = SchemaKey::edge_data(
                    space,
                    edge.src_vid,
                    &edge.edge_type,
                    edge.dst_vid,
                    edge.ranking,
                );
                let forward = self.ctx.kvstore.get(&fwd).await?.ok_or_else(|| {
                    ExecutionError::InvalidOperation("sameAs found an orphan reverse edge".into())
                })?;
                let forward_edge = crate::algo::decode_query_edge(&fwd, &forward)?;
                if forward_edge != edge {
                    return Err(ExecutionError::InvalidOperation(
                        "sameAs forward/reverse edge mismatch".into(),
                    ));
                }
                edges.insert(fwd, edge);
            }
        }
        // Remove all old identities first, then combine collisions at their
        // canonical identities. Existing winner assertions win property conflicts.
        for (key, edge) in &edges {
            let mut deletes = self.edge_index_keys(space, edge).await?;
            deletes.extend([
                key.clone(),
                SchemaKey::in_edge_data(
                    space,
                    edge.dst_vid,
                    &edge.edge_type,
                    edge.src_vid,
                    edge.ranking,
                ),
            ]);
            let versions = if edge.properties.contains_key(INFERRED_MARKER) {
                Vec::new()
            } else {
                Self::build_tombstones(vec![key.clone()])
            };
            self.ctx
                .kvstore
                .batch_apply(Vec::new(), deletes, versions)
                .await?;
        }
        for (_, mut edge) in edges {
            if edge.src_vid == loser {
                edge.src_vid = winner;
            }
            if edge.dst_vid == loser {
                edge.dst_vid = winner;
            }
            self.write_merged_edge(space, edge).await?;
        }
        for (key, _) in self
            .ctx
            .kvstore
            .scan_prefix(&SchemaKey::vtype_prefix(space, loser))
            .await?
        {
            let class = SchemaKey::vtype_class_from_key(&key)
                .filter(|class| SchemaKey::vtype(space, loser, class) == key)
                .ok_or_else(|| {
                    ExecutionError::InvalidOperation(
                        "Invalid inferred type key during sameAs".into(),
                    )
                })?;
            self.ctx
                .kvstore
                .put(&SchemaKey::vtype(space, winner, &class), &[])
                .await?;
            self.ctx.kvstore.delete(&key).await?;
        }
        self.merge_vertex_blob(space, winner, loser).await?;
        let repr = ontology::encode_repr(winner);
        for (key, _) in self
            .ctx
            .kvstore
            .scan_prefix(&SchemaKey::sameas_members_prefix(space, loser))
            .await?
        {
            let member = SchemaKey::sameas_member_from_key(&key)
                .filter(|member| SchemaKey::sameas_member(space, loser, *member) == key)
                .ok_or_else(|| {
                    ExecutionError::InvalidOperation("Invalid sameAs member key".into())
                })?;
            self.ctx
                .kvstore
                .put(&SchemaKey::sameas(space, member), &repr)
                .await?;
            self.ctx
                .kvstore
                .put(&SchemaKey::sameas_member(space, winner, member), &[])
                .await?;
            self.ctx.kvstore.delete(&key).await?;
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

    async fn write_merged_edge(&self, space: &str, mut edge: EdgeData) -> Result<()> {
        let key = SchemaKey::edge_data(
            space,
            edge.src_vid,
            &edge.edge_type,
            edge.dst_vid,
            edge.ranking,
        );
        let mut deletes = Vec::new();
        if let Some(bytes) = self.ctx.kvstore.get(&key).await? {
            let existing = crate::algo::decode_query_edge(&key, &bytes)?;
            deletes = self.edge_index_keys(space, &existing).await?;
            let existing_asserted = !existing.properties.contains_key(INFERRED_MARKER);
            let incoming_asserted = !edge.properties.contains_key(INFERRED_MARKER);
            if existing_asserted || !incoming_asserted {
                let incoming = edge.properties;
                edge = existing;
                if incoming_asserted {
                    for (key, value) in incoming {
                        edge.properties.entry(key).or_insert(value);
                    }
                }
            }
        }
        let data = VertexCodec::encode_edge(&edge)
            .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?;
        let mut puts = vec![
            (key.clone(), data.clone()),
            (
                SchemaKey::in_edge_data(
                    space,
                    edge.dst_vid,
                    &edge.edge_type,
                    edge.src_vid,
                    edge.ranking,
                ),
                data.clone(),
            ),
        ];
        puts.extend(
            self.edge_index_keys(space, &edge)
                .await?
                .into_iter()
                .map(|key| (key, Vec::new())),
        );
        let keep: HashSet<_> = puts.iter().map(|(key, _)| key.clone()).collect();
        deletes.retain(|key| !keep.contains(key));
        let versions = if edge.properties.contains_key(INFERRED_MARKER) {
            Vec::new()
        } else {
            Self::build_versions(vec![(key, data)])
        };
        self.ctx
            .kvstore
            .batch_apply(puts, deletes, versions)
            .await?;
        Ok(())
    }

    async fn merge_vertex_blob(&self, space: &str, winner: i64, loser: i64) -> Result<()> {
        let loser_key = SchemaKey::vertex(space, loser);
        let Some(bytes) = self.ctx.kvstore.get(&loser_key).await? else {
            return Ok(());
        };
        let loser_v = crate::record_decode::decode_vertex(loser, &bytes)?;
        let winner_key = SchemaKey::vertex(space, winner);
        let mut winner_v = match self.ctx.kvstore.get(&winner_key).await? {
            Some(bytes) => crate::record_decode::decode_vertex(winner, &bytes)?,
            None => VertexData {
                vid: winner,
                tags: Vec::new(),
            },
        };
        let mut deletes = self.vertex_index_keys(space, &loser_v).await?;
        deletes.extend(self.vertex_index_keys(space, &winner_v).await?);
        deletes.push(loser_key.clone());
        let mut dirty = HashSet::new();
        for tag in &loser_v.tags {
            deletes.push(SchemaKey::tagvid(space, &tag.name, loser));
            for (prop, value) in &tag.properties {
                if super::recommend::pack_embedding(value).is_some() {
                    deletes.push(SchemaKey::vec_data(space, prop, loser));
                    dirty.insert(prop.clone());
                }
            }
            if let Some(wtag) = winner_v.tags.iter_mut().find(|t| t.name == tag.name) {
                for (key, value) in &tag.properties {
                    wtag.properties
                        .entry(key.clone())
                        .or_insert_with(|| value.clone());
                }
            } else {
                winner_v.tags.push(TagData {
                    name: tag.name.clone(),
                    properties: tag.properties.clone(),
                });
            }
        }
        let data = VertexCodec::encode_vertex(&winner_v)
            .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?;
        let mut puts = vec![(winner_key.clone(), data.clone())];
        puts.extend(
            self.vertex_index_keys(space, &winner_v)
                .await?
                .into_iter()
                .map(|key| (key, Vec::new())),
        );
        for tag in &winner_v.tags {
            puts.push((SchemaKey::tagvid(space, &tag.name, winner), Vec::new()));
            for (prop, value) in &tag.properties {
                if let Some(bytes) = super::recommend::pack_embedding(value) {
                    puts.push((SchemaKey::vec_data(space, prop, winner), bytes));
                    dirty.insert(prop.clone());
                }
            }
        }
        puts.extend(
            dirty
                .into_iter()
                .map(|prop| (SchemaKey::vec_index_dirty(space, &prop), Vec::new())),
        );
        let keep: HashSet<_> = puts.iter().map(|(key, _)| key.clone()).collect();
        deletes.retain(|key| !keep.contains(key));
        let mut versions = Self::build_tombstones(vec![loser_key]);
        versions.extend(Self::build_versions(vec![(winner_key, data)]));
        self.ctx
            .kvstore
            .batch_apply(puts, deletes, versions)
            .await?;
        Ok(())
    }

    async fn rebuild_asserted_degree_counters(&self, space: &str) -> Result<()> {
        let mut counts: HashMap<Vec<u8>, i64> = HashMap::new();
        for (key, value) in self
            .ctx
            .kvstore
            .scan_prefix(format!("{space}:edge:").as_bytes())
            .await?
        {
            let edge = crate::algo::decode_query_edge(&key, &value)?;
            if edge.properties.contains_key(INFERRED_MARKER) {
                continue;
            }
            *counts
                .entry(SchemaKey::indeg_counter(
                    space,
                    &edge.edge_type,
                    edge.dst_vid,
                ))
                .or_default() += 1;
            *counts
                .entry(SchemaKey::outdeg_counter(
                    space,
                    &edge.edge_type,
                    edge.src_vid,
                ))
                .or_default() += 1;
        }
        let mut deletes = Vec::new();
        for prefix in [format!("{space}:indeg:"), format!("{space}:outdeg:")] {
            deletes.extend(
                self.ctx
                    .kvstore
                    .scan_prefix(prefix.as_bytes())
                    .await?
                    .into_iter()
                    .filter(|(key, _)| !counts.contains_key(key))
                    .map(|(key, _)| key),
            );
        }
        self.ctx
            .kvstore
            .batch_apply(
                counts
                    .into_iter()
                    .map(|(key, count)| (key, SchemaKey::encode_count(count)))
                    .collect(),
                deletes,
                Vec::new(),
            )
            .await?;
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
