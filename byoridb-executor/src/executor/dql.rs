// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use super::{Executor, ExecutorResult};
use crate::error::{ExecutionError, Result};
use crate::profile::ProfileOp;
use byoridb_codec::EdgeData as CodecEdgeData;
use byoridb_codec::{VertexCodec, VertexData as CodecVertexData};
use byoridb_common::FilterExpr;
use byoridb_parser::ast::{BinaryOperator, Expression, Literal};
use byoridb_storage::key::IndexValue;
#[cfg(feature = "distributed")]
use byoridb_storage::proto::storage::IndexValue as ProtoIndexValue;

fn json_to_value(val: &serde_json::Value) -> Option<byoridb_common::Value> {
    match val {
        serde_json::Value::String(s) => Some(byoridb_common::Value::String(s.clone())),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(byoridb_common::Value::Int(i))
            } else {
                n.as_f64().map(byoridb_common::Value::Float)
            }
        }
        serde_json::Value::Bool(b) => Some(byoridb_common::Value::Bool(*b)),
        serde_json::Value::Null => Some(byoridb_common::Value::null()),
        _ => None,
    }
}

impl Executor {
    pub(super) async fn execute_fetch(
        &self,
        plan: crate::plan::FetchPlan,
    ) -> Result<ExecutorResult> {
        // Use plan.space or fallback to ctx.space
        let effective_space = if plan.space.is_empty() {
            self.ctx
                .space
                .as_ref()
                .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?
                .clone()
        } else {
            plan.space.clone()
        };

        // Check if distributed mode is enabled
        #[cfg(feature = "distributed")]
        if let Some(distributed) = self.ctx.get_distributed_executor() {
            return self
                .execute_fetch_distributed(&distributed, &plan, &effective_space)
                .await;
        }

        // Local execution (fallback)
        self.execute_fetch_local(&plan, &effective_space).await
    }

    /// Execute FETCH via distributed query execution
    #[cfg(feature = "distributed")]
    pub(super) async fn execute_fetch_distributed(
        &self,
        distributed: &crate::distributed::DistributedQueryExecutor,
        plan: &crate::plan::FetchPlan,
        effective_space: &str,
    ) -> Result<ExecutorResult> {
        let space_id = self.ctx.space_id.ok_or_else(|| {
            ExecutionError::InvalidOperation("Space ID not set for distributed fetch".to_string())
        })?;

        let partition_num = self.ctx.get_partition_num().ok_or_else(|| {
            ExecutionError::InvalidOperation(
                "Partition number not set for distributed fetch".to_string(),
            )
        })?;

        tracing::info!(
            "Executing distributed FETCH: space={}, vids_count={}",
            effective_space,
            plan.vids.len()
        );

        let profiling = self.ctx.profiling();
        let fetch_start = std::time::Instant::now();

        // Execute distributed fetch
        let vertices = distributed
            .execute_fetch(
                space_id,
                partition_num,
                plan.vids.clone(),
                plan.tags.clone(),
                vec![], // All properties
            )
            .await
            .map_err(|e| {
                ExecutionError::InvalidOperation(format!("Distributed fetch failed: {}", e))
            })?;

        if profiling {
            self.ctx.record_profile(
                ProfileOp::GetVertices,
                format!("distributed RPC, {} vid(s)", plan.vids.len()),
                vertices.len() as u64,
                fetch_start.elapsed().as_micros() as u64,
                false,
            );
        }

        // Convert VertexData to rows
        let mut rows = Vec::new();
        for vertex in vertices {
            let mut row = Vec::new();
            row.push(byoridb_common::Value::Int(vertex.vid));

            // Extract tags and their properties
            for tag_data in &vertex.tags {
                let tag_json = serde_json::json!({
                    "name": tag_data.tag_name,
                    "props": tag_data.properties.iter().map(|(k, v)| {
                        let val: serde_json::Value = serde_json::from_slice(v).unwrap_or(serde_json::Value::Null);
                        (k.clone(), val)
                    }).collect::<std::collections::HashMap<_, _>>()
                });
                row.push(byoridb_common::Value::String(tag_json.to_string()));
            }

            rows.push(row);
        }

        let mut columns = vec!["VertexID".to_string()];
        columns.extend(plan.tags.clone());

        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: 0,
        })
    }

    /// Execute FETCH locally (single-node)
    pub(super) async fn execute_fetch_local(
        &self,
        plan: &crate::plan::FetchPlan,
        effective_space: &str,
    ) -> Result<ExecutorResult> {
        if plan.is_edge_fetch {
            return self.execute_fetch_edges_local(plan, effective_space).await;
        }

        // Resolve VIDs: either literal list or $var.col from context
        let resolved_vids: Vec<i64> = if let Some(ref var_ref) = plan.src_var {
            let (var_name, col_name) = if let Some(dot) = var_ref.find('.') {
                (&var_ref[..dot], Some(&var_ref[dot + 1..]))
            } else {
                (var_ref.as_str(), None)
            };
            let bound = self.ctx.lookup_var(var_name).ok_or_else(|| {
                ExecutionError::InvalidOperation(format!("Variable '{}' is not defined", var_name))
            })?;
            let col_idx = col_name
                .and_then(|col| bound.columns.iter().position(|c| c == col))
                .unwrap_or(0);
            bound
                .rows
                .iter()
                .filter_map(|row| {
                    row.get(col_idx).and_then(|v| match v {
                        byoridb_common::Value::Int(i) => Some(*i),
                        _ => None,
                    })
                })
                .collect()
        } else {
            plan.vids.clone()
        };

        // Vertex fetch: batch-get all vertex keys
        let profiling = self.ctx.profiling();
        let fetch_start = std::time::Instant::now();
        let keys: Vec<Vec<u8>> = resolved_vids
            .iter()
            .map(|vid| format!("{}:vertex:{}", effective_space, vid).into_bytes())
            .collect();

        let results = self.ctx.kvstore.batch_get(&keys).await?;

        let mut rows = Vec::new();
        for (vid, data_opt) in resolved_vids.iter().zip(results.iter()) {
            if let Some(data) = data_opt {
                if let Ok(vertex_data) = VertexCodec::decode_vertex(data) {
                    let mut row = Vec::new();
                    row.push(byoridb_common::Value::Int(*vid));

                    for tag in &vertex_data.tags {
                        let tag_json = VertexCodec::vertex_to_json(&CodecVertexData {
                            vid: *vid,
                            tags: vec![tag.clone()],
                        });
                        if let Some(tags_array) = tag_json.get("tags").and_then(|t| t.as_array()) {
                            for t in tags_array {
                                row.push(byoridb_common::Value::String(t.to_string()));
                            }
                        }
                    }

                    rows.push(row);
                }
            }
        }

        if profiling {
            self.ctx.record_profile(
                ProfileOp::GetVertices,
                format!("{} vid(s)", resolved_vids.len()),
                rows.len() as u64,
                fetch_start.elapsed().as_micros() as u64,
                false,
            );
        }

        let mut columns = vec!["VertexID".to_string()];
        columns.extend(plan.tags.clone());

        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: 0,
        })
    }

    /// Execute FETCH PROP ON <edge_type> src->dst locally.
    pub(super) async fn execute_fetch_edges_local(
        &self,
        plan: &crate::plan::FetchPlan,
        effective_space: &str,
    ) -> Result<ExecutorResult> {
        let edge_type = plan.tags.first().map(|s| s.as_str()).unwrap_or("*");
        let mut rows = Vec::new();
        let profiling = self.ctx.profiling();
        let fetch_start = std::time::Instant::now();

        for (src, dst) in &plan.edge_refs {
            // Scan all edges from `src` with the given edge type (ranking = 0 default)
            let prefix = if edge_type == "*" {
                format!("{}:edge:{}:", effective_space, src)
            } else {
                format!("{}:edge:{}:{}:", effective_space, src, edge_type)
            };

            let entries = self.ctx.kvstore.scan_prefix(prefix.as_bytes()).await?;

            for (_key, value) in entries {
                match VertexCodec::decode_edge(&value) {
                    Ok(edge) if edge.dst_vid == *dst => {
                        let edge_json = VertexCodec::edge_to_json(&edge);
                        rows.push(vec![
                            byoridb_common::Value::Int(edge.src_vid),
                            byoridb_common::Value::Int(edge.dst_vid),
                            byoridb_common::Value::String(edge_json.to_string()),
                        ]);
                    }
                    _ => continue,
                }
            }
        }

        if profiling {
            self.ctx.record_profile(
                ProfileOp::GetEdges,
                format!("{} edge ref(s)", plan.edge_refs.len()),
                rows.len() as u64,
                fetch_start.elapsed().as_micros() as u64,
                false,
            );
        }

        Ok(ExecutorResult {
            columns: vec![
                "src".to_string(),
                "dst".to_string(),
                "properties".to_string(),
            ],
            rows,
            latency_ms: 0,
        })
    }

    /// Execute GO statement (graph traversal)
    pub(super) async fn execute_go(&self, plan: crate::plan::GoPlan) -> Result<ExecutorResult> {
        let from_vids = if plan.from_clause.vids.is_empty() {
            if let Some(ref var_ref) = plan.from_clause.src {
                // Resolve $var.column reference from context variables.
                // Format: "$varname.colname" or "$varname" (uses first column).
                let (var_name, col_name) = if let Some(dot) = var_ref.find('.') {
                    (&var_ref[..dot], Some(&var_ref[dot + 1..]))
                } else {
                    (var_ref.as_str(), None)
                };
                // Strip leading '$' if present
                let var_name = var_name.trim_start_matches('$');

                let bound = self.ctx.lookup_var(var_name).ok_or_else(|| {
                    ExecutionError::InvalidOperation(format!(
                        "Variable '{}' is not defined",
                        var_name
                    ))
                })?;

                // Determine which column to use for VIDs
                let col_idx = if let Some(col) = col_name {
                    bound.columns.iter().position(|c| c == col).ok_or_else(|| {
                        ExecutionError::InvalidOperation(format!(
                            "Column '{}' not found in variable '{}'",
                            col, var_name
                        ))
                    })?
                } else {
                    0
                };

                let mut vids = Vec::new();
                for row in &bound.rows {
                    if let Some(val) = row.get(col_idx) {
                        match val {
                            byoridb_common::Value::Int(i) => vids.push(*i),
                            other => {
                                return Err(ExecutionError::InvalidOperation(format!(
                                    "Variable column value is not a VID (integer): {:?}",
                                    other
                                )))
                            }
                        }
                    }
                }
                vids
            } else {
                return Err(ExecutionError::InvalidOperation(
                    "No source vertices specified".to_string(),
                ));
            }
        } else {
            plan.from_clause.vids.clone()
        };

        // Check if distributed mode is enabled
        #[cfg(feature = "distributed")]
        if let Some(distributed) = self.ctx.get_distributed_executor() {
            return self
                .execute_go_distributed(&distributed, &plan, from_vids)
                .await;
        }

        // Local execution (fallback)
        self.execute_go_local(&plan, from_vids).await
    }

    /// Execute GO via distributed query execution
    #[cfg(feature = "distributed")]
    pub(super) async fn execute_go_distributed(
        &self,
        distributed: &crate::distributed::DistributedQueryExecutor,
        plan: &crate::plan::GoPlan,
        from_vids: Vec<i64>,
    ) -> Result<ExecutorResult> {
        let space_id = self.ctx.space_id.ok_or_else(|| {
            ExecutionError::InvalidOperation("Space ID not set for distributed GO".to_string())
        })?;

        let partition_num = self.ctx.get_partition_num().ok_or_else(|| {
            ExecutionError::InvalidOperation(
                "Partition number not set for distributed GO".to_string(),
            )
        })?;

        let edge_type = plan.over_edges.first().cloned().unwrap_or_default();

        if plan.direction != byoridb_parser::ast::EdgeDirection::Outgoing {
            return Err(ExecutionError::InvalidOperation(
                "Distributed GO currently supports only forward traversal".to_string(),
            ));
        }

        tracing::info!(
            "Executing distributed GO: src_vids_count={}, edge_type={}",
            from_vids.len(),
            edge_type
        );

        let profiling = self.ctx.profiling();
        let go_start = std::time::Instant::now();

        // Execute distributed GO
        let edges = distributed
            .execute_go(
                space_id,
                partition_num,
                from_vids.clone(),
                &edge_type,
                vec![], // All properties
            )
            .await
            .map_err(|e| {
                ExecutionError::InvalidOperation(format!("Distributed GO failed: {}", e))
            })?;

        if profiling {
            self.ctx.record_profile(
                ProfileOp::GetNeighbors,
                format!("distributed RPC, edge={}", edge_type),
                edges.len() as u64,
                go_start.elapsed().as_micros() as u64,
                false,
            );
        }

        // Convert EdgeData to rows
        let mut rows = Vec::new();
        for edge in edges {
            if let Some(ref key) = edge.key {
                let row = vec![
                    byoridb_common::Value::Int(key.src_vid),
                    byoridb_common::Value::Int(key.dst_vid),
                ];
                rows.push(row);
            }
        }

        let columns = vec![
            format!(
                "{}.{}",
                from_vids.first().unwrap_or(&0),
                plan.to_clause.variable
            ),
            "dst".to_string(),
        ];

        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: 0,
        })
    }

    /// Execute GO locally (single-node)
    pub(super) async fn execute_go_local(
        &self,
        plan: &crate::plan::GoPlan,
        from_vids: Vec<i64>,
    ) -> Result<ExecutorResult> {
        // Each traversal result: (origin_src, terminal_dst, last_edge)
        let mut traversal: Vec<(i64, i64, Option<CodecEdgeData>)> = Vec::new();

        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;
        let (min_steps, max_steps) = match plan.to_clause.steps {
            crate::plan::StepClause::Exactly(n) => (n, n),
            crate::plan::StepClause::Range(min, max) => (min, max),
        };

        let max_go_steps = self.ctx.config.max_go_steps;
        if max_go_steps > 0 && max_steps > max_go_steps as usize {
            return Err(ExecutionError::InvalidOperation(format!(
                "GO step count {} exceeds maximum allowed {}",
                max_steps, max_go_steps
            )));
        }

        // Incoming / undirected traversal scans the whole edge space per hop
        // (no reverse-edge index yet — see PLAN.md O-1). Flag it for the
        // slow-query log and pick the matching profile operator.
        let scans_incoming =
            !matches!(plan.direction, byoridb_parser::ast::EdgeDirection::Outgoing);
        if scans_incoming {
            self.ctx.mark_full_scan();
        }
        let profiling = self.ctx.profiling();
        let prof_start = std::time::Instant::now();
        let mut scanned_neighbors: u64 = 0;

        for src_vid in from_vids.iter() {
            let mut visited = std::collections::HashSet::new();
            // frontier: (current_vid, last_edge_on_path_to_current)
            let mut frontier: Vec<(i64, Option<CodecEdgeData>)> = vec![(*src_vid, None)];
            visited.insert(*src_vid);

            if min_steps == 0 {
                traversal.push((*src_vid, *src_vid, None));
            }

            for hop in 1..=max_steps {
                let mut next_frontier: Vec<(i64, Option<CodecEdgeData>)> = Vec::new();

                for (current_vid, _) in frontier {
                    let neighbors = match plan.direction {
                        byoridb_parser::ast::EdgeDirection::Outgoing => {
                            crate::algo::get_neighbors(
                                &self.ctx,
                                space,
                                current_vid,
                                &plan.over_edges,
                            )
                            .await?
                        }
                        byoridb_parser::ast::EdgeDirection::Incoming => {
                            crate::algo::get_incoming_neighbors(
                                &self.ctx,
                                space,
                                current_vid,
                                &plan.over_edges,
                            )
                            .await?
                        }
                        byoridb_parser::ast::EdgeDirection::Undirected => {
                            let mut neighbors = crate::algo::get_neighbors(
                                &self.ctx,
                                space,
                                current_vid,
                                &plan.over_edges,
                            )
                            .await?;
                            neighbors.extend(
                                crate::algo::get_incoming_neighbors(
                                    &self.ctx,
                                    space,
                                    current_vid,
                                    &plan.over_edges,
                                )
                                .await?,
                            );
                            neighbors
                        }
                    };

                    scanned_neighbors += neighbors.len() as u64;
                    for neighbor in neighbors {
                        let dst = neighbor.dst;
                        if visited.insert(dst) {
                            if hop >= min_steps {
                                traversal.push((*src_vid, dst, Some(neighbor.edge.clone())));
                            }
                            next_frontier.push((dst, Some(neighbor.edge)));
                        }
                    }
                }

                if next_frontier.is_empty() {
                    break;
                }
                frontier = next_frontier;
            }
        }

        if profiling {
            let op = if scans_incoming {
                ProfileOp::GetIncoming
            } else {
                ProfileOp::GetNeighbors
            };
            self.ctx.record_profile(
                op,
                format!("over [{}]", plan.over_edges.join(", ")),
                scanned_neighbors,
                prof_start.elapsed().as_micros() as u64,
                scans_incoming,
            );
        }

        let proj_start = std::time::Instant::now();

        // If no explicit YIELD columns, return ["src", "dst"] for backward
        // compatibility and compound-statement variable resolution.
        if plan.yield_clause.columns.is_empty() {
            let rows: Vec<Vec<byoridb_common::Value>> = traversal
                .into_iter()
                .map(|(src, dst, _)| {
                    vec![
                        byoridb_common::Value::Int(src),
                        byoridb_common::Value::Int(dst),
                    ]
                })
                .collect();
            if profiling {
                self.ctx.record_profile(
                    ProfileOp::Project,
                    "src, dst".to_string(),
                    rows.len() as u64,
                    proj_start.elapsed().as_micros() as u64,
                    false,
                );
            }
            return Ok(ExecutorResult {
                columns: vec!["src".to_string(), "dst".to_string()],
                rows,
                latency_ms: 0,
            });
        }

        // Apply YIELD columns.
        let columns: Vec<String> = plan
            .yield_clause
            .columns
            .iter()
            .map(|col| {
                if let Some(ref alias) = col.alias {
                    return alias.clone();
                }
                match &col.expression {
                    Expression::Identifier(name) => name.clone(),
                    Expression::PropRef { object, prop } => format!("{}.{}", object, prop),
                    Expression::DstVertexProp { tag, prop } => format!("{}.{}", tag, prop),
                    other => format!("{:?}", other),
                }
            })
            .collect();

        let mut rows = Vec::with_capacity(traversal.len());
        for (src_vid, dst_vid, last_edge) in traversal {
            let mut row = Vec::with_capacity(plan.yield_clause.columns.len());
            for col in &plan.yield_clause.columns {
                let val = self
                    .eval_go_yield_expr(
                        space,
                        src_vid,
                        dst_vid,
                        last_edge.as_ref(),
                        &col.expression,
                    )
                    .await;
                row.push(val);
            }
            rows.push(row);
        }

        if profiling {
            self.ctx.record_profile(
                ProfileOp::Project,
                format!("{} column(s)", columns.len()),
                rows.len() as u64,
                proj_start.elapsed().as_micros() as u64,
                false,
            );
        }

        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: 0,
        })
    }

    /// Evaluate a single YIELD expression in the context of a GO traversal row.
    async fn eval_go_yield_expr(
        &self,
        space: &str,
        src_vid: i64,
        dst_vid: i64,
        last_edge: Option<&CodecEdgeData>,
        expr: &Expression,
    ) -> byoridb_common::Value {
        match expr {
            Expression::Identifier(name) => match name.as_str() {
                "src" | "_src_vid" => byoridb_common::Value::Int(src_vid),
                "dst" | "_dst_vid" => byoridb_common::Value::Int(dst_vid),
                "vertex" => {
                    let key = format!("{}:vertex:{}", space, dst_vid);
                    match self.ctx.kvstore.get(key.as_bytes()).await {
                        Ok(Some(blob)) => match VertexCodec::decode_vertex(&blob) {
                            Ok(v) => byoridb_common::Value::String(
                                VertexCodec::vertex_to_json(&v).to_string(),
                            ),
                            Err(_) => byoridb_common::Value::Null(byoridb_common::NullType::Null),
                        },
                        _ => byoridb_common::Value::Null(byoridb_common::NullType::Null),
                    }
                }
                _ => byoridb_common::Value::Null(byoridb_common::NullType::Null),
            },
            Expression::PropRef { object: _, prop } => {
                if let Some(edge) = last_edge {
                    match prop.as_str() {
                        "_dst" | "dst_id" => byoridb_common::Value::Int(edge.dst_vid),
                        "_src" | "src_id" => byoridb_common::Value::Int(edge.src_vid),
                        "_type" => byoridb_common::Value::String(edge.edge_type.clone()),
                        "_rank" => byoridb_common::Value::Int(edge.ranking),
                        _ => {
                            edge.properties.get(prop).cloned().unwrap_or(
                                byoridb_common::Value::Null(byoridb_common::NullType::Null),
                            )
                        }
                    }
                } else {
                    byoridb_common::Value::Null(byoridb_common::NullType::Null)
                }
            }
            Expression::DstVertexProp { tag, prop } => {
                // Fetch the destination vertex and look up tag.prop.
                let key = format!("{}:vertex:{}", space, dst_vid);
                let blob = match self.ctx.kvstore.get(key.as_bytes()).await {
                    Ok(Some(b)) => b,
                    _ => return byoridb_common::Value::Null(byoridb_common::NullType::Null),
                };
                let vertex = match VertexCodec::decode_vertex(&blob) {
                    Ok(v) => v,
                    Err(_) => return byoridb_common::Value::Null(byoridb_common::NullType::Null),
                };
                vertex
                    .tags
                    .iter()
                    .find(|t| t.name.eq_ignore_ascii_case(tag))
                    .and_then(|t| t.properties.get(prop))
                    .cloned()
                    .unwrap_or(byoridb_common::Value::Null(byoridb_common::NullType::Null))
            }
            Expression::Literal(lit) => match lit {
                Literal::Int(i) => byoridb_common::Value::Int(*i),
                Literal::Float(f) => byoridb_common::Value::Float(*f),
                Literal::String(s) => byoridb_common::Value::String(s.clone()),
                Literal::Bool(b) => byoridb_common::Value::Bool(*b),
                Literal::Null => byoridb_common::Value::Null(byoridb_common::NullType::Null),
            },
            _ => byoridb_common::Value::Null(byoridb_common::NullType::Null),
        }
    }

    /// Execute LOOKUP statement (index-based query)
    pub(super) async fn execute_lookup(
        &self,
        plan: crate::plan::LookupPlan,
    ) -> Result<ExecutorResult> {
        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;

        let (tag_or_edge_name, result_columns) = match &plan.lookup_type {
            crate::plan::LookupType::Tag(tag) => (
                tag.clone(),
                vec![format!("{}.vid", tag), format!("{}.*", tag)],
            ),
            crate::plan::LookupType::Edge(edge) => (
                edge.clone(),
                vec![format!("{}.src", edge), format!("{}.dst", edge)],
            ),
        };
        let lookup_limit = self.lookup_limit(plan.limit);
        #[cfg(feature = "distributed")]
        let lookup_limit_u32 = self.lookup_limit_u32(plan.limit);

        // Try index-based lookup first
        if let Some(ref index_manager) = self.ctx.index_manager {
            if let Some(ref where_expr) = plan.where_clause {
                // Try to extract indexable condition
                if let Some((field, value)) = self.extract_eq_condition(where_expr) {
                    // Find applicable index
                    let space_id = self.ctx.space_id.unwrap_or(1);
                    let indexes = index_manager.list_tag_indexes(space_id).await;

                    // Find an index that covers the field
                    if let Some(index_def) = indexes
                        .iter()
                        .find(|idx| idx.fields.len() == 1 && idx.fields[0] == field)
                    {
                        tracing::debug!(
                            "Using index '{}' for LOOKUP on field '{}'",
                            index_def.index_name,
                            field
                        );

                        // Convert value to IndexValue
                        let index_value = self.byoridb_value_to_index_value(&value);

                        // Check if distributed mode is enabled
                        #[cfg(feature = "distributed")]
                        if self.ctx.is_distributed() {
                            // Use distributed executor
                            if let Some(distributed_executor) = self.ctx.get_distributed_executor()
                            {
                                let partition_num = self.ctx.get_partition_num().unwrap_or(1);
                                let proto_values =
                                    vec![self.byoridb_value_to_proto_index_value(&value)];

                                match distributed_executor
                                    .execute_lookup_tag_index(
                                        space_id,
                                        partition_num,
                                        index_def.id,
                                        &index_def.index_name,
                                        proto_values,
                                        lookup_limit_u32,
                                    )
                                    .await
                                {
                                    Ok(vids) => {
                                        tracing::debug!(
                                            "Distributed index lookup returned {} VIDs",
                                            vids.len()
                                        );
                                        if self.ctx.profiling() {
                                            self.ctx.record_profile(
                                                ProfileOp::IndexScan,
                                                format!(
                                                    "distributed index '{}'",
                                                    index_def.index_name
                                                ),
                                                vids.len() as u64,
                                                0,
                                                false,
                                            );
                                        }

                                        if vids.is_empty() {
                                            return Ok(ExecutorResult {
                                                columns: result_columns,
                                                rows: vec![],
                                                latency_ms: 0,
                                            });
                                        }

                                        // Use distributed fetch for vertex data
                                        match distributed_executor
                                            .execute_fetch(
                                                space_id,
                                                partition_num,
                                                vids.clone(),
                                                vec![],
                                                vec![],
                                            )
                                            .await
                                        {
                                            Ok(vertices) => {
                                                let mut rows = Vec::new();
                                                for vertex in vertices {
                                                    let mut row = Vec::new();
                                                    row.push(byoridb_common::Value::Int(
                                                        vertex.vid,
                                                    ));
                                                    // Format tags as string
                                                    let tags_str = vertex
                                                        .tags
                                                        .iter()
                                                        .map(|t| {
                                                            format!(
                                                                "{}:{:?}",
                                                                t.tag_name, t.properties
                                                            )
                                                        })
                                                        .collect::<Vec<_>>()
                                                        .join(",");
                                                    row.push(byoridb_common::Value::String(
                                                        tags_str,
                                                    ));
                                                    rows.push(row);
                                                }
                                                return Ok(ExecutorResult {
                                                    columns: result_columns,
                                                    rows,
                                                    latency_ms: 0,
                                                });
                                            }
                                            Err(e) => {
                                                tracing::warn!(
                                                    "Distributed vertex fetch failed: {}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "Distributed index lookup failed, falling back to local: {}",
                                            e
                                        );
                                    }
                                }
                            }
                        }

                        // Local mode: run the lookup against every partition
                        // the space owns. Using a fixed part_id (the old
                        // `part_id = 1` shortcut) silently dropped rows
                        // whose VIDs hashed to a different partition. See
                        // docs/MOCK_REMEDIATION_PLAN.md Item 16.
                        let partition_num = self.resolve_local_partition_num(space.as_str()).await;
                        let idx_profiling = self.ctx.profiling();
                        let idx_start = std::time::Instant::now();
                        let mut vids: Vec<i64> = Vec::new();
                        let mut last_err: Option<byoridb_storage::IndexError> = None;
                        for part_id in 1..=partition_num {
                            match index_manager
                                .lookup_tag(
                                    part_id,
                                    index_def,
                                    std::slice::from_ref(&index_value),
                                    lookup_limit.unwrap_or(usize::MAX),
                                )
                                .await
                            {
                                Ok(part_vids) => vids.extend(part_vids),
                                Err(e) => {
                                    tracing::warn!(
                                        "Local index lookup on part {} failed: {}",
                                        part_id,
                                        e
                                    );
                                    last_err = Some(e);
                                }
                            }
                        }

                        if vids.is_empty() && last_err.is_some() {
                            // Every partition errored: surrender to the scan
                            // fallback instead of returning an empty result
                            // that pretends the index succeeded.
                        } else {
                            if idx_profiling {
                                self.ctx.record_profile(
                                    ProfileOp::IndexScan,
                                    format!("index '{}'", index_def.index_name),
                                    vids.len() as u64,
                                    idx_start.elapsed().as_micros() as u64,
                                    false,
                                );
                            }
                            if vids.is_empty() {
                                return Ok(ExecutorResult {
                                    columns: result_columns,
                                    rows: vec![],
                                    latency_ms: 0,
                                });
                            }

                            // Batch fetch vertices by VIDs
                            let keys: Vec<Vec<u8>> = vids
                                .iter()
                                .map(|vid| format!("{}:vertex:{}", space, vid).into_bytes())
                                .collect();

                            let results = self.ctx.kvstore.batch_get(&keys).await?;

                            let mut rows = Vec::new();
                            for (vid, data_opt) in vids.iter().zip(results.iter()) {
                                if let Some(data) = data_opt {
                                    let vertex_data: serde_json::Value =
                                        serde_json::from_slice(data)?;
                                    let mut row = Vec::new();
                                    row.push(byoridb_common::Value::Int(*vid));
                                    if let Some(tags) = vertex_data.get("tags") {
                                        row.push(byoridb_common::Value::String(tags.to_string()));
                                    }
                                    rows.push(row);
                                }
                            }

                            tracing::debug!(
                                "Index lookup returned {} results across {} partitions (scanned {} keys)",
                                rows.len(),
                                partition_num,
                                vids.len()
                            );

                            if let Some(limit) = plan.limit {
                                rows.truncate(limit);
                            }
                            return Ok(ExecutorResult {
                                columns: result_columns,
                                rows,
                                latency_ms: 0,
                            });
                        }
                    }
                }
            }
        }

        // Fallback: Scan with predicate pushdown
        tracing::debug!("Using scan with predicate pushdown for LOOKUP (no suitable index found)");
        self.ctx.mark_full_scan();
        let scan_profiling = self.ctx.profiling();
        let scan_start = std::time::Instant::now();

        let vertex_prefix = format!("{}:vertex:", space);

        // Convert WHERE clause to FilterExpr for predicate pushdown
        let filter_expr = plan
            .where_clause
            .as_ref()
            .and_then(|expr| self.expr_to_filter_expr(expr))
            .unwrap_or(FilterExpr::True);

        let tag_name_filter = tag_or_edge_name.clone();
        let is_tag_lookup = matches!(plan.lookup_type, crate::plan::LookupType::Tag(_));

        // Create a filter closure that evaluates the filter expression.
        // Supports both proto-encoded (0xCA magic byte) and legacy JSON vertices.
        let filter_fn: byoridb_kvstore::FilterFn = Box::new(move |_key: &[u8], value: &[u8]| {
            let vertex_data = if VertexCodec::is_proto_format(value) {
                match VertexCodec::decode_vertex(value) {
                    Ok(v) => VertexCodec::vertex_to_json(&v),
                    Err(_) => return false,
                }
            } else {
                match serde_json::from_slice(value) {
                    Ok(v) => v,
                    Err(_) => return false,
                }
            };

            // Filter by tag name: only return vertices that have the requested tag
            if is_tag_lookup {
                let has_tag = vertex_data
                    .get("tags")
                    .and_then(|t| t.as_array())
                    .map(|tags| {
                        tags.iter().any(|t| {
                            t.get("name").and_then(|n| n.as_str()) == Some(tag_name_filter.as_str())
                        })
                    })
                    .unwrap_or(false);
                if !has_tag {
                    return false;
                }
            }

            // Build field getter from vertex data
            let get_field = |field: &str| -> Option<byoridb_common::Value> {
                if let Some(tags) = vertex_data.get("tags").and_then(|t| t.as_array()) {
                    for tag in tags {
                        if let Some(props) = tag.get("props").and_then(|p| p.as_object()) {
                            if let Some(prop_value) = props.get(field) {
                                return json_to_value(prop_value);
                            }
                            let tag_name = tag.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            let full_key = format!("{}.{}", tag_name, field);
                            if let Some(prop_value) = props.get(&full_key) {
                                return json_to_value(prop_value);
                            }
                        }
                    }
                }
                None
            };

            filter_expr.evaluate(&get_field)
        });

        // Use scan_with_filter for predicate pushdown
        let results = self
            .ctx
            .kvstore
            .scan_with_filter(vertex_prefix.as_bytes(), filter_fn, lookup_limit)
            .await?;

        if scan_profiling {
            self.ctx.record_profile(
                ProfileOp::FullScan,
                format!("scan {} (predicate pushdown)", vertex_prefix),
                results.len() as u64,
                scan_start.elapsed().as_micros() as u64,
                true,
            );
        }

        let mut rows = Vec::new();
        for (key, value) in results {
            let (vid, tag_str) = if VertexCodec::is_proto_format(&value) {
                // Proto-encoded: vid is stored inside the data
                match VertexCodec::decode_vertex(&value) {
                    Ok(v) => {
                        let json = VertexCodec::vertex_to_json(&v);
                        let tags = json.get("tags").map(|t| t.to_string()).unwrap_or_default();
                        (v.vid, tags)
                    }
                    Err(_) => continue,
                }
            } else {
                // Legacy JSON-encoded: extract vid from key
                let key_str = String::from_utf8_lossy(&key);
                let v = key_str
                    .split(':')
                    .nth(2)
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0);
                let vertex_data: serde_json::Value =
                    serde_json::from_slice(&value).unwrap_or(serde_json::Value::Null);
                let tags = vertex_data
                    .get("tags")
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                (v, tags)
            };

            let mut row = Vec::new();
            row.push(byoridb_common::Value::Int(vid));
            if !tag_str.is_empty() {
                row.push(byoridb_common::Value::String(tag_str));
            }
            rows.push(row);
        }

        if let Some(offset) = plan.offset {
            rows = rows.into_iter().skip(offset).collect();
        }
        if let Some(limit) = plan.limit {
            rows.truncate(limit);
        }
        Ok(ExecutorResult {
            columns: result_columns,
            rows,
            latency_ms: 0,
        })
    }

    /// Convert AST Expression to FilterExpr for predicate pushdown
    /// Extract a field name from a comparison operand. Accepts a bare
    /// `Identifier` (`WHERE name = ...`) and a qualified `PropRef`
    /// (`WHERE person.name = ...`, which LDBC/nGQL queries use) — both yield the
    /// property name. Without PropRef support, qualified predicates silently
    /// failed to convert and LOOKUP fell back to FilterExpr::True (no filter).
    fn field_name_of(expr: &Expression) -> Option<String> {
        match expr {
            Expression::Identifier(s) => Some(s.clone()),
            Expression::PropRef { prop, .. } => Some(prop.clone()),
            _ => None,
        }
    }

    /// A `field <op> value` operand pair: field name (Identifier or PropRef) +
    /// a literal value.
    fn field_value_pair(
        &self,
        field_expr: &Expression,
        value_expr: &Expression,
    ) -> Option<(String, byoridb_common::Value)> {
        let field = Self::field_name_of(field_expr)?;
        let value = self.expr_to_value(value_expr)?;
        Some((field, value))
    }

    pub(super) fn expr_to_filter_expr(&self, expr: &Expression) -> Option<FilterExpr> {
        match expr {
            Expression::BinaryOp { op, left, right } => match op {
                BinaryOperator::Eq => {
                    // field == value, or value == field
                    if let Some((f, v)) = self.field_value_pair(left, right) {
                        return Some(FilterExpr::eq(f, v));
                    }
                    if let Some((f, v)) = self.field_value_pair(right, left) {
                        return Some(FilterExpr::eq(f, v));
                    }
                    None
                }
                BinaryOperator::Neq => self
                    .field_value_pair(left, right)
                    .map(|(f, v)| FilterExpr::ne(f, v)),
                BinaryOperator::Lt => self
                    .field_value_pair(left, right)
                    .map(|(f, v)| FilterExpr::lt(f, v)),
                BinaryOperator::Lte => self
                    .field_value_pair(left, right)
                    .map(|(f, v)| FilterExpr::le(f, v)),
                BinaryOperator::Gt => self
                    .field_value_pair(left, right)
                    .map(|(f, v)| FilterExpr::gt(f, v)),
                BinaryOperator::Gte => self
                    .field_value_pair(left, right)
                    .map(|(f, v)| FilterExpr::ge(f, v)),
                BinaryOperator::And => {
                    let left_filter = self.expr_to_filter_expr(left.as_ref())?;
                    let right_filter = self.expr_to_filter_expr(right.as_ref())?;
                    Some(left_filter.and(right_filter))
                }
                BinaryOperator::Or => {
                    let left_filter = self.expr_to_filter_expr(left.as_ref())?;
                    let right_filter = self.expr_to_filter_expr(right.as_ref())?;
                    Some(left_filter.or(right_filter))
                }
                _ => None,
            },
            Expression::UnaryOp { op, operand } => {
                if matches!(op, byoridb_parser::ast::UnaryOperator::Not) {
                    let inner = self.expr_to_filter_expr(operand.as_ref())?;
                    Some(inner.not())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Resolve the partition count for the current space when running in
    /// local / embedded mode.
    ///
    /// Resolution order:
    /// 1. [`ExecutionContext::partition_num`] if set (distributed mode
    ///    populates this at startup).
    /// 2. `meta_client.get_space(space)` when a meta client is configured —
    ///    lets an embedded deployment that still talks to Meta for schema
    ///    enjoy the correct partition count.
    /// 3. Fallback to `1` with a `tracing::warn!` so operators can tell when
    ///    the lookup is scanning only the first partition.
    pub(super) async fn resolve_local_partition_num(&self, space: &str) -> u32 {
        if let Some(n) = self.ctx.get_partition_num() {
            return n.max(1);
        }
        if let Some(n) = self.partition_num_from_meta(space).await {
            return n.max(1);
        }
        tracing::warn!(
            "LOOKUP on space {} cannot determine partition_num; falling back to single-partition scan",
            space
        );
        1
    }

    /// Inner helper that asks the meta client for `partition_num`; returns
    /// `None` when the meta client is absent or the RPC fails, logging a
    /// warning in the latter case.
    pub(super) async fn partition_num_from_meta(&self, space: &str) -> Option<u32> {
        #[cfg(feature = "distributed")]
        {
            let client = self.ctx.meta_client.as_ref()?;
            client.get_space(space).await.map_or_else(
                |e| {
                    tracing::warn!(
                        "LOOKUP could not resolve partition_num for space {} via meta: {}",
                        space,
                        e
                    );
                    None
                },
                |info| Some(info.partition_num),
            )
        }
        // Embedded build has no meta client; partition_num comes from the local
        // resolver's fallback.
        #[cfg(not(feature = "distributed"))]
        {
            let _ = space;
            None
        }
    }

    /// Extract a simple equality condition from WHERE clause
    /// Returns (field_name, value) if the expression is "field == value"
    pub(super) fn extract_eq_condition(
        &self,
        expr: &Expression,
    ) -> Option<(String, byoridb_common::Value)> {
        match expr {
            Expression::BinaryOp {
                op: BinaryOperator::Eq,
                left,
                right,
            } => {
                // Try: field == value (field may be Identifier or PropRef)
                if let Some(pair) = self.field_value_pair(left, right) {
                    return Some(pair);
                }
                // Try: value == field
                if let Some(pair) = self.field_value_pair(right, left) {
                    return Some(pair);
                }
                None
            }
            _ => None,
        }
    }

    /// Convert Expression to byoridb_common::Value
    pub(super) fn expr_to_value(&self, expr: &Expression) -> Option<byoridb_common::Value> {
        match expr {
            Expression::Literal(lit) => Some(match lit {
                Literal::String(s) => byoridb_common::Value::String(s.clone()),
                Literal::Int(i) => byoridb_common::Value::Int(*i),
                Literal::Float(f) => byoridb_common::Value::Float(*f),
                Literal::Bool(b) => byoridb_common::Value::Bool(*b),
                Literal::Null => byoridb_common::Value::null(),
            }),
            _ => None,
        }
    }

    /// Convert byoridb_common::Value to IndexValue
    pub(super) fn byoridb_value_to_index_value(&self, value: &byoridb_common::Value) -> IndexValue {
        match value {
            byoridb_common::Value::String(s) => IndexValue::String(s.clone()),
            byoridb_common::Value::Int(i) => IndexValue::Int(*i),
            byoridb_common::Value::Float(f) => IndexValue::Float(*f),
            byoridb_common::Value::Bool(b) => IndexValue::Bool(*b),
            _ => IndexValue::Null,
        }
    }

    fn lookup_limit(&self, explicit: Option<usize>) -> Option<usize> {
        explicit.or_else(|| {
            if self.ctx.config.max_scan_limit > 0 {
                Some(self.ctx.config.max_scan_limit)
            } else {
                None
            }
        })
    }

    #[cfg(feature = "distributed")]
    fn lookup_limit_u32(&self, explicit: Option<usize>) -> u32 {
        self.lookup_limit(explicit)
            .and_then(|limit| u32::try_from(limit).ok())
            .unwrap_or(u32::MAX)
    }

    /// Convert byoridb_common::Value to proto IndexValue (for distributed queries)
    #[cfg(feature = "distributed")]
    pub(super) fn byoridb_value_to_proto_index_value(
        &self,
        value: &byoridb_common::Value,
    ) -> ProtoIndexValue {
        use byoridb_storage::proto::storage::index_value::Value;
        match value {
            byoridb_common::Value::String(s) => ProtoIndexValue {
                value: Some(Value::StringValue(s.clone())),
            },
            byoridb_common::Value::Int(i) => ProtoIndexValue {
                value: Some(Value::IntValue(*i)),
            },
            byoridb_common::Value::Float(f) => ProtoIndexValue {
                value: Some(Value::FloatValue(*f)),
            },
            byoridb_common::Value::Bool(b) => ProtoIndexValue {
                value: Some(Value::BoolValue(*b)),
            },
            _ => ProtoIndexValue { value: None },
        }
    }

    /// Execute FIND statement (path finding)
    pub(super) async fn execute_find(&self, plan: crate::plan::FindPlan) -> Result<ExecutorResult> {
        // Resolve parameters
        let from_vid = match self.expr_to_value(&plan.from_vid) {
            Some(byoridb_common::Value::Int(i)) => i,
            _ => {
                return Err(ExecutionError::InvalidOperation(
                    "Invalid FROM VID (must be integer)".to_string(),
                ))
            }
        };

        let to_vid = match self.expr_to_value(&plan.to_vid) {
            Some(byoridb_common::Value::Int(i)) => i,
            _ => {
                return Err(ExecutionError::InvalidOperation(
                    "Invalid TO VID (must be integer)".to_string(),
                ))
            }
        };

        // "*" means all edge types → pass empty slice (get_neighbors treats empty as wildcard)
        let edge_types_owned: Vec<String> = if plan.over_edge == "*" {
            vec![]
        } else {
            vec![plan.over_edge.clone()]
        };
        let edge_types: &[String] = &edge_types_owned;
        let profiling = self.ctx.profiling();
        let find_start = std::time::Instant::now();
        let (path_opt, metrics) = if let Some(weight_prop) = plan.weight_prop.as_deref() {
            let (result, metrics) = crate::algo::dijkstra_shortest_path(
                &self.ctx,
                from_vid,
                to_vid,
                edge_types,
                weight_prop,
            )
            .await?;
            (result.map(|(path, _weight)| path), metrics)
        } else {
            crate::algo::bfs_shortest_path(
                &self.ctx,
                from_vid,
                to_vid,
                edge_types,
                plan.upto_steps.unwrap_or(10) as usize,
            )
            .await?
        };

        tracing::info!(
            visited = metrics.visited_vertices,
            scanned_edges = metrics.scanned_edges,
            decoded_edges = metrics.decoded_edges,
            max_frontier = metrics.max_frontier_size,
            cap_reached = metrics.cap_reached,
            weighted = plan.weight_prop.is_some(),
            "FIND traversal metrics"
        );
        if metrics.cap_reached {
            tracing::warn!(
                max_traversal_nodes = self.ctx.config.max_traversal_nodes,
                "FIND shortest path hit max_traversal_nodes cap; result may be incomplete"
            );
        }

        if profiling {
            let elapsed = find_start.elapsed().as_micros() as u64;
            // The neighbour scan is where FIND spends its time; report the
            // traversal counters there, and the visited frontier on PathFind.
            self.ctx.record_profile(
                ProfileOp::GetNeighbors,
                format!(
                    "scanned {} edges, decoded {}{}",
                    metrics.scanned_edges,
                    metrics.decoded_edges,
                    if metrics.cap_reached {
                        " (CAP HIT)"
                    } else {
                        ""
                    }
                ),
                metrics.scanned_edges,
                elapsed,
                false,
            );
            self.ctx.record_profile(
                ProfileOp::PathFind,
                format!("visited {} vertices", metrics.visited_vertices),
                metrics.visited_vertices,
                elapsed,
                false,
            );
        }

        let columns = vec!["path".to_string()];
        let mut rows = Vec::new();

        if let Some(path) = path_opt {
            // Convert path (Vec<i64>) to Value (List or String representation)
            // Using String for now as List value type might not be fully supported in formatting
            let path_str = path
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join("->");
            rows.push(vec![byoridb_common::Value::String(path_str)]);
        }

        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: 0,
        })
    }
}
