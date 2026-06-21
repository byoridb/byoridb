// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use super::{Executor, ExecutorResult};
use crate::error::{ExecutionError, Result};
use crate::key::SchemaKey;
use byoridb_codec::VertexCodec;
#[cfg(feature = "distributed")]
use byoridb_meta::schema::{
    AlterOperation as MetaAlterOperation, DataType as MetaDataType, Field as MetaField,
};
#[cfg(feature = "distributed")]
use byoridb_parser::ast::{Expression, Literal};

impl Executor {
    /// Execute USE statement
    pub(super) async fn execute_use(&self, plan: crate::plan::UsePlan) -> Result<ExecutorResult> {
        // Validate space exists
        let space_key = SchemaKey::space(&plan.space);
        let exists = self.ctx.kvstore.get(&space_key).await?.is_some();

        if !exists {
            return Err(ExecutionError::SpaceNotFound(plan.space));
        }

        Ok(ExecutorResult {
            columns: vec![],
            rows: vec![],
            latency_ms: 0,
        })
    }

    /// Execute CREATE statement
    pub(super) async fn execute_create(
        &self,
        plan: crate::plan::CreatePlan,
    ) -> Result<ExecutorResult> {
        match plan {
            crate::plan::CreatePlan::Space {
                name,
                if_not_exists,
                partition_num,
                replica_factor,
                vid_type,
                partition_strategy,
            } => {
                self.handle_create_space(
                    name,
                    if_not_exists,
                    partition_num,
                    replica_factor,
                    vid_type,
                    partition_strategy,
                )
                .await
            }
            crate::plan::CreatePlan::Tag {
                name,
                if_not_exists,
                props,
            } => self.handle_create_tag(name, if_not_exists, props).await,
            crate::plan::CreatePlan::Edge {
                name,
                if_not_exists,
                props,
                semantics,
            } => {
                self.handle_create_edge(name, if_not_exists, props, semantics)
                    .await
            }
            crate::plan::CreatePlan::Class {
                name,
                if_not_exists,
                props,
                superclasses,
                disjoint,
            } => {
                self.handle_create_class(name, if_not_exists, props, superclasses, disjoint)
                    .await
            }
            crate::plan::CreatePlan::User {
                name,
                if_not_exists,
                password,
                role,
            } => {
                self.handle_create_user(name, if_not_exists, password, role)
                    .await
            }
            crate::plan::CreatePlan::TagIndex {
                name,
                tag_name,
                props,
            } => self.handle_create_tag_index(name, tag_name, props).await,
            crate::plan::CreatePlan::EdgeIndex {
                name,
                edge_name,
                props,
            } => self.handle_create_edge_index(name, edge_name, props).await,
        }
    }

    /// Handle CREATE TAG INDEX
    ///
    /// Index metadata is managed exclusively by the Meta service. When no
    /// MetaClient is configured (e.g. pure embedded mode without a meta
    /// server), the operation returns a clear error rather than silently
    /// persisting into kvstore, because the full index lifecycle
    /// (`IndexManager`, partition-local index KV layout) requires the
    /// Meta and Storage services.
    pub(super) async fn handle_create_tag_index(
        &self,
        name: String,
        tag_name: String,
        props: Vec<String>,
    ) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();
        #[cfg(feature = "distributed")]
        if let Some(client) = self.ctx.meta_client.as_ref() {
            let space_info = client
                .get_space(&space)
                .await
                .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?;

            client
                .create_tag_index(space_info.id, &name, &tag_name, props)
                .await
                .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?;
            return Ok(ExecutorResult::empty());
        }

        let index_manager = self.ctx.index_manager.as_ref().ok_or_else(|| {
            ExecutionError::InvalidOperation("CREATE TAG INDEX requires IndexManager".to_string())
        })?;
        let space_id = self.ctx.space_id.unwrap_or(1);
        let field_indices: Vec<usize> = (0..props.len()).collect();
        let index_id = index_manager
            .create_tag_index(
                space_id,
                name,
                0,
                tag_name.clone(),
                props.clone(),
                field_indices,
            )
            .await
            .map_err(|e| ExecutionError::InvalidOperation(e.to_string()))?;
        self.backfill_tag_index(&space, index_id, &tag_name, &props)
            .await?;

        Ok(ExecutorResult::empty())
    }

    /// Handle CREATE EDGE INDEX
    ///
    /// See [`handle_create_tag_index`] for why MetaClient is required.
    pub(super) async fn handle_create_edge_index(
        &self,
        name: String,
        edge_name: String,
        props: Vec<String>,
    ) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();
        #[cfg(feature = "distributed")]
        if let Some(client) = self.ctx.meta_client.as_ref() {
            let space_info = client
                .get_space(&space)
                .await
                .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?;

            client
                .create_edge_index(space_info.id, &name, &edge_name, props)
                .await
                .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?;
            return Ok(ExecutorResult::empty());
        }

        let index_manager = self.ctx.index_manager.as_ref().ok_or_else(|| {
            ExecutionError::InvalidOperation("CREATE EDGE INDEX requires IndexManager".to_string())
        })?;
        let space_id = self.ctx.space_id.unwrap_or(1);
        let field_indices: Vec<usize> = (0..props.len()).collect();
        let index_id = index_manager
            .create_edge_index(
                space_id,
                name,
                0,
                edge_name.clone(),
                props.clone(),
                field_indices,
            )
            .await
            .map_err(|e| ExecutionError::InvalidOperation(e.to_string()))?;
        self.backfill_edge_index(&space, index_id, &edge_name, &props)
            .await?;

        Ok(ExecutorResult::empty())
    }

    /// Delete every key under `prefix` in chunked batches (one redb commit per
    /// chunk so a large purge doesn't fsync per key). Returns keys removed.
    async fn delete_by_prefix(&self, prefix: &[u8]) -> Result<usize> {
        let entries = self.ctx.kvstore.scan_prefix(prefix).await?;
        let n = entries.len();
        let keys: Vec<Vec<u8>> = entries.into_iter().map(|(k, _)| k).collect();
        for chunk in keys.chunks(4096) {
            self.ctx.kvstore.batch_delete(chunk.to_vec()).await?;
        }
        Ok(n)
    }

    async fn backfill_tag_index(
        &self,
        space: &str,
        index_id: u32,
        tag_name: &str,
        props: &[String],
    ) -> Result<()> {
        if self.ctx.index_manager.is_none() {
            return Ok(());
        }
        let prefix = format!("{}:vertex:", space);
        let rows = self
            .ctx
            .kvstore
            .scan_prefix_limited(prefix.as_bytes(), None)
            .await?;
        // Backfill in chunked batches: one redb commit (one fsync) per chunk
        // instead of one per index entry. Without this, backfilling a large tag
        // (e.g. LDBC post, ~hundreds of thousands of rows) issues a fsync per
        // row and times out.
        const CHUNK: usize = 4096;
        let mut batch: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (key, value) in rows {
            let key_str = String::from_utf8_lossy(&key);
            let vid = key_str
                .rsplit(':')
                .next()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            if vid == 0 {
                continue;
            }
            if let Ok(vertex) = VertexCodec::decode_vertex(&value) {
                for tag in vertex.tags.iter().filter(|tag| tag.name == tag_name) {
                    let values = props
                        .iter()
                        .map(|prop| {
                            self.byoridb_value_to_index_value(
                                tag.properties
                                    .get(prop)
                                    .unwrap_or(&byoridb_common::Value::null()),
                            )
                        })
                        .collect::<Vec<_>>();
                    let idx_key =
                        byoridb_storage::KeyUtils::tag_index_key(1, index_id, &values, vid);
                    batch.push((idx_key, Vec::new()));
                    if batch.len() >= CHUNK {
                        self.ctx
                            .kvstore
                            .batch_put(std::mem::take(&mut batch))
                            .await?;
                    }
                }
            }
        }
        if !batch.is_empty() {
            self.ctx.kvstore.batch_put(batch).await?;
        }
        Ok(())
    }

    async fn backfill_edge_index(
        &self,
        space: &str,
        index_id: u32,
        edge_name: &str,
        props: &[String],
    ) -> Result<()> {
        if self.ctx.index_manager.is_none() {
            return Ok(());
        }
        let prefix = format!("{}:edge:", space);
        let rows = self
            .ctx
            .kvstore
            .scan_prefix_limited(prefix.as_bytes(), None)
            .await?;
        // Chunked batches — one fsync per chunk (see backfill_tag_index).
        const CHUNK: usize = 4096;
        let mut batch: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (_, value) in rows {
            let Ok(edge) = VertexCodec::decode_edge(&value) else {
                continue;
            };
            if edge.edge_type != edge_name {
                continue;
            }
            let values = props
                .iter()
                .map(|prop| {
                    self.byoridb_value_to_index_value(
                        edge.properties
                            .get(prop)
                            .unwrap_or(&byoridb_common::Value::null()),
                    )
                })
                .collect::<Vec<_>>();
            let idx_key = byoridb_storage::KeyUtils::edge_index_key(
                1,
                index_id,
                &values,
                edge.src_vid,
                edge.ranking,
                edge.dst_vid,
            );
            batch.push((idx_key, Vec::new()));
            if batch.len() >= CHUNK {
                self.ctx
                    .kvstore
                    .batch_put(std::mem::take(&mut batch))
                    .await?;
            }
        }
        if !batch.is_empty() {
            self.ctx.kvstore.batch_put(batch).await?;
        }
        Ok(())
    }

    /// Handle CREATE SPACE
    pub(super) async fn handle_create_space(
        &self,
        name: String,
        if_not_exists: bool,
        partition_num: u32,
        replica_factor: u32,
        vid_type: String,
        partition_strategy: byoridb_common::PartitionStrategy,
    ) -> Result<ExecutorResult> {
        let space_key = SchemaKey::space(&name);

        if self.ctx.kvstore.get(&space_key).await?.is_some() {
            if if_not_exists {
                return Ok(ExecutorResult::empty());
            }
            return Err(ExecutionError::InvalidOperation(format!(
                "Space {} already exists",
                name
            )));
        }

        let id = self.allocate_space_id().await?;
        let space_data = serde_json::json!({
            "id": id,
            "name": name,
            "partition_num": partition_num,
            "replica_factor": replica_factor,
            "vid_type": vid_type,
            "partition_strategy": partition_strategy,
        });

        self.ctx
            .kvstore
            .put(&space_key, serde_json::to_vec(&space_data)?.as_slice())
            .await?;

        Ok(ExecutorResult::empty())
    }

    /// Allocate a new space ID using a persistent counter in the kvstore.
    pub(super) async fn allocate_space_id(&self) -> Result<u32> {
        let key = SchemaKey::next_space_id_key();
        let id = match self.ctx.kvstore.get(&key).await? {
            Some(bytes) => std::str::from_utf8(&bytes)
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(1),
            None => 1,
        };
        self.ctx
            .kvstore
            .put(&key, (id + 1).to_string().as_bytes())
            .await?;
        Ok(id)
    }

    /// Handle CREATE TAG
    pub(super) async fn handle_create_tag(
        &self,
        name: String,
        if_not_exists: bool,
        props: Vec<crate::plan::PropertyDef>,
    ) -> Result<ExecutorResult> {
        let space = self.require_space()?;
        let tag_key = SchemaKey::tag(space, &name);

        if self.ctx.kvstore.get(&tag_key).await?.is_some() {
            if if_not_exists {
                return Ok(ExecutorResult::empty());
            }
            return Err(ExecutionError::InvalidOperation(format!(
                "Tag {} already exists",
                name
            )));
        }

        let tag_data = serde_json::json!({
            "name": name,
            "properties": props,
        });

        self.ctx
            .kvstore
            .put(&tag_key, serde_json::to_vec(&tag_data)?.as_slice())
            .await?;

        Ok(ExecutorResult::empty())
    }

    /// Handle CREATE EDGE
    pub(super) async fn handle_create_edge(
        &self,
        name: String,
        if_not_exists: bool,
        props: Vec<crate::plan::PropertyDef>,
        semantics: byoridb_parser::ast::SemanticFlags,
    ) -> Result<ExecutorResult> {
        let space = self.require_space()?.to_string();
        let edge_key = SchemaKey::edge(&space, &name);

        if self.ctx.kvstore.get(&edge_key).await?.is_some() {
            if if_not_exists {
                return Ok(ExecutorResult::empty());
            }
            return Err(ExecutionError::InvalidOperation(format!(
                "Edge {} already exists",
                name
            )));
        }

        // O-4: validate referenced edge types exist (INVERSE OF / SUBPROPERTY OF).
        // The target must be an already-declared edge so materialization (O-5)
        // resolves it; self-reference is rejected as meaningless.
        for (clause, target) in [
            ("INVERSE OF", &semantics.inverse_of),
            ("SUBPROPERTY OF", &semantics.subproperty_of),
        ] {
            if let Some(target) = target {
                if target == &name {
                    return Err(ExecutionError::InvalidOperation(format!(
                        "Edge {} cannot be {} itself",
                        name, clause
                    )));
                }
                if self
                    .ctx
                    .kvstore
                    .get(&SchemaKey::edge(&space, target))
                    .await?
                    .is_none()
                {
                    return Err(ExecutionError::InvalidOperation(format!(
                        "{} target edge type '{}' does not exist",
                        clause, target
                    )));
                }
            }
        }

        // DOMAIN / RANGE reference a *class* (vertex type) — validate it exists
        // as a tag or class so type inference (O-5) resolves it.
        for (clause, target) in [("DOMAIN", &semantics.domain), ("RANGE", &semantics.range)] {
            if let Some(target) = target {
                let is_class = self
                    .ctx
                    .kvstore
                    .get(&SchemaKey::class(&space, target))
                    .await?
                    .is_some();
                let is_tag = self
                    .ctx
                    .kvstore
                    .get(&SchemaKey::tag(&space, target))
                    .await?
                    .is_some();
                if !is_class && !is_tag {
                    return Err(ExecutionError::InvalidOperation(format!(
                        "{} target class '{}' does not exist",
                        clause, target
                    )));
                }
            }
        }

        let edge_data = serde_json::json!({
            "name": name,
            "properties": props,
            "semantics": semantics,
        });

        self.ctx
            .kvstore
            .put(&edge_key, serde_json::to_vec(&edge_data)?.as_slice())
            .await?;

        Ok(ExecutorResult::empty())
    }

    /// Helper to require a selected space
    pub(super) fn require_space(&self) -> Result<&str> {
        self.ctx
            .space
            .as_deref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))
    }

    /// Execute ALTER statement
    pub(super) async fn execute_alter(
        &self,
        plan: crate::plan::AlterPlan,
    ) -> Result<ExecutorResult> {
        // Handle User operations first (no space required)
        if let crate::plan::AlterPlan::User { name, new_password } = plan {
            return self.execute_alter_user(&name, new_password).await;
        }

        let space = self.require_space()?;

        // If MetaClient is available, delegate to MetaService
        #[cfg(feature = "distributed")]
        if let Some(client) = &self.ctx.meta_client {
            return self.execute_alter_with_meta(client, space, plan).await;
        }

        // Fallback: Local execution (Test Mode / Standalone)
        self.execute_alter_local(space, plan).await
    }

    /// Execute ALTER via MetaService RPC
    #[cfg(feature = "distributed")]
    pub(super) async fn execute_alter_with_meta(
        &self,
        client: &byoridb_meta::MetaClient,
        space: &str,
        plan: crate::plan::AlterPlan,
    ) -> Result<ExecutorResult> {
        // Handle User operations first (no space lookup needed)
        if let crate::plan::AlterPlan::User { name, new_password } = plan {
            return self.execute_alter_user(&name, new_password).await;
        }

        let space_info = client
            .get_space(space)
            .await
            .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?;
        let space_id = space_info.id;

        let (schema_type, name, operations) = match plan {
            crate::plan::AlterPlan::Tag { name, operations } => ("Tag", name, operations),
            crate::plan::AlterPlan::Edge { name, operations } => ("Edge", name, operations),
            crate::plan::AlterPlan::User { .. } => unreachable!(), // Handled above
        };

        let meta_ops = self.convert_to_meta_operations(operations)?;

        match schema_type {
            "Tag" => client.alter_tag(space_id, &name, meta_ops).await,
            "Edge" => client.alter_edge(space_id, &name, meta_ops).await,
            _ => unreachable!(),
        }
        .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?;

        Ok(ExecutorResult::success_message(format!(
            "{} {} altered successfully",
            schema_type, name
        )))
    }

    /// Convert plan operations to meta operations
    #[cfg(feature = "distributed")]
    pub(super) fn convert_to_meta_operations(
        &self,
        operations: Vec<crate::plan::AlterColumnOp>,
    ) -> Result<Vec<MetaAlterOperation>> {
        let mut meta_ops = Vec::new();
        for op in operations {
            match op.op_type {
                crate::plan::AlterOpType::AddColumn => {
                    validate_add_column(&op.prop)?;

                    let meta_type = convert_ast_type_to_meta(op.prop.data_type);
                    let default_val = op
                        .prop
                        .default_value
                        .map(|e| convert_expr_to_string(&e))
                        .transpose()?;

                    meta_ops.push(MetaAlterOperation::AddColumn(MetaField {
                        name: op.prop.name,
                        data_type: meta_type,
                        nullable: op.prop.nullable,
                        default: default_val,
                    }));
                }
                crate::plan::AlterOpType::DropColumn => {
                    meta_ops.push(MetaAlterOperation::DropColumn(op.prop.name));
                }
                crate::plan::AlterOpType::ChangeColumn => {
                    let meta_type = convert_ast_type_to_meta(op.prop.data_type);
                    let default_val = op
                        .prop
                        .default_value
                        .map(|e| convert_expr_to_string(&e))
                        .transpose()?;

                    meta_ops.push(MetaAlterOperation::ChangeColumn(MetaField {
                        name: op.prop.name,
                        data_type: meta_type,
                        nullable: op.prop.nullable,
                        default: default_val,
                    }));
                }
            }
        }
        Ok(meta_ops)
    }

    /// Execute ALTER locally (Test Mode / Standalone)
    pub(super) async fn execute_alter_local(
        &self,
        space: &str,
        plan: crate::plan::AlterPlan,
    ) -> Result<ExecutorResult> {
        let (schema_type, name, operations, key) = match plan {
            crate::plan::AlterPlan::Tag { name, operations } => {
                let key = SchemaKey::tag(space, &name);
                ("Tag", name, operations, key)
            }
            crate::plan::AlterPlan::Edge { name, operations } => {
                let key = SchemaKey::edge(space, &name);
                ("Edge", name, operations, key)
            }
            crate::plan::AlterPlan::User { name, new_password } => {
                // User management is handled separately
                return self.execute_alter_user(&name, new_password).await;
            }
        };

        // Get current schema metadata
        let existing_data = self
            .ctx
            .kvstore
            .get(&key)
            .await?
            .ok_or_else(|| match schema_type {
                "Tag" => ExecutionError::TagNotFound(name.clone()),
                _ => ExecutionError::EdgeNotFound(name.clone()),
            })?;

        let mut schema_data: serde_json::Value = serde_json::from_slice(&existing_data)?;

        // Apply operations
        self.apply_alter_operations(&mut schema_data, &operations, schema_type, &name)?;

        // Increment version
        self.increment_schema_version(&mut schema_data);

        // Save updated schema
        self.ctx
            .kvstore
            .put(&key, serde_json::to_vec(&schema_data)?.as_slice())
            .await?;

        Ok(ExecutorResult::success_message(format!(
            "{} {} altered successfully",
            schema_type, name
        )))
    }

    /// Apply ALTER operations to schema JSON
    pub(super) fn apply_alter_operations(
        &self,
        schema_data: &mut serde_json::Value,
        operations: &[crate::plan::AlterColumnOp],
        schema_type: &str,
        name: &str,
    ) -> Result<()> {
        for op in operations {
            let props = schema_data.get_mut("properties").ok_or_else(|| {
                ExecutionError::InvalidOperation(format!(
                    "{} schema is missing 'properties' array",
                    schema_type
                ))
            })?;
            let props_array = props.as_array_mut().ok_or_else(|| {
                ExecutionError::InvalidOperation(format!(
                    "{} schema field 'properties' is not an array",
                    schema_type
                ))
            })?;

            match op.op_type {
                crate::plan::AlterOpType::AddColumn => {
                    validate_add_column(&op.prop)?;

                    if props_array.iter().any(|p| p["name"] == op.prop.name) {
                        return Err(ExecutionError::InvalidOperation(format!(
                            "Column '{}' already exists in {} '{}'",
                            op.prop.name,
                            schema_type.to_lowercase(),
                            name
                        )));
                    }

                    props_array.push(serde_json::json!({
                        "name": op.prop.name,
                        "data_type": format!("{:?}", op.prop.data_type),
                        "nullable": op.prop.nullable,
                        "default_value": op.prop.default_value,
                    }));
                }
                crate::plan::AlterOpType::DropColumn => {
                    let before = props_array.len();
                    props_array.retain(|p| p["name"] != op.prop.name);
                    if props_array.len() == before {
                        return Err(ExecutionError::InvalidOperation(format!(
                            "Column '{}' does not exist in {} '{}'",
                            op.prop.name,
                            schema_type.to_lowercase(),
                            name
                        )));
                    }
                }
                crate::plan::AlterOpType::ChangeColumn => {
                    let entry = props_array
                        .iter_mut()
                        .find(|p| p["name"] == op.prop.name)
                        .ok_or_else(|| {
                            ExecutionError::InvalidOperation(format!(
                                "Column '{}' does not exist in {} '{}'",
                                op.prop.name,
                                schema_type.to_lowercase(),
                                name
                            ))
                        })?;
                    *entry = serde_json::json!({
                        "name": op.prop.name,
                        "data_type": format!("{:?}", op.prop.data_type),
                        "nullable": op.prop.nullable,
                        "default_value": op.prop.default_value,
                    });
                }
            }
        }
        Ok(())
    }

    /// Increment schema version
    pub(super) fn increment_schema_version(&self, schema_data: &mut serde_json::Value) {
        if let Some(version) = schema_data.get_mut("version") {
            if let Some(v) = version.as_i64() {
                *version = serde_json::json!(v + 1);
            }
        } else {
            schema_data["version"] = serde_json::json!(2);
        }
    }

    /// Execute DROP statement
    pub(super) async fn execute_drop(&self, plan: crate::plan::DropPlan) -> Result<ExecutorResult> {
        match plan {
            crate::plan::DropPlan::Space { name, if_exists } => {
                let space_key = format!("space:{}", name);

                // Read the space meta first: we need its id to purge indexes,
                // and to honor IF EXISTS.
                let Some(meta) = self.ctx.kvstore.get(space_key.as_bytes()).await? else {
                    if if_exists {
                        return Ok(ExecutorResult {
                            columns: vec![],
                            rows: vec![],
                            latency_ms: 0,
                        });
                    }
                    return Err(ExecutionError::SpaceNotFound(name));
                };

                let space_id = serde_json::from_slice::<serde_json::Value>(&meta)
                    .ok()
                    .and_then(|v| v.get("id").and_then(|i| i.as_u64()))
                    .map(|i| i as u32);

                // 1. Index entries (by index_id prefix, part_id=1) + in-memory
                //    definitions — so the same space name can be recreated and
                //    its indexes re-created without "already exists".
                if let (Some(im), Some(sid)) = (self.ctx.index_manager.as_ref(), space_id) {
                    let mut defs = im.list_tag_indexes(sid).await;
                    defs.extend(im.list_edge_indexes(sid).await);
                    for def in defs {
                        let prefix = match def.index_type {
                            byoridb_storage::IndexType::Tag => {
                                byoridb_storage::KeyUtils::tag_index_prefix(1, def.id)
                            }
                            byoridb_storage::IndexType::Edge => {
                                byoridb_storage::KeyUtils::edge_index_prefix(1, def.id)
                            }
                        };
                        self.delete_by_prefix(&prefix).await?;
                        let _ = im.drop_index(sid, &def.index_name).await;
                    }
                }

                // 2. Vertex / edge / tag-vid data: "{name}:..."
                self.delete_by_prefix(format!("{}:", name).as_bytes())
                    .await?;
                // 3. Tag / edge schema: "space:{name}:..."
                self.delete_by_prefix(format!("space:{}:", name).as_bytes())
                    .await?;
                // 4. The space meta key itself.
                self.ctx.kvstore.delete(space_key.as_bytes()).await?;

                Ok(ExecutorResult {
                    columns: vec![],
                    rows: vec![],
                    latency_ms: 0,
                })
            }
            crate::plan::DropPlan::Tag { name, if_exists } => {
                let space = self.ctx.space.as_ref().ok_or_else(|| {
                    ExecutionError::InvalidOperation("No space selected".to_string())
                })?;

                let tag_key = format!("space:{}:tag:{}", space, name);

                // Check if tag exists
                if self.ctx.kvstore.get(tag_key.as_bytes()).await?.is_none() {
                    if if_exists {
                        return Ok(ExecutorResult {
                            columns: vec![],
                            rows: vec![],
                            latency_ms: 0,
                        });
                    } else {
                        return Err(ExecutionError::TagNotFound(name));
                    }
                }

                // A class's tag must not be dropped out from under its
                // hierarchy record — that would leave an orphan class.
                if self
                    .ctx
                    .kvstore
                    .get(&crate::key::SchemaKey::class(space, &name))
                    .await?
                    .is_some()
                {
                    return Err(ExecutionError::InvalidOperation(format!(
                        "{} is a class; use DROP CLASS",
                        name
                    )));
                }

                // Delete tag
                self.ctx.kvstore.delete(tag_key.as_bytes()).await?;

                Ok(ExecutorResult {
                    columns: vec![],
                    rows: vec![],
                    latency_ms: 0,
                })
            }
            crate::plan::DropPlan::Class { name, if_exists } => {
                self.handle_drop_class(name, if_exists).await
            }
            crate::plan::DropPlan::Edge { name, if_exists } => {
                // O-8 D7: the sameAs reserved type underpins irreversible merges;
                // dropping it would strand the canonical-representative map.
                if name == crate::executor::sameas::SAMEAS_EDGE {
                    return Err(ExecutionError::InvalidOperation(
                        "the reserved sameAs edge type cannot be dropped — \
                         owl:sameAs merges are irreversible (insertion-only)"
                            .to_string(),
                    ));
                }
                let space = self.ctx.space.as_ref().ok_or_else(|| {
                    ExecutionError::InvalidOperation("No space selected".to_string())
                })?;

                let edge_key = format!("space:{}:edge:{}", space, name);

                // Check if edge exists
                if self.ctx.kvstore.get(edge_key.as_bytes()).await?.is_none() {
                    if if_exists {
                        return Ok(ExecutorResult {
                            columns: vec![],
                            rows: vec![],
                            latency_ms: 0,
                        });
                    } else {
                        return Err(ExecutionError::EdgeNotFound(name));
                    }
                }

                // Delete edge
                self.ctx.kvstore.delete(edge_key.as_bytes()).await?;

                Ok(ExecutorResult {
                    columns: vec![],
                    rows: vec![],
                    latency_ms: 0,
                })
            }
            crate::plan::DropPlan::User { name, if_exists } => {
                self.handle_drop_user(name, if_exists).await
            }
            crate::plan::DropPlan::TagIndex { name, if_exists } => {
                self.handle_drop_tag_index(name, if_exists).await
            }
            crate::plan::DropPlan::EdgeIndex { name, if_exists } => {
                self.handle_drop_edge_index(name, if_exists).await
            }
        }
    }

    /// Handle DROP TAG INDEX
    ///
    /// Like CREATE TAG INDEX, this requires a running Meta service because
    /// the index lifecycle is owned by the Meta/Storage layer, not the
    /// local kvstore.
    pub(super) async fn handle_drop_tag_index(
        &self,
        name: String,
        if_exists: bool,
    ) -> Result<ExecutorResult> {
        let _space = self.require_space()?.to_string();
        #[cfg(feature = "distributed")]
        if let Some(client) = self.ctx.meta_client.as_ref() {
            let space_info = client
                .get_space(&_space)
                .await
                .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?;

            return match client.drop_tag_index(space_info.id, &name).await {
                Ok(()) => Ok(ExecutorResult::empty()),
                Err(byoridb_meta::MetaError::IndexNotFound(_)) if if_exists => {
                    Ok(ExecutorResult::empty())
                }
                Err(byoridb_meta::MetaError::IndexNotFound(_)) => Err(
                    ExecutionError::InvalidOperation(format!("Tag index {} not found", name)),
                ),
                Err(e) => Err(ExecutionError::InvalidOperation(format!(
                    "Meta error: {}",
                    e
                ))),
            };
        }

        let Some(index_manager) = self.ctx.index_manager.as_ref() else {
            return if if_exists {
                Ok(ExecutorResult::empty())
            } else {
                Err(ExecutionError::InvalidOperation(format!(
                    "Tag index {} not found",
                    name
                )))
            };
        };
        match index_manager
            .drop_index(self.ctx.space_id.unwrap_or(1), &name)
            .await
        {
            Ok(()) => Ok(ExecutorResult::empty()),
            Err(byoridb_storage::IndexError::IndexNotFound(_)) if if_exists => {
                Ok(ExecutorResult::empty())
            }
            Err(byoridb_storage::IndexError::IndexNotFound(_)) => Err(
                ExecutionError::InvalidOperation(format!("Tag index {} not found", name)),
            ),
            Err(e) => Err(ExecutionError::InvalidOperation(e.to_string())),
        }
    }

    /// Handle DROP EDGE INDEX
    ///
    /// See [`handle_drop_tag_index`] for why MetaClient is required.
    pub(super) async fn handle_drop_edge_index(
        &self,
        name: String,
        if_exists: bool,
    ) -> Result<ExecutorResult> {
        let _space = self.require_space()?.to_string();
        #[cfg(feature = "distributed")]
        if let Some(client) = self.ctx.meta_client.as_ref() {
            let space_info = client
                .get_space(&_space)
                .await
                .map_err(|e| ExecutionError::InvalidOperation(format!("Meta error: {}", e)))?;

            return match client.drop_edge_index(space_info.id, &name).await {
                Ok(()) => Ok(ExecutorResult::empty()),
                Err(byoridb_meta::MetaError::IndexNotFound(_)) if if_exists => {
                    Ok(ExecutorResult::empty())
                }
                Err(byoridb_meta::MetaError::IndexNotFound(_)) => Err(
                    ExecutionError::InvalidOperation(format!("Edge index {} not found", name)),
                ),
                Err(e) => Err(ExecutionError::InvalidOperation(format!(
                    "Meta error: {}",
                    e
                ))),
            };
        }

        let Some(index_manager) = self.ctx.index_manager.as_ref() else {
            return if if_exists {
                Ok(ExecutorResult::empty())
            } else {
                Err(ExecutionError::InvalidOperation(format!(
                    "Edge index {} not found",
                    name
                )))
            };
        };
        match index_manager
            .drop_index(self.ctx.space_id.unwrap_or(1), &name)
            .await
        {
            Ok(()) => Ok(ExecutorResult::empty()),
            Err(byoridb_storage::IndexError::IndexNotFound(_)) if if_exists => {
                Ok(ExecutorResult::empty())
            }
            Err(byoridb_storage::IndexError::IndexNotFound(_)) => Err(
                ExecutionError::InvalidOperation(format!("Edge index {} not found", name)),
            ),
            Err(e) => Err(ExecutionError::InvalidOperation(e.to_string())),
        }
    }
}

#[cfg(feature = "distributed")]
fn convert_ast_type_to_meta(dt: byoridb_parser::ast::DataType) -> MetaDataType {
    use byoridb_parser::ast::DataType as AstDataType;
    match dt {
        AstDataType::Int64 => MetaDataType::Int64,
        AstDataType::String => MetaDataType::String,
        AstDataType::Bool => MetaDataType::Bool,
        AstDataType::Double => MetaDataType::Double,
        AstDataType::Date => MetaDataType::Date,
        AstDataType::DateTime => MetaDataType::DateTime,
        AstDataType::Timestamp => MetaDataType::Timestamp,
        AstDataType::Int32 => MetaDataType::Int32,
        AstDataType::Float => MetaDataType::Float,
        _ => MetaDataType::String, // Fallback
    }
}

#[cfg(feature = "distributed")]
fn convert_expr_to_string(expr: &Expression) -> Result<String> {
    match expr {
        Expression::Literal(lit) => match lit {
            Literal::String(s) => Ok(s.clone()),
            Literal::Int(i) => Ok(i.to_string()),
            Literal::Float(f) => Ok(f.to_string()),
            Literal::Bool(b) => Ok(b.to_string()),
            Literal::Null => Ok("null".to_string()),
        },
        _ => Err(ExecutionError::InvalidOperation(
            "Default value must be a literal".to_string(),
        )),
    }
}

fn validate_add_column(prop: &crate::plan::PropertyDef) -> Result<()> {
    if !prop.nullable && prop.default_value.is_none() {
        return Err(ExecutionError::InvalidOperation(format!(
            "Column '{}' must be nullable or have a default value for ADD COLUMN",
            prop.name
        )));
    }
    Ok(())
}
