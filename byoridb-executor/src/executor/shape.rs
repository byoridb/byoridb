// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Shape validation — SHACL-style property constraints (PLAN.md Studio-required
//! primitive, the constraint-hooks / shape-validation line item).
//!
//! A shape declares property constraints for the instances of a target class:
//!
//! ```text
//! CREATE SHAPE personShape ON person (
//!     email STRING REQUIRED,   -- datatype (+ presence)
//!     age   INT,               -- datatype
//!     age   CHECK age >= 0     -- value predicate
//! )
//! ```
//!
//! The shape is stored under `space:{space}:shape:{name}` as JSON. A vertex is
//! in scope when the shape's `target_class` is in the vertex's full ontology
//! class set (declared tags ∪ O-5 inferred types ∪ their O-3 ancestors), so
//! subclass instances are validated too (SHACL `targetClass` semantics).
//!
//! Two enforcement points share [`shape_violations`]:
//! - **write-time** ([`Executor::validate_write_shapes`]): INSERT/UPDATE VERTEX
//!   rejects a vertex that violates any in-scope shape. No shapes declared →
//!   the prefix scan is empty and the hook is a no-op (like O-6/O-9).
//! - **`CHECK SHAPE`** ([`Executor::execute_check_shape`]): a non-blocking sweep
//!   reporting every violating vertex (`vid / shape / property / constraint`).
//!
//! Property-graph note: a vertex property is single-valued, so SHACL
//! `minCount≥1` collapses to `Required` and `maxCount` is structural. Relation
//! (edge) cardinality is a separate, higher-cost track.

use super::{Executor, ExecutorResult};
use crate::error::{ExecutionError, Result};
use crate::evaluator::{EvalContext, Evaluator};
use crate::key::SchemaKey;
use byoridb_common::Value;
use byoridb_parser::ast::{DataType, ShapeConstraint, ShapeConstraintKind};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Persisted shape definition (`space:{space}:shape:{name}` → JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ShapeDef {
    pub name: String,
    pub target_class: String,
    pub constraints: Vec<ShapeConstraint>,
}

impl Executor {
    pub(super) async fn handle_create_shape(
        &self,
        name: String,
        if_not_exists: bool,
        target_class: String,
        constraints: Vec<ShapeConstraint>,
    ) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();

        let shape_key = SchemaKey::shape(&space, &name);
        if self.ctx.kvstore.get(&shape_key).await?.is_some() {
            if if_not_exists {
                return Ok(ExecutorResult::empty());
            }
            return Err(ExecutionError::InvalidOperation(format!(
                "Shape {} already exists",
                name
            )));
        }

        // The target must exist as a class or a plain tag — either can appear in
        // a vertex's class set, so both are valid shape targets.
        let is_class = self
            .ctx
            .kvstore
            .get(&SchemaKey::class(&space, &target_class))
            .await?
            .is_some();
        let is_tag = self
            .ctx
            .kvstore
            .get(&SchemaKey::tag(&space, &target_class))
            .await?
            .is_some();
        if !is_class && !is_tag {
            return Err(ExecutionError::InvalidOperation(format!(
                "Shape target {} does not exist as a class or tag",
                target_class
            )));
        }

        if constraints.is_empty() {
            return Err(ExecutionError::InvalidOperation(
                "Shape must declare at least one constraint".to_string(),
            ));
        }

        let def = ShapeDef {
            name: name.clone(),
            target_class,
            constraints,
        };
        self.ctx
            .kvstore
            .put(&shape_key, &serde_json::to_vec(&def)?)
            .await?;

        Ok(ExecutorResult::empty())
    }

    pub(super) async fn handle_drop_shape(
        &self,
        name: String,
        if_exists: bool,
    ) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();
        let shape_key = SchemaKey::shape(&space, &name);
        if self.ctx.kvstore.get(&shape_key).await?.is_none() {
            if if_exists {
                return Ok(ExecutorResult::empty());
            }
            return Err(ExecutionError::InvalidOperation(format!(
                "Shape {} not found",
                name
            )));
        }
        self.ctx.kvstore.delete(&shape_key).await?;
        Ok(ExecutorResult::empty())
    }

    /// Load every shape declared in the space.
    async fn load_shapes(&self, space: &str) -> Result<Vec<ShapeDef>> {
        let prefix = SchemaKey::shape_prefix(space);
        self.ctx
            .kvstore
            .scan_prefix(&prefix)
            .await?
            .into_iter()
            .map(|(k, v)| {
                serde_json::from_slice::<ShapeDef>(&v).map_err(|e| {
                    ExecutionError::InvalidOperation(format!(
                        "Corrupt shape metadata at {}: {}",
                        String::from_utf8_lossy(&k),
                        e
                    ))
                })
            })
            .collect()
    }

    /// Write-time enforcement: reject a vertex that violates any in-scope shape.
    /// `tag_names` are the vertex's declared tags; `props` is the flattened
    /// property map (bare + `{tag}.{prop}`). No-op when no shapes are declared.
    pub(super) async fn validate_write_shapes(
        &self,
        space: &str,
        tag_names: &[String],
        props: &HashMap<String, Value>,
    ) -> Result<()> {
        let shapes = self.load_shapes(space).await?;
        if shapes.is_empty() {
            return Ok(());
        }

        // Class set from declared tags + their ancestors. Inferred (vtype) types
        // are not yet materialized at INSERT VERTEX time; CHECK SHAPE catches
        // any violation that only surfaces through later inference.
        let mut class_set: HashSet<String> = HashSet::new();
        for tag in tag_names {
            class_set.insert(tag.clone());
            for anc in crate::ontology::class_ancestors_of(&self.ctx, space, tag).await? {
                class_set.insert(anc);
            }
        }

        for shape in &shapes {
            if !class_set.contains(&shape.target_class) {
                continue;
            }
            if let Some((prop, reason)) = shape_violations(shape, props).into_iter().next() {
                return Err(ExecutionError::InvalidOperation(format!(
                    "shape {} violated on property {}: {}",
                    shape.name, prop, reason
                )));
            }
        }
        Ok(())
    }

    /// `CHECK SHAPE` — scan the space and report every shape violation.
    /// Columns: `vid / shape / property / constraint`. Empty result = valid.
    pub(super) async fn execute_check_shape(&self) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();
        let columns = vec![
            "vid".to_string(),
            "shape".to_string(),
            "property".to_string(),
            "constraint".to_string(),
        ];

        let shapes = self.load_shapes(&space).await?;
        if shapes.is_empty() {
            return Ok(ExecutorResult {
                columns,
                rows: Vec::new(),
                latency_ms: 0,
            });
        }

        let mut rows = Vec::new();
        for (key, value) in self
            .ctx
            .kvstore
            .scan_prefix(&SchemaKey::vertex_prefix(&space))
            .await?
        {
            let Some(vid) = vid_from_vertex_key(&key) else {
                continue;
            };
            // Full ontology class set (declared ∪ inferred ∪ ancestors).
            let Some(class_set) = crate::ontology::vertex_class_set(&self.ctx, &space, vid).await?
            else {
                continue;
            };
            let in_scope: Vec<&ShapeDef> = shapes
                .iter()
                .filter(|s| class_set.contains(&s.target_class))
                .collect();
            if in_scope.is_empty() {
                continue;
            }

            let props = flatten_vertex_props(&value)?;
            for shape in in_scope {
                for (prop, reason) in shape_violations(shape, &props) {
                    rows.push(vec![
                        Value::Int(vid),
                        Value::String(shape.name.clone()),
                        Value::String(prop),
                        Value::String(reason),
                    ]);
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

/// Evaluate all of a shape's constraints against a vertex's flattened props.
/// Returns `(property, human-readable reason)` for each violation.
fn shape_violations(shape: &ShapeDef, props: &HashMap<String, Value>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for c in &shape.constraints {
        match &c.kind {
            ShapeConstraintKind::Required => {
                let present = props
                    .get(&c.property)
                    .is_some_and(|v| !matches!(v, Value::Null(_) | Value::Empty));
                if !present {
                    out.push((
                        c.property.clone(),
                        "required property is missing".to_string(),
                    ));
                }
            }
            ShapeConstraintKind::DataType(dt) => {
                // Only present, non-null values are type-checked (SHACL sh:datatype
                // constrains the value node; absence is a REQUIRED concern).
                if let Some(v) = props.get(&c.property) {
                    if !matches!(v, Value::Null(_) | Value::Empty) && !value_matches_type(v, dt) {
                        out.push((c.property.clone(), format!("expected type {:?}", dt)));
                    }
                }
            }
            ShapeConstraintKind::Predicate(expr) => {
                let ctx = EvalContext::new().with_current(props.clone());
                // A false result or an evaluation error (e.g. the property is
                // missing) both count as a violation.
                if !Evaluator::evaluate_condition(expr, &ctx).unwrap_or(false) {
                    out.push((
                        c.property.clone(),
                        "value predicate not satisfied".to_string(),
                    ));
                }
            }
        }
    }
    out
}

/// Whether a stored value is compatible with a declared datatype. Absent/null
/// values pass (that is a REQUIRED concern). Integers are accepted where a
/// float/double is declared (an integer literal fits a real-valued field);
/// complex values (list/map/…) are not constrained by scalar datatypes.
fn value_matches_type(value: &Value, dt: &DataType) -> bool {
    use DataType as D;
    match value {
        Value::Null(_) | Value::Empty => true,
        Value::Bool(_) => matches!(dt, D::Bool),
        Value::Int(_) => matches!(
            dt,
            D::Int8 | D::Int16 | D::Int32 | D::Int64 | D::Timestamp | D::Float | D::Double
        ),
        Value::Float(_) => matches!(dt, D::Float | D::Double),
        Value::String(_) => matches!(dt, D::String | D::FixedString(_)),
        Value::Date(_) => matches!(dt, D::Date),
        Value::Time(_) => matches!(dt, D::Time),
        Value::DateTime(_) => matches!(dt, D::DateTime | D::Timestamp),
        Value::Geography(_) => matches!(dt, D::Geography),
        _ => true,
    }
}

/// Decode a stored vertex blob into a flattened property map (bare +
/// `{tag}.{prop}`), matching the RECOMMEND filter convention.
fn flatten_vertex_props(data: &[u8]) -> Result<HashMap<String, Value>> {
    let vertex = byoridb_codec::VertexCodec::decode_vertex(data)
        .map_err(|e| ExecutionError::Io(std::io::Error::other(e.to_string())))?;
    let mut props = HashMap::new();
    for tag in vertex.tags {
        for (k, v) in tag.properties {
            props.insert(format!("{}.{}", tag.name, k), v.clone());
            props.insert(k, v);
        }
    }
    Ok(props)
}

/// Parse the trailing vid from a `{space}:vertex:{vid}` key.
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
    use crate::plan::ExecutionPlanBuilder;
    use byoridb_kvstore::store::MemoryKVStore;
    use std::sync::Arc;

    fn executor() -> Executor {
        Executor::new(Arc::new(
            ExecutionContext::new(Arc::new(MemoryKVStore::new())).with_space("default".to_string()),
        ))
    }

    async fn run(e: &Executor, q: &str) -> Result<ExecutorResult> {
        let stmt = byoridb_parser::parse(q).expect("parse");
        let plan = ExecutionPlanBuilder::build(stmt).expect("plan");
        e.execute(plan).await
    }

    async fn ok(e: &Executor, q: &str) {
        run(e, q)
            .await
            .unwrap_or_else(|err| panic!("query failed: {q}\n{err:?}"));
    }

    #[tokio::test]
    async fn required_constraint_rejects_missing_property_at_write_time() {
        let e = executor();
        ok(&e, "CREATE TAG person(email STRING, age INT)").await;
        ok(
            &e,
            "CREATE SHAPE personShape ON person (email STRING REQUIRED)",
        )
        .await;
        // Conformant insert passes.
        ok(
            &e,
            "INSERT VERTEX person(email, age) VALUES 1:(\"a@b.com\", 30)",
        )
        .await;
        // Missing the required `email` is rejected.
        let err = run(&e, "INSERT VERTEX person(age) VALUES 2:(30)")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("shape personShape violated")
                && err.to_string().contains("email"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn datatype_constraint_rejects_wrong_type() {
        let e = executor();
        // The tag stores `age` as a string; the shape demands INT.
        ok(&e, "CREATE TAG person(age STRING)").await;
        ok(&e, "CREATE SHAPE s ON person (age INT)").await;
        let err = run(&e, "INSERT VERTEX person(age) VALUES 1:(\"notanint\")")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("expected type"), "got: {err}");
    }

    #[tokio::test]
    async fn predicate_constraint_enforces_value_range() {
        let e = executor();
        ok(&e, "CREATE TAG person(age INT)").await;
        ok(&e, "CREATE SHAPE s ON person (age CHECK age >= 0)").await;
        ok(&e, "INSERT VERTEX person(age) VALUES 1:(30)").await;
        let err = run(&e, "INSERT VERTEX person(age) VALUES 2:(-1)")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("predicate"), "got: {err}");
    }

    #[tokio::test]
    async fn check_shape_reports_pre_existing_violations() {
        let e = executor();
        ok(&e, "CREATE TAG person(email STRING)").await;
        ok(&e, "INSERT VERTEX person(email) VALUES 1:(\"ok\")").await;
        // No shape yet, so this non-conformant vertex is admitted.
        ok(&e, "INSERT VERTEX person() VALUES 2:()").await;
        ok(&e, "CREATE SHAPE s ON person (email STRING REQUIRED)").await;

        let r = run(&e, "CHECK SHAPE").await.unwrap();
        assert_eq!(r.columns, vec!["vid", "shape", "property", "constraint"]);
        assert_eq!(r.rows.len(), 1, "only vertex 2 violates");
        assert!(matches!(r.rows[0][0], Value::Int(2)));
        assert!(matches!(&r.rows[0][2], Value::String(s) if s == "email"));
    }

    #[tokio::test]
    async fn shape_applies_to_subclass_instances() {
        // Shape targets the parent class; a subclass vertex is in scope through
        // its ancestor set (SHACL targetClass semantics).
        let e = executor();
        ok(&e, "CREATE CLASS animal(name STRING)").await;
        ok(&e, "CREATE CLASS dog(name STRING) SUBCLASS OF animal").await;
        ok(&e, "CREATE SHAPE s ON animal (name STRING REQUIRED)").await;
        ok(&e, "INSERT VERTEX dog(name) VALUES 1:(\"rex\")").await;
        let err = run(&e, "INSERT VERTEX dog() VALUES 2:()")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("shape s violated"), "got: {err}");
    }

    #[tokio::test]
    async fn update_rejects_transition_to_nonconformant() {
        let e = executor();
        ok(&e, "CREATE TAG person(age INT)").await;
        ok(&e, "CREATE SHAPE s ON person (age CHECK age >= 0)").await;
        ok(&e, "INSERT VERTEX person(age) VALUES 1:(5)").await;
        // A partial UPDATE that would make the vertex non-conformant is rejected.
        let err = run(&e, "UPDATE VERTEX ON person 1 SET age = -3")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("shape s violated"), "got: {err}");
    }

    #[tokio::test]
    async fn no_shapes_declared_is_a_noop() {
        // With no shapes in the space the write-time hook must not block inserts.
        let e = executor();
        ok(&e, "CREATE TAG person(email STRING)").await;
        ok(&e, "INSERT VERTEX person() VALUES 1:()").await;
        let r = run(&e, "CHECK SHAPE").await.unwrap();
        assert!(r.rows.is_empty());
    }

    #[tokio::test]
    async fn create_shape_validation_and_lifecycle() {
        let e = executor();
        // Target must exist.
        assert!(
            run(&e, "CREATE SHAPE s ON nope (x STRING)").await.is_err(),
            "unknown target must error"
        );
        ok(&e, "CREATE TAG person(email STRING)").await;
        ok(&e, "CREATE SHAPE s ON person (email STRING REQUIRED)").await;
        // Duplicate errors unless IF NOT EXISTS.
        assert!(run(&e, "CREATE SHAPE s ON person (email STRING)")
            .await
            .is_err());
        ok(&e, "CREATE SHAPE IF NOT EXISTS s ON person (email STRING)").await;
        // DROP + IF EXISTS.
        ok(&e, "DROP SHAPE s").await;
        assert!(run(&e, "DROP SHAPE s").await.is_err());
        ok(&e, "DROP SHAPE IF EXISTS s").await;
    }

    #[tokio::test]
    async fn drop_space_removes_shape_metadata() {
        let e = Executor::new(Arc::new(
            ExecutionContext::new(Arc::new(MemoryKVStore::new())).with_space("s1".to_string()),
        ));
        ok(&e, "CREATE SPACE s1").await;
        ok(&e, "CREATE TAG person(email STRING)").await;
        ok(&e, "CREATE SHAPE sh ON person (email STRING REQUIRED)").await;
        ok(&e, "DROP SPACE s1").await;
        let leftover = e
            .ctx
            .kvstore
            .scan_prefix(&SchemaKey::shape_prefix("s1"))
            .await
            .unwrap();
        assert!(
            leftover.is_empty(),
            "shape metadata must die with the space"
        );
    }
}
