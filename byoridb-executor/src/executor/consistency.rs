// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Ontology consistency checking (PLAN.md O-6).
//!
//! `CHECK CONSISTENCY` scans the space and reports vertices that violate a
//! `DISJOINT WITH` declaration — i.e. belong to two classes declared disjoint.
//! Class membership is the full ontology set (declared tags ∪ O-5 inferred
//! types ∪ their O-3 superclasses, via [`crate::ontology::vertex_class_set`]),
//! so a violation surfaces even when it arises only through subclassing or
//! domain/range inference.
//!
//! Result columns: `vid / class_a / class_b` (one row per violated disjoint
//! pair per vertex, `class_a < class_b` to dedup symmetric pairs). An empty
//! result means consistent. Open-world note: only disjointness is checked —
//! domain/range are *inferential* (they add types, never violate).

use super::{Executor, ExecutorResult};
use crate::error::Result;
use crate::key::SchemaKey;
use byoridb_common::Value;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Deserialize)]
struct ClassDisjoint {
    name: String,
    #[serde(default)]
    disjoint: Vec<String>,
}

impl Executor {
    pub(super) async fn execute_check_consistency(&self) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();
        let columns = vec![
            "vid".to_string(),
            "class_a".to_string(),
            "class_b".to_string(),
        ];

        // Build the symmetric disjoint map from all class records.
        let mut disjoint: HashMap<String, HashSet<String>> = HashMap::new();
        for (_key, value) in self
            .ctx
            .kvstore
            .scan_prefix(&SchemaKey::class_prefix(&space))
            .await?
        {
            let Ok(def) = serde_json::from_slice::<ClassDisjoint>(&value) else {
                continue;
            };
            for d in def.disjoint {
                disjoint
                    .entry(def.name.clone())
                    .or_default()
                    .insert(d.clone());
                disjoint.entry(d).or_default().insert(def.name.clone());
            }
        }
        if disjoint.is_empty() {
            return Ok(ExecutorResult {
                columns,
                rows: Vec::new(),
                latency_ms: 0,
            });
        }

        let vid_type = crate::vid::space_vid_type(&self.ctx, &space).await?;

        // Scan every vertex; report disjoint-pair memberships.
        let mut rows = Vec::new();
        for (key, _value) in self
            .ctx
            .kvstore
            .scan_prefix(&SchemaKey::vertex_prefix(&space))
            .await?
        {
            let Some(vid) = vid_from_vertex_key(&key) else {
                continue;
            };
            let Some(classes) = crate::ontology::vertex_class_set(&self.ctx, &space, vid).await?
            else {
                continue;
            };
            let mut reported: HashSet<(String, String)> = HashSet::new();
            for c in &classes {
                let Some(targets) = disjoint.get(c) else {
                    continue;
                };
                for t in targets {
                    if !classes.contains(t) {
                        continue;
                    }
                    let pair = if c < t {
                        (c.clone(), t.clone())
                    } else {
                        (t.clone(), c.clone())
                    };
                    if reported.insert(pair.clone()) {
                        rows.push(vec![
                            crate::vid::display_vid(&self.ctx, &space, vid_type, vid).await?,
                            Value::String(pair.0),
                            Value::String(pair.1),
                        ]);
                    }
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

/// Parse the trailing vid from a `{space}:vertex:{vid}` key. Space names are
/// identifiers (no `:`), so the vid is the final colon-delimited segment.
fn vid_from_vertex_key(key: &[u8]) -> Option<i64> {
    std::str::from_utf8(key)
        .ok()?
        .rsplit(':')
        .next()?
        .parse::<i64>()
        .ok()
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

    async fn run(e: &Executor, q: &str) -> Result<ExecutorResult> {
        let stmt = byoridb_parser::parse(q).expect("parse");
        let plan = crate::ExecutionPlanBuilder::build(stmt).expect("plan");
        e.execute(plan).await
    }

    async fn ok(e: &Executor, q: &str) {
        run(e, q)
            .await
            .unwrap_or_else(|err| panic!("query failed: {q}\n{err:?}"));
    }

    #[tokio::test]
    async fn detects_disjoint_violation_via_inferred_type() {
        let e = create_executor();
        ok(&e, "CREATE CLASS person()").await;
        ok(&e, "CREATE CLASS city() DISJOINT WITH person").await;
        ok(&e, "CREATE EDGE bornIn() RANGE city").await;
        // Vertex 2 is declared a person...
        ok(&e, "INSERT VERTEX person() VALUES 2:()").await;
        // ...and inferred a city by RANGE → person ∩ city are disjoint = violation.
        ok(&e, "INSERT EDGE bornIn() VALUES 1->2:()").await;

        let r = run(&e, "CHECK CONSISTENCY").await.unwrap();
        assert_eq!(r.columns, vec!["vid", "class_a", "class_b"]);
        assert_eq!(r.rows.len(), 1, "one violating vertex");
        assert!(matches!(r.rows[0][0], Value::Int(2)));
        // Pair reported sorted (class_a < class_b): city, person.
        assert!(matches!(&r.rows[0][1], Value::String(s) if s == "city"));
        assert!(matches!(&r.rows[0][2], Value::String(s) if s == "person"));
    }

    #[tokio::test]
    async fn detects_violation_through_subclass_ancestors() {
        // Disjoint declared between *parent* classes; the vertex reaches both
        // only through subclassing (dog→animal) and domain/range inference
        // (range car → car→vehicle). animal ⊥ vehicle ⟹ violation.
        let e = create_executor();
        ok(&e, "CREATE CLASS animal()").await;
        ok(&e, "CREATE CLASS vehicle() DISJOINT WITH animal").await;
        ok(&e, "CREATE CLASS dog() SUBCLASS OF animal").await;
        ok(&e, "CREATE CLASS car() SUBCLASS OF vehicle").await;
        ok(&e, "CREATE EDGE owns() RANGE car").await;
        ok(&e, "INSERT VERTEX dog() VALUES 1:()").await; // 1 is dog ⊑ animal
        ok(&e, "INSERT EDGE owns() VALUES 9->1:()").await; // range car ⟹ 1 is car ⊑ vehicle

        let r = run(&e, "CHECK CONSISTENCY").await.unwrap();
        assert_eq!(r.rows.len(), 1);
        assert!(matches!(r.rows[0][0], Value::Int(1)));
        assert!(matches!(&r.rows[0][1], Value::String(s) if s == "animal"));
        assert!(matches!(&r.rows[0][2], Value::String(s) if s == "vehicle"));
    }

    #[tokio::test]
    async fn clean_when_no_disjoint_declared() {
        let e = create_executor();
        ok(&e, "CREATE CLASS person()").await;
        ok(&e, "INSERT VERTEX person() VALUES 1:()").await;
        let r = run(&e, "CHECK CONSISTENCY").await.unwrap();
        assert!(r.rows.is_empty());
        assert_eq!(r.columns, vec!["vid", "class_a", "class_b"]);
    }

    #[tokio::test]
    async fn create_class_disjoint_validation() {
        let e = create_executor();
        assert!(
            run(&e, "CREATE CLASS x() DISJOINT WITH nope")
                .await
                .is_err(),
            "DISJOINT WITH unknown class must error"
        );
        assert!(
            run(&e, "CREATE CLASS foo() DISJOINT WITH foo")
                .await
                .is_err(),
            "self DISJOINT WITH must error"
        );
    }
}
