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
//! **Inserts: incremental.** Declare semantics *before* inserting edges
//! (`CREATE EDGE ... TRANSITIVE` then `INSERT EDGE`); each insert extends the
//! closure over the current graph. A write cap (`max_traversal_nodes`) guards
//! pathological closures and logs if hit.
//!
//! **Deletes — two strategies.**
//! - DELETE EDGE uses **incremental DRed** (O-10 Phase 3,
//!   [`Executor::retract_edges_incremental`]): overdelete the deleted edges'
//!   dependent closure via reverse provenance, then rederive from surviving
//!   neighbors. Touches only the affected region.
//! - DELETE VERTEX still uses **full re-materialization** (O-9,
//!   [`Executor::rematerialize_space`]): drops all inferred facts + provenance
//!   and re-derives from the surviving asserted edges. O(graph) per delete; an
//!   incremental DELETE VERTEX path is a follow-up.
//!
//! Both retract stale entailments while keeping facts still supported by another
//! derivation.

use super::provenance::{Fact, Justification, RuleKind};
use super::Executor;
use crate::algo;
use crate::error::Result;
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
    fn is_empty(&self) -> bool {
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
    /// Materialize the closure entailed by newly inserted `triples`. No-op when
    /// the space declares no semantic relations. Called by INSERT EDGE after the
    /// asserted edges are committed.
    pub(super) async fn materialize_inserted_edges(
        &self,
        space: &str,
        triples: Vec<Triple>,
    ) -> Result<()> {
        let meta = self.load_rel_meta(space).await?;
        if meta.is_empty() {
            return Ok(());
        }
        self.run_materialization(space, triples, &meta).await
    }

    /// Full re-materialization of a space's ontology closure (PLAN.md O-9, the
    /// O-0 phase-1 retraction strategy). No-op when the space declares no
    /// semantic relations. Called by DELETE EDGE/VERTEX after the assertion is
    /// removed: it discards **all** materialized facts (`__inferred__` edges and
    /// every `vtype`) and re-derives them from the surviving asserted edges, so a
    /// deleted fact's stale entailments disappear while entailments still
    /// supported by other paths are re-derived. Idempotent and complete (no
    /// DRed-style overdeletion).
    ///
    /// Cost is O(graph) per call (full `{space}:edge:`/`{space}:vtype:` scans);
    /// the empty-meta guard keeps semantic-free spaces at zero cost. Incremental
    /// (B/F) retraction is a later optimization (O-0 phase 2).
    pub(super) async fn rematerialize_space(&self, space: &str) -> Result<()> {
        let meta = self.load_rel_meta(space).await?;
        if meta.is_empty() {
            return Ok(());
        }

        // Partition the edge keyspace into asserted (kept as re-derivation seeds)
        // and inferred (deleted, both directions).
        let edge_prefix = format!("{}:edge:", space);
        let entries = self.ctx.kvstore.scan_prefix(edge_prefix.as_bytes()).await?;
        let mut asserted: Vec<Triple> = Vec::new();
        for (key, value) in entries {
            let Ok(edge) = VertexCodec::decode_edge(&value) else {
                continue; // tolerate a non-edge / legacy blob under the prefix
            };
            if edge.properties.contains_key(INFERRED_MARKER) {
                self.ctx.kvstore.delete(&key).await?;
                let in_key = SchemaKey::in_edge_data(
                    space,
                    edge.dst_vid,
                    &edge.edge_type,
                    edge.src_vid,
                    edge.ranking,
                );
                self.ctx.kvstore.delete(&in_key).await?;
            } else {
                asserted.push((edge.src_vid, edge.edge_type.clone(), edge.dst_vid));
            }
        }

        // Drop all inferred vertex types (domain/range only produces these), to
        // be re-derived by the run below.
        let vtype_prefix = format!("{}:vtype:", space);
        for (key, _) in self
            .ctx
            .kvstore
            .scan_prefix(vtype_prefix.as_bytes())
            .await?
        {
            self.ctx.kvstore.delete(&key).await?;
        }

        // Drop all provenance: it is rebuilt by the run below from the surviving
        // asserted edges, so a deleted fact's stale justifications disappear.
        self.clear_provenance(space).await?;

        // Re-derive the full closure from the surviving asserted edges.
        self.run_materialization(space, asserted, &meta).await
    }

    /// Load every edge schema's `semantics` and build the [`RelMeta`] index.
    async fn load_rel_meta(&self, space: &str) -> Result<RelMeta> {
        let prefix = format!("space:{}:edge:", space);
        let entries = self.ctx.kvstore.scan_prefix(prefix.as_bytes()).await?;
        let mut meta = RelMeta::default();
        for (_key, value) in entries {
            let json: serde_json::Value = match serde_json::from_slice(&value) {
                Ok(j) => j,
                Err(_) => continue, // tolerate a non-schema / legacy blob
            };
            let Some(name) = json.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            let name = name.to_string();
            let sem: SemanticFlags = json
                .get("semantics")
                .cloned()
                .and_then(|s| serde_json::from_value(s).ok())
                .unwrap_or_default();
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
            // CHAIN [p1, p2]: index by both links so the chain closes whichever
            // edge is inserted last. Ignore malformed chains (len != 2) — the
            // DDL validator already rejects them; this is defensive.
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
                for n in algo::get_neighbors(&self.ctx, space, d, &etype).await? {
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
                for n in algo::get_incoming_neighbors(&self.ctx, space, s, &etype).await? {
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
                    for n in algo::get_neighbors(&self.ctx, space, d, &et).await? {
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
                    for n in algo::get_incoming_neighbors(&self.ctx, space, s, &et).await? {
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

            // domain/range ⟹ vertex type inference (subject is-a domain,
            // object is-a range). Types do not entail further edges, so they
            // are written (with provenance) but not enqueued.
            if let Some(class) = meta.domain.get(&p) {
                self.record_provenance(
                    space,
                    &Fact::Vtype {
                        vid: s,
                        class: class.clone(),
                    },
                    Justification {
                        rule: RuleKind::Domain,
                        premises: vec![premise.clone()],
                    },
                )
                .await?;
                if self.assert_vertex_type(space, s, class).await? {
                    written += 1;
                }
            }
            if let Some(class) = meta.range.get(&p) {
                self.record_provenance(
                    space,
                    &Fact::Vtype {
                        vid: d,
                        class: class.clone(),
                    },
                    Justification {
                        rule: RuleKind::Range,
                        premises: vec![premise.clone()],
                    },
                )
                .await?;
                if self.assert_vertex_type(space, d, class).await? {
                    written += 1;
                }
            }
            if written >= cap {
                tracing::warn!(
                    cap,
                    "O-5 materialization hit max_traversal_nodes; closure may be partial"
                );
                return Ok(());
            }

            for (triple, justification) in derived {
                let (s2, p2, d2) = (triple.0, triple.1.clone(), triple.2);
                // Record provenance for *every* derivation — even of an edge
                // already in the graph — so the justification set is complete
                // for retraction. record_provenance dedups identical entries.
                self.record_provenance(
                    space,
                    &Fact::Edge {
                        s: s2,
                        p: p2.clone(),
                        d: d2,
                    },
                    justification,
                )
                .await?;

                if !seen.insert(triple.clone()) {
                    continue; // already queued/processed this run
                }
                // Already in the graph (asserted, or inferred at an earlier
                // insert): its own consequences are already materialized — skip
                // writing/enqueueing (its provenance above is still recorded).
                if self.triple_exists(space, s2, &p2, d2).await? {
                    continue;
                }
                self.write_inferred_edge(space, s2, &p2, d2).await?;
                written += 1;
                if written >= cap {
                    tracing::warn!(
                        cap,
                        "O-5 materialization hit max_traversal_nodes; closure may be partial"
                    );
                    return Ok(());
                }
                worklist.push_back(triple);
            }
        }
        Ok(())
    }

    /// True if any edge `(s)-p->(d)` exists at any ranking (asserted or inferred).
    pub(super) async fn triple_exists(&self, space: &str, s: i64, p: &str, d: i64) -> Result<bool> {
        // Trailing colon prevents dst prefix collisions (`:1:` vs `:10:`).
        let prefix = format!("{}:edge:{}:{}:{}:", space, s, p, d);
        let hits = self
            .ctx
            .kvstore
            .scan_with_filter(prefix.as_bytes(), Box::new(|_, _| true), Some(1))
            .await?;
        Ok(!hits.is_empty())
    }

    /// Persist an inferred edge `(s)-p->(d)` at ranking 0 — forward + reverse
    /// index — tagged `__inferred__` so it is queryable yet distinguishable.
    async fn write_inferred_edge(&self, space: &str, s: i64, p: &str, d: i64) -> Result<()> {
        let mut properties = HashMap::new();
        properties.insert(INFERRED_MARKER.to_string(), Value::Bool(true));
        let edge = CodecEdgeData {
            src_vid: s,
            dst_vid: d,
            edge_type: p.to_string(),
            ranking: 0,
            properties,
        };
        let data = VertexCodec::encode_edge(&edge)
            .map_err(|e| crate::error::ExecutionError::Io(std::io::Error::other(e.to_string())))?;
        let fwd = format!("{}:edge:{}:{}:{}:0", space, s, p, d);
        // Immediate puts (not batched) so the running fixpoint's neighbor/
        // existence reads observe edges written earlier in this same run.
        self.ctx.kvstore.put(fwd.as_bytes(), &data).await?;
        let in_key = SchemaKey::in_edge_data(space, d, p, s, 0);
        self.ctx.kvstore.put(&in_key, &data).await?;
        Ok(())
    }

    /// Record inferred class membership `{space}:vtype:{vid}:{class}` (from
    /// domain/range). Returns `true` if newly written, `false` if already
    /// present (so the caller counts only new inferences against the cap).
    async fn assert_vertex_type(&self, space: &str, vid: i64, class: &str) -> Result<bool> {
        let key = SchemaKey::vtype(space, vid, class);
        if self.ctx.kvstore.get(&key).await?.is_some() {
            return Ok(false);
        }
        self.ctx.kvstore.put(&key, &[]).await?;
        Ok(true)
    }

    /// Incremental retraction via provenance-based **DRed** (PLAN.md O-10
    /// Phase 3), replacing the O(graph) full re-materialization for DELETE EDGE.
    ///
    /// `deleted` are the asserted edges just removed by the caller. Two phases:
    ///
    /// 1. **Overdelete** — transitive closure over the reverse-provenance index
    ///    (`dependents_of`) from each deleted edge: every inferred fact that
    ///    named a removed fact as a premise is tentatively deleted (edge keys +
    ///    its provenance), recursing through inferred edges. An edge that *also*
    ///    exists as asserted survives and is not recursed through (it still
    ///    supports its dependents). vtypes never entail, so they are leaves.
    /// 2. **Rederive** — re-run materialization seeded with the surviving
    ///    asserted edges incident to the affected vertices. Rules are local
    ///    (1-edge for symmetric/inverse/subprop/domain/range; adjacent-edge for
    ///    transitive, whose neighbor lookups read the live graph), so this
    ///    neighborhood seed re-derives exactly the facts still supported — the
    ///    DRed rederive pass that gives correct retraction under recursion
    ///    (transitive cycles) without DRed's classic over-deletion surviving.
    ///
    /// No-op for spaces without semantic relations (nothing was inferred).
    pub(super) async fn retract_edges_incremental(
        &self,
        space: &str,
        deleted: Vec<Triple>,
    ) -> Result<()> {
        let meta = self.load_rel_meta(space).await?;
        if meta.is_empty() {
            return Ok(());
        }

        let mut affected: HashSet<i64> = HashSet::new();
        let mut visited: HashSet<Fact> = HashSet::new();
        let mut processed: HashSet<Triple> = HashSet::new();
        let mut frontier: VecDeque<Triple> = VecDeque::new();
        for (s, p, d) in deleted {
            affected.insert(s);
            affected.insert(d);
            frontier.push_back((s, p, d));
        }

        // ---- Overdelete ----
        while let Some((s, p, d)) = frontier.pop_front() {
            if !processed.insert((s, p.clone(), d)) {
                continue;
            }
            for g in self.dependents_of(space, s, &p, d).await? {
                if !visited.insert(g.clone()) {
                    continue;
                }
                match &g {
                    Fact::Edge {
                        s: gs,
                        p: gp,
                        d: gd,
                    } => {
                        // An asserted copy survives → it still supports its own
                        // dependents; don't delete or recurse through it.
                        if self.edge_has_asserted(space, *gs, gp, *gd).await? {
                            continue;
                        }
                        affected.insert(*gs);
                        affected.insert(*gd);
                        // Inferred edges are written at ranking 0 (write_inferred_edge).
                        self.ctx
                            .kvstore
                            .delete(&SchemaKey::edge_data(space, *gs, gp, *gd, 0))
                            .await?;
                        self.ctx
                            .kvstore
                            .delete(&SchemaKey::in_edge_data(space, *gd, gp, *gs, 0))
                            .await?;
                        self.purge_fact_provenance(space, &g).await?;
                        frontier.push_back((*gs, gp.clone(), *gd));
                    }
                    Fact::Vtype { vid, class } => {
                        affected.insert(*vid);
                        self.ctx
                            .kvstore
                            .delete(&SchemaKey::vtype(space, *vid, class))
                            .await?;
                        self.purge_fact_provenance(space, &g).await?;
                    }
                }
            }
            // Dependents are now traversed, so the removed edge's reverse-prov
            // entry can go (it supports nothing). Done *after* reading dependents
            // above — deleting it earlier would lose the traversal links.
            self.delete_prov_rev_edge(space, s, &p, d).await?;
        }

        // ---- Rederive from surviving asserted edges incident to affected vertices ----
        let mut seeds: Vec<Triple> = Vec::new();
        let mut seen: HashSet<Triple> = HashSet::new();
        for v in &affected {
            for t in self.surviving_asserted_incident(space, *v).await? {
                if seen.insert(t.clone()) {
                    seeds.push(t);
                }
            }
        }
        self.run_materialization(space, seeds, &meta).await
    }

    /// True if an **asserted** (non-`__inferred__`) edge `(s)-p->(d)` exists at
    /// any ranking. Distinguishes a fact that survives a delete (asserted) from
    /// one that is purely materialized (retractable).
    async fn edge_has_asserted(&self, space: &str, s: i64, p: &str, d: i64) -> Result<bool> {
        let prefix = format!("{}:edge:{}:{}:{}:", space, s, p, d);
        for (_k, v) in self.ctx.kvstore.scan_prefix(prefix.as_bytes()).await? {
            if let Ok(edge) = VertexCodec::decode_edge(&v) {
                if !edge.properties.contains_key(INFERRED_MARKER) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Surviving **asserted** edges incident to vertex `v` (both out- and
    /// in-edges), as `(src, type, dst)` triples — the rederive seed set. Inferred
    /// edges are excluded (they are re-derived, not seeds).
    async fn surviving_asserted_incident(&self, space: &str, v: i64) -> Result<Vec<Triple>> {
        let mut out = Vec::new();
        let push_asserted = |edge: byoridb_codec::EdgeData, acc: &mut Vec<Triple>| {
            if !edge.properties.contains_key(INFERRED_MARKER) {
                acc.push((edge.src_vid, edge.edge_type, edge.dst_vid));
            }
        };
        // Out-edges: {space}:edge:{v}:
        let out_prefix = SchemaKey::edge_data_src_prefix(space, v);
        for (_k, val) in self.ctx.kvstore.scan_prefix(&out_prefix).await? {
            if let Ok(edge) = VertexCodec::decode_edge(&val) {
                push_asserted(edge, &mut out);
            }
        }
        // In-edges: {space}:in-edge:{v}: (value is the denormalized edge payload)
        let in_prefix = SchemaKey::in_edge_data_dst_prefix(space, v);
        for (_k, val) in self.ctx.kvstore.scan_prefix(&in_prefix).await? {
            if let Ok(edge) = VertexCodec::decode_edge(&val) {
                push_asserted(edge, &mut out);
            }
        }
        Ok(out)
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
