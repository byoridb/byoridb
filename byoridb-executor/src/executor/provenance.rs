// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Provenance for materialized inference (PLAN.md O-provenance, Phase 1).
//!
//! Every fact O-5 derives — an inferred edge or an inferred vertex type — is
//! recorded with its **justification(s)**: which rule fired and which premise
//! facts it consumed. Provenance lives in a side keyspace `{space}:prov:`,
//! disjoint from the `{space}:edge:`/`{space}:vtype:` data, so it adds no cost
//! to read paths and no risk to the existing materialization keyspace.
//!
//! A fact can be entailed more than one way (e.g. `1->3` via `transitive` and
//! also asserted), so justifications **accumulate as a deduplicated set** rather
//! than overwrite. Capturing *all* justifications — even re-derivations of a
//! fact already in the graph — is what makes correct retraction possible later.
//!
//! `WHY` reads these records; corrupt values fail explicitly. Recovery rebuilds
//! all provenance from assertions. Each newly inferred fact and both directions
//! of its provenance are committed together in a single Memory/redb batch.

use super::{Executor, ExecutorResult};
use crate::error::{ExecutionError, Result};
use crate::key::SchemaKey;
use byoridb_common::Value;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Which O-5 rule entailed a fact. Mirrors the rule set in `inference.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum RuleKind {
    Symmetric,
    InverseOf,
    SubPropertyOf,
    EquivalentProperty,
    Transitive,
    PropertyChain,
    Domain,
    Range,
}

/// A fact in the materialized graph — the unit a justification refers to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) enum Fact {
    /// An edge `(s)-p->(d)`.
    Edge { s: i64, p: String, d: i64 },
    /// A vertex type membership `vid is-a class`.
    Vtype { vid: i64, class: String },
}

/// One way a fact was entailed: a rule plus the premise facts it consumed.
/// `transitive` has two premises; the other rules have one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Justification {
    pub rule: RuleKind,
    pub premises: Vec<Fact>,
}

/// KV key under which a fact's justification set is stored.
fn prov_key(space: &str, fact: &Fact) -> Vec<u8> {
    match fact {
        Fact::Edge { s, p, d } => SchemaKey::prov_edge(space, *s, p, *d),
        Fact::Vtype { vid, class } => SchemaKey::prov_vtype(space, *vid, class),
    }
}

impl Executor {
    /// Build all forward/reverse provenance writes without committing, allowing
    /// the derived fact itself to be persisted in the same atomic batch.
    pub(super) async fn provenance_puts(
        &self,
        space: &str,
        fact: &Fact,
        justification: Justification,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let key = prov_key(space, fact);
        let mut justs: Vec<Justification> = match self.ctx.kvstore.get(&key).await? {
            Some(bytes) => serde_json::from_slice(&bytes)?,
            None => Vec::new(),
        };
        if justs.contains(&justification) {
            return Ok(Vec::new());
        }
        let mut puts = Vec::new();
        let mut premises = HashSet::new();
        for premise in &justification.premises {
            if !premises.insert(premise) {
                continue;
            }
            if let Fact::Edge { s, p, d } = premise {
                let key = SchemaKey::prov_rev_edge(space, *s, p, *d);
                let mut deps: Vec<Fact> = match self.ctx.kvstore.get(&key).await? {
                    Some(bytes) => serde_json::from_slice(&bytes)?,
                    None => Vec::new(),
                };
                if !deps.contains(fact) {
                    deps.push(fact.clone());
                    puts.push((key, serde_json::to_vec(&deps)?));
                }
            }
        }
        justs.push(justification);
        puts.push((key, serde_json::to_vec(&justs)?));
        Ok(puts)
    }

    pub(super) async fn provenance_of(
        &self,
        space: &str,
        fact: &Fact,
    ) -> Result<Vec<Justification>> {
        match self.ctx.kvstore.get(&prov_key(space, fact)).await? {
            Some(bytes) => Ok(serde_json::from_slice(&bytes)?),
            None => Ok(Vec::new()),
        }
    }

    /// Explain how an inferred edge `(src)-edge_type->(dst)` was entailed, by
    /// walking its provenance tree (O-provenance Phase 2 — `WHY` statement).
    ///
    /// Returns a flattened pre-order tree: each row is a fact with the rule
    /// that derived it and its premises. Asserted facts (no provenance) are
    /// leaves; a fact reached more than once (shared or cyclic support, e.g.
    /// transitive cycles) is shown once and not re-expanded. Columns:
    /// `depth | fact | status | rule | premises`.
    pub(super) async fn explain_inference(
        &self,
        src: i64,
        edge_type: &str,
        dst: i64,
    ) -> Result<ExecutorResult> {
        let space = self
            .ctx
            .space
            .clone()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;

        let columns = vec![
            "depth".to_string(),
            "fact".to_string(),
            "status".to_string(),
            "rule".to_string(),
            "premises".to_string(),
        ];
        let dash = || Value::String("—".to_string());
        let mut rows: Vec<Vec<Value>> = Vec::new();

        let root = Fact::Edge {
            s: src,
            p: edge_type.to_string(),
            d: dst,
        };

        // A fact that isn't in the graph at all gets a clear "not found" row
        // rather than being mislabeled "asserted" (which empty provenance means).
        if !self.triple_exists(&space, src, edge_type, dst).await? {
            rows.push(vec![
                Value::Int(0),
                Value::String(fact_to_string(&root)),
                Value::String("not found".to_string()),
                dash(),
                dash(),
            ]);
            return Ok(ExecutorResult {
                columns,
                rows,
                latency_ms: 0,
            });
        }

        // Pre-order DFS over the justification tree.
        let mut visited: HashSet<Fact> = HashSet::new();
        let mut stack: Vec<(Fact, i64)> = vec![(root, 0)];
        while let Some((fact, depth)) = stack.pop() {
            let fact_str = fact_to_string(&fact);
            if !visited.insert(fact.clone()) {
                rows.push(vec![
                    Value::Int(depth),
                    Value::String(fact_str),
                    Value::String("(already explained above)".to_string()),
                    dash(),
                    dash(),
                ]);
                continue;
            }
            let justs = self.provenance_of(&space, &fact).await?;
            if justs.is_empty() {
                // No provenance ⟹ an asserted ground fact (leaf).
                rows.push(vec![
                    Value::Int(depth),
                    Value::String(fact_str),
                    Value::String("asserted".to_string()),
                    dash(),
                    dash(),
                ]);
                continue;
            }
            for j in &justs {
                let premises = j
                    .premises
                    .iter()
                    .map(fact_to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                rows.push(vec![
                    Value::Int(depth),
                    Value::String(fact_str.clone()),
                    Value::String("inferred".to_string()),
                    Value::String(rule_to_string(&j.rule).to_string()),
                    Value::String(premises),
                ]);
            }
            // Expand premises depth-first (reverse push keeps a stable pre-order).
            for j in justs.into_iter().rev() {
                for prem in j.premises.into_iter().rev() {
                    stack.push((prem, depth + 1));
                }
            }
        }

        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: 0,
        })
    }
}

/// Human-readable rendering of a fact for the `WHY` result.
fn fact_to_string(fact: &Fact) -> String {
    match fact {
        Fact::Edge { s, p, d } => format!("{} -{}-> {}", s, p, d),
        Fact::Vtype { vid, class } => format!("{} is-a {}", vid, class),
    }
}

/// Rule name as shown in the `WHY` result.
fn rule_to_string(rule: &RuleKind) -> &'static str {
    match rule {
        RuleKind::Symmetric => "symmetric",
        RuleKind::InverseOf => "inverseOf",
        RuleKind::SubPropertyOf => "subPropertyOf",
        RuleKind::EquivalentProperty => "equivalentProperty",
        RuleKind::Transitive => "transitive",
        RuleKind::PropertyChain => "propertyChain",
        RuleKind::Domain => "domain",
        RuleKind::Range => "range",
    }
}
