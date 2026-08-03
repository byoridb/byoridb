// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Logical plan tree for `EXPLAIN` / `PROFILE`.
//!
//! [`ExecutionPlan`] is a flat enum — there is no operator tree to print. This
//! module *derives* a logical operator tree ([`PlanNode`]) from each plan and
//! renders it as an indented, multi-row [`ExecutorResult`] (so the existing
//! `DataSet` wire format is reused, no proto change).
//!
//! - **EXPLAIN** builds the tree and statically determines each scan's access
//!   path (named index, tag-vid index, edge prefix, or full scan) by consulting
//!   the index manager / tag-vid secondary index — without executing.
//! - **PROFILE** runs the query with a [`ProfileCollector`] attached, then
//!   overlays the collected per-operator row counts and timings onto the tree.

use crate::context::ExecutionContext;
use crate::executor::ExecutorResult;
use crate::plan::{ExecutionPlan, LookupPlan, LookupType};
use crate::profile::{ProfileOp, ProfileRecord};
use byoridb_common::Value;
use byoridb_parser::ast::{
    BinaryOperator, EdgeDirection, Expression, Literal, NodePattern, Pattern,
};
use byoridb_storage::index::IndexDef;
use std::collections::HashMap;

/// How an operator reaches its data.
#[derive(Debug, Clone)]
pub enum AccessPath {
    /// Used a named secondary index.
    Index(String),
    /// Label-only `{space}:tagvid:` secondary index.
    TagVidIndex,
    /// Edge prefix scan bounded by source VID (not a full scan).
    EdgePrefix,
    /// Reverse-edge index prefix scan (`{space}:in-edge:{dst}:`) for reverse
    /// traversal — bounded by destination VID, not a full scan.
    ReverseEdgeIndex,
    /// Point lookup of vertices by VID (batch_get).
    PointLookup,
    /// Un-indexed full scan.
    FullScan,
    /// No data access (DDL, projection-only, output).
    None,
}

impl AccessPath {
    fn display(&self) -> String {
        match self {
            AccessPath::Index(name) => format!("index: {}", name),
            AccessPath::TagVidIndex => "index: tag-vid".to_string(),
            AccessPath::EdgePrefix => "edge prefix scan".to_string(),
            AccessPath::ReverseEdgeIndex => "reverse-edge index".to_string(),
            AccessPath::PointLookup => "point lookup".to_string(),
            AccessPath::FullScan => "⚠ FULL SCAN".to_string(),
            AccessPath::None => "-".to_string(),
        }
    }

    fn is_full_scan(&self) -> bool {
        matches!(self, AccessPath::FullScan)
    }
}

/// One node in the derived logical operator tree.
#[derive(Debug)]
pub struct PlanNode {
    pub operator: String,
    pub detail: String,
    pub access: AccessPath,
    /// Key linking this node to runtime profile records (None = not measured).
    pub profile_op: Option<ProfileOp>,
    pub children: Vec<PlanNode>,
    // Filled by the PROFILE overlay.
    pub rows: Option<u64>,
    pub time_us: Option<u64>,
}

impl PlanNode {
    fn new(operator: impl Into<String>, detail: impl Into<String>, access: AccessPath) -> Self {
        Self {
            operator: operator.into(),
            detail: detail.into(),
            access,
            profile_op: None,
            children: Vec::new(),
            rows: None,
            time_us: None,
        }
    }

    fn with_profile(mut self, op: ProfileOp) -> Self {
        self.profile_op = Some(op);
        self
    }

    fn child(mut self, node: PlanNode) -> Self {
        self.children.push(node);
        self
    }
}

// ===== Tree construction =====

/// Build the logical operator tree for a plan, resolving access paths against
/// the live index metadata. Async because index/tag-vid checks hit storage.
pub async fn build_plan_tree(ctx: &ExecutionContext, plan: &ExecutionPlan) -> PlanNode {
    match plan {
        ExecutionPlan::Go(p) => build_go(p),
        ExecutionPlan::Lookup(p) => build_lookup(ctx, p).await,
        ExecutionPlan::Match(p) => build_match(ctx, p).await,
        ExecutionPlan::Find(p) => build_find(p),
        ExecutionPlan::Fetch(p) => build_fetch(p),
        ExecutionPlan::Compound(clauses) => {
            let mut root = PlanNode::new(
                "Compound",
                format!("{} clauses", clauses.len()),
                AccessPath::None,
            );
            for (i, clause) in clauses.iter().enumerate() {
                let label = clause
                    .var
                    .as_ref()
                    .map(|v| format!("${} =", v))
                    .unwrap_or_else(|| format!("clause {}", i));
                let mut sub = Box::pin(build_plan_tree(ctx, &clause.plan)).await;
                sub.detail = if sub.detail.is_empty() {
                    label
                } else {
                    format!("{} {}", label, sub.detail)
                };
                root.children.push(sub);
            }
            root
        }
        // EXPLAIN of EXPLAIN/PROFILE: unwrap one level.
        ExecutionPlan::Explain { plan, .. } => Box::pin(build_plan_tree(ctx, plan)).await,
        // DDL / DML / admin: single descriptive node, no data-access path.
        other => PlanNode::new(plan_kind(other), "", AccessPath::None),
    }
}

fn build_go(p: &crate::plan::GoPlan) -> PlanNode {
    let steps = match &p.to_clause.steps {
        crate::plan::StepClause::Exactly(n) => format!("{} step(s)", n),
        crate::plan::StepClause::Range(a, b) => format!("{}..{} steps", a, b),
    };
    let (access, op) = match p.direction {
        EdgeDirection::Outgoing => (AccessPath::EdgePrefix, ProfileOp::GetNeighbors),
        // Reverse traversal reads the reverse-edge index (`{space}:in-edge:{dst}:`).
        EdgeDirection::Incoming => (AccessPath::ReverseEdgeIndex, ProfileOp::GetIncoming),
        // Undirected combines the forward edge prefix with the reverse-edge index.
        EdgeDirection::Undirected => (AccessPath::ReverseEdgeIndex, ProfileOp::GetIncoming),
    };

    let source = if !p.from_clause.vids.is_empty() {
        format!("from {} vid(s)", p.from_clause.vids.len())
    } else if let Some(src) = &p.from_clause.src {
        format!("from {}", src)
    } else {
        "from <unbound>".to_string()
    };
    let start = PlanNode::new("StartVids", source, AccessPath::None);

    let neighbors = PlanNode::new(
        "GetNeighbors",
        format!(
            "over [{}], {}, {:?}",
            p.over_edges.join(", "),
            steps,
            p.direction
        ),
        access,
    )
    .with_profile(op)
    .child(start);

    let needs_dst_vertices = p.yield_clause.columns.iter().any(|col| {
        matches!(&col.expression, Expression::DstVertexProp { .. })
            || matches!(&col.expression, Expression::Identifier(name) if name == "vertex")
    });
    let input = if needs_dst_vertices {
        PlanNode::new(
            "GetVertices",
            "batch destination projection",
            AccessPath::PointLookup,
        )
        .with_profile(ProfileOp::GetVertices)
        .child(neighbors)
    } else {
        neighbors
    };

    let proj_detail = yield_columns(&p.yield_clause);
    PlanNode::new("Project", proj_detail, AccessPath::None)
        .with_profile(ProfileOp::Project)
        .child(input)
}

async fn build_lookup(ctx: &ExecutionContext, p: &LookupPlan) -> PlanNode {
    let (kind, name) = match &p.lookup_type {
        LookupType::Tag(t) => ("Tag", t.clone()),
        LookupType::Edge(e) => ("Edge", e.clone()),
    };
    let access = lookup_access(ctx, p).await;
    let where_str = p
        .where_clause
        .as_ref()
        .map(expr_to_string)
        .unwrap_or_else(|| "(all)".to_string());

    let (op, scan_op) = match &access {
        AccessPath::Index(_) => ("IndexScan", ProfileOp::IndexScan),
        _ => ("TagScan", ProfileOp::FullScan),
    };
    let scan = PlanNode::new(
        op,
        format!("on {} {} where {}", kind, name, where_str),
        access,
    )
    .with_profile(scan_op);

    let mut node = scan;
    if let Some(limit) = p.limit {
        node = PlanNode::new("Limit", format!("{}", limit), AccessPath::None).child(node);
    }
    let cols = match &p.lookup_type {
        LookupType::Tag(t) => format!("{}.vid", t),
        LookupType::Edge(e) => format!("{}.src,{}.dst", e, e),
    };
    PlanNode::new("Project", cols, AccessPath::None).child(node)
}

/// For a single-edge path `(start)-[e]->(end)` with `WHERE id(end)==X`, return
/// the end node. This is the case `MatchExecutor` serves via the reverse-edge
/// index — it starts from the id-bound end vertex (point lookup) and
/// reverse-expands, never scanning the start label — so EXPLAIN must reflect
/// that rather than a misleading start-label full scan.
fn reverse_single_edge_end(p: &crate::plan::MatchPlan) -> Option<&NodePattern> {
    if !p.optional_patterns.is_empty() {
        return None;
    }
    let path = match &p.pattern {
        Pattern::Path(path) if path.edges.len() == 1 => path,
        _ => return None,
    };
    let end = path.nodes.first()?;
    let end_var = end.variable.as_deref()?;
    let bindings = crate::match_impl::extract_id_eq_bindings(p.where_clause.as_ref()?);
    bindings.contains_key(end_var).then_some(end)
}

async fn build_match(ctx: &ExecutionContext, p: &crate::plan::MatchPlan) -> PlanNode {
    let start_node = pattern_start(&p.pattern);
    let edge_count = pattern_edge_count(&p.pattern);
    let reverse_end = reverse_single_edge_end(p);

    // Start-node scan. For the reverse single-edge optimisation, execution
    // starts from the id-bound END node (point lookup) and reads the
    // reverse-edge index; otherwise from the start node via index / tag-vid /
    // full scan.
    let (start_access, start_op, start_label) = if let Some(end) = reverse_end {
        let label = end.labels.first().cloned().unwrap_or_default();
        (AccessPath::PointLookup, ProfileOp::GetVertices, label)
    } else {
        match start_node {
            Some(node) => {
                let access = match_start_access(ctx, node).await;
                let op = match &access {
                    AccessPath::Index(_) => ProfileOp::IndexScan,
                    AccessPath::TagVidIndex => ProfileOp::TagVidScan,
                    _ => ProfileOp::FullScan,
                };
                let label = node.labels.first().cloned().unwrap_or_default();
                (access, op, label)
            }
            None => (AccessPath::FullScan, ProfileOp::FullScan, String::new()),
        }
    };
    let scan_detail = if start_label.is_empty() {
        "all vertices".to_string()
    } else {
        format!("label={}", start_label)
    };
    let mut node = PlanNode::new("NodeScan", scan_detail, start_access).with_profile(start_op);

    // Edge expansion: reverse single-edge expands via the reverse-edge index,
    // forward expansion via the source-bounded edge prefix.
    if edge_count > 0 {
        let expand_access = if reverse_end.is_some() {
            AccessPath::ReverseEdgeIndex
        } else {
            AccessPath::EdgePrefix
        };
        let ranges = pattern_var_length_ranges(&p.pattern);
        let detail = if ranges.is_empty() {
            format!("{} hop(s)", edge_count)
        } else {
            let spec = ranges
                .iter()
                .map(|(lo, hi)| format!("*{}..{}", lo, hi))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} hop(s), var-length {}", edge_count, spec)
        };
        node = PlanNode::new("Expand", detail, expand_access)
            .with_profile(ProfileOp::Expand)
            .child(node);
    }

    // Comma-separated multi-pattern → inner join.
    if let Pattern::Multiple(patterns) = &p.pattern {
        if patterns.len() > 1 {
            node = PlanNode::new(
                "Join",
                format!("inner, {} patterns", patterns.len()),
                AccessPath::None,
            )
            .with_profile(ProfileOp::Join)
            .child(node);
        }
    }

    // OPTIONAL MATCH → left join.
    if !p.optional_patterns.is_empty() {
        node = PlanNode::new(
            "OptionalJoin",
            format!("left, {} optional pattern(s)", p.optional_patterns.len()),
            AccessPath::None,
        )
        .with_profile(ProfileOp::Join)
        .child(node);
    }

    // WHERE post-filter.
    if let Some(w) = &p.where_clause {
        node = PlanNode::new("Filter", expr_to_string(w), AccessPath::None)
            .with_profile(ProfileOp::Filter)
            .child(node);
    }

    // OFFSET / LIMIT.
    if p.offset.is_some() || p.limit.is_some() {
        let detail = format!(
            "offset={} limit={}",
            p.offset
                .map(|o| o.to_string())
                .unwrap_or_else(|| "-".into()),
            p.limit.map(|l| l.to_string()).unwrap_or_else(|| "-".into()),
        );
        node = PlanNode::new("Limit", detail, AccessPath::None).child(node);
    }

    let proj = p
        .return_clause
        .as_ref()
        .map(|cols| {
            cols.iter()
                .map(|c| {
                    c.alias
                        .clone()
                        .unwrap_or_else(|| expr_to_string(&c.expression))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "*".to_string());

    // Aggregate (COUNT/SUM/…) or GROUP BY reduces rows; otherwise plain project.
    let is_aggregate = p.group_by.is_some()
        || p.return_clause
            .as_ref()
            .is_some_and(|cols| cols.iter().any(|c| is_aggregate_expr(&c.expression)));
    if is_aggregate {
        let detail = match &p.group_by {
            Some(keys) => format!(
                "group by [{}] → {}",
                keys.iter()
                    .map(expr_to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                proj
            ),
            None => proj,
        };
        PlanNode::new("Aggregate", detail, AccessPath::None)
            .with_profile(ProfileOp::Aggregate)
            .child(node)
    } else {
        PlanNode::new("Project", proj, AccessPath::None)
            .with_profile(ProfileOp::Project)
            .child(node)
    }
}

fn is_aggregate_expr(expr: &Expression) -> bool {
    matches!(expr, Expression::FunctionCall { name, .. }
        if matches!(name.to_uppercase().as_str(), "COUNT" | "SUM" | "AVG" | "MAX" | "MIN"))
}

fn build_find(p: &crate::plan::FindPlan) -> PlanNode {
    let mut algo = match (p.weight_prop.as_ref(), &p.find_type) {
        (Some(w), _) => format!("Dijkstra (weight by {})", w),
        (None, crate::plan::FindType::ShortestPath) => "BFS shortest path".to_string(),
        (None, crate::plan::FindType::AllShortestPaths) => "BFS all shortest paths".to_string(),
        (None, crate::plan::FindType::Path) => "BFS all paths".to_string(),
    };
    if p.bidirect {
        algo.push_str(" BIDIRECT");
    }
    let neighbors = PlanNode::new(
        "GetNeighbors",
        format!(
            "over [{}]{}",
            p.over_edge,
            if p.bidirect {
                " + reverse-edge index"
            } else {
                ""
            }
        ),
        AccessPath::EdgePrefix,
    )
    .with_profile(ProfileOp::GetNeighbors);
    PlanNode::new("PathFind", algo, AccessPath::None)
        .with_profile(ProfileOp::PathFind)
        .child(neighbors)
}

fn build_fetch(p: &crate::plan::FetchPlan) -> PlanNode {
    if p.is_edge_fetch {
        PlanNode::new(
            "GetEdges",
            format!("on [{}], {} ref(s)", p.tags.join(", "), p.edge_refs.len()),
            AccessPath::EdgePrefix,
        )
        .with_profile(ProfileOp::GetEdges)
    } else {
        PlanNode::new(
            "GetVertices",
            format!("on [{}], {} vid(s)", p.tags.join(", "), p.vids.len()),
            AccessPath::PointLookup,
        )
        .with_profile(ProfileOp::GetVertices)
    }
}

// ===== Access-path resolution (static, EXPLAIN) =====

async fn lookup_access(ctx: &ExecutionContext, p: &LookupPlan) -> AccessPath {
    // Only tag lookups currently have an index path in the executor.
    if let LookupType::Tag(tag) = &p.lookup_type {
        if let (Some(im), Some(expr)) = (ctx.index_manager.as_ref(), p.where_clause.as_ref()) {
            if let Some(field) = lookup_index_field(expr) {
                if let Some(value) = range_lookup_literal(expr) {
                    let Some(space) = ctx.space.as_deref() else {
                        return AccessPath::FullScan;
                    };
                    if crate::executor::range_index_boundary(ctx, space, tag, &field, &value)
                        .await
                        .is_none()
                    {
                        return AccessPath::FullScan;
                    }
                }
                let space_id = ctx.resolve_space_id().await;
                let indexes = im.list_tag_indexes(space_id).await;
                if let Some(idx) = indexes
                    .iter()
                    .find(|i| i.fields.len() == 1 && i.fields[0] == field)
                {
                    return AccessPath::Index(idx.index_name.clone());
                }
            }
        }
    }
    AccessPath::FullScan
}

fn range_lookup_literal(expr: &Expression) -> Option<Value> {
    let Expression::BinaryOp { op, left, right } = expr else {
        return None;
    };
    if !matches!(
        op,
        BinaryOperator::Lt | BinaryOperator::Lte | BinaryOperator::Gt | BinaryOperator::Gte
    ) {
        return None;
    }
    literal_value(right).or_else(|| literal_value(left))
}

async fn match_start_access(ctx: &ExecutionContext, node: &NodePattern) -> AccessPath {
    let label = node.labels.first();

    // Literal property predicates → try a tag index.
    let props: Vec<(String, Value)> = node
        .props
        .iter()
        .filter_map(|(f, e)| literal_value(e).map(|v| (f.clone(), v)))
        .collect();

    if !props.is_empty() {
        if let (Some(im), Some(label)) = (ctx.index_manager.as_ref(), label) {
            let space_id = ctx.resolve_space_id().await;
            let indexes = im.list_tag_indexes(space_id).await;
            let label_idx: Vec<&IndexDef> = indexes
                .iter()
                .filter(|i| i.schema_name.eq_ignore_ascii_case(label))
                .collect();
            if let Some((idx, _)) = crate::match_impl::pick_index_plan(&label_idx, &props) {
                return AccessPath::Index(idx.index_name.clone());
            }
        }
        return AccessPath::FullScan;
    }

    // Label-only → tag-vid secondary index when it has entries.
    if let Some(label) = label {
        let space = ctx.space.as_deref().unwrap_or("");
        let prefix = format!("{}:tagvid:{}:", space, label);
        if let Ok(res) = ctx
            .kvstore
            .scan_prefix_limited(prefix.as_bytes(), Some(1))
            .await
        {
            if !res.is_empty() {
                return AccessPath::TagVidIndex;
            }
        }
    }

    AccessPath::FullScan
}

// ===== Profile overlay (PROFILE) =====

/// Overlay runtime profile records onto the tree, plus root totals.
pub fn overlay_profile(
    tree: &mut PlanNode,
    records: &[ProfileRecord],
    total_rows: u64,
    total_us: u64,
) {
    // Aggregate records by operator kind (e.g. GetNeighbors/Expand fire per hop).
    let mut by_op: HashMap<ProfileOp, (u64, u64, bool, String)> = HashMap::new();
    for r in records {
        let e = by_op.entry(r.op).or_insert((0, 0, false, String::new()));
        e.0 += r.rows;
        e.1 += r.time_us;
        e.2 |= r.full_scan;
        if e.3.is_empty() {
            e.3 = r.detail.clone();
        }
    }
    apply_overlay(tree, &by_op);
    // The root is the output/projection operator: report the final row count
    // and the whole query's wall-clock time.
    tree.rows = Some(total_rows);
    tree.time_us = Some(total_us);
}

fn apply_overlay(node: &mut PlanNode, by_op: &HashMap<ProfileOp, (u64, u64, bool, String)>) {
    if let Some(op) = node.profile_op {
        if let Some((rows, time_us, full_scan, detail)) = by_op.get(&op) {
            node.rows = Some(*rows);
            node.time_us = Some(*time_us);
            if *full_scan && !node.access.is_full_scan() {
                node.access = AccessPath::FullScan;
            }
            // Append the runtime detail only when it adds information beyond
            // the static node detail (avoids "1 hop(s) | 1 hop(s)" echoes).
            if !detail.is_empty() && !node.detail.contains(detail.as_str()) {
                node.detail = if node.detail.is_empty() {
                    detail.clone()
                } else {
                    format!("{} | {}", node.detail, detail)
                };
            }
        }
    }
    for child in &mut node.children {
        apply_overlay(child, by_op);
    }
}

// ===== Rendering to a DataSet =====

/// Render the tree as a multi-row `ExecutorResult`. `profile = true` adds the
/// rows/time columns.
pub fn render(tree: &PlanNode, profile: bool) -> ExecutorResult {
    let mut rows = Vec::new();
    let mut id = 0usize;
    walk(tree, 0, &mut id, profile, &mut rows);

    let columns: Vec<String> = if profile {
        vec!["id", "operator", "rows", "time(us)", "access", "detail"]
    } else {
        vec!["id", "operator", "access", "detail"]
    }
    .into_iter()
    .map(String::from)
    .collect();

    ExecutorResult {
        columns,
        rows,
        latency_ms: 0,
    }
}

fn walk(node: &PlanNode, depth: usize, id: &mut usize, profile: bool, out: &mut Vec<Vec<Value>>) {
    let my_id = *id as i64;
    *id += 1;

    let indent = "  ".repeat(depth);
    let operator = format!("{}{}", indent, node.operator);

    let mut row = vec![Value::Int(my_id), Value::String(operator)];
    if profile {
        row.push(
            node.rows
                .map(|r| Value::Int(r as i64))
                .unwrap_or(Value::null()),
        );
        row.push(
            node.time_us
                .map(|t| Value::Int(t as i64))
                .unwrap_or(Value::null()),
        );
    }
    row.push(Value::String(node.access.display()));
    row.push(Value::String(node.detail.clone()));
    out.push(row);

    for child in &node.children {
        walk(child, depth + 1, id, profile, out);
    }
}

// ===== Small helpers =====

fn yield_columns(y: &crate::plan::YieldClause) -> String {
    if y.columns.is_empty() {
        "src, dst".to_string()
    } else {
        y.columns
            .iter()
            .map(|c| {
                c.alias
                    .clone()
                    .unwrap_or_else(|| expr_to_string(&c.expression))
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn pattern_start(pattern: &Pattern) -> Option<&NodePattern> {
    match pattern {
        Pattern::Path(p) => Some(&p.start),
        Pattern::Multiple(ps) => match ps.first() {
            Some(Pattern::Path(p)) => Some(&p.start),
            _ => None,
        },
    }
}

fn pattern_edge_count(pattern: &Pattern) -> usize {
    match pattern {
        Pattern::Path(p) => p.edges.len(),
        Pattern::Multiple(ps) => ps
            .iter()
            .map(|p| match p {
                Pattern::Path(pp) => pp.edges.len(),
                Pattern::Multiple(_) => 0,
            })
            .sum(),
    }
}

/// Collect `*min..max` ranges across the pattern so EXPLAIN can surface
/// variable-length hops in the Expand node.
fn pattern_var_length_ranges(pattern: &Pattern) -> Vec<(u64, u64)> {
    fn from_path(p: &byoridb_parser::ast::PathPattern) -> Vec<(u64, u64)> {
        p.edges.iter().filter_map(|e| e.range).collect()
    }
    match pattern {
        Pattern::Path(p) => from_path(p),
        Pattern::Multiple(ps) => ps
            .iter()
            .flat_map(|p| match p {
                Pattern::Path(pp) => from_path(pp),
                Pattern::Multiple(_) => Vec::new(),
            })
            .collect(),
    }
}

fn lookup_index_field(expr: &Expression) -> Option<String> {
    // Recognize both bare identifiers (`name == "x"`) and qualified property
    // refs (`person.name == "x"`) — mirrors dql.rs `field_name_of` (PR#7);
    // the stale Identifier-only copy here made EXPLAIN report FULL SCAN for
    // lookups the executor actually serves from the index.
    fn field_of(e: &Expression) -> Option<String> {
        match e {
            Expression::Identifier(f) => Some(f.clone()),
            Expression::PropRef { prop, .. } => Some(prop.clone()),
            _ => None,
        }
    }
    fn ordered_range_literal(e: &Expression) -> bool {
        matches!(
            e,
            Expression::Literal(Literal::Int(_))
                | Expression::Literal(Literal::Float(_))
                | Expression::Literal(Literal::Bool(_))
        ) || matches!(
            e,
            Expression::UnaryOp {
                op: byoridb_parser::ast::UnaryOperator::Neg,
                operand,
            } if matches!(operand.as_ref(), Expression::Literal(Literal::Int(_) | Literal::Float(_)))
        )
    }

    let Expression::BinaryOp { op, left, right } = expr else {
        return None;
    };
    match op {
        BinaryOperator::Eq => {
            if literal_value(right).is_some() {
                field_of(left)
            } else if literal_value(left).is_some() {
                field_of(right)
            } else {
                None
            }
        }
        BinaryOperator::Lt | BinaryOperator::Lte | BinaryOperator::Gt | BinaryOperator::Gte => {
            if ordered_range_literal(right) {
                field_of(left)
            } else if ordered_range_literal(left) {
                field_of(right)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn literal_value(expr: &Expression) -> Option<Value> {
    match expr {
        Expression::Literal(Literal::Int(i)) => Some(Value::Int(*i)),
        Expression::Literal(Literal::Float(f)) => Some(Value::Float(*f)),
        Expression::Literal(Literal::String(s)) => Some(Value::String(s.clone())),
        Expression::Literal(Literal::Bool(b)) => Some(Value::Bool(*b)),
        Expression::Literal(Literal::Null) => Some(Value::null()),
        Expression::UnaryOp {
            op: byoridb_parser::ast::UnaryOperator::Neg,
            operand,
        } => match literal_value(operand) {
            Some(Value::Int(value)) => Some(Value::Int(-value)),
            Some(Value::Float(value)) => Some(Value::Float(-value)),
            _ => None,
        },
        _ => None,
    }
}

/// Compact, human-readable rendering of an expression for plan detail columns.
fn expr_to_string(expr: &Expression) -> String {
    match expr {
        Expression::Identifier(n) => n.clone(),
        Expression::Literal(Literal::Int(i)) => i.to_string(),
        Expression::Literal(Literal::Float(f)) => f.to_string(),
        Expression::Literal(Literal::String(s)) => format!("\"{}\"", s),
        Expression::Literal(Literal::Bool(b)) => b.to_string(),
        Expression::Literal(Literal::Null) => "null".to_string(),
        Expression::PropRef { object, prop } => format!("{}.{}", object, prop),
        Expression::DstVertexProp { tag, prop } => format!("$$.{}.{}", tag, prop),
        Expression::BinaryOp { op, left, right } => {
            format!(
                "{} {} {}",
                expr_to_string(left),
                binop_str(op),
                expr_to_string(right)
            )
        }
        Expression::UnaryOp { op, operand } => format!("{:?} {}", op, expr_to_string(operand)),
        Expression::FunctionCall { name, args } => {
            let a = args
                .iter()
                .map(expr_to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({})", name, a)
        }
        other => format!("{:?}", other),
    }
}

fn binop_str(op: &BinaryOperator) -> &'static str {
    match op {
        BinaryOperator::Eq => "==",
        BinaryOperator::Neq => "!=",
        BinaryOperator::Lt => "<",
        BinaryOperator::Lte => "<=",
        BinaryOperator::Gt => ">",
        BinaryOperator::Gte => ">=",
        BinaryOperator::And => "AND",
        BinaryOperator::Or => "OR",
        BinaryOperator::Add => "+",
        BinaryOperator::Sub => "-",
        BinaryOperator::Mul => "*",
        BinaryOperator::Div => "/",
        BinaryOperator::Mod => "%",
        BinaryOperator::Contains => "CONTAINS",
        BinaryOperator::NotContains => "NOT CONTAINS",
        BinaryOperator::StartsWith => "STARTS WITH",
        BinaryOperator::EndsWith => "ENDS WITH",
        BinaryOperator::Regex => "=~",
    }
}

/// Short label for a plan variant used by the single-node fallback.
fn plan_kind(plan: &ExecutionPlan) -> &'static str {
    match plan {
        ExecutionPlan::Show(_) => "Show",
        ExecutionPlan::Describe(_) => "Describe",
        ExecutionPlan::Use(_) => "Use",
        ExecutionPlan::Create(_) => "Create",
        ExecutionPlan::Alter(_) => "Alter",
        ExecutionPlan::Drop(_) => "Drop",
        ExecutionPlan::Grant(_) => "Grant",
        ExecutionPlan::Revoke(_) => "Revoke",
        ExecutionPlan::Balance(_) => "Balance",
        ExecutionPlan::Insert(_) => "Insert",
        ExecutionPlan::Update(_) => "Update",
        ExecutionPlan::Delete(_) => "Delete",
        ExecutionPlan::DeleteEdge(_) => "DeleteEdge",
        ExecutionPlan::Fetch(_) => "Fetch",
        ExecutionPlan::Find(_) => "Find",
        ExecutionPlan::Match(_) => "Match",
        ExecutionPlan::Go(_) => "Go",
        ExecutionPlan::Lookup(_) => "Lookup",
        ExecutionPlan::Recommend(_) => "Recommend",
        ExecutionPlan::CheckConsistency => "CheckConsistency",
        ExecutionPlan::CheckShape => "CheckShape",
        ExecutionPlan::ExplainInference { .. } => "ExplainInference",
        ExecutionPlan::Compound(_) => "Compound",
        ExecutionPlan::Explain { .. } => "Explain",
    }
}

#[cfg(test)]
mod tests {
    use crate::context::ExecutionContext;
    use crate::executor::{Executor, ExecutorResult};
    use crate::plan::ExecutionPlanBuilder;
    use byoridb_codec::{EdgeData, TagData, VertexCodec, VertexData};
    use byoridb_kvstore::store::MemoryKVStore;
    use byoridb_kvstore::KVStore as _;
    use std::sync::Arc;

    /// Executor over a single `post` vertex with its tag-vid index entry, so a
    /// label-only MATCH can resolve via the tag-vid secondary index.
    async fn exec_with_post() -> Executor {
        let kv = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kv.clone())
                .with_space("default".to_string())
                .with_space_id(1),
        );
        let blob = VertexCodec::encode_vertex(&VertexData {
            vid: 1,
            tags: vec![TagData {
                name: "post".to_string(),
                properties: Default::default(),
            }],
        })
        .unwrap();
        kv.put(b"default:vertex:1", &blob).await.unwrap();
        kv.put(b"default:tagvid:post:1", &[]).await.unwrap();
        Executor::new(ctx)
    }

    async fn exec_with_age_index() -> Executor {
        let kv = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kv.clone())
                .with_space("default".to_string())
                .with_space_id(1),
        );
        kv.put(
            &crate::key::SchemaKey::tag("default", "person"),
            &serde_json::to_vec(&serde_json::json!({
                "name": "person",
                "properties": [{"name": "age", "data_type": "Int64"}]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        let index_manager = ctx.index_manager.as_ref().unwrap();
        let index_id = index_manager
            .create_tag_index(
                1,
                "person_age_idx".to_string(),
                10,
                "person".to_string(),
                vec!["age".to_string()],
                vec![0],
            )
            .await
            .unwrap();
        index_manager
            .insert_tag_index(1, index_id, &[byoridb_storage::key::IndexValue::Int(40)], 1)
            .await
            .unwrap();
        let blob = VertexCodec::encode_vertex(&VertexData {
            vid: 1,
            tags: vec![TagData {
                name: "person".to_string(),
                properties: [("age".to_string(), byoridb_common::Value::Int(40))]
                    .into_iter()
                    .collect(),
            }],
        })
        .unwrap();
        kv.put(b"default:vertex:1", &blob).await.unwrap();
        Executor::new(ctx)
    }

    fn access_col(res: &ExecutorResult) -> usize {
        res.columns.iter().position(|c| c == "access").unwrap()
    }

    async fn run(exec: &Executor, q: &str) -> ExecutorResult {
        let stmt = byoridb_parser::parse(q).unwrap();
        let plan = ExecutionPlanBuilder::build(stmt).unwrap();
        exec.execute(plan).await.unwrap()
    }

    #[tokio::test]
    async fn explain_renders_a_tree_not_one_line() {
        let exec = exec_with_post().await;
        let res = run(&exec, "EXPLAIN MATCH (n:post) RETURN n").await;
        assert_eq!(res.columns, vec!["id", "operator", "access", "detail"]);
        // Project + NodeScan = at least 2 rows (a tree, not a single string).
        assert!(res.rows.len() >= 2, "expected a tree, got {:?}", res.rows);
    }

    #[tokio::test]
    async fn explain_label_only_uses_tagvid_index() {
        let exec = exec_with_post().await;
        let res = run(&exec, "EXPLAIN MATCH (n:post) RETURN n").await;
        let ai = access_col(&res);
        let has_tagvid = res
            .rows
            .iter()
            .any(|r| matches!(&r[ai], byoridb_common::Value::String(s) if s.contains("tag-vid")));
        assert!(
            has_tagvid,
            "label-only MATCH should use tag-vid index: {:?}",
            res.rows
        );
    }

    #[tokio::test]
    async fn explain_no_index_flags_full_scan() {
        // `ghost` has neither tag-vid entries nor a tag index → full scan.
        let kv = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kv)
                .with_space("default".to_string())
                .with_space_id(1),
        );
        let exec = Executor::new(ctx);
        let res = run(&exec, "EXPLAIN MATCH (n:ghost) RETURN n").await;
        let ai = access_col(&res);
        let has_full = res
            .rows
            .iter()
            .any(|r| matches!(&r[ai], byoridb_common::Value::String(s) if s.contains("FULL SCAN")));
        assert!(has_full, "no index should flag FULL SCAN: {:?}", res.rows);
    }

    #[tokio::test]
    async fn explain_and_profile_lookup_range_use_index() {
        let exec = exec_with_age_index().await;
        for query in [
            "EXPLAIN LOOKUP ON person WHERE person.age > 30",
            "PROFILE LOOKUP ON person WHERE person.age <= 40",
        ] {
            let res = run(&exec, query).await;
            let ai = access_col(&res);
            assert!(
                res.rows.iter().any(|row| matches!(
                    &row[ai],
                    byoridb_common::Value::String(access) if access.contains("person_age_idx")
                )),
                "range lookup should report its index for {query}: {:?}",
                res.rows
            );
            assert!(
                !res.rows.iter().any(|row| matches!(
                    &row[ai],
                    byoridb_common::Value::String(access) if access.contains("FULL SCAN")
                )),
                "range lookup must not report a full scan for {query}: {:?}",
                res.rows
            );
        }

        let cross_type = run(&exec, "EXPLAIN LOOKUP ON person WHERE person.age > 30.5").await;
        let ai = access_col(&cross_type);
        assert!(
            cross_type.rows.iter().any(|row| matches!(
                &row[ai],
                byoridb_common::Value::String(access) if access.contains("FULL SCAN")
            )),
            "cross-type range must retain the correctness-preserving fallback: {:?}",
            cross_type.rows
        );
    }

    #[tokio::test]
    async fn profile_attaches_rows_and_time_columns() {
        let exec = exec_with_post().await;
        let res = run(&exec, "PROFILE MATCH (n:post) RETURN n").await;
        assert_eq!(
            res.columns,
            vec!["id", "operator", "rows", "time(us)", "access", "detail"]
        );
        // The "rows" column (index 2) must carry at least one measured count.
        let has_rows = res
            .rows
            .iter()
            .any(|r| matches!(&r[2], byoridb_common::Value::Int(_)));
        assert!(has_rows, "PROFILE should attach row counts: {:?}", res.rows);
    }

    // ===== expanded instrumentation coverage =====

    fn operators(res: &ExecutorResult) -> Vec<String> {
        let oi = res.columns.iter().position(|c| c == "operator").unwrap();
        res.rows
            .iter()
            .filter_map(|r| match &r[oi] {
                byoridb_common::Value::String(s) => Some(s.trim().to_string()),
                _ => None,
            })
            .collect()
    }

    /// Two products both pointing at one category, with tag-vid index entries.
    async fn exec_graph() -> Executor {
        let kv = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kv.clone())
                .with_space("s".to_string())
                .with_space_id(1),
        );
        for (vid, tag) in [(1i64, "product"), (2, "product"), (100, "category")] {
            let b = VertexCodec::encode_vertex(&VertexData {
                vid,
                tags: vec![TagData {
                    name: tag.to_string(),
                    properties: Default::default(),
                }],
            })
            .unwrap();
            kv.put(format!("s:vertex:{vid}").as_bytes(), &b)
                .await
                .unwrap();
            kv.put(format!("s:tagvid:{tag}:{vid}").as_bytes(), &[])
                .await
                .unwrap();
        }
        for src in [1i64, 2] {
            let e = VertexCodec::encode_edge(&EdgeData {
                src_vid: src,
                dst_vid: 100,
                edge_type: "belongs_to".to_string(),
                ranking: 0,
                properties: Default::default(),
            })
            .unwrap();
            kv.put(format!("s:edge:{src}:belongs_to:100:0").as_bytes(), &e)
                .await
                .unwrap();
        }
        Executor::new(ctx)
    }

    #[tokio::test]
    async fn profile_go_has_getneighbors_and_project() {
        let exec = exec_graph().await;
        let res = run(&exec, "PROFILE GO FROM 1 OVER belongs_to").await;
        let ops = operators(&res);
        assert!(ops.iter().any(|o| o == "GetNeighbors"), "{:?}", ops);
        assert!(ops.iter().any(|o| o == "Project"), "{:?}", ops);
    }

    #[tokio::test]
    async fn profile_go_destination_projection_reports_batch_getvertices() {
        let exec = exec_graph().await;
        let res = run(
            &exec,
            "PROFILE GO FROM 1 OVER belongs_to YIELD $$.category.name",
        )
        .await;
        let ops = operators(&res);
        assert!(ops.iter().any(|o| o == "GetVertices"), "{:?}", ops);
        let detail_idx = res.columns.iter().position(|c| c == "detail").unwrap();
        assert!(res.rows.iter().any(|row| {
            matches!(&row[detail_idx], byoridb_common::Value::String(detail)
                if detail.contains("batch destination projection"))
        }));
    }

    #[tokio::test]
    async fn profile_fetch_reports_getvertices() {
        let exec = exec_graph().await;
        let res = run(&exec, "PROFILE FETCH PROP ON product 1").await;
        let ops = operators(&res);
        assert!(ops.iter().any(|o| o == "GetVertices"), "{:?}", ops);
    }

    #[tokio::test]
    async fn profile_count_uses_aggregate_node() {
        let exec = exec_graph().await;
        let res = run(&exec, "PROFILE MATCH (p:product) RETURN count(p)").await;
        let ops = operators(&res);
        assert!(ops.iter().any(|o| o == "Aggregate"), "{:?}", ops);
        // count(2 products) reported on the aggregate root
        assert!(matches!(&res.rows[0][2], byoridb_common::Value::Int(_)));
    }

    #[tokio::test]
    async fn profile_reverse_single_edge_uses_reverse_edge_index() {
        // WHERE id(c)==X on a single edge triggers the reverse-edge index path:
        // start from the id-bound end vertex and reverse-expand — O(in-degree),
        // NOT a full scan. EXPLAIN must surface the reverse-edge index and the
        // full-scan flag must stay clear (PLAN.md O-1).
        let exec = exec_graph().await;
        let res = run(
            &exec,
            "PROFILE MATCH (p:product)-[:belongs_to]->(c:category) WHERE id(c)==100 RETURN p",
        )
        .await;
        let ai = access_col(&res);
        let has_reverse_index = res
            .rows
            .iter()
            .any(|r| matches!(&r[ai], byoridb_common::Value::String(s) if s.contains("reverse-edge index")));
        assert!(
            has_reverse_index,
            "reverse single-edge should use the reverse-edge index: {:?}",
            res.rows
        );
        let has_full = res
            .rows
            .iter()
            .any(|r| matches!(&r[ai], byoridb_common::Value::String(s) if s.contains("FULL SCAN")));
        assert!(
            !has_full,
            "reverse single-edge must not full scan: {:?}",
            res.rows
        );
        assert!(
            !exec.ctx().took_full_scan(),
            "full_scan flag must stay clear for the reverse-edge index path"
        );
    }

    #[tokio::test]
    async fn explain_where_clause_shows_filter_node() {
        let exec = exec_graph().await;
        let res = run(
            &exec,
            "EXPLAIN MATCH (p:product)-[:belongs_to]->(c:category) WHERE id(c)==100 RETURN p",
        )
        .await;
        let ops = operators(&res);
        assert!(ops.iter().any(|o| o == "Filter"), "{:?}", ops);
        assert!(ops.iter().any(|o| o == "Expand"), "{:?}", ops);
    }
}
