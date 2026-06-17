// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Ontology class DDL (PLAN.md O-3 TBox).
//!
//! A class is a tag superset: `CREATE CLASS` writes the ordinary tag schema
//! under `space:{space}:tag:{name}` — so INSERT VERTEX, MATCH labels,
//! tag-vid indexes and schema validation apply unchanged — plus a hierarchy
//! record under `space:{space}:class:{name}`:
//! `{"name": ..., "superclasses": [...]}`. Plain tags are classes without
//! hierarchy and never participate in `SUBCLASS OF`.
//!
//! Standalone-first: like `handle_create_tag`, schema is written directly to
//! the kvstore; distributed Meta-service integration follows G-2.

use super::{Executor, ExecutorResult};
use crate::error::{ExecutionError, Result};
use crate::key::SchemaKey;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ClassDef {
    pub name: String,
    pub superclasses: Vec<String>,
}

impl Executor {
    pub(super) async fn handle_create_class(
        &self,
        name: String,
        if_not_exists: bool,
        props: Vec<crate::plan::PropertyDef>,
        superclasses: Vec<String>,
    ) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();

        let class_key = SchemaKey::class(&space, &name);
        if self.ctx.kvstore.get(&class_key).await?.is_some() {
            if if_not_exists {
                return Ok(ExecutorResult::empty());
            }
            return Err(ExecutionError::InvalidOperation(format!(
                "Class {} already exists",
                name
            )));
        }
        // A plain tag with the same name blocks class creation — the class
        // would silently inherit foreign fields otherwise.
        let tag_key = SchemaKey::tag(&space, &name);
        if self.ctx.kvstore.get(&tag_key).await?.is_some() {
            return Err(ExecutionError::InvalidOperation(format!(
                "Tag {} already exists; classes and tags share one namespace",
                name
            )));
        }

        // Dedup parents, preserving declaration order.
        let mut parents: Vec<String> = Vec::new();
        for parent in superclasses {
            if !parents.contains(&parent) {
                parents.push(parent);
            }
        }

        for parent in &parents {
            if *parent == name {
                return Err(ExecutionError::InvalidOperation(format!(
                    "Class {} cannot be a subclass of itself",
                    name
                )));
            }
            // Parents must be *classes*; a tag-only name is rejected.
            if self
                .ctx
                .kvstore
                .get(&SchemaKey::class(&space, parent))
                .await?
                .is_none()
            {
                return Err(ExecutionError::InvalidOperation(format!(
                    "Superclass {} does not exist as a class",
                    parent
                )));
            }
            // Hierarchy is append-only today (no ALTER CLASS), so existing
            // ancestors cannot include the not-yet-created class — but walk
            // anyway: it enforces the depth cap and detects corrupt cycles.
            let ancestors = self.class_ancestors(&space, parent).await?;
            if ancestors.contains(&name) {
                return Err(ExecutionError::InvalidOperation(format!(
                    "SUBCLASS OF {} would create a cycle through {}",
                    parent, name
                )));
            }
        }

        let tag_data = serde_json::json!({
            "name": name,
            "properties": props,
        });
        let class_def = ClassDef {
            name: name.clone(),
            superclasses: parents,
        };

        // One transaction: the tag schema and the class record must not be
        // observable separately (a tag without its hierarchy record would
        // pass DDL re-runs as a "plain tag" and block re-creation).
        self.ctx
            .kvstore
            .batch_put(vec![
                (tag_key, serde_json::to_vec(&tag_data)?),
                (class_key, serde_json::to_vec(&class_def)?),
            ])
            .await?;

        Ok(ExecutorResult::empty())
    }

    pub(super) async fn handle_drop_class(
        &self,
        name: String,
        if_exists: bool,
    ) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();

        let class_key = SchemaKey::class(&space, &name);
        if self.ctx.kvstore.get(&class_key).await?.is_none() {
            if if_exists {
                return Ok(ExecutorResult::empty());
            }
            return Err(ExecutionError::InvalidOperation(format!(
                "Class {} not found",
                name
            )));
        }

        // RESTRICT: refuse while subclasses point at this class.
        let children: Vec<String> = self
            .list_classes(&space)
            .await?
            .into_iter()
            .filter(|def| def.superclasses.contains(&name))
            .map(|def| def.name)
            .collect();
        if !children.is_empty() {
            return Err(ExecutionError::InvalidOperation(format!(
                "Class {} has subclasses ({}); drop them first",
                name,
                children.join(", ")
            )));
        }

        self.ctx
            .kvstore
            .batch_delete(vec![class_key, SchemaKey::tag(&space, &name)])
            .await?;

        Ok(ExecutorResult::empty())
    }

    /// `SHOW CLASSES` — one row per class: Name, Superclasses (comma-joined).
    pub(super) async fn execute_show_classes(&self) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();
        let mut defs = self.list_classes(&space).await?;
        defs.sort_by(|a, b| a.name.cmp(&b.name));

        let rows = defs
            .into_iter()
            .map(|def| {
                vec![
                    byoridb_common::Value::String(def.name),
                    byoridb_common::Value::String(def.superclasses.join(", ")),
                ]
            })
            .collect();

        Ok(ExecutorResult {
            columns: vec!["Name".to_string(), "Superclasses".to_string()],
            rows,
            latency_ms: 0,
        })
    }

    /// `DESCRIBE CLASS <name>` — the tag's field rows (Field/Type/Null/
    /// Default) followed by synthetic `(superclass)` / `(ancestor)` rows so
    /// the hierarchy is visible in the same table.
    pub(super) async fn execute_describe_class(&self, name: &str) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();

        let def = self
            .load_class(&space, name)
            .await?
            .ok_or_else(|| ExecutionError::InvalidOperation(format!("Class {} not found", name)))?;

        let mut result = self.execute_describe_tag(name).await?;

        let direct: std::collections::HashSet<&String> = def.superclasses.iter().collect();
        let ancestors = self.class_ancestors(&space, name).await?;
        for parent in &def.superclasses {
            result.rows.push(vec![
                byoridb_common::Value::String("(superclass)".to_string()),
                byoridb_common::Value::String(parent.clone()),
                byoridb_common::Value::String(String::new()),
                byoridb_common::Value::Null(byoridb_common::NullType::Null),
            ]);
        }
        for ancestor in ancestors.iter().filter(|a| !direct.contains(a)) {
            result.rows.push(vec![
                byoridb_common::Value::String("(ancestor)".to_string()),
                byoridb_common::Value::String(ancestor.clone()),
                byoridb_common::Value::String(String::new()),
                byoridb_common::Value::Null(byoridb_common::NullType::Null),
            ]);
        }

        Ok(result)
    }

    /// All transitive superclasses of `name` (BFS, deduped, excludes `name`).
    /// Delegates to the shared [`crate::ontology`] helper so RECOMMEND, DESCRIBE
    /// CLASS and the MATCH `is_a` filter share one implementation.
    pub(super) async fn class_ancestors(&self, space: &str, name: &str) -> Result<Vec<String>> {
        crate::ontology::class_ancestors_of(&self.ctx, space, name).await
    }

    pub(super) async fn load_class(&self, space: &str, name: &str) -> Result<Option<ClassDef>> {
        let Some(bytes) = self.ctx.kvstore.get(&SchemaKey::class(space, name)).await? else {
            return Ok(None);
        };
        let def = serde_json::from_slice(&bytes).map_err(|e| {
            ExecutionError::InvalidOperation(format!("Corrupt class metadata for {}: {}", name, e))
        })?;
        Ok(Some(def))
    }

    async fn list_classes(&self, space: &str) -> Result<Vec<ClassDef>> {
        let prefix = SchemaKey::class_prefix(space);
        // Corrupt metadata is an error, not a silent skip — the DROP CLASS
        // RESTRICT check relies on this list seeing every child.
        self.ctx
            .kvstore
            .scan_prefix(&prefix)
            .await?
            .into_iter()
            .map(|(k, v)| {
                serde_json::from_slice::<ClassDef>(&v).map_err(|e| {
                    ExecutionError::InvalidOperation(format!(
                        "Corrupt class metadata at {}: {}",
                        String::from_utf8_lossy(&k),
                        e
                    ))
                })
            })
            .collect()
    }
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

    async fn run(exec: &Executor, query: &str) -> Result<ExecutorResult> {
        let stmt = byoridb_parser::parse(query).unwrap();
        let plan = ExecutionPlanBuilder::build(stmt).unwrap();
        exec.execute(plan).await
    }

    fn string_cell(result: &ExecutorResult, row: usize, col: usize) -> &str {
        match &result.rows[row][col] {
            byoridb_common::Value::String(s) => s,
            other => panic!("expected string cell, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn create_class_and_show_classes() {
        let exec = executor();
        run(&exec, "CREATE CLASS animal(name STRING)")
            .await
            .unwrap();
        run(&exec, "CREATE CLASS dog(breed STRING) SUBCLASS OF animal")
            .await
            .unwrap();

        let res = run(&exec, "SHOW CLASSES").await.unwrap();
        assert_eq!(res.columns, vec!["Name", "Superclasses"]);
        assert_eq!(res.rows.len(), 2);
        assert_eq!(string_cell(&res, 0, 0), "animal");
        assert_eq!(string_cell(&res, 0, 1), "");
        assert_eq!(string_cell(&res, 1, 0), "dog");
        assert_eq!(string_cell(&res, 1, 1), "animal");
    }

    #[tokio::test]
    async fn duplicate_create_errors_unless_if_not_exists() {
        let exec = executor();
        run(&exec, "CREATE CLASS animal(name STRING)")
            .await
            .unwrap();

        let err = run(&exec, "CREATE CLASS animal(name STRING)")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already exists"));

        run(&exec, "CREATE CLASS IF NOT EXISTS animal(name STRING)")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn describe_class_lists_superclasses_and_ancestors() {
        let exec = executor();
        run(&exec, "CREATE CLASS living(name STRING)")
            .await
            .unwrap();
        run(&exec, "CREATE CLASS animal(legs INT64) SUBCLASS OF living")
            .await
            .unwrap();
        run(&exec, "CREATE CLASS dog(breed STRING) SUBCLASS OF animal")
            .await
            .unwrap();

        let res = run(&exec, "DESCRIBE CLASS dog").await.unwrap();
        let pairs: Vec<(String, String)> = res
            .rows
            .iter()
            .map(|r| {
                (
                    format!("{:?}", r[0])
                        .replace("String(\"", "")
                        .replace("\")", ""),
                    format!("{:?}", r[1])
                        .replace("String(\"", "")
                        .replace("\")", ""),
                )
            })
            .collect();
        assert!(pairs.contains(&("breed".to_string(), "STRING".to_string())));
        assert!(pairs.contains(&("(superclass)".to_string(), "animal".to_string())));
        assert!(pairs.contains(&("(ancestor)".to_string(), "living".to_string())));
    }

    #[tokio::test]
    async fn multi_parent_superclasses_are_recorded() {
        let exec = executor();
        run(&exec, "CREATE CLASS animal(name STRING)")
            .await
            .unwrap();
        run(&exec, "CREATE CLASS pet(owner STRING)").await.unwrap();
        run(
            &exec,
            "CREATE CLASS dog(breed STRING) SUBCLASS OF animal, pet",
        )
        .await
        .unwrap();

        let res = run(&exec, "SHOW CLASSES").await.unwrap();
        let dog_row = res
            .rows
            .iter()
            .find(|r| matches!(&r[0], byoridb_common::Value::String(s) if s == "dog"))
            .unwrap();
        assert_eq!(
            dog_row[1],
            byoridb_common::Value::String("animal, pet".to_string())
        );
    }

    #[tokio::test]
    async fn self_subclass_is_rejected() {
        let exec = executor();
        let err = run(&exec, "CREATE CLASS dog(breed STRING) SUBCLASS OF dog")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("subclass of itself"));
    }

    #[tokio::test]
    async fn parent_must_exist_as_class_not_tag() {
        let exec = executor();
        let err = run(&exec, "CREATE CLASS dog(breed STRING) SUBCLASS OF animal")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not exist as a class"));

        run(&exec, "CREATE TAG animal(name STRING)").await.unwrap();
        let err = run(&exec, "CREATE CLASS cat(name STRING) SUBCLASS OF animal")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not exist as a class"));
    }

    #[tokio::test]
    async fn drop_class_restricts_while_children_exist() {
        let exec = executor();
        run(&exec, "CREATE CLASS animal(name STRING)")
            .await
            .unwrap();
        run(&exec, "CREATE CLASS dog(breed STRING) SUBCLASS OF animal")
            .await
            .unwrap();

        let err = run(&exec, "DROP CLASS animal").await.unwrap_err();
        assert!(err.to_string().contains("has subclasses"));

        run(&exec, "DROP CLASS dog").await.unwrap();
        run(&exec, "DROP CLASS animal").await.unwrap();
        let res = run(&exec, "SHOW CLASSES").await.unwrap();
        assert!(res.rows.is_empty());

        // IF EXISTS swallows the missing-class case; bare DROP errors.
        run(&exec, "DROP CLASS IF EXISTS animal").await.unwrap();
        assert!(run(&exec, "DROP CLASS animal").await.is_err());
    }

    #[tokio::test]
    async fn drop_tag_on_class_is_rejected() {
        let exec = executor();
        run(&exec, "CREATE CLASS animal(name STRING)")
            .await
            .unwrap();

        let err = run(&exec, "DROP TAG animal").await.unwrap_err();
        assert!(err.to_string().contains("use DROP CLASS"));
    }

    #[tokio::test]
    async fn tag_name_collision_blocks_class_creation() {
        let exec = executor();
        run(&exec, "CREATE TAG dog(breed STRING)").await.unwrap();

        let err = run(&exec, "CREATE CLASS dog(breed STRING)")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Tag dog already exists"));
    }

    #[tokio::test]
    async fn class_works_as_tag_for_insert_and_match() {
        let exec = executor();
        run(&exec, "CREATE CLASS dog(breed STRING)").await.unwrap();
        run(&exec, "INSERT VERTEX dog(breed) VALUES 7:(\"corgi\")")
            .await
            .unwrap();

        let res = run(&exec, "MATCH (x:dog) RETURN id(x) AS vid")
            .await
            .unwrap();
        assert_eq!(res.rows, vec![vec![byoridb_common::Value::Int(7)]]);
    }

    #[tokio::test]
    async fn drop_space_removes_class_metadata() {
        // Class keys live under the `space:{name}:` schema range that DROP
        // SPACE deletes; pin that with a regression test.
        let exec = Executor::new(Arc::new(
            ExecutionContext::new(Arc::new(MemoryKVStore::new())).with_space("s1".to_string()),
        ));
        run(&exec, "CREATE SPACE s1").await.unwrap();
        run(&exec, "CREATE CLASS animal(name STRING)")
            .await
            .unwrap();
        run(&exec, "DROP SPACE s1").await.unwrap();

        let leftover = exec
            .ctx
            .kvstore
            .scan_prefix(&SchemaKey::class_prefix("s1"))
            .await
            .unwrap();
        assert!(
            leftover.is_empty(),
            "class metadata must die with the space"
        );
    }
}
