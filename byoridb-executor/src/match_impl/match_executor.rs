// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Cypher-like MATCH execution
//!
//! This module consumes the structured `byoridb_parser::ast::Pattern` carried
//! by `MatchPlan` and walks the stored graph, applying label and property
//! filters via [`PatternMatcher::matches_node`] / `matches_edge_data`.

use super::pattern_matcher::PatternMatcher;
use crate::algo;
use crate::context::ExecutionContext;
use crate::error::{ExecutionError, Result};
use crate::executor::ExecutorResult;
use crate::profile::ProfileOp;
use byoridb_codec::{EdgeData, VertexCodec};
use byoridb_parser::ast::{BinaryOperator, EdgePattern, Expression, Literal, NodePattern, Pattern};
use byoridb_storage::index::IndexDef;
use byoridb_storage::key::IndexValue;
use std::collections::HashMap;
use std::sync::Arc;

pub struct MatchExecutor {
    ctx: Arc<ExecutionContext>,
}

impl MatchExecutor {
    pub fn new(ctx: Arc<ExecutionContext>) -> Self {
        Self { ctx }
    }

    /// Execute a MATCH query
    pub async fn execute_match(&self, plan: crate::plan::MatchPlan) -> Result<ExecutorResult> {
        let start = std::time::Instant::now();

        let space = self
            .ctx
            .space
            .clone()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;

        let flat = flatten_pattern(&plan.pattern)?;
        let matcher = PatternMatcher::new(self.ctx.clone());

        if let Some((columns, rows)) = self
            .try_execute_simple_count(&plan, &space, &matcher)
            .await?
        {
            return Ok(ExecutorResult {
                columns,
                rows,
                latency_ms: start.elapsed().as_millis() as u64,
            });
        }

        // Collect rows as binding maps: variable_name → Value::Int(vid)
        let mut binding_rows: Vec<HashMap<String, byoridb_common::Value>> = Vec::new();
        let mut bindings: HashMap<String, byoridb_common::Value> = HashMap::new();

        // Optimisation: detect WHERE id(end_var)==X for single-edge patterns.
        // Instead of scanning all start-node candidates and traversing forward,
        // we call get_incoming_neighbors(X) once — an O(in-degree) reverse-edge
        // index lookup vs N forward prefix scans (N = candidate count, 100K+).
        let id_bindings = plan
            .where_clause
            .as_ref()
            .map(extract_id_eq_bindings)
            .unwrap_or_default();

        let used_reverse = if flat.edges.len() == 1 && plan.optional_patterns.is_empty() {
            let end_node = flat.nodes[0];
            let end_var = end_node.variable.as_deref().unwrap_or("");
            if let Some(&end_vid) = id_bindings.get(end_var) {
                self.match_reverse_single_edge(
                    &space,
                    &flat,
                    end_vid,
                    end_var,
                    &matcher,
                    &mut bindings,
                    &mut binding_rows,
                )
                .await?;
                true
            } else {
                false
            }
        } else {
            false
        };

        // Comma-separated multi-patterns (`(a)-[]->(b), (a)-[]->(c)`) need ALL
        // first-pattern rows before the inner join, so the early-exit limit is
        // unsafe — LIMIT is applied after the join, at the end.
        let is_multiple = matches!(&plan.pattern, Pattern::Multiple(ps) if ps.len() > 1);

        if !used_reverse {
            // Compute an early-exit limit: offset + limit rows are enough to
            // satisfy LIMIT after OFFSET without materializing all candidates.
            // Only safe when there is no WHERE clause (post-filter could reduce
            // the count) and not a multi-pattern join.
            let row_limit = if plan.where_clause.is_none() && !is_multiple {
                match (plan.offset, plan.limit) {
                    (Some(off), Some(lim)) => Some(off + lim),
                    (None, Some(lim)) => Some(lim),
                    _ => None,
                }
            } else {
                None
            };
            // If WHERE id(start_var)==X, skip the full candidate scan.
            let start_var_name = flat.start.variable.as_deref().unwrap_or("");
            let start_vid_override = id_bindings.get(start_var_name).copied();
            self.match_flat_pattern(
                &flat,
                &matcher,
                &mut bindings,
                &mut binding_rows,
                row_limit,
                start_vid_override,
            )
            .await?;
        }

        // Comma-separated multi-patterns: INNER join each additional pattern
        // onto the rows produced by the first pattern. Unlike OPTIONAL MATCH
        // (left join, keeps unmatched rows with NULLs), a required pattern that
        // produces no match drops the base row. Shared variables (e.g. `p` in
        // `(p)-[]->(c), (p)-[]->(t)`) are already bound, so we traverse from
        // the bound VID instead of re-scanning candidates.
        if let Pattern::Multiple(patterns) = &plan.pattern {
            let join_profiling = self.ctx.profiling();
            let join_start = std::time::Instant::now();
            for add_pattern in patterns.iter().skip(1) {
                let add_flat = flatten_pattern(add_pattern)?;
                let mut next_rows = Vec::new();
                for base_row in binding_rows {
                    let already_bound_vid = add_flat.start.variable.as_ref().and_then(|v| {
                        base_row.get(v).and_then(|val| match val {
                            byoridb_common::Value::Int(i) => Some(*i),
                            _ => None,
                        })
                    });

                    let mut rows_for_base: Vec<HashMap<String, byoridb_common::Value>> = Vec::new();
                    if let Some(start_vid) = already_bound_vid {
                        let mut b = base_row.clone();
                        self.match_edges(
                            &space,
                            &add_flat.edges,
                            &add_flat.nodes,
                            0,
                            start_vid,
                            &matcher,
                            &mut b,
                            &mut rows_for_base,
                        )
                        .await?;
                    } else {
                        let mut b = base_row.clone();
                        self.match_flat_pattern(
                            &add_flat,
                            &matcher,
                            &mut b,
                            &mut rows_for_base,
                            None,
                            None,
                        )
                        .await?;
                    }

                    // INNER join: only keep base rows that matched.
                    for r in rows_for_base {
                        next_rows.push(r);
                    }
                }
                binding_rows = next_rows;
            }
            if join_profiling {
                self.ctx.record_profile(
                    ProfileOp::Join,
                    format!("inner join, {} patterns", patterns.len()),
                    binding_rows.len() as u64,
                    join_start.elapsed().as_micros() as u64,
                    false,
                );
            }
        }

        // OPTIONAL MATCH — for each row from the main MATCH, try each optional
        // pattern. If the optional yields results, merge bindings; otherwise
        // keep the original row with NULL values for optional variables.
        let binding_rows = if !plan.optional_patterns.is_empty() {
            let opt_profiling = self.ctx.profiling();
            let opt_start = std::time::Instant::now();
            let mut expanded: Vec<HashMap<String, byoridb_common::Value>> = Vec::new();
            for row in binding_rows {
                let mut current_rows = vec![row];
                for opt_pattern in &plan.optional_patterns {
                    let opt_flat = match flatten_pattern(opt_pattern) {
                        Ok(f) => f,
                        Err(_) => break,
                    };
                    let mut next_rows = Vec::new();
                    for base_row in current_rows {
                        let mut opt_rows: Vec<HashMap<String, byoridb_common::Value>> = Vec::new();

                        // If the start variable is already bound (common case:
                        // OPTIONAL MATCH (p)-[...]->(...) where p came from
                        // main MATCH), traverse from that VID instead of
                        // re-scanning all candidates.
                        let already_bound_vid = opt_flat.start.variable.as_ref().and_then(|v| {
                            base_row.get(v).and_then(|val| match val {
                                byoridb_common::Value::Int(i) => Some(*i),
                                _ => None,
                            })
                        });

                        if let Some(start_vid) = already_bound_vid {
                            let start_var = opt_flat.start.variable.as_ref().unwrap().clone();
                            let mut opt_bindings = base_row.clone();
                            opt_bindings.insert(start_var, byoridb_common::Value::Int(start_vid));
                            let _ = self
                                .match_edges(
                                    &space,
                                    &opt_flat.edges,
                                    &opt_flat.nodes,
                                    0,
                                    start_vid,
                                    &matcher,
                                    &mut opt_bindings,
                                    &mut opt_rows,
                                )
                                .await;
                        } else {
                            // Start variable not bound — scan all candidates
                            let mut opt_bindings = base_row.clone();
                            let _ = self
                                .match_flat_pattern(
                                    &opt_flat,
                                    &matcher,
                                    &mut opt_bindings,
                                    &mut opt_rows,
                                    None,
                                    None,
                                )
                                .await;
                        }

                        if opt_rows.is_empty() {
                            // No matches: keep base row (optional vars → NULL in projection)
                            next_rows.push(base_row);
                        } else {
                            for opt_row in opt_rows {
                                let mut merged = base_row.clone();
                                for (k, v) in opt_row {
                                    merged.insert(k, v);
                                }
                                next_rows.push(merged);
                            }
                        }
                    }
                    current_rows = next_rows;
                }
                expanded.extend(current_rows);
            }
            if opt_profiling {
                self.ctx.record_profile(
                    ProfileOp::Join,
                    format!(
                        "left join, {} optional pattern(s)",
                        plan.optional_patterns.len()
                    ),
                    expanded.len() as u64,
                    opt_start.elapsed().as_micros() as u64,
                    false,
                );
            }
            expanded
        } else {
            binding_rows
        };

        // Apply WHERE clause against actual vertex properties
        let filtered = if let Some(ref where_expr) = plan.where_clause {
            let filter_profiling = self.ctx.profiling();
            let filter_start = std::time::Instant::now();
            let rows_in = binding_rows.len();
            let mut out = Vec::new();
            for row_bindings in binding_rows {
                if self.eval_condition(where_expr, &row_bindings, &space).await {
                    out.push(row_bindings);
                }
            }
            if filter_profiling {
                self.ctx.record_profile(
                    ProfileOp::Filter,
                    format!("{} → {} rows", rows_in, out.len()),
                    out.len() as u64,
                    filter_start.elapsed().as_micros() as u64,
                    false,
                );
            }
            out
        } else {
            binding_rows
        };

        // Project RETURN clause: evaluate each expression, name columns
        let proj_profiling = self.ctx.profiling();
        let proj_start = std::time::Instant::now();
        let proj_is_agg = plan.group_by.is_some()
            || plan
                .return_clause
                .as_ref()
                .is_some_and(|cols| cols.iter().any(|c| is_aggregate_expr(&c.expression)));
        let (columns, rows) = if let Some(ref return_cols) = plan.return_clause {
            let col_names: Vec<String> = return_cols
                .iter()
                .map(|c| {
                    c.alias
                        .clone()
                        .unwrap_or_else(|| expr_to_col_name(&c.expression))
                })
                .collect();

            let has_agg = return_cols.iter().any(|c| is_aggregate_expr(&c.expression));
            let has_group_by = plan.group_by.is_some();

            if has_group_by {
                // GROUP BY + (optional) aggregate:
                // 1. Group filtered binding rows by key expressions
                // 2. For each group, project non-agg columns + compute agg columns
                let group_exprs = plan.group_by.as_ref().unwrap();

                // Group raw binding rows (not yet projected) by key values
                let mut groups: std::collections::BTreeMap<
                    Vec<String>,
                    Vec<HashMap<String, byoridb_common::Value>>,
                > = std::collections::BTreeMap::new();
                for row_bindings in &filtered {
                    let mut key_parts = Vec::new();
                    for key_expr in group_exprs {
                        let v = self.eval_return_expr(key_expr, row_bindings, &space).await;
                        key_parts.push(format!("{:?}", v)); // stable key
                    }
                    groups
                        .entry(key_parts)
                        .or_default()
                        .push(row_bindings.clone());
                }

                // For each group, compute the output row
                let mut result_rows = Vec::new();
                for (_key, group_bindings) in groups {
                    let mut row = Vec::new();
                    for col in return_cols {
                        let val = if is_aggregate_expr(&col.expression) {
                            // Aggregate over the group
                            self.compute_aggregate_row(
                                std::slice::from_ref(col),
                                &group_bindings,
                                &space,
                            )
                            .await
                            .into_iter()
                            .next()
                            .unwrap_or(byoridb_common::Value::Null(byoridb_common::NullType::Null))
                        } else {
                            // Non-aggregate: take first row's value
                            self.eval_return_expr(
                                &col.expression,
                                group_bindings.first().unwrap(),
                                &space,
                            )
                            .await
                        };
                        row.push(val);
                    }
                    result_rows.push(row);
                }
                (col_names, result_rows)
            } else if has_agg {
                // Global aggregate (no GROUP BY): reduce all rows to one
                let agg_row = self
                    .compute_aggregate_row(return_cols, &filtered, &space)
                    .await;
                (
                    col_names,
                    if filtered.is_empty() {
                        vec![]
                    } else {
                        vec![agg_row]
                    },
                )
            } else {
                let mut projected = Vec::new();
                for row_bindings in filtered {
                    let mut row = Vec::new();
                    for col in return_cols {
                        let val = self
                            .eval_return_expr(&col.expression, &row_bindings, &space)
                            .await;
                        row.push(val);
                    }
                    projected.push(row);
                }
                (col_names, projected)
            }
        } else {
            // No RETURN: emit named (non-anonymous, non-edge-prop) variables
            let col_names: Vec<String> = filtered
                .first()
                .map(|r| {
                    let mut ks: Vec<_> = r
                        .keys()
                        .filter(|k| !k.starts_with("__") && !k.contains('.'))
                        .cloned()
                        .collect();
                    ks.sort();
                    ks
                })
                .unwrap_or_default();
            let rows: Vec<Vec<byoridb_common::Value>> = filtered
                .into_iter()
                .map(|rb| {
                    col_names
                        .iter()
                        .map(|k| {
                            rb.get(k).cloned().unwrap_or(byoridb_common::Value::Null(
                                byoridb_common::NullType::Null,
                            ))
                        })
                        .collect()
                })
                .collect();
            (col_names, rows)
        };

        if proj_profiling {
            self.ctx.record_profile(
                if proj_is_agg {
                    ProfileOp::Aggregate
                } else {
                    ProfileOp::Project
                },
                format!("{} column(s)", columns.len()),
                rows.len() as u64,
                proj_start.elapsed().as_micros() as u64,
                false,
            );
        }

        // OFFSET — skip rows before LIMIT
        let mut rows = rows;
        if let Some(offset) = plan.offset {
            rows = rows.into_iter().skip(offset).collect();
        }
        if let Some(limit) = plan.limit {
            rows.truncate(limit);
        }

        let latency = start.elapsed().as_millis() as u64;
        Ok(ExecutorResult {
            columns,
            rows,
            latency_ms: latency,
        })
    }

    async fn try_execute_simple_count(
        &self,
        plan: &crate::plan::MatchPlan,
        space: &str,
        matcher: &PatternMatcher,
    ) -> Result<Option<(Vec<String>, Vec<Vec<byoridb_common::Value>>)>> {
        if plan.where_clause.is_some()
            || !plan.optional_patterns.is_empty()
            || plan.group_by.is_some()
            || plan.offset.is_some()
        {
            return Ok(None);
        }

        let return_cols = match &plan.return_clause {
            Some(cols) if cols.len() == 1 => cols,
            _ => return Ok(None),
        };

        let is_count = matches!(
            &return_cols[0].expression,
            Expression::FunctionCall { name, .. } if name.eq_ignore_ascii_case("COUNT")
        );
        if !is_count {
            return Ok(None);
        }

        let path = match &plan.pattern {
            Pattern::Path(path) if path.edges.is_empty() => path,
            _ => return Ok(None),
        };

        let agg_start = std::time::Instant::now();
        let count = self
            .count_node_matches_unlimited(space, &path.start, matcher)
            .await?;
        if self.ctx.profiling() {
            self.ctx.record_profile(
                ProfileOp::Aggregate,
                "count(*)".to_string(),
                1,
                agg_start.elapsed().as_micros() as u64,
                false,
            );
        }
        let col_name = return_cols[0]
            .alias
            .clone()
            .unwrap_or_else(|| expr_to_col_name(&return_cols[0].expression));

        Ok(Some((
            vec![col_name],
            vec![vec![byoridb_common::Value::Int(count as i64)]],
        )))
    }

    async fn count_node_matches_unlimited(
        &self,
        space: &str,
        node: &NodePattern,
        matcher: &PatternMatcher,
    ) -> Result<usize> {
        let profiling = self.ctx.profiling();
        if node.props.is_empty() {
            if let Some(label) = node.labels.first() {
                let prefix = format!("{}:tagvid:{}:", space, label);
                let t = std::time::Instant::now();
                let results = self
                    .ctx
                    .kvstore
                    .scan_prefix_limited(prefix.as_bytes(), None)
                    .await?;
                if !results.is_empty() {
                    if profiling {
                        self.ctx.record_profile(
                            ProfileOp::TagVidScan,
                            format!("label={} (count)", label),
                            results.len() as u64,
                            t.elapsed().as_micros() as u64,
                            false,
                        );
                    }
                    return Ok(results.len());
                }
            }
        }

        self.ctx.mark_full_scan();
        let prefix = format!("{}:vertex:", space);
        let t = std::time::Instant::now();
        let results = self
            .ctx
            .kvstore
            .scan_prefix_limited(prefix.as_bytes(), None)
            .await?;
        let scanned = results.len();
        let mut count = 0;
        for (_, value) in results {
            if matcher.matches_node(&value, node)? {
                count += 1;
            }
        }
        if profiling {
            self.ctx.record_profile(
                ProfileOp::FullScan,
                format!("scanned {} vertices (count)", scanned),
                count as u64,
                t.elapsed().as_micros() as u64,
                true,
            );
        }
        Ok(count)
    }

    /// Evaluate a return expression against bound variables, fetching vertex
    /// properties from KV when needed.
    async fn eval_return_expr(
        &self,
        expr: &Expression,
        bindings: &HashMap<String, byoridb_common::Value>,
        space: &str,
    ) -> byoridb_common::Value {
        match expr {
            // RETURN v — vertex/edge variable: return full object, not just VID
            Expression::Identifier(name) => {
                match bindings.get(name) {
                    Some(byoridb_common::Value::Int(vid)) => {
                        // Check if this is an edge variable (has "name.prop" entries)
                        let is_edge = bindings
                            .keys()
                            .any(|k| k.starts_with(&format!("{}.", name)));
                        if is_edge {
                            self.build_edge_value(*vid, name, bindings).await
                        } else {
                            self.build_vertex_value(*vid, space).await
                        }
                    }
                    Some(other) => other.clone(),
                    None => byoridb_common::Value::Null(byoridb_common::NullType::Null),
                }
            }

            Expression::Literal(lit) => match lit {
                Literal::Int(i) => byoridb_common::Value::Int(*i),
                Literal::Float(f) => byoridb_common::Value::Float(*f),
                Literal::String(s) => byoridb_common::Value::String(s.clone()),
                Literal::Bool(b) => byoridb_common::Value::Bool(*b),
                Literal::Null => byoridb_common::Value::Null(byoridb_common::NullType::Null),
            },

            // "var.tag.prop" → PropRef { object: "var.tag", prop: "prop" }
            Expression::PropRef { object, prop } => {
                self.fetch_prop_ref(object, prop, bindings, space).await
            }

            // id(v) — returns the VID of bound variable v
            Expression::FunctionCall { name, args } if name.to_lowercase() == "id" => {
                if let Some(Expression::Identifier(var)) = args.first() {
                    match bindings.get(var) {
                        Some(byoridb_common::Value::Int(v)) => byoridb_common::Value::Int(*v),
                        Some(other) => other.clone(),
                        None => byoridb_common::Value::Null(byoridb_common::NullType::Null),
                    }
                } else {
                    byoridb_common::Value::Null(byoridb_common::NullType::Null)
                }
            }

            // properties(v) / properties(e) — flat map of all properties
            Expression::FunctionCall { name, args } if name.to_lowercase() == "properties" => {
                if let Some(Expression::Identifier(var)) = args.first() {
                    if let Some(byoridb_common::Value::Int(vid)) = bindings.get(var) {
                        let is_edge = bindings.keys().any(|k| k.starts_with(&format!("{}.", var)));
                        if is_edge {
                            // Edge props already stored as "var.prop_name"
                            let mut map = std::collections::HashMap::new();
                            for (k, v) in bindings {
                                let prefix = format!("{}.", var);
                                if let Some(prop) = k.strip_prefix(&prefix) {
                                    map.insert(prop.to_string(), v.clone());
                                }
                            }
                            byoridb_common::Value::Map(byoridb_common::datatypes::map::Map {
                                data: map,
                            })
                        } else {
                            // Vertex: merge all tag properties
                            self.fetch_vertex_props_flat(*vid, space).await
                        }
                    } else {
                        byoridb_common::Value::Null(byoridb_common::NullType::Null)
                    }
                } else {
                    byoridb_common::Value::Null(byoridb_common::NullType::Null)
                }
            }

            // tags(v) / labels(v) — list of tag names
            Expression::FunctionCall { name, args }
                if matches!(name.to_lowercase().as_str(), "tags" | "labels") =>
            {
                if let Some(Expression::Identifier(var)) = args.first() {
                    if let Some(byoridb_common::Value::Int(vid)) = bindings.get(var) {
                        let key = format!("{}:vertex:{}", space, vid);
                        if let Ok(Some(blob)) = self.ctx.kvstore.get(key.as_bytes()).await {
                            if let Ok(v) = VertexCodec::decode_vertex(&blob) {
                                let tag_names: Vec<byoridb_common::Value> = v
                                    .tags
                                    .iter()
                                    .map(|t| byoridb_common::Value::String(t.name.clone()))
                                    .collect();
                                return byoridb_common::Value::List(
                                    byoridb_common::datatypes::list::List { values: tag_names },
                                );
                            }
                        }
                    }
                }
                byoridb_common::Value::Null(byoridb_common::NullType::Null)
            }

            _ => byoridb_common::Value::Null(byoridb_common::NullType::Null),
        }
    }

    /// Build a full `Value::Vertex` object from a VID by fetching from KV.
    async fn build_vertex_value(&self, vid: i64, space: &str) -> byoridb_common::Value {
        let key = format!("{}:vertex:{}", space, vid);
        let blob = match self.ctx.kvstore.get(key.as_bytes()).await {
            Ok(Some(b)) => b,
            _ => {
                // No vertex data → return bare VID as Vertex
                return byoridb_common::Value::Vertex(Box::new(byoridb_common::Vertex {
                    vid: byoridb_common::Value::Int(vid),
                    tags: vec![],
                }));
            }
        };
        let codec_vertex = match VertexCodec::decode_vertex(&blob) {
            Ok(v) => v,
            Err(_) => {
                return byoridb_common::Value::Vertex(Box::new(byoridb_common::Vertex {
                    vid: byoridb_common::Value::Int(vid),
                    tags: vec![],
                }))
            }
        };

        let tags: Vec<byoridb_common::datatypes::vertex::Tag> = codec_vertex
            .tags
            .iter()
            .map(|t| {
                let props: std::collections::HashMap<String, byoridb_common::Value> = t
                    .properties
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                byoridb_common::datatypes::vertex::Tag::with_props(t.name.clone(), props)
            })
            .collect();

        byoridb_common::Value::Vertex(Box::new(byoridb_common::Vertex {
            vid: byoridb_common::Value::Int(vid),
            tags,
        }))
    }

    /// Build a `Value::Edge` object from edge variable bindings.
    async fn build_edge_value(
        &self,
        dst_vid: i64,
        var: &str,
        bindings: &HashMap<String, byoridb_common::Value>,
    ) -> byoridb_common::Value {
        // Edge properties are stored as "var.propname" in bindings
        let prefix = format!("{}.", var);
        let mut props = std::collections::HashMap::new();
        let mut edge_type = String::new();
        let mut src_vid: i64 = 0;

        for (k, v) in bindings {
            if let Some(prop) = k.strip_prefix(&prefix) {
                if prop == "__src__" {
                    if let byoridb_common::Value::Int(i) = v {
                        src_vid = *i;
                    }
                } else if prop == "__type__" {
                    if let byoridb_common::Value::String(s) = v {
                        edge_type = s.clone();
                    }
                } else {
                    props.insert(prop.to_string(), v.clone());
                }
            }
        }

        byoridb_common::Value::Edge(Box::new(byoridb_common::Edge::with_props(
            byoridb_common::Value::Int(src_vid),
            byoridb_common::Value::Int(dst_vid),
            0, // edge_type numeric (unused for display)
            edge_type,
            0, // ranking
            props,
        )))
    }

    /// Return all vertex properties as a flat map (merging all tags).
    async fn fetch_vertex_props_flat(&self, vid: i64, space: &str) -> byoridb_common::Value {
        let key = format!("{}:vertex:{}", space, vid);
        let blob = match self.ctx.kvstore.get(key.as_bytes()).await {
            Ok(Some(b)) => b,
            _ => {
                return byoridb_common::Value::Map(byoridb_common::datatypes::map::Map {
                    data: std::collections::HashMap::new(),
                })
            }
        };
        let codec_vertex = match VertexCodec::decode_vertex(&blob) {
            Ok(v) => v,
            Err(_) => {
                return byoridb_common::Value::Map(byoridb_common::datatypes::map::Map {
                    data: std::collections::HashMap::new(),
                })
            }
        };

        let mut map = std::collections::HashMap::new();
        for tag in &codec_vertex.tags {
            if codec_vertex.tags.len() == 1 {
                // Single tag → flat (no prefix)
                for (k, v) in &tag.properties {
                    map.insert(k.clone(), v.clone());
                }
            } else {
                // Multiple tags → prefix with tag name
                for (k, v) in &tag.properties {
                    map.insert(format!("{}.{}", tag.name, k), v.clone());
                }
            }
        }
        byoridb_common::Value::Map(byoridb_common::datatypes::map::Map { data: map })
    }

    /// Resolve a PropRef { object, prop } where `object` is either
    /// "var.tag" (three-level MATCH syntax) or "var" (two-level).
    async fn fetch_prop_ref(
        &self,
        object: &str,
        prop: &str,
        bindings: &HashMap<String, byoridb_common::Value>,
        space: &str,
    ) -> byoridb_common::Value {
        let (var_name, tag_name) = if let Some(dot) = object.find('.') {
            (&object[..dot], Some(&object[dot + 1..]))
        } else {
            (object, None)
        };

        let vid = match bindings.get(var_name) {
            Some(byoridb_common::Value::Int(v)) => *v,
            _ => return byoridb_common::Value::Null(byoridb_common::NullType::Null),
        };

        let key = format!("{}:vertex:{}", space, vid);
        let blob = match self.ctx.kvstore.get(key.as_bytes()).await {
            Ok(Some(b)) => b,
            _ => return byoridb_common::Value::Null(byoridb_common::NullType::Null),
        };
        let vertex = match VertexCodec::decode_vertex(&blob) {
            Ok(v) => v,
            Err(_) => return byoridb_common::Value::Null(byoridb_common::NullType::Null),
        };

        if let Some(_tag) = tag_name {
            // First check if this is an edge property stored as "var.prop"
            // (edge vars store props as "e.propname" during match traversal)
            let edge_prop_key = format!("{}.{}", var_name, prop);
            if let Some(val) = bindings.get(&edge_prop_key) {
                return val.clone();
            }
            // Fall back to vertex tag property lookup
            vertex
                .tags
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(_tag))
                .and_then(|t| t.properties.get(prop))
                .cloned()
                .unwrap_or(byoridb_common::Value::Null(byoridb_common::NullType::Null))
        } else {
            // Two-level: "var.prop" — search all tags for the property
            for tag in &vertex.tags {
                if let Some(val) = tag.properties.get(prop) {
                    return val.clone();
                }
            }
            byoridb_common::Value::Null(byoridb_common::NullType::Null)
        }
    }

    /// Evaluate a WHERE condition against bound variables (async for property
    /// lookups).
    fn eval_condition<'a>(
        &'a self,
        expr: &'a Expression,
        bindings: &'a HashMap<String, byoridb_common::Value>,
        space: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            match expr {
                Expression::BinaryOp { op, left, right } => match op {
                    BinaryOperator::And => {
                        self.eval_condition(left, bindings, space).await
                            && self.eval_condition(right, bindings, space).await
                    }
                    BinaryOperator::Or => {
                        self.eval_condition(left, bindings, space).await
                            || self.eval_condition(right, bindings, space).await
                    }
                    BinaryOperator::Regex => {
                        // `expr =~ "pattern"` — regex match
                        let lv = self.eval_return_expr(left, bindings, space).await;
                        let rv = self.eval_return_expr(right, bindings, space).await;
                        match (&lv, &rv) {
                            (
                                byoridb_common::Value::String(s),
                                byoridb_common::Value::String(pat),
                            ) => regex::Regex::new(pat)
                                .map(|re| re.is_match(s))
                                .unwrap_or(false),
                            _ => false,
                        }
                    }
                    _ => {
                        let lv = self.eval_return_expr(left, bindings, space).await;
                        let rv = self.eval_return_expr(right, bindings, space).await;
                        compare_values(&lv, &rv, op)
                    }
                },
                _ => false,
            }
        })
    }

    /// Compute a single aggregate row from all binding rows.
    async fn compute_aggregate_row(
        &self,
        cols: &[crate::plan::MatchReturnColumn],
        all_bindings: &[HashMap<String, byoridb_common::Value>],
        space: &str,
    ) -> Vec<byoridb_common::Value> {
        let mut row = Vec::new();
        for col in cols {
            let val = match &col.expression {
                Expression::FunctionCall { name, args } => {
                    let arg = args.first();
                    match name.to_uppercase().as_str() {
                        "COUNT" => byoridb_common::Value::Int(all_bindings.len() as i64),
                        "SUM" => {
                            let mut sum = 0f64;
                            let mut all_int = true;
                            if let Some(a) = arg {
                                for b in all_bindings {
                                    match self.eval_return_expr(a, b, space).await {
                                        byoridb_common::Value::Int(i) => sum += i as f64,
                                        byoridb_common::Value::Float(f) => {
                                            sum += f;
                                            all_int = false;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            if all_int {
                                byoridb_common::Value::Int(sum as i64)
                            } else {
                                byoridb_common::Value::Float(sum)
                            }
                        }
                        "AVG" => {
                            let mut sum = 0f64;
                            let mut count = 0usize;
                            if let Some(a) = arg {
                                for b in all_bindings {
                                    match self.eval_return_expr(a, b, space).await {
                                        byoridb_common::Value::Int(i) => {
                                            sum += i as f64;
                                            count += 1;
                                        }
                                        byoridb_common::Value::Float(f) => {
                                            sum += f;
                                            count += 1;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            if count == 0 {
                                byoridb_common::Value::Null(byoridb_common::NullType::Null)
                            } else {
                                byoridb_common::Value::Float(sum / count as f64)
                            }
                        }
                        "MAX" => {
                            let mut best: Option<byoridb_common::Value> = None;
                            if let Some(a) = arg {
                                for b in all_bindings {
                                    let v = self.eval_return_expr(a, b, space).await;
                                    best = Some(match best {
                                        None => v,
                                        Some(cur) => {
                                            if compare_values(&v, &cur, &BinaryOperator::Gt) {
                                                v
                                            } else {
                                                cur
                                            }
                                        }
                                    });
                                }
                            }
                            best.unwrap_or(byoridb_common::Value::Null(
                                byoridb_common::NullType::Null,
                            ))
                        }
                        "MIN" => {
                            let mut best: Option<byoridb_common::Value> = None;
                            if let Some(a) = arg {
                                for b in all_bindings {
                                    let v = self.eval_return_expr(a, b, space).await;
                                    best = Some(match best {
                                        None => v,
                                        Some(cur) => {
                                            if compare_values(&v, &cur, &BinaryOperator::Lt) {
                                                v
                                            } else {
                                                cur
                                            }
                                        }
                                    });
                                }
                            }
                            best.unwrap_or(byoridb_common::Value::Null(
                                byoridb_common::NullType::Null,
                            ))
                        }
                        _ => byoridb_common::Value::Null(byoridb_common::NullType::Null),
                    }
                }
                other => {
                    // Non-aggregate in aggregate context: first row's value
                    match all_bindings.first() {
                        Some(b) => self.eval_return_expr(other, b, space).await,
                        None => byoridb_common::Value::Null(byoridb_common::NullType::Null),
                    }
                }
            };
            row.push(val);
        }
        row
    }

    /// Reverse-traversal optimisation for single-edge patterns where the end
    /// node has a known VID from a WHERE `id(end_var)==X` constraint.
    ///
    /// Instead of scanning all start-node candidates and doing one edge
    /// prefix-scan per candidate (O(N × scan)), we call
    /// `get_incoming_neighbors(end_vid)` once — an O(in-degree) reverse-edge
    /// index lookup (`{space}:in-edge:{end_vid}:` prefix scan) — and treat each
    /// returned source as a start-node candidate. Far cheaper than N forward
    /// prefix-scans when N is large (e.g. 100K products).
    #[allow(clippy::too_many_arguments)]
    async fn match_reverse_single_edge(
        &self,
        space: &str,
        flat: &FlatPattern<'_>,
        end_vid: i64,
        end_var: &str,
        matcher: &PatternMatcher,
        bindings: &mut HashMap<String, byoridb_common::Value>,
        rows: &mut Vec<HashMap<String, byoridb_common::Value>>,
    ) -> Result<()> {
        let edge_pat = flat.edges[0];
        let start_node = flat.start;
        let end_node = flat.nodes[0];

        // Fetch the end node blob to verify it matches the end-node pattern.
        let end_key = format!("{}:vertex:{}", space, end_vid);
        let end_blob = match self.ctx.kvstore.get(end_key.as_bytes()).await? {
            Some(b) => b,
            None => return Ok(()), // end vertex doesn't exist
        };
        if !matcher.matches_node(&end_blob, end_node)? {
            return Ok(()); // end vertex doesn't match label/prop filter
        }

        // Find all edges pointing INTO end_vid via the reverse-edge index
        // (`{space}:in-edge:{end_vid}:` prefix scan) — O(in-degree), not a full
        // scan (PLAN.md O-1).
        let rev_profiling = self.ctx.profiling();
        let rev_start = std::time::Instant::now();
        let incoming =
            algo::get_incoming_neighbors(&self.ctx, space, end_vid, &edge_pat.edge_types).await?;
        if rev_profiling {
            self.ctx.record_profile(
                ProfileOp::Expand,
                format!("reverse-edge index into vid={}", end_vid),
                incoming.len() as u64,
                rev_start.elapsed().as_micros() as u64,
                false,
            );
        }

        let start_var = start_node
            .variable
            .clone()
            .unwrap_or_else(|| "__anon_0__".to_string());

        // Batch-fetch start-node vertex blobs.
        let surviving: Vec<(i64, EdgeData)> = incoming
            .into_iter()
            .filter(|n| {
                matcher
                    .matches_edge_data(&n.edge, edge_pat)
                    .unwrap_or(false)
            })
            .map(|n| (n.dst, n.edge)) // dst here == src of the forward edge
            .collect();

        if surviving.is_empty() {
            return Ok(());
        }

        let src_keys: Vec<Vec<u8>> = surviving
            .iter()
            .map(|(src_vid, _)| format!("{}:vertex:{}", space, src_vid).into_bytes())
            .collect();
        let src_blobs = self.ctx.kvstore.batch_get(&src_keys).await?;

        for ((src_vid, edge_data), src_blob_opt) in surviving.into_iter().zip(src_blobs.into_iter())
        {
            let src_blob = match src_blob_opt {
                Some(b) => b,
                None => continue,
            };
            if !matcher.matches_node(&src_blob, start_node)? {
                continue;
            }

            bindings.insert(start_var.clone(), byoridb_common::Value::Int(src_vid));
            bindings.insert(end_var.to_string(), byoridb_common::Value::Int(end_vid));

            if let Some(ref edge_var) = edge_pat.variable {
                bindings.insert(edge_var.clone(), byoridb_common::Value::Int(end_vid));
                bindings.insert(
                    format!("{}.__src__", edge_var),
                    byoridb_common::Value::Int(src_vid),
                );
                bindings.insert(
                    format!("{}.__type__", edge_var),
                    byoridb_common::Value::String(edge_data.edge_type.clone()),
                );
                for (prop_name, prop_val) in &edge_data.properties {
                    bindings.insert(format!("{}.{}", edge_var, prop_name), prop_val.clone());
                }
            }

            rows.push(bindings.clone());

            // Clean up edge-var bindings
            if let Some(ref edge_var) = edge_pat.variable {
                bindings.remove(edge_var);
                bindings.remove(&format!("{}.__src__", edge_var));
                bindings.remove(&format!("{}.__type__", edge_var));
                for prop_name in edge_data.properties.keys() {
                    bindings.remove(&format!("{}.{}", edge_var, prop_name));
                }
            }
            bindings.remove(&start_var);
            bindings.remove(end_var);
        }

        tracing::debug!(
            space = space,
            end_vid = end_vid,
            matched = rows.len(),
            "MATCH reverse single-edge traversal completed"
        );
        Ok(())
    }

    /// Walk the flattened pattern, binding variables and collecting rows.
    ///
    /// `row_limit` is an optional early-exit threshold: if we already have
    /// this many rows we stop processing candidates. ORDER BY is currently
    /// discarded by the parser (no-op), so stopping early is safe — results
    /// are in storage order regardless.
    async fn match_flat_pattern(
        &self,
        flat: &FlatPattern<'_>,
        matcher: &PatternMatcher,
        bindings: &mut HashMap<String, byoridb_common::Value>,
        rows: &mut Vec<HashMap<String, byoridb_common::Value>>,
        row_limit: Option<usize>,
        start_vid_override: Option<i64>,
    ) -> Result<()> {
        let space = self
            .ctx
            .space
            .as_ref()
            .ok_or_else(|| ExecutionError::InvalidOperation("No space selected".to_string()))?;

        // Anonymous start node (no variable) gets a synthetic key that is
        // excluded from the RETURN projection (it starts with "__").
        let start_var = flat
            .start
            .variable
            .clone()
            .unwrap_or_else(|| "__anon_0__".to_string());

        // WHERE id(start_var)==X shortcut: skip the full candidate scan and
        // use the single bound VID directly.
        let candidates = if let Some(vid) = start_vid_override {
            vec![vid]
        } else {
            self.find_node_candidates(space, flat.start, matcher)
                .await?
        };

        // Profile the edge-expansion phase separately from the start-node scan
        // (which records its own NodeScan/IndexScan/FullScan above). Only
        // meaningful when the pattern actually has edges to traverse.
        let expand_profiling = self.ctx.profiling() && !flat.edges.is_empty();
        let mut expand_us: u64 = 0;
        let rows_before = rows.len();

        for candidate_vid in candidates {
            if let Some(lim) = row_limit {
                if rows.len() >= lim {
                    break;
                }
            }

            bindings.insert(start_var.clone(), byoridb_common::Value::Int(candidate_vid));

            let edge_timer = if expand_profiling {
                Some(std::time::Instant::now())
            } else {
                None
            };
            self.match_edges(
                space,
                &flat.edges,
                &flat.nodes,
                0,
                candidate_vid,
                matcher,
                bindings,
                rows,
            )
            .await?;
            if let Some(t) = edge_timer {
                expand_us += t.elapsed().as_micros() as u64;
            }

            bindings.remove(&start_var);
        }

        if expand_profiling {
            self.ctx.record_profile(
                ProfileOp::Expand,
                format!("{} hop(s)", flat.edges.len()),
                (rows.len() - rows_before) as u64,
                expand_us,
                false,
            );
        }

        Ok(())
    }

    /// Find candidate vertices matching labels and property filters.
    ///
    /// Tries the tag-index path first via [`Self::find_indexed_node_candidates`].
    /// If no usable index exists, falls back to a full `{space}:vertex:` scan
    /// and emits a `tracing::warn!` so operators can see *why* the fallback
    /// happened — typically that signals a missing index for the label or
    /// property being matched.
    pub(crate) async fn find_node_candidates(
        &self,
        space: &str,
        node: &NodePattern,
        matcher: &PatternMatcher,
    ) -> Result<Vec<i64>> {
        if let Some(candidates) = self
            .find_indexed_node_candidates(space, node, matcher)
            .await?
        {
            return Ok(candidates);
        }

        let label = node.labels.first().map(|s| s.as_str()).unwrap_or("");
        let has_property_filter = !node.props.is_empty();
        tracing::warn!(
            space = space,
            label = label,
            label_only = !has_property_filter && !node.labels.is_empty(),
            no_filter = label.is_empty() && !has_property_filter,
            "MATCH falling back to full vertex scan (no usable tag index for label/property)"
        );
        self.ctx.mark_full_scan();
        let scan_profiling = self.ctx.profiling();
        let scan_start = std::time::Instant::now();

        let mut candidates = Vec::new();

        let prefix = format!("{}:vertex:", space);
        let scan_limit = if self.ctx.config.max_scan_limit > 0 {
            Some(self.ctx.config.max_scan_limit)
        } else {
            None
        };
        let results = self
            .ctx
            .kvstore
            .scan_prefix_limited(prefix.as_bytes(), scan_limit)
            .await?;
        let scanned = results.len();

        for (key, value) in results {
            let key_str = String::from_utf8_lossy(&key);
            let vid_str = match key_str.split(':').nth(2) {
                Some(s) => s,
                None => continue,
            };
            let vid: i64 = match vid_str.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };

            if matcher.matches_node(&value, node)? {
                candidates.push(vid);
            }
        }

        if scan_profiling {
            self.ctx.record_profile(
                ProfileOp::FullScan,
                format!("scanned {} vertices, matched {}", scanned, candidates.len()),
                candidates.len() as u64,
                scan_start.elapsed().as_micros() as u64,
                true,
            );
        }

        tracing::info!(
            space = space,
            label = label,
            scanned_vertices = scanned,
            matched = candidates.len(),
            "MATCH full vertex scan completed"
        );

        Ok(candidates)
    }

    /// Try to satisfy the MATCH start-node from a tag index.
    ///
    /// Strategy:
    /// 1. **Full-cover index**: if a multi-field index on the label has the
    ///    same field set as the pattern's literal property predicates (in any
    ///    order), use it directly with all values bound — no post-filter for
    ///    those fields needed beyond `matches_node` for label re-check.
    /// 2. **Prefix-cover index**: if a multi-field index's *leading* fields
    ///    are all present in the pattern, use a prefix lookup with just
    ///    those leading values. Reduces candidates to ≤ index prefix matches.
    /// 3. **Single-field index**: fall back to any single-field index on one
    ///    of the pattern's literal properties.
    ///
    /// In every case `matches_node` runs on the fetched vertex blobs so
    /// residual (non-indexed) property predicates and label re-check still
    /// apply — the index just shrinks the candidate set.
    async fn find_indexed_node_candidates(
        &self,
        space: &str,
        node: &NodePattern,
        matcher: &PatternMatcher,
    ) -> Result<Option<Vec<i64>>> {
        let index_manager = match self.ctx.index_manager.as_ref() {
            Some(manager) => manager,
            None => return Ok(None),
        };

        let label = match node.labels.first() {
            Some(label) => label,
            None => return Ok(None),
        };

        // Collect indexable (literal) (field, value) pairs from the pattern.
        let pattern_props: Vec<(String, byoridb_common::Value)> = node
            .props
            .iter()
            .filter_map(|(field, expr)| {
                expression_as_value(expr).map(|value| (field.clone(), value))
            })
            .collect();

        if pattern_props.is_empty() {
            // Label-only pattern: use the tag-vid secondary index written by INSERT VERTEX.
            // Key format: {space}:tagvid:{tag_name}:{vid}
            // If the index has entries for this label, use them directly — no vertex
            // blob decode needed for filtering. Stale entries (from deleted vertices)
            // are harmless: edge traversal or dst-blob fetch will produce 0 results.
            if let Some(label) = node.labels.first() {
                let prefix = format!("{}:tagvid:{}:", space, label);
                let scan_limit = if self.ctx.config.max_scan_limit > 0 {
                    Some(self.ctx.config.max_scan_limit)
                } else {
                    None
                };
                let tagvid_profiling = self.ctx.profiling();
                let tagvid_start = std::time::Instant::now();
                let results = self
                    .ctx
                    .kvstore
                    .scan_prefix_limited(prefix.as_bytes(), scan_limit)
                    .await?;
                if !results.is_empty() {
                    if tagvid_profiling {
                        self.ctx.record_profile(
                            ProfileOp::TagVidScan,
                            format!("label={}", label),
                            results.len() as u64,
                            tagvid_start.elapsed().as_micros() as u64,
                            false,
                        );
                    }
                    let vids: Vec<i64> = results
                        .iter()
                        .filter_map(|(key, _)| {
                            // Last colon-separated segment is the vid
                            String::from_utf8_lossy(key)
                                .rsplit(':')
                                .next()
                                .and_then(|s| s.parse().ok())
                        })
                        .collect();
                    tracing::debug!(
                        space = space,
                        label = label.as_str(),
                        candidates = vids.len(),
                        "MATCH used tag-vid secondary index for label-only pattern"
                    );
                    return Ok(Some(vids));
                }
            }
            return Ok(None);
        }

        let space_id = self.ctx.space_id.unwrap_or(1);
        let indexes = index_manager.list_tag_indexes(space_id).await;
        let label_indexes: Vec<_> = indexes
            .iter()
            .filter(|idx| idx.schema_name.eq_ignore_ascii_case(label))
            .collect();

        let chosen = pick_index_plan(&label_indexes, &pattern_props);
        let (index_def, lookup_values) = match chosen {
            Some(plan) => plan,
            None => return Ok(None),
        };

        let partition_num = self.ctx.get_partition_num().unwrap_or(1);
        let idx_profiling = self.ctx.profiling();
        let idx_start = std::time::Instant::now();
        let mut vids = Vec::new();
        let mut lookup_succeeded = false;

        for part_id in 1..=partition_num {
            match index_manager
                .lookup_tag(part_id, index_def, &lookup_values, 1000)
                .await
            {
                Ok(part_vids) => {
                    lookup_succeeded = true;
                    vids.extend(part_vids);
                }
                Err(e) => {
                    tracing::warn!(
                        "MATCH index lookup on part {} failed for index '{}': {}",
                        part_id,
                        index_def.index_name,
                        e
                    );
                }
            }
        }

        if !lookup_succeeded {
            return Ok(None);
        }

        if vids.is_empty() {
            tracing::debug!(
                "MATCH tag index '{}' returned 0 candidates (label={}, indexed_fields={:?})",
                index_def.index_name,
                label,
                index_def.fields
            );
            return Ok(Some(vec![]));
        }

        let keys: Vec<Vec<u8>> = vids
            .iter()
            .map(|vid| format!("{}:vertex:{}", space, vid).into_bytes())
            .collect();
        let blobs = self.ctx.kvstore.batch_get(&keys).await?;

        let mut candidates = Vec::new();
        for (vid, blob) in vids.into_iter().zip(blobs.into_iter()) {
            if let Some(blob) = blob {
                if matcher.matches_node(&blob, node)? {
                    candidates.push(vid);
                }
            }
        }

        if idx_profiling {
            self.ctx.record_profile(
                ProfileOp::IndexScan,
                format!("index '{}' (label={})", index_def.index_name, label),
                candidates.len() as u64,
                idx_start.elapsed().as_micros() as u64,
                false,
            );
        }

        tracing::debug!(
            "MATCH used tag index '{}' (fields={:?}, bound={}) for label={} → {} candidates",
            index_def.index_name,
            index_def.fields,
            lookup_values.len(),
            label,
            candidates.len()
        );

        Ok(Some(candidates))
    }

    /// Match edges starting from a vertex.
    ///
    /// Uses the shared [`algo::get_neighbors`] helper so MATCH benefits from
    /// the same edge-type prefilter and proto/JSON auto-decoding as BFS and
    /// `GO`. Destination vertex blobs are fetched in a single `batch_get`
    /// rather than one round-trip per surviving edge.
    #[allow(clippy::too_many_arguments)]
    fn match_edges<'a>(
        &'a self,
        space: &'a str,
        edges: &'a [&'a EdgePattern],
        nodes: &'a [&'a NodePattern],
        edge_idx: usize,
        current_vid: i64,
        matcher: &'a PatternMatcher,
        bindings: &'a mut HashMap<String, byoridb_common::Value>,
        rows: &'a mut Vec<HashMap<String, byoridb_common::Value>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            if edge_idx >= edges.len() {
                rows.push(bindings.clone());
                return Ok(());
            }

            let edge = edges[edge_idx];
            let node = nodes[edge_idx];

            // Variable-length hop (`*min..max`): expand to all distinct
            // terminal vertices in range, filter them against the node
            // pattern, and recurse — intermediate vertices are not bound.
            if edge.range.is_some() {
                let terminals = super::var_length::expand_var_length(
                    &self.ctx,
                    space,
                    current_vid,
                    edge,
                    matcher,
                )
                .await?;
                if terminals.is_empty() {
                    return Ok(());
                }
                let dst_keys: Vec<Vec<u8>> = terminals
                    .iter()
                    .map(|dst| format!("{}:vertex:{}", space, dst).into_bytes())
                    .collect();
                let dst_blobs = self.ctx.kvstore.batch_get(&dst_keys).await?;
                for (dst_vid, dst_blob_opt) in terminals.into_iter().zip(dst_blobs.into_iter()) {
                    let dst_blob = match dst_blob_opt {
                        Some(b) => b,
                        None => continue,
                    };
                    if !matcher.matches_node(&dst_blob, node)? {
                        continue;
                    }
                    if let Some(ref node_var) = node.variable {
                        bindings.insert(node_var.clone(), byoridb_common::Value::Int(dst_vid));
                    }
                    self.match_edges(
                        space,
                        edges,
                        nodes,
                        edge_idx + 1,
                        dst_vid,
                        matcher,
                        bindings,
                        rows,
                    )
                    .await?;
                    if let Some(ref node_var) = node.variable {
                        bindings.remove(node_var);
                    }
                }
                return Ok(());
            }

            // Fixed single hop. Undirected unions both scan directions —
            // previously it silently fell through to outgoing-only.
            let neighbors = super::var_length::neighbors_for_direction(
                &self.ctx,
                space,
                current_vid,
                &edge.edge_types,
                &edge.direction,
            )
            .await?;

            let mut surviving: Vec<(i64, EdgeData)> = Vec::with_capacity(neighbors.len());
            for neighbor in neighbors {
                if matcher.matches_edge_data(&neighbor.edge, edge)? {
                    surviving.push((neighbor.dst, neighbor.edge));
                }
            }

            if surviving.is_empty() {
                return Ok(());
            }

            let dst_keys: Vec<Vec<u8>> = surviving
                .iter()
                .map(|(dst, _)| format!("{}:vertex:{}", space, dst).into_bytes())
                .collect();
            let dst_blobs = self.ctx.kvstore.batch_get(&dst_keys).await?;

            for ((dst_vid, edge_data), dst_blob_opt) in
                surviving.into_iter().zip(dst_blobs.into_iter())
            {
                let dst_blob = match dst_blob_opt {
                    Some(b) => b,
                    None => continue,
                };
                if !matcher.matches_node(&dst_blob, node)? {
                    continue;
                }

                if let Some(ref edge_var) = edge.variable {
                    // dst_vid stored as the edge binding (for VID access)
                    bindings.insert(edge_var.clone(), byoridb_common::Value::Int(dst_vid));
                    // Store src VID and edge type so build_edge_value can reconstruct the edge
                    bindings.insert(
                        format!("{}.__src__", edge_var),
                        byoridb_common::Value::Int(edge_data.src_vid),
                    );
                    bindings.insert(
                        format!("{}.__type__", edge_var),
                        byoridb_common::Value::String(edge_data.edge_type.clone()),
                    );
                    // Store each edge property as "edge_var.prop_name"
                    for (prop_name, prop_val) in &edge_data.properties {
                        bindings.insert(format!("{}.{}", edge_var, prop_name), prop_val.clone());
                    }
                }
                if let Some(ref node_var) = node.variable {
                    bindings.insert(node_var.clone(), byoridb_common::Value::Int(dst_vid));
                }

                self.match_edges(
                    space,
                    edges,
                    nodes,
                    edge_idx + 1,
                    dst_vid,
                    matcher,
                    bindings,
                    rows,
                )
                .await?;

                if let Some(ref edge_var) = edge.variable {
                    bindings.remove(edge_var);
                    bindings.remove(&format!("{}.__src__", edge_var));
                    bindings.remove(&format!("{}.__type__", edge_var));
                    for prop_name in edge_data.properties.keys() {
                        bindings.remove(&format!("{}.{}", edge_var, prop_name));
                    }
                }
                if let Some(ref node_var) = node.variable {
                    bindings.remove(node_var);
                }
            }

            Ok(())
        })
    }
}

fn is_aggregate_expr(expr: &Expression) -> bool {
    matches!(expr, Expression::FunctionCall { name, .. }
        if matches!(name.to_uppercase().as_str(), "COUNT" | "SUM" | "AVG" | "MAX" | "MIN"))
}

/// Derive a display name from a return expression (used when no alias given).
fn expr_to_col_name(expr: &Expression) -> String {
    match expr {
        Expression::Identifier(name) => name.clone(),
        Expression::PropRef { object, prop } => format!("{}.{}", object, prop),
        Expression::DstVertexProp { tag, prop } => format!("{}.{}", tag, prop),
        _ => "value".to_string(),
    }
}

/// Compare two Values with a binary operator; returns bool.
fn compare_values(
    lv: &byoridb_common::Value,
    rv: &byoridb_common::Value,
    op: &BinaryOperator,
) -> bool {
    use byoridb_common::Value;
    let ord = match (lv, rv) {
        (Value::Int(a), Value::Int(b)) => a.partial_cmp(b),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
        (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
        (Value::String(a), Value::String(b)) => a.partial_cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.partial_cmp(b),
        _ => None,
    };
    // String containment operators don't use ord
    if let (Value::String(s), Value::String(sub)) = (lv, rv) {
        match op {
            BinaryOperator::Contains => return s.contains(sub.as_str()),
            BinaryOperator::NotContains => return !s.contains(sub.as_str()),
            BinaryOperator::StartsWith => return s.starts_with(sub.as_str()),
            BinaryOperator::EndsWith => return s.ends_with(sub.as_str()),
            _ => {}
        }
    }
    match op {
        BinaryOperator::Eq => ord == Some(std::cmp::Ordering::Equal),
        BinaryOperator::Neq => ord.is_some_and(|o| o != std::cmp::Ordering::Equal),
        BinaryOperator::Lt => ord == Some(std::cmp::Ordering::Less),
        BinaryOperator::Lte => ord.is_some_and(|o| o != std::cmp::Ordering::Greater),
        BinaryOperator::Gt => ord == Some(std::cmp::Ordering::Greater),
        BinaryOperator::Gte => ord.is_some_and(|o| o != std::cmp::Ordering::Less),
        _ => false,
    }
}

/// Extract `id(var) == literal_int` (or reversed) bindings from a WHERE expression.
/// Handles AND chains. Returns map from variable name → VID.
pub(crate) fn extract_id_eq_bindings(expr: &Expression) -> HashMap<String, i64> {
    match expr {
        Expression::BinaryOp {
            op: BinaryOperator::And,
            left,
            right,
        } => {
            let mut m = extract_id_eq_bindings(left);
            m.extend(extract_id_eq_bindings(right));
            m
        }
        Expression::BinaryOp {
            op: BinaryOperator::Eq,
            left,
            right,
        } => {
            let try_extract = |func: &Expression, val: &Expression| -> Option<(String, i64)> {
                if let (
                    Expression::FunctionCall { name, args },
                    Expression::Literal(Literal::Int(v)),
                ) = (func, val)
                {
                    if name.to_lowercase() == "id" {
                        if let Some(Expression::Identifier(var)) = args.first() {
                            return Some((var.clone(), *v));
                        }
                    }
                }
                None
            };
            let found = try_extract(left, right).or_else(|| try_extract(right, left));
            found.into_iter().collect()
        }
        _ => HashMap::new(),
    }
}

pub(super) struct FlatPattern<'a> {
    start: &'a NodePattern,
    edges: Vec<&'a EdgePattern>,
    nodes: Vec<&'a NodePattern>,
}

pub(super) fn flatten_pattern(pattern: &Pattern) -> Result<FlatPattern<'_>> {
    let path = match pattern {
        Pattern::Path(p) => p,
        Pattern::Multiple(patterns) => match patterns.first() {
            Some(Pattern::Path(p)) => p,
            Some(Pattern::Multiple(_)) => {
                return Err(ExecutionError::InvalidOperation(
                    "Nested multi-patterns are not supported".to_string(),
                ))
            }
            None => {
                return Err(ExecutionError::InvalidOperation(
                    "Empty pattern".to_string(),
                ))
            }
        },
    };

    // Use the actual interior node patterns from the AST.
    // `path.nodes[i]` is the filter for the node reached via `path.edges[i]`.
    // If the AST has fewer nodes than edges (shouldn't happen with a correct
    // parse, but guard anyway), fall back to the start node as a no-op filter.
    let edges: Vec<&EdgePattern> = path.edges.iter().collect();
    let nodes: Vec<&NodePattern> = path
        .nodes
        .iter()
        .chain(std::iter::repeat(&path.start))
        .take(edges.len())
        .collect();

    Ok(FlatPattern {
        start: &path.start,
        edges,
        nodes,
    })
}

pub(super) fn expression_as_value(expr: &Expression) -> Option<byoridb_common::Value> {
    match expr {
        Expression::Literal(Literal::Int(i)) => Some(byoridb_common::Value::Int(*i)),
        Expression::Literal(Literal::Float(f)) => Some(byoridb_common::Value::Float(*f)),
        Expression::Literal(Literal::String(s)) => Some(byoridb_common::Value::String(s.clone())),
        Expression::Literal(Literal::Bool(b)) => Some(byoridb_common::Value::Bool(*b)),
        Expression::Literal(Literal::Null) => Some(byoridb_common::Value::null()),
        _ => None,
    }
}

pub(super) fn byoridb_value_to_index_value(value: &byoridb_common::Value) -> IndexValue {
    match value {
        byoridb_common::Value::String(s) => IndexValue::String(s.clone()),
        byoridb_common::Value::Int(i) => IndexValue::Int(*i),
        byoridb_common::Value::Float(f) => IndexValue::Float(*f),
        byoridb_common::Value::Bool(b) => IndexValue::Bool(*b),
        _ => IndexValue::Null,
    }
}

/// Choose the best index lookup plan for a MATCH start-node pattern.
///
/// Preference order:
/// 1. **Full cover** — a multi-field index whose field set equals the
///    pattern's literal-property field set. All values bind, lookup is exact.
/// 2. **Prefix cover (longest prefix wins)** — an index whose leading `k`
///    fields are all present in the pattern. We bind those `k` values and
///    leave the rest for residual filtering on the fetched blobs.
/// 3. **Single-field fallback** — any index with a single field that's
///    present in the pattern. Equivalent to old behavior; kept for compat.
///
/// Returns `Some((index_def, lookup_values))` if any plan is viable.
pub(crate) fn pick_index_plan<'a>(
    label_indexes: &[&'a IndexDef],
    pattern_props: &[(String, byoridb_common::Value)],
) -> Option<(&'a IndexDef, Vec<IndexValue>)> {
    if pattern_props.is_empty() || label_indexes.is_empty() {
        return None;
    }

    let prop_lookup: HashMap<&str, &byoridb_common::Value> =
        pattern_props.iter().map(|(k, v)| (k.as_str(), v)).collect();

    // (1) full cover: same field set as the pattern.
    if let Some(idx) = label_indexes.iter().find(|idx| {
        idx.fields.len() == pattern_props.len()
            && idx
                .fields
                .iter()
                .all(|f| prop_lookup.contains_key(f.as_str()))
    }) {
        let values: Vec<IndexValue> = idx
            .fields
            .iter()
            .map(|f| byoridb_value_to_index_value(prop_lookup[f.as_str()]))
            .collect();
        return Some((idx, values));
    }

    // (2) longest prefix cover.
    let mut best: Option<(&IndexDef, usize)> = None;
    for idx in label_indexes {
        let mut prefix_len = 0;
        for field in &idx.fields {
            if prop_lookup.contains_key(field.as_str()) {
                prefix_len += 1;
            } else {
                break;
            }
        }
        if prefix_len == 0 {
            continue;
        }
        // Prefer the longest matching prefix; on ties, prefer the index
        // with fewer total fields (more selective lookup).
        match best {
            None => best = Some((idx, prefix_len)),
            Some((_, cur_len)) if prefix_len > cur_len => best = Some((idx, prefix_len)),
            Some((cur_idx, cur_len))
                if prefix_len == cur_len && idx.fields.len() < cur_idx.fields.len() =>
            {
                best = Some((idx, prefix_len));
            }
            _ => {}
        }
    }
    if let Some((idx, prefix_len)) = best {
        let values: Vec<IndexValue> = idx
            .fields
            .iter()
            .take(prefix_len)
            .map(|f| byoridb_value_to_index_value(prop_lookup[f.as_str()]))
            .collect();
        return Some((idx, values));
    }

    // (3) single-field fallback (covered by (2) when prefix_len == 1, but
    // kept explicit for clarity and as a final safety net).
    label_indexes
        .iter()
        .find(|idx| idx.fields.len() == 1 && prop_lookup.contains_key(idx.fields[0].as_str()))
        .map(|idx| {
            let value = byoridb_value_to_index_value(prop_lookup[idx.fields[0].as_str()]);
            (*idx, vec![value])
        })
}

/// Equality between a stored `byoridb_common::Value` (e.g. from a decoded
/// `EdgeData.properties`) and an expected literal pulled out of the pattern.
pub(super) fn byoridb_value_equals(
    stored: &byoridb_common::Value,
    expected: &byoridb_common::Value,
) -> bool {
    match (stored, expected) {
        (byoridb_common::Value::Null(_), byoridb_common::Value::Null(_)) => true,
        (byoridb_common::Value::Bool(a), byoridb_common::Value::Bool(b)) => a == b,
        (byoridb_common::Value::Int(a), byoridb_common::Value::Int(b)) => a == b,
        (byoridb_common::Value::Int(a), byoridb_common::Value::Float(b)) => (*a as f64) == *b,
        (byoridb_common::Value::Float(a), byoridb_common::Value::Float(b)) => a == b,
        (byoridb_common::Value::Float(a), byoridb_common::Value::Int(b)) => *a == (*b as f64),
        (byoridb_common::Value::String(a), byoridb_common::Value::String(b)) => a == b,
        _ => false,
    }
}

#[allow(dead_code)]
pub(super) fn json_value_equals(
    stored: &serde_json::Value,
    expected: &byoridb_common::Value,
) -> bool {
    match (stored, expected) {
        (serde_json::Value::Null, byoridb_common::Value::Null(_)) => true,
        (serde_json::Value::Bool(a), byoridb_common::Value::Bool(b)) => a == b,
        (serde_json::Value::Number(n), byoridb_common::Value::Int(b)) => n.as_i64() == Some(*b),
        (serde_json::Value::Number(n), byoridb_common::Value::Float(b)) => n.as_f64() == Some(*b),
        (serde_json::Value::String(s), byoridb_common::Value::String(b)) => s == b,
        _ => false,
    }
}
