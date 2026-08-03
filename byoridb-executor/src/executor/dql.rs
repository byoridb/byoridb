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
use byoridb_storage::index::RangeOperator;
use byoridb_storage::key::IndexValue;
#[cfg(feature = "distributed")]
use byoridb_storage::proto::storage::IndexValue as ProtoIndexValue;
use std::collections::HashSet;

const INDEX_VERTEX_FETCH_CHUNK_SIZE: usize = 256;

#[derive(Debug, Clone, Copy)]
struct LookupWindow {
    offset: usize,
    limit: Option<usize>,
    fetch_limit: Option<usize>,
    index_limit: Option<usize>,
}

impl LookupWindow {
    fn is_satisfied(self, decoded_rows: usize) -> bool {
        self.fetch_limit
            .is_some_and(|required| decoded_rows >= required)
    }

    fn apply<T>(self, rows: Vec<T>) -> Vec<T> {
        let rows = rows.into_iter().skip(self.offset);
        match self.limit {
            Some(limit) => rows.take(limit).collect(),
            None => rows.collect(),
        }
    }

    #[cfg(feature = "distributed")]
    fn index_limit_u32(self) -> Result<u32> {
        match self.index_limit {
            Some(limit) => u32::try_from(limit).map_err(|_| {
                ExecutionError::InvalidOperation(format!(
                    "LOOKUP OFFSET + LIMIT exceeds the distributed query limit: {limit}"
                ))
            }),
            None => Ok(u32::MAX),
        }
    }
}

fn stable_dedupe_vids(vids: Vec<i64>) -> Vec<i64> {
    let mut seen = HashSet::with_capacity(vids.len());
    vids.into_iter().filter(|vid| seen.insert(*vid)).collect()
}

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

fn vid_value_to_json(value: byoridb_common::Value) -> serde_json::Value {
    match value {
        byoridb_common::Value::Int(value) => serde_json::Value::from(value),
        byoridb_common::Value::String(value) => serde_json::Value::from(value),
        _ => serde_json::Value::Null,
    }
}

fn json_vertex_matches_tag(
    vertex_data: &serde_json::Value,
    tag_name: &str,
    filter_expr: &FilterExpr,
) -> bool {
    let Some(tag) = vertex_data
        .get("tags")
        .and_then(|tags| tags.as_array())
        .and_then(|tags| {
            tags.iter()
                .find(|tag| tag.get("name").and_then(|name| name.as_str()) == Some(tag_name))
        })
    else {
        return false;
    };
    let get_field = |field: &str| -> Option<byoridb_common::Value> {
        let props = tag.get("props")?.as_object()?;
        props
            .get(field)
            .or_else(|| {
                let qualified = format!("{tag_name}.{field}");
                props.get(&qualified)
            })
            .and_then(json_to_value)
    };
    filter_expr.evaluate(&get_field)
}

#[cfg(feature = "distributed")]
fn proto_vertex_matches_tag(
    vertex: &byoridb_storage::proto::storage::VertexData,
    tag_name: &str,
    filter_expr: &FilterExpr,
) -> bool {
    let Some(tag) = vertex.tags.iter().find(|tag| tag.tag_name == tag_name) else {
        return false;
    };
    let get_field = |field: &str| {
        tag.properties
            .get(field)
            .or_else(|| tag.properties.get(&format!("{tag_name}.{field}")))
            .and_then(|bytes| bincode::deserialize::<byoridb_common::Value>(bytes).ok())
    };
    filter_expr.evaluate(&get_field)
}

#[cfg(feature = "distributed")]
fn distributed_lookup_fetch_selection() -> (Vec<String>, Vec<String>) {
    // Empty selectors request every tag/property. LOOKUP returns `<tag>.*`, so
    // narrowing this fetch to the indexed field would silently shrink output.
    (Vec::new(), Vec::new())
}

fn go_expr_needs_dst_vertex(expr: &Expression) -> bool {
    match expr {
        Expression::DstVertexProp { .. } => true,
        Expression::Identifier(name) => name == "vertex",
        _ => false,
    }
}

#[derive(Clone, Copy)]
struct GoYieldRow<'a> {
    space: &'a str,
    src_vid: i64,
    dst_vid: i64,
    last_edge: Option<&'a CodecEdgeData>,
    dst_vertex: Option<&'a CodecVertexData>,
    vid_type: crate::vid::SpaceVidType,
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
        let vid_type = crate::vid::space_vid_type(&self.ctx, effective_space).await?;
        let mut resolved_vids = Vec::with_capacity(plan.vids.len());
        for vid in &plan.vids {
            if let Some(internal) =
                crate::vid::resolve_vid(&self.ctx, effective_space, vid_type, vid, false).await?
            {
                resolved_vids.push(internal);
            }
        }

        // Execute distributed fetch
        let vertices = distributed
            .execute_fetch(
                space_id,
                partition_num,
                resolved_vids,
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
        let mut result_bytes = 0usize; // OOM guard: bound accumulated result memory
        for vertex in vertices {
            // Tag membership: only emit vertices carrying a requested tag, and
            // only the requested tags' data (mirrors execute_fetch_local).
            if !plan.tags.is_empty()
                && !vertex
                    .tags
                    .iter()
                    .any(|t| plan.tags.iter().any(|req| req == &t.tag_name))
            {
                continue;
            }

            let mut row = Vec::new();
            row.push(
                crate::vid::display_vid(&self.ctx, effective_space, vid_type, vertex.vid).await?,
            );

            // Extract tags and their properties
            for tag_data in &vertex.tags {
                if !plan.tags.is_empty() && !plan.tags.iter().any(|req| req == &tag_data.tag_name) {
                    continue;
                }
                let tag_json = serde_json::json!({
                    "name": tag_data.tag_name,
                    "props": tag_data.properties.iter().map(|(k, v)| {
                        let val: serde_json::Value = serde_json::from_slice(v).unwrap_or(serde_json::Value::Null);
                        (k.clone(), val)
                    }).collect::<std::collections::HashMap<_, _>>()
                });
                row.push(byoridb_common::Value::String(tag_json.to_string()));
            }

            result_bytes += crate::context::estimate_row_bytes(&row);
            rows.push(row);
            if rows.len().is_multiple_of(16384) {
                self.ctx.check_result_budget(result_bytes)?;
            }
        }
        self.ctx.check_result_budget(result_bytes)?;

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

        let vid_type = crate::vid::space_vid_type(&self.ctx, effective_space).await?;

        // Resolve VIDs: either literal list or $var.col from context.
        let external_vids: Vec<crate::plan::Vid> = if let Some(ref var_ref) = plan.src_var {
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
                .filter_map(|row| match row.get(col_idx) {
                    Some(byoridb_common::Value::Int(i)) => Some(crate::plan::Vid::Int(*i)),
                    Some(byoridb_common::Value::String(s)) => {
                        Some(crate::plan::Vid::String(s.clone()))
                    }
                    _ => None,
                })
                .collect()
        } else {
            plan.vids.clone()
        };

        let mut resolved_vids = Vec::with_capacity(external_vids.len());
        for vid in &external_vids {
            if let Some(internal) =
                crate::vid::resolve_vid(&self.ctx, effective_space, vid_type, vid, false).await?
            {
                resolved_vids.push(internal);
            }
        }

        // O-8 D5: normalize to sameAs representatives so a fetch of a merged-away
        // vid returns the surviving node that holds the facts.
        for vid in resolved_vids.iter_mut() {
            *vid = crate::ontology::representative_of(&self.ctx, effective_space, *vid).await?;
        }

        // Vertex fetch: batch-get all vertex keys
        let profiling = self.ctx.profiling();
        let fetch_start = std::time::Instant::now();
        let keys: Vec<Vec<u8>> = resolved_vids
            .iter()
            .map(|vid| format!("{}:vertex:{}", effective_space, vid).into_bytes())
            .collect();

        // T-트랙: `AS OF <ts>` 면 이력에서 resolution(빈 payload=tombstone→없음),
        // 아니면 기존 현재뷰 batch_get(무회귀).
        let results: Vec<Option<Vec<u8>>> = if let Some(ts) = plan.as_of {
            let mut out = Vec::with_capacity(keys.len());
            for key in &keys {
                let v = self.ctx.kvstore.get_as_of(key, ts, ts).await?;
                out.push(v.filter(|b| !b.is_empty()));
            }
            out
        } else {
            self.ctx.kvstore.batch_get(&keys).await?
        };

        let mut rows = Vec::new();
        let mut result_bytes = 0usize; // OOM guard: bound accumulated result memory
        for (vid, data_opt) in resolved_vids.iter().zip(results.iter()) {
            if let Some(data) = data_opt {
                if let Ok(vertex_data) = VertexCodec::decode_vertex(data) {
                    // Tag membership: when specific tags are requested, only emit
                    // vertices that actually carry one of them — and only the
                    // requested tags' data. Without this, `FETCH PROP ON product
                    // <vid>` would return a vertex holding only a `sku` tag (bug).
                    if !plan.tags.is_empty()
                        && !vertex_data
                            .tags
                            .iter()
                            .any(|tag| plan.tags.iter().any(|req| req == &tag.name))
                    {
                        continue;
                    }

                    let mut row = Vec::new();
                    row.push(
                        crate::vid::display_vid(&self.ctx, effective_space, vid_type, *vid).await?,
                    );

                    for tag in &vertex_data.tags {
                        if !plan.tags.is_empty() && !plan.tags.iter().any(|req| req == &tag.name) {
                            continue;
                        }
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

                    result_bytes += crate::context::estimate_row_bytes(&row);
                    rows.push(row);
                    if rows.len().is_multiple_of(16384) {
                        self.ctx.check_result_budget(result_bytes)?;
                    }
                }
            }
        }
        self.ctx.check_result_budget(result_bytes)?;

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
        let mut result_bytes = 0usize; // OOM guard: bound accumulated result memory
        let profiling = self.ctx.profiling();
        let fetch_start = std::time::Instant::now();
        let vid_type = crate::vid::space_vid_type(&self.ctx, effective_space).await?;

        for (external_src, external_dst) in &plan.edge_refs {
            let Some(src) =
                crate::vid::resolve_vid(&self.ctx, effective_space, vid_type, external_src, false)
                    .await?
            else {
                continue;
            };
            let Some(dst) =
                crate::vid::resolve_vid(&self.ctx, effective_space, vid_type, external_dst, false)
                    .await?
            else {
                continue;
            };
            // Scan all edges from `src` with the given edge type (ranking = 0 default)
            let prefix = if edge_type == "*" {
                format!("{}:edge:{}:", effective_space, src)
            } else {
                format!("{}:edge:{}:{}:", effective_space, src, edge_type)
            };

            // T-트랙 v2: `AS OF <ts>` 는 현재뷰 대신 이력에서 resolution 한다.
            // 이력에 존재했던 엔티티(삭제돼 현재뷰에 없는 엣지 포함)를 열거하고
            // 각각 (ts, ts) 시점 값을 고른다. 빈 payload = tombstone = 그 시점 부재.
            let values: Vec<Vec<u8>> = if let Some(ts) = plan.as_of {
                let mut vals = Vec::new();
                for ekey in self
                    .ctx
                    .kvstore
                    .scan_history_entity_keys(prefix.as_bytes())
                    .await?
                {
                    if let Some(v) = self.ctx.kvstore.get_as_of(&ekey, ts, ts).await? {
                        if !v.is_empty() {
                            vals.push(v);
                        }
                    }
                }
                vals
            } else {
                self.ctx
                    .kvstore
                    .scan_prefix(prefix.as_bytes())
                    .await?
                    .into_iter()
                    .map(|(_k, v)| v)
                    .collect()
            };

            for value in values {
                match VertexCodec::decode_edge(&value) {
                    Ok(edge) if edge.dst_vid == dst => {
                        let src_value = crate::vid::display_vid(
                            &self.ctx,
                            effective_space,
                            vid_type,
                            edge.src_vid,
                        )
                        .await?;
                        let dst_value = crate::vid::display_vid(
                            &self.ctx,
                            effective_space,
                            vid_type,
                            edge.dst_vid,
                        )
                        .await?;
                        let mut edge_json = VertexCodec::edge_to_json(&edge);
                        edge_json["src"] = vid_value_to_json(src_value.clone());
                        edge_json["dst"] = vid_value_to_json(dst_value.clone());
                        let row = vec![
                            src_value,
                            dst_value,
                            byoridb_common::Value::String(edge_json.to_string()),
                        ];
                        result_bytes += crate::context::estimate_row_bytes(&row);
                        rows.push(row);
                        if rows.len().is_multiple_of(16384) {
                            self.ctx.check_result_budget(result_bytes)?;
                        }
                    }
                    _ => continue,
                }
            }
        }
        self.ctx.check_result_budget(result_bytes)?;

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
        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;
        let vid_type = crate::vid::space_vid_type(&self.ctx, space).await?;
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
                            byoridb_common::Value::Int(i) => {
                                let external = crate::plan::Vid::Int(*i);
                                if let Some(internal) = crate::vid::resolve_vid(
                                    &self.ctx, space, vid_type, &external, false,
                                )
                                .await?
                                {
                                    vids.push(internal);
                                }
                            }
                            byoridb_common::Value::String(s) => {
                                let external = crate::plan::Vid::String(s.clone());
                                if let Some(internal) = crate::vid::resolve_vid(
                                    &self.ctx, space, vid_type, &external, false,
                                )
                                .await?
                                {
                                    vids.push(internal);
                                }
                            }
                            other => {
                                return Err(ExecutionError::InvalidOperation(format!(
                                    "Variable column value is not an integer or string VID: {:?}",
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
            let mut vids = Vec::with_capacity(plan.from_clause.vids.len());
            for external in &plan.from_clause.vids {
                if let Some(internal) =
                    crate::vid::resolve_vid(&self.ctx, space, vid_type, external, false).await?
                {
                    vids.push(internal);
                }
            }
            vids
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
        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;
        let vid_type = crate::vid::space_vid_type(&self.ctx, space).await?;
        let mut rows = Vec::new();
        for edge in edges {
            if let Some(ref key) = edge.key {
                let row = vec![
                    crate::vid::display_vid(&self.ctx, space, vid_type, key.src_vid).await?,
                    crate::vid::display_vid(&self.ctx, space, vid_type, key.dst_vid).await?,
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
        let vid_type = crate::vid::space_vid_type(&self.ctx, space).await?;

        // O-8 D5: normalize source vids to their owl:sameAs representatives so the
        // traversal starts from the node that actually holds the merged facts.
        let mut from_vids = from_vids;
        for vid in from_vids.iter_mut() {
            *vid = crate::ontology::representative_of(&self.ctx, space, *vid).await?;
        }

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

        // Reverse / undirected traversal reads the reverse-edge index
        // (`{space}:in-edge:{dst}:`), an O(in-degree) prefix scan — no longer a
        // space-wide full scan (PLAN.md O-1 done). Tracked only to pick the
        // matching profile operator.
        let is_reverse = !matches!(plan.direction, byoridb_parser::ast::EdgeDirection::Outgoing);
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
            let op = if is_reverse {
                ProfileOp::GetIncoming
            } else {
                ProfileOp::GetNeighbors
            };
            self.ctx.record_profile(
                op,
                format!("over [{}]", plan.over_edges.join(", ")),
                scanned_neighbors,
                prof_start.elapsed().as_micros() as u64,
                false, // reverse-edge index scan, not a full scan
            );
        }

        // Apply the WHERE predicate over edge / dest / source properties. Without
        // this, `GO ... WHERE <cond>` silently returned every neighbor (the plan
        // carried `where_clause` but the local executor never evaluated it).
        if let Some(ref where_expr) = plan.where_clause {
            let mut filtered = Vec::with_capacity(traversal.len());
            for (src_vid, dst_vid, last_edge) in traversal {
                if self
                    .go_row_matches_where(space, src_vid, dst_vid, last_edge.as_ref(), where_expr)
                    .await?
                {
                    filtered.push((src_vid, dst_vid, last_edge));
                }
            }
            traversal = filtered;
        }

        let proj_start = std::time::Instant::now();

        // If no explicit YIELD columns, return ["src", "dst"] for backward
        // compatibility and compound-statement variable resolution.
        if plan.yield_clause.columns.is_empty() {
            let mut rows = Vec::with_capacity(traversal.len());
            for (src, dst, _) in traversal {
                rows.push(vec![
                    crate::vid::display_vid(&self.ctx, space, vid_type, src).await?,
                    crate::vid::display_vid(&self.ctx, space, vid_type, dst).await?,
                ]);
            }
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
                    Expression::FunctionCall { name, args } => match args.first() {
                        Some(Expression::Identifier(a)) => format!("{}({})", name, a),
                        _ => name.clone(),
                    },
                    other => format!("{:?}", other),
                }
            })
            .collect();

        // Destination projections used to perform one point get per result
        // row *and* projected column. Resolve every distinct destination once
        // instead, then evaluate all `$$.tag.prop` / `vertex` expressions from
        // the decoded batch. This is the hot path for LDBC-style fan-out reads.
        let needs_dst_vertices = plan
            .yield_clause
            .columns
            .iter()
            .any(|col| go_expr_needs_dst_vertex(&col.expression));
        let mut dst_vertices: std::collections::HashMap<i64, CodecVertexData> =
            std::collections::HashMap::new();
        if needs_dst_vertices {
            let batch_start = std::time::Instant::now();
            let mut seen = std::collections::HashSet::new();
            let dst_vids: Vec<i64> = traversal
                .iter()
                .map(|(_, dst, _)| *dst)
                .filter(|dst| seen.insert(*dst))
                .collect();
            let keys: Vec<Vec<u8>> = dst_vids
                .iter()
                .map(|dst| format!("{}:vertex:{}", space, dst).into_bytes())
                .collect();
            let blobs = if keys.is_empty() {
                Vec::new()
            } else {
                self.ctx.kvstore.batch_get(&keys).await?
            };
            for (dst, blob) in dst_vids.iter().zip(blobs) {
                if let Some(vertex) = blob.and_then(|data| VertexCodec::decode_vertex(&data).ok()) {
                    dst_vertices.insert(*dst, vertex);
                }
            }
            if profiling {
                self.ctx.record_profile(
                    ProfileOp::GetVertices,
                    format!(
                        "batch destination projection: {} unique vid(s), {} found",
                        dst_vids.len(),
                        dst_vertices.len()
                    ),
                    dst_vertices.len() as u64,
                    batch_start.elapsed().as_micros() as u64,
                    false,
                );
            }
        }

        let mut rows = Vec::with_capacity(traversal.len());
        let mut result_bytes = 0usize; // OOM guard: bound accumulated result memory
        for (src_vid, dst_vid, last_edge) in traversal {
            let mut row = Vec::with_capacity(plan.yield_clause.columns.len());
            let yield_row = GoYieldRow {
                space,
                src_vid,
                dst_vid,
                last_edge: last_edge.as_ref(),
                dst_vertex: dst_vertices.get(&dst_vid),
                vid_type,
            };
            for col in &plan.yield_clause.columns {
                let val = self.eval_go_yield_expr(yield_row, &col.expression).await?;
                row.push(val);
            }
            result_bytes += crate::context::estimate_row_bytes(&row);
            rows.push(row);
            if rows.len().is_multiple_of(16384) {
                self.ctx.check_result_budget(result_bytes)?;
            }
        }
        self.ctx.check_result_budget(result_bytes)?;

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

    /// Evaluate a GO `WHERE` predicate for one traversal row. The predicate may
    /// reference edge properties (bare `prop` or `edgetype.prop`), destination
    /// vertex properties (`$$.tag.prop`), and source vertex properties
    /// (`$^.tag.prop`). Errors (e.g. an unresolvable reference) propagate so a
    /// malformed predicate fails the query rather than being silently ignored.
    async fn go_row_matches_where(
        &self,
        space: &str,
        src_vid: i64,
        dst_vid: i64,
        last_edge: Option<&CodecEdgeData>,
        where_expr: &Expression,
    ) -> Result<bool> {
        let mut current: std::collections::HashMap<String, byoridb_common::Value> =
            std::collections::HashMap::new();
        if let Some(e) = last_edge {
            for (k, v) in &e.properties {
                current.insert(format!("{}.{}", e.edge_type, k), v.clone());
                current.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        let mut ctx = crate::evaluator::EvalContext::new();
        ctx.current = current;
        ctx.destination = self.vertex_props_flat(space, dst_vid).await;
        ctx.source = self.vertex_props_flat(space, src_vid).await;
        crate::evaluator::Evaluator::evaluate_condition(where_expr, &ctx)
    }

    /// Fetch a vertex's properties as a flat map, keyed both as `tag.prop` and as
    /// bare `prop` (first tag wins for the bare key). Used to build the GO WHERE
    /// evaluation context for destination/source vertices.
    async fn vertex_props_flat(
        &self,
        space: &str,
        vid: i64,
    ) -> std::collections::HashMap<String, byoridb_common::Value> {
        let mut out = std::collections::HashMap::new();
        let key = format!("{}:vertex:{}", space, vid);
        if let Ok(Some(blob)) = self.ctx.kvstore.get(key.as_bytes()).await {
            if let Ok(v) = VertexCodec::decode_vertex(&blob) {
                for tag in &v.tags {
                    for (p, val) in &tag.properties {
                        out.insert(format!("{}.{}", tag.name, p), val.clone());
                        out.entry(p.clone()).or_insert_with(|| val.clone());
                    }
                }
            }
        }
        out
    }

    /// Evaluate a single YIELD expression in the context of a GO traversal row.
    async fn eval_go_yield_expr(
        &self,
        row: GoYieldRow<'_>,
        expr: &Expression,
    ) -> Result<byoridb_common::Value> {
        let GoYieldRow {
            space,
            src_vid,
            dst_vid,
            last_edge,
            dst_vertex,
            vid_type,
        } = row;
        let value = match expr {
            Expression::Identifier(name) => match name.as_str() {
                "src" | "_src_vid" => {
                    crate::vid::display_vid(&self.ctx, space, vid_type, src_vid).await?
                }
                "dst" | "_dst_vid" => {
                    crate::vid::display_vid(&self.ctx, space, vid_type, dst_vid).await?
                }
                "vertex" => match dst_vertex {
                    Some(vertex) => {
                        let mut json = VertexCodec::vertex_to_json(vertex);
                        json["vid"] = vid_value_to_json(
                            crate::vid::display_vid(&self.ctx, space, vid_type, dst_vid).await?,
                        );
                        byoridb_common::Value::String(json.to_string())
                    }
                    None => byoridb_common::Value::Null(byoridb_common::NullType::Null),
                },
                // `edge` refers to the edge currently being traversed. Needed for
                // `OVER *` where a typed prefix like `has_brand._dst` isn't usable
                // (the edge type varies per row). Serialised as JSON like `vertex`.
                "edge" => match last_edge {
                    Some(e) => {
                        let src =
                            crate::vid::display_vid(&self.ctx, space, vid_type, e.src_vid).await?;
                        let dst =
                            crate::vid::display_vid(&self.ctx, space, vid_type, e.dst_vid).await?;
                        byoridb_common::Value::String(
                            serde_json::json!({
                                "src": vid_value_to_json(src),
                                "dst": vid_value_to_json(dst),
                                "type": e.edge_type,
                                "rank": e.ranking,
                            })
                            .to_string(),
                        )
                    }
                    None => byoridb_common::Value::Null(byoridb_common::NullType::Null),
                },
                _ => byoridb_common::Value::Null(byoridb_common::NullType::Null),
            },
            Expression::PropRef { object: _, prop } => {
                if let Some(edge) = last_edge {
                    match prop.as_str() {
                        "_dst" | "dst_id" => {
                            crate::vid::display_vid(&self.ctx, space, vid_type, edge.dst_vid)
                                .await?
                        }
                        "_src" | "src_id" => {
                            crate::vid::display_vid(&self.ctx, space, vid_type, edge.src_vid)
                                .await?
                        }
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
            Expression::DstVertexProp { tag, prop } => dst_vertex
                .into_iter()
                .flat_map(|vertex| vertex.tags.iter())
                .find(|t| t.name.eq_ignore_ascii_case(tag))
                .and_then(|t| t.properties.get(prop))
                .cloned()
                .unwrap_or(byoridb_common::Value::Null(byoridb_common::NullType::Null)),
            Expression::Literal(lit) => match lit {
                Literal::Int(i) => byoridb_common::Value::Int(*i),
                Literal::Float(f) => byoridb_common::Value::Float(*f),
                Literal::String(s) => byoridb_common::Value::String(s.clone()),
                Literal::Bool(b) => byoridb_common::Value::Bool(*b),
                Literal::Null => byoridb_common::Value::Null(byoridb_common::NullType::Null),
            },
            // Edge accessor functions in a GO row context: type(edge), dst(edge),
            // src(edge), rank(edge). The argument identifies the edge but in GO
            // there is exactly one edge per row (`last_edge`), so it is implicit.
            // Previously these fell through to the catch-all and yielded NULL —
            // breaking `OVER * YIELD type(edge)`.
            Expression::FunctionCall { name, .. } => {
                match (name.to_lowercase().as_str(), last_edge) {
                    ("type", Some(e)) => byoridb_common::Value::String(e.edge_type.clone()),
                    ("dst", Some(e)) => {
                        crate::vid::display_vid(&self.ctx, space, vid_type, e.dst_vid).await?
                    }
                    ("src", Some(e)) => {
                        crate::vid::display_vid(&self.ctx, space, vid_type, e.src_vid).await?
                    }
                    ("rank" | "ranking", Some(e)) => byoridb_common::Value::Int(e.ranking),
                    _ => byoridb_common::Value::Null(byoridb_common::NullType::Null),
                }
            }
            _ => byoridb_common::Value::Null(byoridb_common::NullType::Null),
        };
        Ok(value)
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
        let vid_type = crate::vid::space_vid_type(&self.ctx, space).await?;

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

        // The parser emits `LookupType::Tag` for `LOOKUP ON <name>` (it has no
        // schema to tell tag from edge). If <name> is actually an edge type, the
        // tag-only path below would silently return an empty/tag-filtered result.
        // Reject it clearly — full edge LOOKUP is not implemented yet.
        if matches!(plan.lookup_type, crate::plan::LookupType::Tag(_)) {
            let is_tag = self
                .ctx
                .kvstore
                .get(&crate::key::SchemaKey::tag(space, &tag_or_edge_name))
                .await?
                .is_some();
            if !is_tag {
                let is_edge = self
                    .ctx
                    .kvstore
                    .get(&crate::key::SchemaKey::edge(space, &tag_or_edge_name))
                    .await?
                    .is_some();
                if is_edge {
                    return Err(ExecutionError::InvalidOperation(format!(
                        "LOOKUP ON edge type '{}' is not supported yet — only tag LOOKUP is \
                         implemented",
                        tag_or_edge_name
                    )));
                }
            }
        }

        let lookup_window = self.lookup_window(plan.offset, plan.limit)?;
        if lookup_window.limit == Some(0) {
            return Ok(ExecutorResult {
                columns: result_columns,
                rows: vec![],
                latency_ms: 0,
            });
        }
        #[cfg(feature = "distributed")]
        if self.ctx.is_distributed()
            && plan
                .where_clause
                .as_ref()
                .is_some_and(super::expression_contains_ordered_range)
        {
            return Err(ExecutionError::InvalidOperation(
                super::DISTRIBUTED_LOOKUP_RANGE_UNSUPPORTED.to_string(),
            ));
        }
        #[cfg(feature = "distributed")]
        let lookup_limit_u32 = lookup_window.index_limit_u32()?;

        // Try index-based lookup first
        if let Some(ref index_manager) = self.ctx.index_manager {
            if let Some(ref where_expr) = plan.where_clause {
                if let Some(result) = self
                    .try_execute_range_index_lookup(
                        index_manager,
                        where_expr,
                        space.as_str(),
                        &tag_or_edge_name,
                        lookup_window,
                    )
                    .await?
                {
                    return Ok(result);
                }

                // Try to extract indexable condition
                if let Some((field, value)) = self.extract_eq_condition(where_expr) {
                    // Find applicable index
                    let space_id = self.ctx.resolve_space_id().await;
                    let indexes = index_manager.list_tag_indexes(space_id).await;

                    // Find an index that covers the field
                    if let Some(index_def) = indexes.iter().find(|idx| {
                        idx.schema_name.eq_ignore_ascii_case(&tag_or_edge_name)
                            && idx.fields.len() == 1
                            && idx.fields[0] == field
                    }) {
                        tracing::debug!(
                            "Using index '{}' for LOOKUP on field '{}'",
                            index_def.index_name,
                            field
                        );

                        // Convert value to IndexValue
                        let index_value = self.byoridb_value_to_index_value(&value);
                        let index_filter = FilterExpr::eq(field.clone(), value.clone());

                        // Check if distributed mode is enabled
                        #[cfg(feature = "distributed")]
                        if self.ctx.is_distributed() {
                            if self.ctx.get_distributed_executor().is_none() {
                                return Err(ExecutionError::InvalidOperation(
                                    "Distributed LOOKUP index executor is unavailable".to_string(),
                                ));
                            }
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
                                        let candidate_cap_reached = lookup_window
                                            .index_limit
                                            .is_some_and(|limit| vids.len() >= limit);
                                        let vids = stable_dedupe_vids(vids);
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

                                        // Fetch full vertices in bounded chunks. Index candidate
                                        // scans intentionally overfetch so stale entries cannot
                                        // under-fill OFFSET/LIMIT, but one small result must not
                                        // materialize every candidate's full property payload.
                                        let mut rows = Vec::new();
                                        'fetch_chunks: for vid_chunk in
                                            vids.chunks(INDEX_VERTEX_FETCH_CHUNK_SIZE)
                                        {
                                            // The selectors stay empty because the result projects
                                            // `<tag>.*`.
                                            let (tag_names, prop_names) =
                                                distributed_lookup_fetch_selection();
                                            let vertices = distributed_executor
                                                .execute_fetch(
                                                    space_id,
                                                    partition_num,
                                                    vid_chunk.to_vec(),
                                                    tag_names,
                                                    prop_names,
                                                )
                                                .await
                                                .map_err(|error| {
                                                    ExecutionError::InvalidOperation(format!(
                                                        "Distributed vertex fetch failed: {error}"
                                                    ))
                                                })?;
                                            for vertex in vertices {
                                                if !proto_vertex_matches_tag(
                                                    &vertex,
                                                    &tag_or_edge_name,
                                                    &index_filter,
                                                ) {
                                                    continue;
                                                }
                                                let mut row = Vec::new();
                                                row.push(
                                                    crate::vid::display_vid(
                                                        &self.ctx, space, vid_type, vertex.vid,
                                                    )
                                                    .await?,
                                                );
                                                // Format tags as string
                                                let tags_str = vertex
                                                    .tags
                                                    .iter()
                                                    .map(|t| {
                                                        format!("{}:{:?}", t.tag_name, t.properties)
                                                    })
                                                    .collect::<Vec<_>>()
                                                    .join(",");
                                                row.push(byoridb_common::Value::String(tags_str));
                                                rows.push(row);
                                                if lookup_window.is_satisfied(rows.len()) {
                                                    break 'fetch_chunks;
                                                }
                                            }
                                        }
                                        self.ensure_index_window_satisfied(
                                            lookup_window,
                                            candidate_cap_reached,
                                            rows.len(),
                                        )?;
                                        let rows = lookup_window.apply(rows);
                                        return Ok(ExecutorResult {
                                            columns: result_columns,
                                            rows,
                                            latency_ms: 0,
                                        });
                                    }
                                    Err(e) => {
                                        return Err(ExecutionError::InvalidOperation(format!(
                                            "Distributed index lookup failed: {e}"
                                        )));
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
                        let mut candidate_cap_reached = false;
                        let mut last_err: Option<byoridb_storage::IndexError> = None;
                        for part_id in 1..=partition_num {
                            match index_manager
                                .lookup_tag(
                                    part_id,
                                    index_def,
                                    std::slice::from_ref(&index_value),
                                    lookup_window.index_limit.unwrap_or(usize::MAX),
                                )
                                .await
                            {
                                Ok(part_vids) => {
                                    candidate_cap_reached |= lookup_window
                                        .index_limit
                                        .is_some_and(|limit| part_vids.len() >= limit);
                                    vids.extend(part_vids);
                                }
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

                        if last_err.is_some() {
                            // A single failed partition makes the accumulated
                            // VIDs incomplete. Discard them and use the complete
                            // predicate-scan fallback instead of returning a
                            // partial result.
                        } else {
                            let vids = stable_dedupe_vids(vids);
                            if idx_profiling {
                                self.ctx.record_profile(
                                    ProfileOp::IndexScan,
                                    format!("index '{}'", index_def.index_name),
                                    vids.len() as u64,
                                    idx_start.elapsed().as_micros() as u64,
                                    false,
                                );
                            }
                            tracing::debug!(
                                "Index lookup returned {} candidates across {} partitions",
                                vids.len(),
                                partition_num
                            );
                            return self
                                .build_local_tag_index_result(
                                    space.as_str(),
                                    &tag_or_edge_name,
                                    &index_filter,
                                    vids,
                                    lookup_window,
                                    candidate_cap_reached,
                                )
                                .await;
                        }
                    }
                }
            }
        }

        self.execute_lookup_scan(
            &plan,
            space,
            vid_type,
            tag_or_edge_name,
            result_columns,
            lookup_window,
        )
        .await
    }

    /// LOOKUP fallback when no usable local or distributed index produced a
    /// result. Keeping scan decoding and result shaping here also keeps the
    /// index-selection path small enough to audit independently.
    async fn execute_lookup_scan(
        &self,
        plan: &crate::plan::LookupPlan,
        space: &str,
        vid_type: crate::vid::SpaceVidType,
        tag_or_edge_name: String,
        result_columns: Vec<String>,
        lookup_window: LookupWindow,
    ) -> Result<ExecutorResult> {
        // Fallback: Scan with predicate pushdown
        #[cfg(feature = "distributed")]
        if self.ctx.is_distributed() {
            return Err(ExecutionError::InvalidOperation(
                super::DISTRIBUTED_LOOKUP_FULL_SCAN_UNSUPPORTED.to_string(),
            ));
        }
        tracing::debug!("Using scan with predicate pushdown for LOOKUP (no suitable index found)");
        self.ctx.mark_full_scan();
        let scan_profiling = self.ctx.profiling();
        let scan_start = std::time::Instant::now();

        let vertex_prefix = format!("{}:vertex:", space);
        // scan_with_filter applies this cap after the predicate matches. Index
        // candidates use the separate safety cap because stale entries may be
        // discarded while vertex rows are decoded.
        let filtered_scan_limit = lookup_window.fetch_limit;

        // Convert WHERE clause to FilterExpr for predicate pushdown. A predicate
        // the pushdown can't express (e.g. CONTAINS / STARTS WITH / regex, or a
        // field-to-field comparison) must NOT silently become `True` — that
        // returned every row (fail-open). Reject it with a clear error instead.
        let filter_expr = match plan.where_clause.as_ref() {
            None => FilterExpr::True,
            Some(expr) => self.expr_to_filter_expr(expr).ok_or_else(|| {
                ExecutionError::InvalidOperation(format!(
                    "unsupported LOOKUP predicate — only ==, !=, <, <=, >, >=, AND, OR, NOT \
                     over a field and a literal are supported here: {:?}",
                    expr
                ))
            })?,
        };

        let tag_name_filter = tag_or_edge_name.clone();

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

            json_vertex_matches_tag(&vertex_data, &tag_name_filter, &filter_expr)
        });

        // Use scan_with_filter for predicate pushdown
        let results = self
            .ctx
            .kvstore
            .scan_with_filter(vertex_prefix.as_bytes(), filter_fn, filtered_scan_limit)
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
        let mut result_bytes = 0usize; // OOM guard: bound accumulated result memory
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
            row.push(crate::vid::display_vid(&self.ctx, space, vid_type, vid).await?);
            if !tag_str.is_empty() {
                row.push(byoridb_common::Value::String(tag_str));
            }
            result_bytes += crate::context::estimate_row_bytes(&row);
            rows.push(row);
            if rows.len().is_multiple_of(16384) {
                self.ctx.check_result_budget(result_bytes)?;
            }
        }
        self.ctx.check_result_budget(result_bytes)?;

        let rows = lookup_window.apply(rows);
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
        let value = Self::expr_to_value(value_expr)?;
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
                BinaryOperator::Lt
                | BinaryOperator::Lte
                | BinaryOperator::Gt
                | BinaryOperator::Gte => {
                    let (field, value, operator) = self.extract_range_condition(expr)?;
                    Some(match operator {
                        RangeOperator::GreaterThan => FilterExpr::gt(field, value),
                        RangeOperator::GreaterThanOrEqual => FilterExpr::ge(field, value),
                        RangeOperator::LessThan => FilterExpr::lt(field, value),
                        RangeOperator::LessThanOrEqual => FilterExpr::le(field, value),
                    })
                }
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

    /// Extract a simple ordered range predicate and normalize it so the field
    /// is always on the left. For example, `30 < person.age` becomes
    /// `person.age > 30`.
    pub(super) fn extract_range_condition(
        &self,
        expr: &Expression,
    ) -> Option<(String, byoridb_common::Value, RangeOperator)> {
        let Expression::BinaryOp { op, left, right } = expr else {
            return None;
        };

        let direct = match op {
            BinaryOperator::Gt => RangeOperator::GreaterThan,
            BinaryOperator::Gte => RangeOperator::GreaterThanOrEqual,
            BinaryOperator::Lt => RangeOperator::LessThan,
            BinaryOperator::Lte => RangeOperator::LessThanOrEqual,
            _ => return None,
        };
        if let Some((field, value)) = self.field_value_pair(left, right) {
            return Some((field, value, direct));
        }

        let reversed = match direct {
            RangeOperator::GreaterThan => RangeOperator::LessThan,
            RangeOperator::GreaterThanOrEqual => RangeOperator::LessThanOrEqual,
            RangeOperator::LessThan => RangeOperator::GreaterThan,
            RangeOperator::LessThanOrEqual => RangeOperator::GreaterThanOrEqual,
        };
        self.field_value_pair(right, left)
            .map(|(field, value)| (field, value, reversed))
    }

    async fn try_execute_range_index_lookup(
        &self,
        index_manager: &byoridb_storage::IndexManager,
        where_expr: &Expression,
        space: &str,
        tag: &str,
        lookup_window: LookupWindow,
    ) -> Result<Option<ExecutorResult>> {
        // String index encoding is length-prefixed and therefore not globally
        // lexical. Unsupported or cross-type boundaries return None and keep
        // the correctness-preserving predicate-pushdown fallback.
        let Some((field, value, operator)) = self.extract_range_condition(where_expr) else {
            return Ok(None);
        };
        let filter_expr = match operator {
            RangeOperator::GreaterThan => FilterExpr::gt(field.clone(), value.clone()),
            RangeOperator::GreaterThanOrEqual => FilterExpr::ge(field.clone(), value.clone()),
            RangeOperator::LessThan => FilterExpr::lt(field.clone(), value.clone()),
            RangeOperator::LessThanOrEqual => FilterExpr::le(field.clone(), value.clone()),
        };
        #[cfg(feature = "distributed")]
        if self.ctx.is_distributed() {
            return Err(ExecutionError::InvalidOperation(
                super::DISTRIBUTED_LOOKUP_RANGE_UNSUPPORTED.to_string(),
            ));
        }
        let space_id = self.ctx.resolve_space_id().await;
        let indexes = index_manager.list_tag_indexes(space_id).await;
        let Some(index_def) = indexes.iter().find(|index| {
            index.schema_name.eq_ignore_ascii_case(tag)
                && index.fields.len() == 1
                && index.fields[0] == field
        }) else {
            return Ok(None);
        };
        let Some(index_value) =
            super::range_index_boundary(&self.ctx, space, tag, &field, &value).await
        else {
            return Ok(None);
        };

        let partition_num = self.resolve_local_partition_num(space).await;
        let profiling = self.ctx.profiling();
        let start = std::time::Instant::now();
        let mut vids = Vec::new();
        let mut candidate_cap_reached = false;
        for part_id in 1..=partition_num {
            match index_manager
                .lookup_tag_range(
                    part_id,
                    index_def,
                    index_value.clone(),
                    operator,
                    lookup_window.index_limit.unwrap_or(usize::MAX),
                )
                .await
            {
                Ok(part_vids) => {
                    candidate_cap_reached |= lookup_window
                        .index_limit
                        .is_some_and(|limit| part_vids.len() >= limit);
                    vids.extend(part_vids);
                }
                Err(error) => {
                    tracing::warn!(
                        "Local range index lookup on part {} failed; discarding all partial results: {}",
                        part_id,
                        error
                    );
                    return Ok(None);
                }
            }
        }
        let vids = stable_dedupe_vids(vids);

        if profiling {
            self.ctx.record_profile(
                ProfileOp::IndexScan,
                format!("range index '{}'", index_def.index_name),
                vids.len() as u64,
                start.elapsed().as_micros() as u64,
                false,
            );
        }
        self.build_local_tag_index_result(
            space,
            tag,
            &filter_expr,
            vids,
            lookup_window,
            candidate_cap_reached,
        )
        .await
        .map(Some)
    }

    async fn build_local_tag_index_result(
        &self,
        space: &str,
        tag: &str,
        filter_expr: &FilterExpr,
        vids: Vec<i64>,
        lookup_window: LookupWindow,
        candidate_cap_reached: bool,
    ) -> Result<ExecutorResult> {
        let vid_type = crate::vid::space_vid_type(&self.ctx, space).await?;
        let result_columns = vec![format!("{tag}.vid"), format!("{tag}.*")];
        if vids.is_empty() {
            return Ok(ExecutorResult {
                columns: result_columns,
                rows: vec![],
                latency_ms: 0,
            });
        }

        let mut rows = Vec::new();
        'fetch_chunks: for vid_chunk in vids.chunks(INDEX_VERTEX_FETCH_CHUNK_SIZE) {
            let keys: Vec<Vec<u8>> = vid_chunk
                .iter()
                .map(|vid| format!("{}:vertex:{}", space, vid).into_bytes())
                .collect();
            let results = self.ctx.kvstore.batch_get(&keys).await?;
            for (vid, data) in vid_chunk.iter().zip(results.iter()) {
                let Some(data) = data else {
                    continue;
                };
                let vertex_data = if VertexCodec::is_proto_format(data) {
                    match VertexCodec::decode_vertex(data) {
                        Ok(vertex) => VertexCodec::vertex_to_json(&vertex),
                        Err(error) => {
                            tracing::warn!(
                                "Skipping undecodable vertex {} in tag index lookup: {}",
                                vid,
                                error
                            );
                            continue;
                        }
                    }
                } else {
                    serde_json::from_slice(data)?
                };
                if !json_vertex_matches_tag(&vertex_data, tag, filter_expr) {
                    continue;
                }
                let mut row =
                    vec![crate::vid::display_vid(&self.ctx, space, vid_type, *vid).await?];
                if let Some(tags) = vertex_data.get("tags") {
                    row.push(byoridb_common::Value::String(tags.to_string()));
                }
                rows.push(row);
                if lookup_window.is_satisfied(rows.len()) {
                    break 'fetch_chunks;
                }
            }
        }

        self.ensure_index_window_satisfied(lookup_window, candidate_cap_reached, rows.len())?;
        let rows = lookup_window.apply(rows);
        Ok(ExecutorResult {
            columns: result_columns,
            rows,
            latency_ms: 0,
        })
    }

    /// Convert Expression to byoridb_common::Value
    pub(super) fn expr_to_value(expr: &Expression) -> Option<byoridb_common::Value> {
        match expr {
            Expression::Literal(lit) => Some(match lit {
                Literal::String(s) => byoridb_common::Value::String(s.clone()),
                Literal::Int(i) => byoridb_common::Value::Int(*i),
                Literal::Float(f) => byoridb_common::Value::Float(*f),
                Literal::Bool(b) => byoridb_common::Value::Bool(*b),
                Literal::Null => byoridb_common::Value::null(),
            }),
            // Negative numeric literals arrive as `-(N)` after parsing.
            Expression::UnaryOp {
                op: byoridb_parser::ast::UnaryOperator::Neg,
                operand,
            } => match Self::expr_to_value(operand) {
                Some(byoridb_common::Value::Int(i)) => Some(byoridb_common::Value::Int(-i)),
                Some(byoridb_common::Value::Float(f)) => Some(byoridb_common::Value::Float(-f)),
                _ => None,
            },
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

    fn lookup_window(
        &self,
        offset: Option<usize>,
        explicit: Option<usize>,
    ) -> Result<LookupWindow> {
        let offset = offset.unwrap_or(0);
        let limit = explicit.or_else(|| {
            if self.ctx.config.max_scan_limit > 0 {
                Some(self.ctx.config.max_scan_limit)
            } else {
                None
            }
        });
        let fetch_limit = limit
            .map(|limit| {
                offset.checked_add(limit).ok_or_else(|| {
                    ExecutionError::InvalidOperation(
                        "LOOKUP OFFSET + LIMIT exceeds the platform size".to_string(),
                    )
                })
            })
            .transpose()?;
        let index_limit = if self.ctx.config.max_scan_limit > 0 {
            Some(fetch_limit.unwrap_or(0).max(self.ctx.config.max_scan_limit))
        } else {
            None
        };
        Ok(LookupWindow {
            offset,
            limit,
            fetch_limit,
            index_limit,
        })
    }

    fn ensure_index_window_satisfied(
        &self,
        lookup_window: LookupWindow,
        candidate_cap_reached: bool,
        decoded_rows: usize,
    ) -> Result<()> {
        if candidate_cap_reached
            && lookup_window
                .fetch_limit
                .is_some_and(|required| decoded_rows < required)
        {
            return Err(ExecutionError::ResourceExhausted(format!(
                "LOOKUP index candidate scan reached its safety cap after decoding {decoded_rows} rows"
            )));
        }
        Ok(())
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
        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;
        let vid_type = crate::vid::space_vid_type(&self.ctx, space).await?;

        // Resolve integer or string parameters through the selected space's VID contract.
        let from_external = match Self::expr_to_value(&plan.from_vid) {
            Some(byoridb_common::Value::Int(i)) => crate::plan::Vid::Int(i),
            Some(byoridb_common::Value::String(s)) => crate::plan::Vid::String(s),
            _ => {
                return Err(ExecutionError::InvalidOperation(
                    "Invalid FROM VID (must be an integer or string literal)".to_string(),
                ))
            }
        };
        let to_external = match Self::expr_to_value(&plan.to_vid) {
            Some(byoridb_common::Value::Int(i)) => crate::plan::Vid::Int(i),
            Some(byoridb_common::Value::String(s)) => crate::plan::Vid::String(s),
            _ => {
                return Err(ExecutionError::InvalidOperation(
                    "Invalid TO VID (must be an integer or string literal)".to_string(),
                ))
            }
        };
        let Some(from_vid) =
            crate::vid::resolve_vid(&self.ctx, space, vid_type, &from_external, false).await?
        else {
            return Ok(ExecutorResult {
                columns: vec!["path".to_string()],
                rows: Vec::new(),
                latency_ms: 0,
            });
        };
        let Some(to_vid) =
            crate::vid::resolve_vid(&self.ctx, space, vid_type, &to_external, false).await?
        else {
            return Ok(ExecutorResult {
                columns: vec!["path".to_string()],
                rows: Vec::new(),
                latency_ms: 0,
            });
        };

        // "*" means all edge types → pass empty slice (get_neighbors treats empty as wildcard)
        let edge_types_owned: Vec<String> = if plan.over_edge == "*" {
            vec![]
        } else {
            vec![plan.over_edge.clone()]
        };
        let edge_types: &[String] = &edge_types_owned;

        // UPTO follows the same cap as GO steps (S-5).
        let max_go_steps = self.ctx.config.max_go_steps;
        if let Some(upto) = plan.upto_steps {
            if max_go_steps > 0 && upto > max_go_steps {
                return Err(ExecutionError::InvalidOperation(format!(
                    "UPTO {} STEPS exceeds the maximum of {}",
                    upto, max_go_steps
                )));
            }
        }
        let max_steps = plan.upto_steps.unwrap_or(10) as usize;
        let all_shortest = matches!(plan.find_type, crate::plan::FindType::AllShortestPaths);

        let profiling = self.ctx.profiling();
        let find_start = std::time::Instant::now();
        let (paths, metrics) = if let Some(weight_prop) = plan.weight_prop.as_deref() {
            if plan.bidirect || all_shortest {
                return Err(ExecutionError::InvalidOperation(
                    "WEIGHT BY cannot be combined with BIDIRECT or ALL SHORTEST PATHS".to_string(),
                ));
            }
            let (result, metrics) = crate::algo::dijkstra_shortest_path(
                &self.ctx,
                from_vid,
                to_vid,
                edge_types,
                weight_prop,
            )
            .await?;
            (
                result.map(|(path, _weight)| path).into_iter().collect(),
                metrics,
            )
        } else if all_shortest {
            let max_paths = self.ctx.config.max_find_paths;
            let (paths, metrics) = crate::algo_paths::all_shortest_paths(
                &self.ctx,
                from_vid,
                to_vid,
                edge_types,
                max_steps,
                plan.bidirect,
                max_paths,
            )
            .await?;
            if max_paths > 0 && paths.len() >= max_paths {
                tracing::warn!(
                    max_find_paths = max_paths,
                    "FIND ALL SHORTEST PATHS hit max_find_paths; result is truncated"
                );
            }
            (paths, metrics)
        } else {
            let (path, metrics) = crate::algo_paths::shortest_path(
                &self.ctx,
                from_vid,
                to_vid,
                edge_types,
                max_steps,
                plan.bidirect,
            )
            .await?;
            (path.into_iter().collect(), metrics)
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
        // One row per path; the path is a list of vids so clients can read
        // hop count and intermediate vertices without string parsing.
        let mut rows = Vec::with_capacity(paths.len());
        for path in paths {
            let mut displayed = Vec::with_capacity(path.len());
            for vid in path {
                displayed.push(crate::vid::display_vid(&self.ctx, space, vid_type, vid).await?);
            }
            rows.push(vec![byoridb_common::Value::List(
                byoridb_common::datatypes::list::List::with_values(displayed),
            )]);
        }

        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: 0,
        })
    }
}

#[cfg(all(test, feature = "distributed"))]
mod distributed_lookup_tests {
    use super::*;
    use byoridb_storage::proto::storage::{TagData, VertexData};
    use std::collections::HashMap;

    #[test]
    fn equality_revalidation_keeps_full_vertex_projection() {
        let age = byoridb_common::Value::Int(30);
        let name = byoridb_common::Value::String("Alice".to_string());
        let vertex = VertexData {
            vid: 1,
            tags: vec![TagData {
                tag_name: "person".to_string(),
                properties: HashMap::from([
                    ("age".to_string(), bincode::serialize(&age).unwrap()),
                    ("name".to_string(), bincode::serialize(&name).unwrap()),
                ]),
            }],
        };

        let (tag_names, prop_names) = distributed_lookup_fetch_selection();
        assert!(tag_names.is_empty());
        assert!(prop_names.is_empty());
        assert!(proto_vertex_matches_tag(
            &vertex,
            "person",
            &FilterExpr::eq("age", age)
        ));
        assert!(vertex.tags[0].properties.contains_key("name"));
    }
}
