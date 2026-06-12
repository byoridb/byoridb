// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Execution plan for nGQL queries

use crate::error::Result;
use byoridb_parser::ast::{Expression, YieldColumn as ParserYieldColumn};
use byoridb_parser::Statement;
use serde::Serialize;

/// Execution plan for a query
pub enum ExecutionPlan {
    /// Show spaces, tags, edges, users
    Show(ShowPlan),

    /// Describe the schema of a tag, edge, or space
    Describe(DescribePlan),

    /// Use a specific space
    Use(UsePlan),

    /// Create space, tag, edge, index, user
    Create(CreatePlan),

    /// Alter tag or edge schema (add columns)
    Alter(AlterPlan),

    /// Drop space, tag, edge, index, user
    Drop(DropPlan),

    /// Grant role to user
    Grant(GrantPlan),

    /// Revoke role from user
    Revoke(RevokePlan),

    /// Balance operations for partition management
    Balance(BalancePlan),

    /// Insert vertices or edges
    Insert(InsertPlan),

    /// Update vertices or edges
    Update(UpdatePlan),

    /// Delete vertices
    Delete(DeletePlan),

    /// Delete edges
    DeleteEdge(DeleteEdgePlan),

    /// Fetch vertex or edge data
    Fetch(FetchPlan),

    /// Find paths
    Find(FindPlan),

    /// Match pattern (Cypher-style)
    Match(MatchPlan),

    /// GO traversal
    Go(GoPlan),

    /// LOOKUP query
    Lookup(LookupPlan),

    /// Compound query — a sequence of clauses where each may bind its
    /// `ExecutorResult` to a variable consumed by subsequent clauses.
    /// The final non-assignment clause's result is the query output.
    Compound(Vec<CompoundPlanClause>),

    /// EXPLAIN/PROFILE — `profile = false` returns the logical plan without
    /// executing; `profile = true` executes and annotates each operator with
    /// runtime metrics collected via the context's profile collector.
    Explain {
        profile: bool,
        plan: Box<ExecutionPlan>,
    },
}

/// One clause inside an [`ExecutionPlan::Compound`].
pub struct CompoundPlanClause {
    pub var: Option<String>,
    pub plan: Box<ExecutionPlan>,
}

pub enum ShowPlan {
    Spaces,
    Tags,
    Edges,
    TagIndexes,
    EdgeIndexes,
    Users,
    Parts,
    Hosts,
    Stats,
    Sessions,
    CreateTag(String),
    CreateEdge(String),
    TagIndexStatuses,
    EdgeIndexStatuses,
}

/// DESCRIBE plan — identify the schema object to describe.
pub enum DescribePlan {
    Tag(String),
    Edge(String),
    Space(String),
    TagIndex(String),
    EdgeIndex(String),
}

/// Balance plan for partition management
pub enum BalancePlan {
    /// Trigger leader rebalance
    Leader,
    /// Trigger data rebalance
    Data,
    /// Show balance status
    Status,
    /// Stop ongoing balance
    Stop,
    /// Reset balance plan
    Reset,
}

pub struct UsePlan {
    pub space: String,
}

pub enum CreatePlan {
    Space {
        name: String,
        if_not_exists: bool,
        partition_num: u32,
        replica_factor: u32,
        vid_type: String,
        partition_strategy: byoridb_common::PartitionStrategy,
    },
    Tag {
        name: String,
        if_not_exists: bool,
        props: Vec<PropertyDef>,
    },
    Edge {
        name: String,
        if_not_exists: bool,
        props: Vec<PropertyDef>,
    },
    TagIndex {
        name: String,
        tag_name: String,
        props: Vec<String>,
    },
    EdgeIndex {
        name: String,
        edge_name: String,
        props: Vec<String>,
    },
    User {
        name: String,
        if_not_exists: bool,
        password: String,
        role: Option<String>,
    },
}

pub enum DropPlan {
    Space { name: String, if_exists: bool },
    Tag { name: String, if_exists: bool },
    Edge { name: String, if_exists: bool },
    TagIndex { name: String, if_exists: bool },
    EdgeIndex { name: String, if_exists: bool },
    User { name: String, if_exists: bool },
}

pub enum AlterPlan {
    Tag {
        name: String,
        operations: Vec<AlterColumnOp>,
    },
    Edge {
        name: String,
        operations: Vec<AlterColumnOp>,
    },
    User {
        name: String,
        new_password: Option<String>,
    },
}

pub struct GrantPlan {
    pub role: String,
    pub username: String,
}

pub struct RevokePlan {
    pub role: String,
    pub username: String,
}

pub struct AlterColumnOp {
    pub op_type: AlterOpType,
    pub prop: PropertyDef,
}

pub enum AlterOpType {
    AddColumn,
    DropColumn,
    ChangeColumn,
}

pub enum InsertPlan {
    Vertex {
        space: String,
        vertices: Vec<VertexInsert>,
    },
    Edge {
        space: String,
        edges: Vec<EdgeInsert>,
    },
}

pub struct VertexInsert {
    pub vid: i64,
    pub tags: Vec<TagData>,
}

#[derive(Serialize)]
pub struct TagData {
    pub name: String,
    pub props: std::collections::HashMap<String, byoridb_common::Value>,
}

pub struct EdgeInsert {
    pub src: i64,
    pub dst: i64,
    pub edge_type: String,
    pub ranking: i64,
    pub props: std::collections::HashMap<String, byoridb_common::Value>,
}

pub struct UpdatePlan {
    pub space: String,
    pub vid: i64,
    pub tag_name: Option<String>,
    pub updates: std::collections::HashMap<String, byoridb_common::Value>,
    pub conditions: Option<Expression>,
    pub yield_clause: Option<String>,
}

pub struct DeletePlan {
    pub space: String,
    pub vids: Vec<i64>,
    pub conditions: Option<Expression>,
}

pub struct DeleteEdgePlan {
    pub space: String,
    pub edge_name: String,
    /// (src_vid, dst_vid, ranking)
    pub edge_refs: Vec<(i64, i64, i64)>,
}

pub struct FetchPlan {
    pub space: String,
    /// Vertex VIDs for `FETCH PROP ON tag_name vid1, vid2`
    pub vids: Vec<i64>,
    pub tags: Vec<String>,
    pub yield_clause: Option<String>,
    /// Edge refs `(src, dst)` for `FETCH PROP ON edge_type src->dst`
    pub edge_refs: Vec<(i64, i64)>,
    /// When true the plan fetches edge data; otherwise vertex data
    pub is_edge_fetch: bool,
    /// `$var.col` variable reference — resolved at runtime from ctx.vars
    pub src_var: Option<String>,
}

pub struct FindPlan {
    pub find_type: FindType,
    pub from_vid: Expression,
    pub to_vid: Expression,
    pub over_edge: String,
    pub weight_prop: Option<String>,
    /// Expand edges in both directions (reads the O-1 in-edge index for the
    /// reverse direction). See `FindStatement::bidirect`.
    pub bidirect: bool,
    pub upto_steps: Option<u32>,
    pub where_clause: Option<Expression>,
    pub yield_clause: Option<String>,
}

#[derive(Debug)]
pub enum FindType {
    Path,
    ShortestPath,
    AllShortestPaths,
}

pub struct MatchReturnColumn {
    pub expression: Expression,
    pub alias: Option<String>,
}

pub struct MatchPlan {
    pub pattern: byoridb_parser::ast::Pattern,
    pub where_clause: Option<Expression>,
    pub optional_patterns: Vec<byoridb_parser::ast::Pattern>,
    pub return_clause: Option<Vec<MatchReturnColumn>>,
    pub group_by: Option<Vec<Expression>>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub struct GoPlan {
    pub from_clause: FromClause,
    pub over_edges: Vec<String>,
    pub direction: byoridb_parser::ast::EdgeDirection,
    pub to_clause: ToClause,
    pub where_clause: Option<Expression>,
    pub yield_clause: YieldClause,
}

pub struct FromClause {
    pub vids: Vec<i64>,
    pub src: Option<String>,
}

pub struct ToClause {
    pub variable: String,
    pub steps: StepClause,
}

#[derive(Debug)]
pub enum StepClause {
    Exactly(usize),
    Range(usize, usize),
}

pub struct YieldColumn {
    pub expression: Expression,
    pub alias: Option<String>,
}

pub struct YieldClause {
    pub columns: Vec<YieldColumn>,
}

pub struct LookupPlan {
    pub lookup_type: LookupType,
    pub where_clause: Option<Expression>,
    pub yield_clause: YieldClause,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub enum LookupType {
    Tag(String),
    Edge(String),
}

#[derive(Serialize)]
pub struct PropertyDef {
    pub name: String,
    pub data_type: byoridb_parser::ast::DataType,
    pub nullable: bool,
    pub default_value: Option<byoridb_parser::ast::Expression>,
}

/// Build execution plan from parsed statement
pub struct ExecutionPlanBuilder;

impl ExecutionPlanBuilder {
    pub fn build(stmt: Statement) -> Result<ExecutionPlan> {
        match stmt {
            Statement::Show(show) => Ok(ExecutionPlan::Show(match show {
                byoridb_parser::ast::ShowStatement::Spaces => ShowPlan::Spaces,
                byoridb_parser::ast::ShowStatement::Tags => ShowPlan::Tags,
                byoridb_parser::ast::ShowStatement::Edges => ShowPlan::Edges,
                byoridb_parser::ast::ShowStatement::TagIndexes => ShowPlan::TagIndexes,
                byoridb_parser::ast::ShowStatement::EdgeIndexes => ShowPlan::EdgeIndexes,
                byoridb_parser::ast::ShowStatement::Users => ShowPlan::Users,
                byoridb_parser::ast::ShowStatement::Roles => ShowPlan::Users,
                byoridb_parser::ast::ShowStatement::Parts => ShowPlan::Parts,
                byoridb_parser::ast::ShowStatement::Hosts => ShowPlan::Hosts,
                byoridb_parser::ast::ShowStatement::Stats => ShowPlan::Stats,
                byoridb_parser::ast::ShowStatement::Sessions => ShowPlan::Sessions,
                byoridb_parser::ast::ShowStatement::CreateTag(n) => ShowPlan::CreateTag(n),
                byoridb_parser::ast::ShowStatement::CreateEdge(n) => ShowPlan::CreateEdge(n),
                byoridb_parser::ast::ShowStatement::TagIndexStatuses => ShowPlan::TagIndexStatuses,
                byoridb_parser::ast::ShowStatement::EdgeIndexStatuses => {
                    ShowPlan::EdgeIndexStatuses
                }
            })),
            Statement::Describe(desc) => Ok(ExecutionPlan::Describe(match desc {
                byoridb_parser::ast::DescribeStatement::Tag(n) => DescribePlan::Tag(n),
                byoridb_parser::ast::DescribeStatement::Edge(n) => DescribePlan::Edge(n),
                byoridb_parser::ast::DescribeStatement::Space(n) => DescribePlan::Space(n),
                byoridb_parser::ast::DescribeStatement::TagIndex(n) => DescribePlan::TagIndex(n),
                byoridb_parser::ast::DescribeStatement::EdgeIndex(n) => DescribePlan::EdgeIndex(n),
            })),
            Statement::Use(use_stmt) => Ok(ExecutionPlan::Use(UsePlan {
                space: use_stmt.space,
            })),
            Statement::Create(create) => Ok(ExecutionPlan::Create(match create {
                byoridb_parser::ast::CreateStatement::Space(space) => CreatePlan::Space {
                    name: space.name,
                    if_not_exists: space.if_not_exists,
                    partition_num: space.partition_num.unwrap_or(100),
                    replica_factor: space.replica_factor.unwrap_or(1),
                    vid_type: match space.vid_type {
                        Some(byoridb_parser::ast::VidType::Int64) => "INT64".to_string(),
                        Some(byoridb_parser::ast::VidType::FixedString(len)) => {
                            format!("FIXED_STRING({})", len)
                        }
                        None => "INT64".to_string(),
                    },
                    partition_strategy: match space.partition_strategy {
                        Some(byoridb_parser::ast::PartitionStrategySpec::Hash) => {
                            byoridb_common::PartitionStrategy::Hash
                        }
                        Some(byoridb_parser::ast::PartitionStrategySpec::Range { boundaries }) => {
                            byoridb_common::PartitionStrategy::Range { boundaries }
                        }
                        Some(byoridb_parser::ast::PartitionStrategySpec::Modulo) => {
                            byoridb_common::PartitionStrategy::Modulo
                        }
                        None => byoridb_common::PartitionStrategy::Hash,
                    },
                },
                byoridb_parser::ast::CreateStatement::Tag(tag) => CreatePlan::Tag {
                    name: tag.name,
                    if_not_exists: tag.if_not_exists,
                    props: tag
                        .props
                        .into_iter()
                        .map(|p| PropertyDef {
                            name: p.name,
                            data_type: p.data_type,
                            nullable: p.nullable,
                            default_value: p.default,
                        })
                        .collect(),
                },
                byoridb_parser::ast::CreateStatement::Edge(edge) => CreatePlan::Edge {
                    name: edge.name,
                    if_not_exists: edge.if_not_exists,
                    props: edge
                        .props
                        .into_iter()
                        .map(|p| PropertyDef {
                            name: p.name,
                            data_type: p.data_type,
                            nullable: p.nullable,
                            default_value: p.default,
                        })
                        .collect(),
                },
                byoridb_parser::ast::CreateStatement::User(user) => CreatePlan::User {
                    name: user.username,
                    if_not_exists: user.if_not_exists,
                    password: user.password,
                    role: user.role,
                },
                byoridb_parser::ast::CreateStatement::TagIndex(idx) => CreatePlan::TagIndex {
                    name: idx.index_name,
                    tag_name: idx.tag_name,
                    props: idx.props,
                },
                byoridb_parser::ast::CreateStatement::EdgeIndex(idx) => CreatePlan::EdgeIndex {
                    name: idx.index_name,
                    edge_name: idx.edge_name,
                    props: idx.props,
                },
            })),
            Statement::Drop(drop) => Ok(ExecutionPlan::Drop(match drop {
                byoridb_parser::ast::DropStatement::Space(space) => DropPlan::Space {
                    name: space.name,
                    if_exists: space.if_exists,
                },
                byoridb_parser::ast::DropStatement::Tag(tag) => DropPlan::Tag {
                    name: tag.name,
                    if_exists: tag.if_exists,
                },
                byoridb_parser::ast::DropStatement::Edge(edge) => DropPlan::Edge {
                    name: edge.name,
                    if_exists: edge.if_exists,
                },
                byoridb_parser::ast::DropStatement::User(user) => DropPlan::User {
                    name: user.username,
                    if_exists: user.if_exists,
                },
                byoridb_parser::ast::DropStatement::TagIndex(idx) => DropPlan::TagIndex {
                    name: idx.index_name,
                    if_exists: idx.if_exists,
                },
                byoridb_parser::ast::DropStatement::EdgeIndex(idx) => DropPlan::EdgeIndex {
                    name: idx.index_name,
                    if_exists: idx.if_exists,
                },
            })),
            Statement::Alter(alter) => Ok(ExecutionPlan::Alter(match alter {
                byoridb_parser::ast::AlterStatement::Tag(tag_alter) => AlterPlan::Tag {
                    name: tag_alter.name,
                    operations: tag_alter
                        .operations
                        .into_iter()
                        .map(|op| match op {
                            byoridb_parser::ast::AlterOperation::AddColumn(prop) => AlterColumnOp {
                                op_type: AlterOpType::AddColumn,
                                prop: PropertyDef {
                                    name: prop.name,
                                    data_type: prop.data_type,
                                    nullable: prop.nullable,
                                    default_value: prop.default,
                                },
                            },
                            byoridb_parser::ast::AlterOperation::DropColumn(col_name) => {
                                AlterColumnOp {
                                    op_type: AlterOpType::DropColumn,
                                    prop: PropertyDef {
                                        name: col_name,
                                        data_type: byoridb_parser::ast::DataType::String,
                                        nullable: true,
                                        default_value: None,
                                    },
                                }
                            }
                            byoridb_parser::ast::AlterOperation::ChangeColumn(prop) => {
                                AlterColumnOp {
                                    op_type: AlterOpType::ChangeColumn,
                                    prop: PropertyDef {
                                        name: prop.name,
                                        data_type: prop.data_type,
                                        nullable: prop.nullable,
                                        default_value: prop.default,
                                    },
                                }
                            }
                        })
                        .collect(),
                },
                byoridb_parser::ast::AlterStatement::Edge(edge_alter) => AlterPlan::Edge {
                    name: edge_alter.name,
                    operations: edge_alter
                        .operations
                        .into_iter()
                        .map(|op| match op {
                            byoridb_parser::ast::AlterOperation::AddColumn(prop) => AlterColumnOp {
                                op_type: AlterOpType::AddColumn,
                                prop: PropertyDef {
                                    name: prop.name,
                                    data_type: prop.data_type,
                                    nullable: prop.nullable,
                                    default_value: prop.default,
                                },
                            },
                            byoridb_parser::ast::AlterOperation::DropColumn(col_name) => {
                                AlterColumnOp {
                                    op_type: AlterOpType::DropColumn,
                                    prop: PropertyDef {
                                        name: col_name,
                                        data_type: byoridb_parser::ast::DataType::String,
                                        nullable: true,
                                        default_value: None,
                                    },
                                }
                            }
                            byoridb_parser::ast::AlterOperation::ChangeColumn(prop) => {
                                AlterColumnOp {
                                    op_type: AlterOpType::ChangeColumn,
                                    prop: PropertyDef {
                                        name: prop.name,
                                        data_type: prop.data_type,
                                        nullable: prop.nullable,
                                        default_value: prop.default,
                                    },
                                }
                            }
                        })
                        .collect(),
                },
                byoridb_parser::ast::AlterStatement::User(user_alter) => AlterPlan::User {
                    name: user_alter.username,
                    new_password: user_alter.new_password,
                },
            })),
            Statement::Grant(grant) => Ok(ExecutionPlan::Grant(GrantPlan {
                role: grant.role,
                username: grant.username,
            })),
            Statement::Revoke(revoke) => Ok(ExecutionPlan::Revoke(RevokePlan {
                role: revoke.role,
                username: revoke.username,
            })),
            Statement::Balance(balance) => Ok(ExecutionPlan::Balance(match balance {
                byoridb_parser::ast::BalanceStatement::Leader => BalancePlan::Leader,
                byoridb_parser::ast::BalanceStatement::Data => BalancePlan::Data,
                byoridb_parser::ast::BalanceStatement::Status => BalancePlan::Status,
                byoridb_parser::ast::BalanceStatement::Stop => BalancePlan::Stop,
                byoridb_parser::ast::BalanceStatement::Reset => BalancePlan::Reset,
            })),
            Statement::Insert(insert) => {
                let space = insert.space.unwrap_or_default();
                Ok(ExecutionPlan::Insert(match insert.insert_type {
                    byoridb_parser::ast::InsertType::Vertex => InsertPlan::Vertex {
                        space,
                        vertices: insert
                            .vertices
                            .into_iter()
                            .map(|v| {
                                let vid = match v.vid {
                                    byoridb_parser::ast::Expression::Literal(
                                        byoridb_parser::ast::Literal::Int(i),
                                    ) => i,
                                    _ => {
                                        return Err(crate::error::ExecutionError::InvalidOperation(
                                            "Vertex ID must be an integer literal".to_string(),
                                        ))
                                    }
                                };
                                Ok(VertexInsert {
                                    vid,
                                    tags: v
                                        .tags
                                        .into_iter()
                                        .map(|t| {
                                            let mut props = std::collections::HashMap::new();
                                            for (k, v_expr) in t.props {
                                                props.insert(k, Self::expr_to_value(v_expr)?);
                                            }
                                            Ok(TagData {
                                                name: t.name,
                                                props,
                                            })
                                        })
                                        .collect::<Result<Vec<_>>>()?,
                                })
                            })
                            .collect::<Result<Vec<_>>>()?,
                    },
                    byoridb_parser::ast::InsertType::Edge => InsertPlan::Edge {
                        space,
                        edges: insert
                            .edges
                            .into_iter()
                            .map(|e| {
                                let src = match e.src_vid {
                                    byoridb_parser::ast::Expression::Literal(
                                        byoridb_parser::ast::Literal::Int(i),
                                    ) => i,
                                    _ => {
                                        return Err(crate::error::ExecutionError::InvalidOperation(
                                            "Edge source vertex ID must be an integer literal"
                                                .to_string(),
                                        ))
                                    }
                                };
                                let dst = match e.dst_vid {
                                    byoridb_parser::ast::Expression::Literal(
                                        byoridb_parser::ast::Literal::Int(i),
                                    ) => i,
                                    _ => {
                                        return Err(crate::error::ExecutionError::InvalidOperation(
                                            "Edge destination vertex ID must be an integer literal"
                                                .to_string(),
                                        ))
                                    }
                                };
                                let mut props = std::collections::HashMap::new();
                                for (k, v_expr) in e.props {
                                    props.insert(k, Self::expr_to_value(v_expr)?);
                                }
                                Ok(EdgeInsert {
                                    src,
                                    dst,
                                    edge_type: e.edge_name,
                                    ranking: e.ranking.unwrap_or(0),
                                    props,
                                })
                            })
                            .collect::<Result<Vec<_>>>()?,
                    },
                }))
            }
            Statement::Update(update) => {
                let space = update.space.unwrap_or_default();
                let vid = match update.vid {
                    byoridb_parser::ast::Expression::Literal(
                        byoridb_parser::ast::Literal::Int(i),
                    ) => i,
                    _ => {
                        return Err(crate::error::ExecutionError::InvalidOperation(
                            "Vertex ID must be an integer literal".to_string(),
                        ))
                    }
                };
                let mut updates = std::collections::HashMap::new();
                for (k, v_expr) in update.updates {
                    updates.insert(k, Self::expr_to_value(v_expr)?);
                }
                Ok(ExecutionPlan::Update(UpdatePlan {
                    space,
                    vid,
                    tag_name: update.tag_name,
                    updates,
                    conditions: update.conditions,
                    yield_clause: update.yield_clause.map(|c| format!("{:?}", c)),
                }))
            }
            Statement::Delete(delete) => {
                let space = delete.space.unwrap_or_default();
                match delete.delete_type {
                    byoridb_parser::ast::DeleteType::Vertex => {
                        let vids = delete
                            .vids
                            .into_iter()
                            .map(|v| match v {
                                byoridb_parser::ast::Expression::Literal(
                                    byoridb_parser::ast::Literal::Int(i),
                                ) => Ok(i),
                                _ => Err(crate::error::ExecutionError::InvalidOperation(
                                    "Vertex ID must be an integer literal".to_string(),
                                )),
                            })
                            .collect::<Result<Vec<_>>>()?;
                        Ok(ExecutionPlan::Delete(DeletePlan {
                            space,
                            vids,
                            conditions: delete.conditions,
                        }))
                    }
                    byoridb_parser::ast::DeleteType::Edge => {
                        let edge_refs = delete
                            .edge_refs
                            .into_iter()
                            .map(|e| {
                                let src = match e.src_vid {
                                    byoridb_parser::ast::Expression::Literal(
                                        byoridb_parser::ast::Literal::Int(i),
                                    ) => Ok(i),
                                    _ => Err(crate::error::ExecutionError::InvalidOperation(
                                        "Edge src VID must be an integer literal".to_string(),
                                    )),
                                }?;
                                let dst = match e.dst_vid {
                                    byoridb_parser::ast::Expression::Literal(
                                        byoridb_parser::ast::Literal::Int(i),
                                    ) => Ok(i),
                                    _ => Err(crate::error::ExecutionError::InvalidOperation(
                                        "Edge dst VID must be an integer literal".to_string(),
                                    )),
                                }?;
                                let ranking = e.ranking.unwrap_or(0);
                                Ok((src, dst, ranking))
                            })
                            .collect::<Result<Vec<_>>>()?;
                        Ok(ExecutionPlan::DeleteEdge(DeleteEdgePlan {
                            space,
                            edge_name: delete.edge_name.unwrap_or_default(),
                            edge_refs,
                        }))
                    }
                }
            }
            Statement::Fetch(fetch) => {
                let space = fetch.space.unwrap_or_default();
                let is_edge_fetch = matches!(
                    fetch.fetch_type,
                    byoridb_parser::ast::FetchType::Edge | byoridb_parser::ast::FetchType::EdgeProp
                );

                let mut vids: Vec<i64> = Vec::new();
                let mut edge_refs: Vec<(i64, i64)> = Vec::new();

                if is_edge_fetch {
                    // Edge fetch: parser encodes src->dst as two consecutive Int literals
                    let ints: Vec<i64> = fetch
                        .vids
                        .into_iter()
                        .map(|v| match v {
                            byoridb_parser::ast::Expression::Literal(
                                byoridb_parser::ast::Literal::Int(i),
                            ) => Ok(i),
                            _ => Err(crate::error::ExecutionError::InvalidOperation(
                                "Edge VID must be an integer literal".to_string(),
                            )),
                        })
                        .collect::<Result<Vec<_>>>()?;
                    for pair in ints.chunks(2) {
                        if pair.len() == 2 {
                            edge_refs.push((pair[0], pair[1]));
                        }
                    }
                } else {
                    vids = fetch
                        .vids
                        .into_iter()
                        .map(|v| match v {
                            byoridb_parser::ast::Expression::Literal(
                                byoridb_parser::ast::Literal::Int(i),
                            ) => Ok(i),
                            _ => Err(crate::error::ExecutionError::InvalidOperation(
                                "Vertex ID must be an integer literal".to_string(),
                            )),
                        })
                        .collect::<Result<Vec<_>>>()?;
                }

                Ok(ExecutionPlan::Fetch(FetchPlan {
                    space,
                    vids,
                    tags: fetch.tags,
                    yield_clause: fetch.yield_clause.map(|c| format!("{:?}", c)),
                    edge_refs,
                    is_edge_fetch,
                    src_var: fetch.src_var,
                }))
            }
            Statement::Go(go_stmt) => {
                let vids = go_stmt
                    .from_clause
                    .vids
                    .into_iter()
                    .map(|v| match v {
                        byoridb_parser::ast::Expression::Literal(
                            byoridb_parser::ast::Literal::Int(i),
                        ) => Ok(i),
                        _ => Err(crate::error::ExecutionError::InvalidOperation(
                            "Vertex ID must be an integer literal".to_string(),
                        )),
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(ExecutionPlan::Go(GoPlan {
                    from_clause: FromClause {
                        vids,
                        src: go_stmt.from_clause.src,
                    },
                    over_edges: go_stmt.over_edges,
                    direction: go_stmt.direction,
                    to_clause: ToClause {
                        variable: go_stmt.to_clause.variable,
                        steps: match go_stmt.to_clause.steps {
                            byoridb_parser::ast::StepClause::Exactly(n) => {
                                StepClause::Exactly(n as usize)
                            }
                            byoridb_parser::ast::StepClause::Range(min, max) => {
                                StepClause::Range(min as usize, max as usize)
                            }
                            byoridb_parser::ast::StepClause::Upto(n) => {
                                StepClause::Range(0, n as usize)
                            }
                        },
                    },
                    where_clause: go_stmt.where_clause,
                    yield_clause: YieldClause {
                        columns: go_stmt
                            .yield_clause
                            .columns
                            .into_iter()
                            .map(|c: ParserYieldColumn| YieldColumn {
                                expression: c.expression,
                                alias: c.alias,
                            })
                            .collect(),
                    },
                }))
            }
            Statement::Lookup(lookup) => Ok(ExecutionPlan::Lookup(LookupPlan {
                lookup_type: match lookup.lookup_type {
                    byoridb_parser::ast::LookupType::Tag(tag) => LookupType::Tag(tag),
                    byoridb_parser::ast::LookupType::Edge(edge) => LookupType::Edge(edge),
                },
                where_clause: lookup.where_clause,
                yield_clause: YieldClause {
                    columns: lookup
                        .yield_clause
                        .columns
                        .into_iter()
                        .map(|c: ParserYieldColumn| YieldColumn {
                            expression: c.expression,
                            alias: c.alias,
                        })
                        .collect(),
                },
                limit: lookup.limit,
                offset: lookup.offset,
            })),
            Statement::Find(find_stmt) => Ok(ExecutionPlan::Find(FindPlan {
                find_type: match find_stmt.find_type {
                    byoridb_parser::ast::FindType::Path => FindType::Path,
                    byoridb_parser::ast::FindType::ShortestPath => FindType::ShortestPath,
                    byoridb_parser::ast::FindType::AllShortestPaths => FindType::AllShortestPaths,
                },
                from_vid: find_stmt.from_vid,
                to_vid: find_stmt.to_vid,
                over_edge: find_stmt.over_edge,
                weight_prop: find_stmt.weight_prop,
                bidirect: find_stmt.bidirect,
                upto_steps: find_stmt.upto_steps,
                where_clause: find_stmt.where_clause,
                yield_clause: find_stmt.yield_clause.map(|c| format!("{:?}", c)),
            })),
            Statement::Match(match_stmt) => Ok(ExecutionPlan::Match(MatchPlan {
                pattern: match_stmt.pattern,
                where_clause: match_stmt.where_clause,
                optional_patterns: match_stmt.optional_patterns,
                return_clause: match_stmt.return_clause.map(|c| {
                    c.columns
                        .into_iter()
                        .map(|col| MatchReturnColumn {
                            expression: col.expression,
                            alias: col.alias,
                        })
                        .collect()
                }),
                group_by: match_stmt.group_by,
                limit: match_stmt.limit,
                offset: match_stmt.offset,
            })),
            Statement::Compound(clauses) => {
                let mut planned = Vec::with_capacity(clauses.len());
                for clause in clauses {
                    let plan = Self::build(*clause.stmt)?;
                    planned.push(CompoundPlanClause {
                        var: clause.var,
                        plan: Box::new(plan),
                    });
                }
                Ok(ExecutionPlan::Compound(planned))
            }
            Statement::Explain { profile, statement } => {
                let inner_plan = Self::build(*statement)?;
                Ok(ExecutionPlan::Explain {
                    profile,
                    plan: Box::new(inner_plan),
                })
            }
        }
    }

    /// Helper: Convert expression to value
    fn expr_to_value(expr: byoridb_parser::ast::Expression) -> Result<byoridb_common::Value> {
        Ok(match expr {
            byoridb_parser::ast::Expression::Literal(lit) => match lit {
                byoridb_parser::ast::Literal::String(s) => byoridb_common::Value::String(s),
                byoridb_parser::ast::Literal::Int(i) => byoridb_common::Value::Int(i),
                byoridb_parser::ast::Literal::Float(f) => byoridb_common::Value::Float(f),
                byoridb_parser::ast::Literal::Bool(b) => byoridb_common::Value::Bool(b),
                byoridb_parser::ast::Literal::Null => byoridb_common::Value::null(),
            },
            byoridb_parser::ast::Expression::Identifier(s) => byoridb_common::Value::String(s),
            _ => {
                return Err(crate::error::ExecutionError::InvalidOperation(format!(
                    "Expression not supported: {:?}",
                    expr
                )))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byoridb_parser::ast;

    fn build(stmt: Statement) -> ExecutionPlan {
        ExecutionPlanBuilder::build(stmt).expect("plan build")
    }

    #[test]
    fn weighted_find_statement_maps_weight_property_to_plan() {
        let stmt =
            byoridb_parser::parse("FIND SHORTEST PATH FROM 1 TO 2 OVER knows WEIGHT BY cost")
                .expect("parse weighted find");

        match build(stmt) {
            ExecutionPlan::Find(plan) => {
                assert!(matches!(plan.find_type, FindType::ShortestPath));
                assert_eq!(plan.over_edge, "knows");
                assert_eq!(plan.weight_prop.as_deref(), Some("cost"));
            }
            _ => panic!("Expected Find plan"),
        }
    }

    #[test]
    fn create_tag_index_statement_maps_to_create_tag_index_plan() {
        let stmt = Statement::Create(ast::CreateStatement::TagIndex(
            ast::CreateTagIndexStatement {
                if_not_exists: false,
                index_name: "person_name_idx".to_string(),
                tag_name: "person".to_string(),
                props: vec!["name".to_string(), "age".to_string()],
            },
        ));

        match build(stmt) {
            ExecutionPlan::Create(CreatePlan::TagIndex {
                name,
                tag_name,
                props,
            }) => {
                assert_eq!(name, "person_name_idx");
                assert_eq!(tag_name, "person");
                assert_eq!(props, vec!["name", "age"]);
            }
            _ => panic!("Expected Create(CreatePlan::TagIndex)"),
        }
    }

    #[test]
    fn create_edge_index_statement_maps_to_create_edge_index_plan() {
        let stmt = Statement::Create(ast::CreateStatement::EdgeIndex(
            ast::CreateEdgeIndexStatement {
                if_not_exists: true,
                index_name: "knows_since_idx".to_string(),
                edge_name: "knows".to_string(),
                props: vec!["since".to_string()],
            },
        ));

        match build(stmt) {
            ExecutionPlan::Create(CreatePlan::EdgeIndex {
                name,
                edge_name,
                props,
            }) => {
                assert_eq!(name, "knows_since_idx");
                assert_eq!(edge_name, "knows");
                assert_eq!(props, vec!["since"]);
            }
            _ => panic!("Expected Create(CreatePlan::EdgeIndex)"),
        }
    }

    #[test]
    fn drop_tag_index_statement_maps_to_drop_tag_index_plan() {
        let stmt = Statement::Drop(ast::DropStatement::TagIndex(ast::DropTagIndexStatement {
            if_exists: true,
            index_name: "person_name_idx".to_string(),
        }));

        match build(stmt) {
            ExecutionPlan::Drop(DropPlan::TagIndex { name, if_exists }) => {
                assert_eq!(name, "person_name_idx");
                assert!(if_exists);
            }
            _ => panic!("Expected Drop(DropPlan::TagIndex)"),
        }
    }

    #[test]
    fn drop_edge_index_statement_maps_to_drop_edge_index_plan() {
        let stmt = Statement::Drop(ast::DropStatement::EdgeIndex(ast::DropEdgeIndexStatement {
            if_exists: false,
            index_name: "e_idx".to_string(),
        }));

        match build(stmt) {
            ExecutionPlan::Drop(DropPlan::EdgeIndex { name, if_exists }) => {
                assert_eq!(name, "e_idx");
                assert!(!if_exists);
            }
            _ => panic!("Expected Drop(DropPlan::EdgeIndex)"),
        }
    }

    #[test]
    fn show_tag_indexes_statement_maps_to_show_tag_indexes_plan() {
        // Regression: previously mapped to ShowPlan::Tags which returned tag
        // names instead of index metadata.
        match build(Statement::Show(ast::ShowStatement::TagIndexes)) {
            ExecutionPlan::Show(ShowPlan::TagIndexes) => {}
            _ => panic!("Expected Show(ShowPlan::TagIndexes)"),
        }
    }

    #[test]
    fn show_edge_indexes_statement_maps_to_show_edge_indexes_plan() {
        // Regression: previously mapped to ShowPlan::Edges.
        match build(Statement::Show(ast::ShowStatement::EdgeIndexes)) {
            ExecutionPlan::Show(ShowPlan::EdgeIndexes) => {}
            _ => panic!("Expected Show(ShowPlan::EdgeIndexes)"),
        }
    }

    #[test]
    fn insert_edge_statement_carries_edge_name_into_plan() {
        // Regression: previously edge_type was hardcoded to 0, causing every
        // INSERT EDGE to fail schema lookup with "Edge not found: 0".
        let stmt =
            byoridb_parser::parse("INSERT EDGE follows (since) VALUES 1->2:(2020), 3->4:(2022)")
                .expect("parse INSERT EDGE");

        match build(stmt) {
            ExecutionPlan::Insert(InsertPlan::Edge { edges, .. }) => {
                assert_eq!(edges.len(), 2);
                for e in &edges {
                    assert_eq!(
                        e.edge_type, "follows",
                        "edge_type must carry the parser's edge_name, not a hardcoded ID"
                    );
                }
                assert_eq!(edges[0].src, 1);
                assert_eq!(edges[0].dst, 2);
                assert_eq!(edges[1].src, 3);
                assert_eq!(edges[1].dst, 4);
            }
            _ => panic!("Expected Insert(InsertPlan::Edge)"),
        }
    }

    #[test]
    fn insert_edge_multi_row_preserves_edge_name_for_each_row() {
        // The bug surfaced especially with multi-row INSERTs because every row
        // collapsed into edge_type=0. Cover that explicitly.
        let stmt = byoridb_parser::parse(
            "INSERT EDGE likes (weight) VALUES 1->3:(0.9), 2->5:(0.4), 4->1:(0.75)",
        )
        .expect("parse INSERT EDGE multi-row");

        match build(stmt) {
            ExecutionPlan::Insert(InsertPlan::Edge { edges, .. }) => {
                assert_eq!(edges.len(), 3);
                assert!(edges.iter().all(|e| e.edge_type == "likes"));
            }
            _ => panic!("Expected Insert(InsertPlan::Edge)"),
        }
    }
}
