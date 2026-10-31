// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Ontology forward-chaining materialization (PLAN.md O-5).
//!
//! When edges are inserted, the entailed edges under the declared semantic
//! relation types (O-4) are derived and **stored** as ordinary edges, so
//! MATCH / GO see them with no query-time reasoning (the materialization
//! strategy fixed in O-0). Inferred edges live in the same
//! `{space}:edge:` / `{space}:in-edge:` keyspace as asserted ones, tagged with
//! an `__inferred__` property, ranking 0.
//!
//! **Rules (RDFS-Plus, edge level).** For a triple `(s, p, d)`:
//! - `symmetric(p)`            ⟹ `(d, p, s)`
//! - `inverseOf(p, q)`         ⟹ `(d, q, s)` (registered both ways — owl:inverseOf is symmetric)
//! - `subPropertyOf(p, q)`     ⟹ `(s, q, d)`
//! - `transitive(p)`           ⟹ `(s, p, x)` for each `(d, p, x)`, and `(y, p, d)` for each `(y, p, s)`
//!
//! Derivations cascade to a **fixpoint** via a worklist: every newly written
//! inferred edge is itself fed back through the rules, so chained entailments
//! (e.g. `subPropertyOf` into a `transitive` superproperty, or `inverseOf`
//! composed with `symmetric`) are fully closed.
//!
//! Semantic mutations and `REPAIR INFERENCE` rebuild the complete closure
//! from current assertions under a per-space write lock. The durable dirty
//! marker is committed with asserted graph/history changes and cleared only
//! after successful derivation. Errors and cancellation leave reads fenced.
//! `max_traversal_nodes` bounds newly derived edges/types: zero is unbounded,
//! exactly the cap succeeds, and discovering another fact returns an error.
//! Assertions/history may already be committed when that error is returned.
//! Full rebuild is O(graph) and replaces incremental DRed for this release.

use super::provenance::{Fact, Justification, RuleKind};
use super::Executor;
use crate::algo;
use crate::error::{ExecutionError, Result};
use crate::key::SchemaKey;
use byoridb_codec::{EdgeData as CodecEdgeData, VertexCodec};
use byoridb_common::Value;
use byoridb_parser::ast::SemanticFlags;
use std::collections::{HashMap, HashSet, VecDeque};

/// Property marker distinguishing materialized edges from asserted ones.
pub(super) const INFERRED_MARKER: &str = "__inferred__";

/// A directed triple `(src, edge_type, dst)` — the unit of inference.
type Triple = (i64, String, i64);

/// Resolved semantic relation metadata for a space, indexed for fast rule
/// application during materialization.
#[derive(Default)]
pub(super) struct RelMeta {
    transitive: HashSet<String>,
    symmetric: HashSet<String>,
    /// edge type → inverse edge types (bidirectional closure of `inverseOf`).
    inverse: HashMap<String, Vec<String>>,
    /// edge type → its direct superproperties.
    superprops: HashMap<String, Vec<String>>,
    /// edge type → equivalent edge types (bidirectional closure of
    /// `owl:equivalentProperty`). Like `superprops` but symmetric.
    equivalent_props: HashMap<String, Vec<String>>,
    /// edge type → domain class (subject vertex type).
    domain: HashMap<String, String>,
    /// edge type → range class (object vertex type).
    range: HashMap<String, String>,
    /// First chain link `p1` → list of `(q, p2)`: for an edge `(s)-p1->(d)`,
    /// each `p2`-neighbor `b` of `d` entails `(s)-q->(b)` (owl:propertyChainAxiom
    /// `q ← p1 ∘ p2`).
    chain_first: HashMap<String, Vec<(String, String)>>,
    /// Second chain link `p2` → list of `(q, p1)`: for an edge `(x)-p2->(b)`,
    /// each `p1`-predecessor `a` of `x` entails `(a)-q->(b)`. The backward
    /// counterpart of `chain_first`, so a chain closes regardless of which link
    /// is inserted last.
    chain_second: HashMap<String, Vec<(String, String)>>,
}

impl RelMeta {
    pub(super) fn is_empty(&self) -> bool {
        self.transitive.is_empty()
            && self.symmetric.is_empty()
            && self.inverse.is_empty()
            && self.superprops.is_empty()
            && self.equivalent_props.is_empty()
            && self.domain.is_empty()
            && self.range.is_empty()
            && self.chain_first.is_empty()
            && self.chain_second.is_empty()
    }
}

impl Executor {
    /// Remove derived state and rebuild from surviving assertions. Even empty
    /// semantic metadata must clean old derived facts after schema removal.
    /// The caller holds the space write lock and maintains the dirty marker.
    pub(super) async fn rematerialize_space(&self, space: &str) -> Result<()> {
        let meta = self.load_rel_meta(space).await?;
        let entries = self
            .ctx
            .kvstore
            .scan_prefix(format!("{space}:edge:").as_bytes())
            .await?;
        let mut asserted = Vec::new();
        let mut deletes = Vec::new();
        let mut puts = Vec::new();
        for (key, value) in entries {
            let edge = algo::decode_query_edge(&key, &value)?;
            if edge.properties.contains_key(INFERRED_MARKER) {
                deletes.push(key);
            } else {
                asserted.push((edge.src_vid, edge.edge_type.clone(), edge.dst_vid));
                puts.push((
                    SchemaKey::in_edge_data(
                        space,
                        edge.dst_vid,
                        &edge.edge_type,
                        edge.src_vid,
                        edge.ranking,
                    ),
                    value,
                ));
            }
        }
        // Rebuild the reverse view exclusively from asserted forward records.
        // This also removes orphaned derived reverse entries after older failures.
        for (key, value) in self
            .ctx
            .kvstore
            .scan_prefix(format!("{space}:in-edge:").as_bytes())
            .await?
        {
            algo::decode_query_edge(&key, &value)?;
            deletes.push(key);
        }
        for prefix in [
            format!("{space}:vtype:").into_bytes(),
            SchemaKey::prov_prefix(space),
            SchemaKey::prov_rev_prefix(space),
        ] {
            deletes.extend(
                self.ctx
                    .kvstore
                    .scan_prefix(&prefix)
                    .await?
                    .into_iter()
                    .map(|(key, _)| key),
            );
        }
        // batch_apply applies puts before deletes; retain asserted reverse keys.
        let keep: HashSet<_> = puts.iter().map(|(k, _)| k.clone()).collect();
        deletes.retain(|key| !keep.contains(key));
        self.ctx
            .kvstore
            .batch_apply(puts, deletes, Vec::new())
            .await?;

        // Re-derive the full closure from the surviving asserted edges.
        self.run_materialization(space, asserted, &meta).await
    }

    /// Load every edge schema's `semantics` and build the [`RelMeta`] index.
    pub(super) async fn load_rel_meta(&self, space: &str) -> Result<RelMeta> {
        let prefix = format!("space:{}:edge:", space);
        let entries = self.ctx.kvstore.scan_prefix(prefix.as_bytes()).await?;
        let mut meta = RelMeta::default();
        for (key, value) in entries {
            let json: serde_json::Value = serde_json::from_slice(&value)?;
            let name = json
                .get("name")
                .and_then(|n| n.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ExecutionError::InvalidOperation("Invalid inference edge schema name".into())
                })?
                .to_owned();
            if key != SchemaKey::edge(space, &name) {
                return Err(ExecutionError::InvalidOperation(
                    "Inference schema key/name mismatch".into(),
                ));
            }
            let sem: SemanticFlags = match json.get("semantics") {
                Some(value) => serde_json::from_value(value.clone())?,
                None => SemanticFlags::default(),
            };
            if sem
                .property_chain
                .as_ref()
                .is_some_and(|chain| chain.len() != 2)
            {
                return Err(ExecutionError::InvalidOperation(
                    "Invalid inference property chain".into(),
                ));
            }
            if sem.transitive {
                meta.transitive.insert(name.clone());
            }
            if sem.symmetric {
                meta.symmetric.insert(name.clone());
            }
            if let Some(q) = sem.inverse_of {
                // owl:inverseOf is symmetric — register both directions.
                meta.inverse
                    .entry(name.clone())
                    .or_default()
                    .push(q.clone());
                meta.inverse.entry(q).or_default().push(name.clone());
            }
            if let Some(q) = sem.subproperty_of {
                meta.superprops.entry(name.clone()).or_default().push(q);
            }
            if let Some(q) = sem.equivalent_property {
                // owl:equivalentProperty is symmetric — register both directions.
                meta.equivalent_props
                    .entry(name.clone())
                    .or_default()
                    .push(q.clone());
                meta.equivalent_props
                    .entry(q)
                    .or_default()
                    .push(name.clone());
            }
            if let Some(c) = sem.domain {
                meta.domain.insert(name.clone(), c);
            }
            if let Some(c) = sem.range {
                meta.range.insert(name.clone(), c);
            }
            // CHAIN [p1, p2]: index both links. The schema decoder above
            // rejects malformed chains before any graph changes.
            if let Some(chain) = sem.property_chain {
                if let [p1, p2] = chain.as_slice() {
                    meta.chain_first
                        .entry(p1.clone())
                        .or_default()
                        .push((name.clone(), p2.clone()));
                    meta.chain_second
                        .entry(p2.clone())
                        .or_default()
                        .push((name.clone(), p1.clone()));
                }
            }
        }
        Ok(meta)
    }

    /// Worklist fixpoint: derive and persist entailed edges until none are new.
    async fn run_materialization(
        &self,
        space: &str,
        seeds: Vec<Triple>,
        meta: &RelMeta,
    ) -> Result<()> {
        let cap = self.ctx.config.max_traversal_nodes;
        let mut seen: HashSet<Triple> = HashSet::new();
        let mut worklist: VecDeque<Triple> = VecDeque::new();
        for t in seeds {
            if seen.insert(t.clone()) {
                worklist.push_back(t);
            }
        }

        let mut written = 0usize;
        while let Some((s, p, d)) = worklist.pop_front() {
            // The edge whose consequences we derive this iteration is the
            // premise of every justification recorded below.
            let premise = Fact::Edge { s, p: p.clone(), d };
            // Each entry pairs a derived triple with how it was entailed.
            let mut derived: Vec<(Triple, Justification)> = Vec::new();

            if meta.symmetric.contains(&p) {
                derived.push((
                    (d, p.clone(), s),
                    Justification {
                        rule: RuleKind::Symmetric,
                        premises: vec![premise.clone()],
                    },
                ));
            }
            if let Some(invs) = meta.inverse.get(&p) {
                for q in invs {
                    derived.push((
                        (d, q.clone(), s),
                        Justification {
                            rule: RuleKind::InverseOf,
                            premises: vec![premise.clone()],
                        },
                    ));
                }
            }
            if let Some(sups) = meta.superprops.get(&p) {
                for q in sups {
                    derived.push((
                        (s, q.clone(), d),
                        Justification {
                            rule: RuleKind::SubPropertyOf,
                            premises: vec![premise.clone()],
                        },
                    ));
                }
            }
            // owl:equivalentProperty: (a)-p->(b) ∧ p ≡ q ⟹ (a)-q->(b). The
            // `equivalent_props` index is symmetric, so the reverse (q ⟹ p) is
            // derived when the q-edge is processed.
            if let Some(eqs) = meta.equivalent_props.get(&p) {
                for q in eqs {
                    derived.push((
                        (s, q.clone(), d),
                        Justification {
                            rule: RuleKind::EquivalentProperty,
                            premises: vec![premise.clone()],
                        },
                    ));
                }
            }
            if meta.transitive.contains(&p) {
                let etype = [p.clone()];
                // (s,p,d) ∧ (d,p,x) ⟹ (s,p,x)
                for n in self.inference_neighbors(space, d, &etype, false).await? {
                    derived.push((
                        (s, p.clone(), n.dst),
                        Justification {
                            rule: RuleKind::Transitive,
                            premises: vec![
                                premise.clone(),
                                Fact::Edge {
                                    s: d,
                                    p: p.clone(),
                                    d: n.dst,
                                },
                            ],
                        },
                    ));
                }
                // (y,p,s) ∧ (s,p,d) ⟹ (y,p,d)  (n.dst is the source vertex y)
                for n in self.inference_neighbors(space, s, &etype, true).await? {
                    derived.push((
                        (n.dst, p.clone(), d),
                        Justification {
                            rule: RuleKind::Transitive,
                            premises: vec![
                                Fact::Edge {
                                    s: n.dst,
                                    p: p.clone(),
                                    d: s,
                                },
                                premise.clone(),
                            ],
                        },
                    ));
                }
            }

            // property chain (owl:propertyChainAxiom) q ← p1 ∘ p2.
            // Link 1: (s)-p->(d) is p1 ⟹ for each p2-neighbor b of d, (s)-q->(b).
            if let Some(axioms) = meta.chain_first.get(&p) {
                for (q, p2) in axioms {
                    let et = [p2.clone()];
                    for n in self.inference_neighbors(space, d, &et, false).await? {
                        derived.push((
                            (s, q.clone(), n.dst),
                            Justification {
                                rule: RuleKind::PropertyChain,
                                premises: vec![
                                    premise.clone(),
                                    Fact::Edge {
                                        s: d,
                                        p: p2.clone(),
                                        d: n.dst,
                                    },
                                ],
                            },
                        ));
                    }
                }
            }
            // Link 2: (s)-p->(d) is p2 ⟹ for each p1-predecessor a of s, (a)-q->(d).
            // (get_incoming_neighbors yields edges into s; n.dst is the source a.)
            if let Some(axioms) = meta.chain_second.get(&p) {
                for (q, p1) in axioms {
                    let et = [p1.clone()];
                    for n in self.inference_neighbors(space, s, &et, true).await? {
                        derived.push((
                            (n.dst, q.clone(), d),
                            Justification {
                                rule: RuleKind::PropertyChain,
                                premises: vec![
                                    Fact::Edge {
                                        s: n.dst,
                                        p: p1.clone(),
                                        d: s,
                                    },
                                    premise.clone(),
                                ],
                            },
                        ));
                    }
                }
            }

            for (vid, class, rule) in [
                (s, meta.domain.get(&p), RuleKind::Domain),
                (d, meta.range.get(&p), RuleKind::Range),
            ] {
                if let Some(class) = class {
                    let fact = Fact::Vtype {
                        vid,
                        class: class.clone(),
                    };
                    let key = SchemaKey::vtype(space, vid, class);
                    let is_new = self.ctx.kvstore.get(&key).await?.is_none();
                    Self::charge_inference(cap, &mut written, is_new)?;
                    let mut puts = self
                        .provenance_puts(
                            space,
                            &fact,
                            Justification {
                                rule,
                                premises: vec![premise.clone()],
                            },
                        )
                        .await?;
                    if is_new {
                        puts.push((key, Vec::new()));
                    }
                    self.ctx
                        .kvstore
                        .batch_apply(puts, Vec::new(), Vec::new())
                        .await?;
                }
            }
            for (triple, justification) in derived {
                let (s2, p2, d2) = (triple.0, triple.1.clone(), triple.2);
                let fact = Fact::Edge {
                    s: s2,
                    p: p2.clone(),
                    d: d2,
                };
                let exists = self.triple_exists(space, s2, &p2, d2).await?;
                Self::charge_inference(cap, &mut written, !exists)?;
                let mut puts = self.provenance_puts(space, &fact, justification).await?;
                if !exists {
                    let mut properties = HashMap::new();
                    properties.insert(INFERRED_MARKER.to_string(), Value::Bool(true));
                    let edge = CodecEdgeData {
                        src_vid: s2,
                        dst_vid: d2,
                        edge_type: p2.clone(),
                        ranking: 0,
                        properties,
                    };
                    let data = VertexCodec::encode_edge(&edge)
                        .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?;
                    puts.push((SchemaKey::edge_data(space, s2, &p2, d2, 0), data.clone()));
                    puts.push((SchemaKey::in_edge_data(space, d2, &p2, s2, 0), data));
                }
                // Forward edge, reverse edge, forward provenance and reverse
                // provenance share one atomic batch on Memory/redb.
                self.ctx
                    .kvstore
                    .batch_apply(puts, Vec::new(), Vec::new())
                    .await?;
                if seen.insert(triple.clone()) && !exists {
                    worklist.push_back(triple);
                }
            }
        }
        Ok(())
    }

    /// True if any edge `(s)-p->(d)` exists at any ranking (asserted or inferred).
    pub(super) async fn triple_exists(&self, space: &str, s: i64, p: &str, d: i64) -> Result<bool> {
        // Trailing colon prevents dst prefix collisions (`:1:` vs `:10:`).
        let prefix = format!("{}:edge:{}:{}:{}:", space, s, p, d);
        let hits = self.ctx.kvstore.scan_prefix(prefix.as_bytes()).await?;
        for (key, value) in &hits {
            algo::decode_query_edge(key, value)?;
        }
        Ok(!hits.is_empty())
    }

    fn charge_inference(cap: usize, written: &mut usize, is_new: bool) -> Result<()> {
        if is_new {
            if cap != 0 && *written >= cap {
                return Err(ExecutionError::ResourceExhausted(format!(
                    "Inference closure exceeds max_traversal_nodes ({cap}); assertions/history may already be committed. Inference remains dirty; increase the budget and run REPAIR INFERENCE."
                )));
            }
            *written += 1;
        }
        Ok(())
    }

    async fn inference_neighbors(
        &self,
        space: &str,
        vid: i64,
        types: &[String],
        incoming: bool,
    ) -> Result<Vec<algo::GraphNeighbor>> {
        let prefix = if incoming {
            SchemaKey::in_edge_data_dst_prefix(space, vid)
        } else {
            SchemaKey::edge_data_src_prefix(space, vid)
        };
        let mut result = Vec::new();
        for (key, value) in self.ctx.kvstore.scan_prefix(&prefix).await? {
            let edge = algo::decode_query_edge(&key, &value)?;
            if types.contains(&edge.edge_type) {
                result.push(algo::GraphNeighbor {
                    dst: if incoming { edge.src_vid } else { edge.dst_vid },
                    edge,
                });
            }
        }
        Ok(result)
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

    /// True if edge `(s)-p->(d)` exists at any ranking (asserted or inferred).
    async fn has(executor: &Executor, s: i64, p: &str, d: i64) -> bool {
        executor.triple_exists("default", s, p, d).await.unwrap()
    }

    #[tokio::test]
    async fn symmetric_materializes_reverse() {
        let e = create_executor();
        ok(&e, "CREATE EDGE knows() SYMMETRIC").await;
        ok(&e, "INSERT EDGE knows() VALUES 1->2:()").await;
        assert!(has(&e, 1, "knows", 2).await, "asserted edge present");
        assert!(has(&e, 2, "knows", 1).await, "symmetric reverse inferred");
    }

    #[tokio::test]
    async fn inverse_of_materializes_both_directions() {
        let e = create_executor();
        ok(&e, "CREATE EDGE parent()").await;
        ok(&e, "CREATE EDGE child() INVERSE OF parent").await;
        // child 1->2 ⟹ parent 2->1
        ok(&e, "INSERT EDGE child() VALUES 1->2:()").await;
        assert!(has(&e, 2, "parent", 1).await, "inverse parent inferred");
        // owl:inverseOf is symmetric: parent 3->4 ⟹ child 4->3
        ok(&e, "INSERT EDGE parent() VALUES 3->4:()").await;
        assert!(
            has(&e, 4, "child", 3).await,
            "inverse child inferred (bidirectional)"
        );
    }

    #[tokio::test]
    async fn subproperty_materializes_superproperty() {
        let e = create_executor();
        ok(&e, "CREATE EDGE related()").await;
        ok(&e, "CREATE EDGE knows() SUBPROPERTY OF related").await;
        ok(&e, "INSERT EDGE knows() VALUES 1->2:()").await;
        assert!(
            has(&e, 1, "related", 2).await,
            "superproperty edge inferred"
        );
    }

    #[tokio::test]
    async fn equivalent_property_materializes_both_directions() {
        let e = create_executor();
        ok(&e, "CREATE EDGE likes()").await;
        ok(&e, "CREATE EDGE enjoys() EQUIVALENT TO likes").await;
        // likes 1->2 ⟹ enjoys 1->2
        ok(&e, "INSERT EDGE likes() VALUES 1->2:()").await;
        assert!(
            has(&e, 1, "enjoys", 2).await,
            "equivalent enjoys inferred from likes"
        );
        // owl:equivalentProperty is symmetric: enjoys 3->4 ⟹ likes 3->4
        ok(&e, "INSERT EDGE enjoys() VALUES 3->4:()").await;
        assert!(
            has(&e, 3, "likes", 4).await,
            "equivalent likes inferred (bidirectional)"
        );
    }

    #[tokio::test]
    async fn equivalent_property_retracts_on_delete() {
        let e = create_executor();
        ok(&e, "CREATE EDGE likes()").await;
        ok(&e, "CREATE EDGE enjoys() EQUIVALENT TO likes").await;
        ok(&e, "INSERT EDGE likes() VALUES 1->2:()").await;
        assert!(has(&e, 1, "enjoys", 2).await, "equivalent enjoys inferred");
        ok(&e, "DELETE EDGE likes 1->2").await;
        assert!(
            !has(&e, 1, "enjoys", 2).await,
            "inferred enjoys retracted (DRed)"
        );
    }

    #[tokio::test]
    async fn transitive_closes_chain() {
        let e = create_executor();
        ok(&e, "CREATE EDGE ancestor() TRANSITIVE").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 2->3:()").await;
        assert!(has(&e, 1, "ancestor", 3).await, "1->3 closed");
        ok(&e, "INSERT EDGE ancestor() VALUES 3->4:()").await;
        assert!(has(&e, 1, "ancestor", 4).await, "1->4 closed");
        assert!(has(&e, 2, "ancestor", 4).await, "2->4 closed");
    }

    #[tokio::test]
    async fn cascading_subproperty_into_transitive() {
        let e = create_executor();
        ok(&e, "CREATE EDGE ancestor() TRANSITIVE").await;
        ok(&e, "CREATE EDGE parent() SUBPROPERTY OF ancestor").await;
        ok(&e, "INSERT EDGE parent() VALUES 1->2:()").await; // ⟹ ancestor 1->2
        ok(&e, "INSERT EDGE parent() VALUES 2->3:()").await; // ⟹ ancestor 2->3, then transitive ancestor 1->3
        assert!(has(&e, 1, "ancestor", 2).await, "subproperty 1->2");
        assert!(has(&e, 2, "ancestor", 3).await, "subproperty 2->3");
        assert!(has(&e, 1, "ancestor", 3).await, "cascaded transitive 1->3");
    }

    #[tokio::test]
    async fn inferred_edges_are_visible_to_traversal() {
        // The whole point: MATCH/GO read inferred edges from the shared keyspace.
        let e = create_executor();
        ok(&e, "CREATE EDGE ancestor() TRANSITIVE").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 2->3:()").await;
        // get_neighbors (used by GO/MATCH) sees the inferred 1->3 edge.
        let neighbors = crate::algo::get_neighbors(&e.ctx, "default", 1, &["ancestor".to_string()])
            .await
            .unwrap();
        let dsts: std::collections::HashSet<i64> = neighbors.iter().map(|n| n.dst).collect();
        assert!(dsts.contains(&2), "asserted 1->2 visible");
        assert!(dsts.contains(&3), "inferred 1->3 visible to traversal");
    }

    #[tokio::test]
    async fn no_semantics_no_inference() {
        let e = create_executor();
        ok(&e, "CREATE EDGE plain()").await;
        ok(&e, "INSERT EDGE plain() VALUES 1->2:()").await;
        assert!(has(&e, 1, "plain", 2).await, "asserted present");
        assert!(
            !has(&e, 2, "plain", 1).await,
            "no reverse without SYMMETRIC"
        );
    }

    #[tokio::test]
    async fn domain_range_infers_vertex_types() {
        let e = create_executor();
        ok(&e, "CREATE CLASS person()").await;
        ok(&e, "CREATE CLASS location()").await;
        ok(&e, "CREATE CLASS city() SUBCLASS OF location").await;
        ok(&e, "CREATE TAG place()").await; // neutral tag for the object vertex
        ok(&e, "CREATE EDGE bornIn() DOMAIN person RANGE city").await;
        ok(&e, "INSERT VERTEX person() VALUES 1:()").await;
        ok(&e, "INSERT VERTEX place() VALUES 2:()").await; // tagged place, NOT city
        ok(&e, "INSERT EDGE bornIn() VALUES 1->2:()").await;

        // RANGE city ⟹ object vertex 2 is-a city, and via SUBCLASS OF, location.
        let set2 = crate::ontology::vertex_class_set(&e.ctx, "default", 2)
            .await
            .unwrap()
            .unwrap();
        assert!(set2.contains("city"), "range inferred city type");
        assert!(
            set2.contains("location"),
            "inferred type's ancestor included"
        );
        // DOMAIN person ⟹ subject vertex 1 is-a person (already tagged).
        let set1 = crate::ontology::vertex_class_set(&e.ctx, "default", 1)
            .await
            .unwrap()
            .unwrap();
        assert!(set1.contains("person"));

        // MATCH sees the inferred type: vertex 2 (tagged place) is-a city.
        let r = run(&e, "MATCH (n:place) WHERE is_a(n, \"city\") RETURN id(n)")
            .await
            .unwrap();
        assert_eq!(r.rows.len(), 1, "inferred city type visible to MATCH is_a");
    }

    #[tokio::test]
    async fn create_edge_rejects_unknown_domain_class() {
        let e = create_executor();
        assert!(
            run(&e, "CREATE EDGE bad() DOMAIN nonexistent_class")
                .await
                .is_err(),
            "DOMAIN of unknown class must error"
        );
    }

    #[tokio::test]
    async fn match_where_is_a_uses_class_hierarchy() {
        // O-7: ontology-aware matching in the main query language.
        let e = create_executor();
        ok(&e, "CREATE CLASS animal()").await;
        ok(&e, "CREATE CLASS dog() SUBCLASS OF animal").await;
        ok(&e, "CREATE TAG cat()").await;
        ok(&e, "INSERT VERTEX dog() VALUES 1:()").await;
        ok(&e, "INSERT VERTEX dog() VALUES 2:()").await;
        ok(&e, "INSERT VERTEX cat() VALUES 3:()").await;

        // dog ⊂ animal → both dog vertices match is_a("animal").
        let r = run(&e, "MATCH (n:dog) WHERE is_a(n, \"animal\") RETURN id(n)")
            .await
            .unwrap();
        assert_eq!(r.rows.len(), 2, "dog vertices are animals (subclass)");

        // cat is unrelated → no match.
        let r2 = run(&e, "MATCH (n:cat) WHERE is_a(n, \"animal\") RETURN id(n)")
            .await
            .unwrap();
        assert!(r2.rows.is_empty(), "cat is not an animal");

        // Negative class → no match even for dogs.
        let r3 = run(&e, "MATCH (n:dog) WHERE is_a(n, \"plant\") RETURN id(n)")
            .await
            .unwrap();
        assert!(r3.rows.is_empty(), "dog is not a plant");
    }

    // ---- O-9 retraction (full re-materialization on DELETE) ----

    #[tokio::test]
    async fn delete_retracts_transitive_inference() {
        let e = create_executor();
        ok(&e, "CREATE EDGE ancestor() TRANSITIVE").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 2->3:()").await;
        assert!(has(&e, 1, "ancestor", 3).await, "1->3 inferred");
        // Removing 2->3 must retract the inferred 1->3, keep asserted 1->2.
        ok(&e, "DELETE EDGE ancestor 2->3").await;
        assert!(!has(&e, 2, "ancestor", 3).await, "asserted 2->3 gone");
        assert!(!has(&e, 1, "ancestor", 3).await, "inferred 1->3 retracted");
        assert!(has(&e, 1, "ancestor", 2).await, "asserted 1->2 kept");
    }

    #[tokio::test]
    async fn delete_retracts_symmetric_inference() {
        let e = create_executor();
        ok(&e, "CREATE EDGE knows() SYMMETRIC").await;
        ok(&e, "INSERT EDGE knows() VALUES 1->2:()").await;
        assert!(has(&e, 2, "knows", 1).await, "symmetric 2->1 inferred");
        ok(&e, "DELETE EDGE knows 1->2").await;
        assert!(!has(&e, 1, "knows", 2).await, "asserted 1->2 gone");
        assert!(!has(&e, 2, "knows", 1).await, "inferred 2->1 retracted");
    }

    #[tokio::test]
    async fn delete_keeps_inference_still_supported_by_another_path() {
        // 1->3 is BOTH asserted and inferable (transitive via 1->2->3). Deleting
        // 2->3 retracts the transitive support, but asserted 1->3 must survive.
        let e = create_executor();
        ok(&e, "CREATE EDGE ancestor() TRANSITIVE").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 2->3:()").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 1->3:()").await; // asserted
        ok(&e, "DELETE EDGE ancestor 2->3").await;
        assert!(has(&e, 1, "ancestor", 3).await, "asserted 1->3 survives");
        assert!(!has(&e, 2, "ancestor", 3).await, "2->3 gone");
    }

    #[tokio::test]
    async fn delete_retracts_domain_range_vertex_type() {
        let e = create_executor();
        ok(&e, "CREATE CLASS person()").await;
        ok(&e, "CREATE CLASS city()").await;
        ok(&e, "CREATE TAG place()").await;
        ok(&e, "CREATE EDGE bornIn() DOMAIN person RANGE city").await;
        ok(&e, "INSERT VERTEX place() VALUES 2:()").await;
        ok(&e, "INSERT EDGE bornIn() VALUES 1->2:()").await;
        // RANGE city ⟹ vertex 2 is-a city (inferred vtype).
        let set = crate::ontology::vertex_class_set(&e.ctx, "default", 2)
            .await
            .unwrap()
            .unwrap();
        assert!(set.contains("city"), "range inferred city type");
        // Deleting the edge retracts the inferred type.
        ok(&e, "DELETE EDGE bornIn 1->2").await;
        let set2 = crate::ontology::vertex_class_set(&e.ctx, "default", 2)
            .await
            .unwrap()
            .unwrap();
        assert!(!set2.contains("city"), "inferred city type retracted");
    }

    #[tokio::test]
    async fn delete_without_semantics_is_noop_rematerialize() {
        // A space with no semantic edges must not be perturbed by retraction.
        let e = create_executor();
        ok(&e, "CREATE EDGE plain()").await;
        ok(&e, "INSERT EDGE plain() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE plain() VALUES 1->3:()").await;
        ok(&e, "DELETE EDGE plain 1->2").await;
        assert!(!has(&e, 1, "plain", 2).await, "deleted edge gone");
        assert!(has(&e, 1, "plain", 3).await, "untouched edge kept");
    }

    #[tokio::test]
    async fn create_edge_rejects_unknown_and_self_reference() {
        let e = create_executor();
        assert!(
            run(&e, "CREATE EDGE bad() INVERSE OF nonexistent")
                .await
                .is_err(),
            "INVERSE OF unknown edge must error"
        );
        assert!(
            run(&e, "CREATE EDGE foo() INVERSE OF foo").await.is_err(),
            "self INVERSE OF must error"
        );
    }

    // ---- Provenance (Phase 1): justification capture ----

    #[tokio::test]
    async fn transitive_records_provenance() {
        // 1->2, 2->3 over a TRANSITIVE edge infers 1->3; its provenance must
        // record the Transitive rule with both premise edges.
        let e = create_executor();
        ok(&e, "CREATE EDGE ancestor() TRANSITIVE").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 2->3:()").await;
        assert!(has(&e, 1, "ancestor", 3).await, "1->3 inferred");

        let justs = e
            .provenance_of(
                "default",
                &Fact::Edge {
                    s: 1,
                    p: "ancestor".to_string(),
                    d: 3,
                },
            )
            .await
            .unwrap();
        assert!(
            justs.iter().any(|j| j.rule == RuleKind::Transitive
                && j.premises
                    == vec![
                        Fact::Edge {
                            s: 1,
                            p: "ancestor".to_string(),
                            d: 2
                        },
                        Fact::Edge {
                            s: 2,
                            p: "ancestor".to_string(),
                            d: 3
                        },
                    ]),
            "transitive justification with both premises recorded, got {justs:?}"
        );
    }

    #[tokio::test]
    async fn symmetric_records_provenance() {
        let e = create_executor();
        ok(&e, "CREATE EDGE knows() SYMMETRIC").await;
        ok(&e, "INSERT EDGE knows() VALUES 1->2:()").await;
        let justs = e
            .provenance_of(
                "default",
                &Fact::Edge {
                    s: 2,
                    p: "knows".to_string(),
                    d: 1,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            justs.len(),
            1,
            "one justification for the symmetric reverse"
        );
        assert_eq!(justs[0].rule, RuleKind::Symmetric);
        assert_eq!(
            justs[0].premises,
            vec![Fact::Edge {
                s: 1,
                p: "knows".to_string(),
                d: 2
            }]
        );
    }

    #[tokio::test]
    async fn delete_clears_inferred_fact_provenance() {
        // Deleting 2->3 retracts the inferred 1->3 (full re-materialization),
        // and its provenance must be cleared with it (not left stale).
        let e = create_executor();
        ok(&e, "CREATE EDGE ancestor() TRANSITIVE").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 2->3:()").await;
        let fact = Fact::Edge {
            s: 1,
            p: "ancestor".to_string(),
            d: 3,
        };
        assert!(
            !e.provenance_of("default", &fact).await.unwrap().is_empty(),
            "provenance present before delete"
        );
        ok(&e, "DELETE EDGE ancestor 2->3").await;
        assert!(!has(&e, 1, "ancestor", 3).await, "inferred 1->3 retracted");
        assert!(
            e.provenance_of("default", &fact).await.unwrap().is_empty(),
            "provenance cleared after retraction"
        );
    }

    #[tokio::test]
    async fn domain_range_records_vtype_provenance() {
        let e = create_executor();
        ok(&e, "CREATE CLASS person()").await;
        ok(&e, "CREATE CLASS city()").await;
        ok(&e, "CREATE TAG place()").await;
        ok(&e, "CREATE EDGE bornIn() DOMAIN person RANGE city").await;
        ok(&e, "INSERT VERTEX place() VALUES 2:()").await;
        ok(&e, "INSERT EDGE bornIn() VALUES 1->2:()").await;
        // RANGE city ⟹ vertex 2 is-a city, justified by the bornIn edge.
        let justs = e
            .provenance_of(
                "default",
                &Fact::Vtype {
                    vid: 2,
                    class: "city".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(
            justs.iter().any(|j| j.rule == RuleKind::Range
                && j.premises
                    == vec![Fact::Edge {
                        s: 1,
                        p: "bornIn".to_string(),
                        d: 2
                    }]),
            "range justification recorded for inferred vtype, got {justs:?}"
        );
    }

    // ---- Phase 2: WHY explanation surface ----

    #[tokio::test]
    async fn why_explains_transitive_inference() {
        // `WHY 1 -> 3 OVER ancestor` should show 1->3 as inferred via transitive
        // from the two asserted edges, which appear as asserted leaves.
        let e = create_executor();
        ok(&e, "CREATE EDGE ancestor() TRANSITIVE").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 2->3:()").await;

        let r = run(&e, "WHY 1 -> 3 OVER ancestor").await.unwrap();
        assert_eq!(
            r.columns,
            vec!["depth", "fact", "status", "rule", "premises"]
        );
        // Root row: inferred via transitive, premises = both asserted edges.
        let root = &r.rows[0];
        assert_eq!(root[1], Value::String("1 -ancestor-> 3".to_string()));
        assert_eq!(root[2], Value::String("inferred".to_string()));
        assert_eq!(root[3], Value::String("transitive".to_string()));
        assert_eq!(
            root[4],
            Value::String("1 -ancestor-> 2, 2 -ancestor-> 3".to_string())
        );
        // Both premises appear as asserted leaves somewhere in the tree.
        let has_asserted = |f: &str| {
            r.rows.iter().any(|row| {
                row[1] == Value::String(f.to_string())
                    && row[2] == Value::String("asserted".to_string())
            })
        };
        assert!(
            has_asserted("1 -ancestor-> 2"),
            "premise 1->2 asserted leaf"
        );
        assert!(
            has_asserted("2 -ancestor-> 3"),
            "premise 2->3 asserted leaf"
        );
    }

    #[tokio::test]
    async fn why_unknown_edge_reports_not_found() {
        let e = create_executor();
        ok(&e, "CREATE EDGE ancestor() TRANSITIVE").await;
        let r = run(&e, "WHY 9 -> 99 OVER ancestor").await.unwrap();
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.rows[0][2], Value::String("not found".to_string()));
    }

    // ---- Phase 3: incremental DRed retraction — the hard cases ----

    #[tokio::test]
    async fn dred_retracts_cycle_well_founded() {
        // A transitive 3-cycle 1->2->3->1 closes to ALL 9 pairs (incl. self-loops
        // 1->1,2->2,3->3 and back-edges 2->1,3->2). Deleting asserted 3->1 must
        // collapse the closure to exactly {1->2, 2->3, 1->3}: every fact whose
        // derivation passed through 3->1 — including 1->3's *cyclic* alternative
        // justification via 1->1 — is overdeleted, then 1->3 is REDERIVED from
        // the surviving 1->2,2->3. This is the well-founded-support case where a
        // naive cascade (no rederive) would wrongly drop 1->3 or keep cycle facts.
        let e = create_executor();
        ok(&e, "CREATE EDGE ancestor() TRANSITIVE").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 2->3:()").await;
        ok(&e, "INSERT EDGE ancestor() VALUES 3->1:()").await;
        // Sanity: the cycle fully closed.
        assert!(
            has(&e, 1, "ancestor", 1).await,
            "self-loop inferred in cycle"
        );
        assert!(
            has(&e, 2, "ancestor", 1).await,
            "back-edge inferred in cycle"
        );

        ok(&e, "DELETE EDGE ancestor 3->1").await;

        // Survivors + the one still-supported inference.
        assert!(has(&e, 1, "ancestor", 2).await, "asserted 1->2 kept");
        assert!(has(&e, 2, "ancestor", 3).await, "asserted 2->3 kept");
        assert!(
            has(&e, 1, "ancestor", 3).await,
            "1->3 rederived from 1->2,2->3"
        );
        // Everything that depended on 3->1 is gone.
        assert!(!has(&e, 3, "ancestor", 1).await, "deleted 3->1 gone");
        assert!(!has(&e, 2, "ancestor", 1).await, "back-edge 2->1 retracted");
        assert!(!has(&e, 3, "ancestor", 2).await, "back-edge 3->2 retracted");
        assert!(!has(&e, 1, "ancestor", 1).await, "self-loop 1->1 retracted");
        assert!(!has(&e, 2, "ancestor", 2).await, "self-loop 2->2 retracted");
        assert!(!has(&e, 3, "ancestor", 3).await, "self-loop 3->3 retracted");
    }

    #[tokio::test]
    async fn dred_keeps_fact_with_surviving_second_justification() {
        // related has TWO subproperties; knows 1->2 and likes 1->2 each entail
        // related 1->2 (two justifications). Deleting knows 1->2 must overdelete
        // the inferred related 1->2 then REDERIVE it from the surviving likes 1->2.
        let e = create_executor();
        ok(&e, "CREATE EDGE related()").await;
        ok(&e, "CREATE EDGE knows() SUBPROPERTY OF related").await;
        ok(&e, "CREATE EDGE likes() SUBPROPERTY OF related").await;
        ok(&e, "INSERT EDGE knows() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE likes() VALUES 1->2:()").await;
        assert!(has(&e, 1, "related", 2).await, "related inferred (both)");

        ok(&e, "DELETE EDGE knows 1->2").await;
        assert!(!has(&e, 1, "knows", 2).await, "knows deleted");
        assert!(has(&e, 1, "likes", 2).await, "likes survives");
        assert!(
            has(&e, 1, "related", 2).await,
            "related kept — still supported by likes"
        );

        // Deleting the second support too now fully retracts related.
        ok(&e, "DELETE EDGE likes 1->2").await;
        assert!(
            !has(&e, 1, "related", 2).await,
            "related retracted once all support gone"
        );
    }

    // ---- Property chain (owl:propertyChainAxiom, prp-spo2) ----

    #[tokio::test]
    async fn property_chain_two_step_derives() {
        // grandparent ← parent ∘ parent (same-type chain — the transitive-like case).
        let e = create_executor();
        ok(&e, "CREATE EDGE parent()").await;
        ok(&e, "CREATE EDGE grandparent() CHAIN parent, parent").await;
        ok(&e, "INSERT EDGE parent() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE parent() VALUES 2->3:()").await;
        assert!(
            has(&e, 1, "grandparent", 3).await,
            "1-parent->2-parent->3 ⟹ 1-grandparent->3"
        );
        assert!(
            !has(&e, 1, "grandparent", 2).await,
            "single hop is not grandparent"
        );
        // parent itself is not transitive, so no inferred parent 1->3.
        assert!(
            !has(&e, 1, "parent", 3).await,
            "parent stays non-transitive"
        );
    }

    #[tokio::test]
    async fn property_chain_distinct_link_types() {
        // q ← p1 ∘ p2 with p1 ≠ p2; closes via the *second* link inserted last
        // (exercises the backward chain rule).
        let e = create_executor();
        ok(&e, "CREATE EDGE knows()").await;
        ok(&e, "CREATE EDGE worksAt()").await;
        ok(&e, "CREATE EDGE colleagueOrg() CHAIN knows, worksAt").await;
        ok(&e, "INSERT EDGE knows() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE worksAt() VALUES 2->100:()").await;
        assert!(
            has(&e, 1, "colleagueOrg", 100).await,
            "1-knows->2-worksAt->100 ⟹ 1-colleagueOrg->100"
        );
        assert!(!has(&e, 2, "colleagueOrg", 100).await, "no spurious chain");
    }

    #[tokio::test]
    async fn why_explains_property_chain() {
        let e = create_executor();
        ok(&e, "CREATE EDGE parent()").await;
        ok(&e, "CREATE EDGE grandparent() CHAIN parent, parent").await;
        ok(&e, "INSERT EDGE parent() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE parent() VALUES 2->3:()").await;
        let r = run(&e, "WHY 1 -> 3 OVER grandparent").await.unwrap();
        let root = &r.rows[0];
        assert_eq!(root[2], Value::String("inferred".to_string()));
        assert_eq!(root[3], Value::String("propertyChain".to_string()));
        assert_eq!(
            root[4],
            Value::String("1 -parent-> 2, 2 -parent-> 3".to_string())
        );
    }

    #[tokio::test]
    async fn property_chain_retracts_on_delete() {
        // DRed retraction works for chain-derived edges too.
        let e = create_executor();
        ok(&e, "CREATE EDGE parent()").await;
        ok(&e, "CREATE EDGE grandparent() CHAIN parent, parent").await;
        ok(&e, "INSERT EDGE parent() VALUES 1->2:()").await;
        ok(&e, "INSERT EDGE parent() VALUES 2->3:()").await;
        assert!(has(&e, 1, "grandparent", 3).await, "grandparent inferred");
        ok(&e, "DELETE EDGE parent 2->3").await;
        assert!(
            !has(&e, 1, "grandparent", 3).await,
            "grandparent retracted when a chain link is deleted"
        );
        assert!(has(&e, 1, "parent", 2).await, "surviving link kept");
    }

    #[tokio::test]
    async fn create_edge_rejects_bad_chain() {
        let e = create_executor();
        ok(&e, "CREATE EDGE parent()").await;
        assert!(
            run(&e, "CREATE EDGE gp() CHAIN parent").await.is_err(),
            "1-link chain must be rejected (exactly 2 required)"
        );
        assert!(
            run(&e, "CREATE EDGE gp() CHAIN parent, parent, parent")
                .await
                .is_err(),
            "3-link chain must be rejected"
        );
        assert!(
            run(&e, "CREATE EDGE gp() CHAIN parent, nope")
                .await
                .is_err(),
            "unknown chain target must be rejected"
        );
    }
}
