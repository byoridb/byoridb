// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Abstract Syntax Tree for nGQL

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Statement type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Statement {
    Show(ShowStatement),
    Describe(DescribeStatement),
    Use(UseStatement),
    Create(CreateStatement),
    Alter(AlterStatement),
    Drop(DropStatement),
    Insert(InsertStatement),
    Update(UpdateStatement),
    Delete(DeleteStatement),
    Fetch(FetchStatement),
    Find(FindStatement),
    Match(MatchStatement),
    Go(GoStatement),
    Lookup(LookupStatement),
    Grant(GrantStatement),
    Revoke(RevokeStatement),
    Balance(BalanceStatement),
    /// Compound query: a sequence of clauses where each may bind its result
    /// to a variable (`$var = GO FROM 1 OVER e`) consumed by later clauses
    /// (`GO FROM $var.dst OVER e`).
    Compound(Vec<CompoundClause>),
    /// EXPLAIN/PROFILE <statement>. `profile = false` (EXPLAIN) returns the
    /// logical plan without executing; `profile = true` (PROFILE) executes the
    /// statement and returns the plan annotated with per-operator runtime
    /// metrics.
    Explain {
        profile: bool,
        statement: Box<Statement>,
    },
}

/// One clause inside a [`Statement::Compound`]. `var` is `Some(name)` for an
/// assignment (`$name = stmt`) and `None` for a plain statement whose result
/// is emitted as the final query output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompoundClause {
    pub var: Option<String>,
    pub stmt: Box<Statement>,
}

/// SHOW statements
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShowStatement {
    Spaces,
    Tags,
    Edges,
    TagIndexes,
    EdgeIndexes,
    Users,
    Roles,
    /// SHOW PARTS - show partition allocation
    Parts,
    /// SHOW HOSTS - show storage hosts
    Hosts,
    /// SHOW STATS — vertex/edge counts per type
    Stats,
    /// SHOW SESSIONS — active session list
    Sessions,
    /// SHOW CREATE TAG <name>
    CreateTag(String),
    /// SHOW CREATE EDGE <name>
    CreateEdge(String),
    /// SHOW TAG INDEX STATUS
    TagIndexStatuses,
    /// SHOW EDGE INDEX STATUS
    EdgeIndexStatuses,
}

/// DESCRIBE statement — describe the schema of a tag, edge, or space.
///
/// Both `DESCRIBE` and `DESC` keywords produce the same AST.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DescribeStatement {
    /// DESCRIBE TAG <name>
    Tag(String),
    /// DESCRIBE EDGE <name>
    Edge(String),
    /// DESCRIBE SPACE <name>
    Space(String),
    /// DESCRIBE TAG INDEX <name>
    TagIndex(String),
    /// DESCRIBE EDGE INDEX <name>
    EdgeIndex(String),
}

/// USE statement
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UseStatement {
    pub space: String,
}

/// CREATE statements
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CreateStatement {
    Space(CreateSpaceStatement),
    Tag(CreateTagStatement),
    Edge(CreateEdgeStatement),
    TagIndex(CreateTagIndexStatement),
    EdgeIndex(CreateEdgeIndexStatement),
    User(CreateUserStatement),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateSpaceStatement {
    pub if_not_exists: bool,
    pub name: String,
    pub partition_num: Option<u32>,
    pub replica_factor: Option<u32>,
    pub vid_type: Option<VidType>,
    pub partition_strategy: Option<PartitionStrategySpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VidType {
    Int64,
    FixedString(usize),
}

/// Partition strategy specification for CREATE SPACE statement
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PartitionStrategySpec {
    /// Hash-based partitioning: hash(vid) % N + 1
    Hash,
    /// Range-based partitioning with boundary values
    Range { boundaries: Vec<i64> },
    /// Simple modulo partitioning: vid % N + 1
    Modulo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTagStatement {
    pub if_not_exists: bool,
    pub name: String,
    pub props: Vec<PropertySpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateEdgeStatement {
    pub if_not_exists: bool,
    pub name: String,
    pub props: Vec<PropertySpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertySpec {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
    pub default: Option<Expression>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataType {
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    Float,
    Double,
    String,
    FixedString(usize),
    Timestamp,
    Date,
    Time,
    DateTime,
    Geography,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expression {
    Literal(Literal),
    Identifier(String),
    BinaryOp {
        op: BinaryOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    UnaryOp {
        op: UnaryOperator,
        operand: Box<Expression>,
    },
    FunctionCall {
        name: String,
        args: Vec<Expression>,
    },
    List(Vec<Expression>),
    Map(HashMap<String, Expression>),
    /// `object.prop` — used in GO YIELD to reference edge or variable properties.
    /// e.g. `YIELD works_at.role` → `PropRef { object: "works_at", prop: "role" }`
    PropRef {
        object: String,
        prop: String,
    },
    /// `$$.tag.prop` — destination vertex property in GO YIELD.
    /// e.g. `YIELD $$.person.name`
    DstVertexProp {
        tag: String,
        prop: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Literal {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Null,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BinaryOperator {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
    And,
    Or,
    /// `=~` regex match
    Regex,
    /// `CONTAINS` substring check
    Contains,
    /// `NOT CONTAINS` negative substring check
    NotContains,
    /// `STARTS WITH`
    StartsWith,
    /// `ENDS WITH`
    EndsWith,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UnaryOperator {
    Neg,
    Not,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTagIndexStatement {
    pub if_not_exists: bool,
    pub index_name: String,
    pub tag_name: String,
    pub props: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateEdgeIndexStatement {
    pub if_not_exists: bool,
    pub index_name: String,
    pub edge_name: String,
    pub props: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateUserStatement {
    pub if_not_exists: bool,
    pub username: String,
    pub password: String,
    pub role: Option<String>,
}

/// DROP statements
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DropStatement {
    Space(DropSpaceStatement),
    Tag(DropTagStatement),
    Edge(DropEdgeStatement),
    TagIndex(DropTagIndexStatement),
    EdgeIndex(DropEdgeIndexStatement),
    User(DropUserStatement),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropSpaceStatement {
    pub if_exists: bool,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropTagStatement {
    pub if_exists: bool,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropEdgeStatement {
    pub if_exists: bool,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropTagIndexStatement {
    pub if_exists: bool,
    pub index_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropEdgeIndexStatement {
    pub if_exists: bool,
    pub index_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropUserStatement {
    pub if_exists: bool,
    pub username: String,
}

/// ALTER statements
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AlterStatement {
    Tag(AlterTagStatement),
    Edge(AlterEdgeStatement),
    User(AlterUserStatement),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlterTagStatement {
    pub name: String,
    pub operations: Vec<AlterOperation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlterEdgeStatement {
    pub name: String,
    pub operations: Vec<AlterOperation>,
}

/// ALTER operation type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AlterOperation {
    /// ADD (property_name property_type [NULL] [DEFAULT value])
    AddColumn(PropertySpec),
    /// DROP property_name
    DropColumn(String),
    /// CHANGE property_name new_type [NULL] [DEFAULT value]
    ChangeColumn(PropertySpec),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlterUserStatement {
    pub username: String,
    pub new_password: Option<String>,
}

/// GRANT statement: GRANT ROLE role TO user
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrantStatement {
    pub role: String,
    pub username: String,
}

/// REVOKE statement: REVOKE ROLE role FROM user
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RevokeStatement {
    pub role: String,
    pub username: String,
}

/// BALANCE statements for partition management
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BalanceStatement {
    /// BALANCE LEADER - trigger leader rebalance
    Leader,
    /// BALANCE DATA - trigger data rebalance
    Data,
    /// BALANCE STATUS - show balance status
    Status,
    /// BALANCE STOP - stop ongoing balance
    Stop,
    /// BALANCE RESET - reset balance plan
    Reset,
}

/// INSERT statement
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsertStatement {
    pub insert_type: InsertType,
    pub space: Option<String>,
    pub vertices: Vec<VertexInsertSpec>,
    pub edges: Vec<EdgeInsertSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InsertType {
    Vertex,
    Edge,
}

/// Edge insert specification: src_vid -> dst_vid @ ranking
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeInsertSpec {
    pub src_vid: Expression,
    pub dst_vid: Expression,
    pub ranking: Option<i64>,
    pub edge_name: String,
    pub props: HashMap<String, Expression>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VertexInsertSpec {
    pub vid: Expression,
    pub tags: Vec<TagInsertSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagInsertSpec {
    pub name: String,
    pub props: HashMap<String, Expression>,
}

/// UPDATE statement
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateStatement {
    pub update_type: UpdateType,
    pub space: Option<String>,
    pub vid: Expression,
    pub dst_vid: Option<Expression>,
    pub ranking: Option<i64>,
    pub tag_name: Option<String>,
    pub edge_name: Option<String>,
    pub updates: HashMap<String, Expression>,
    pub conditions: Option<Expression>,
    pub yield_clause: Option<YieldClause>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UpdateType {
    Vertex,
    Edge,
}

/// DELETE statement
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteStatement {
    pub delete_type: DeleteType,
    pub space: Option<String>,
    pub vids: Vec<Expression>,
    pub edge_refs: Vec<EdgeRef>,
    pub edge_name: Option<String>,
    pub conditions: Option<Expression>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeleteType {
    Vertex,
    Edge,
}

/// Edge reference for deletion: src_vid -> dst_vid @ ranking
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeRef {
    pub src_vid: Expression,
    pub dst_vid: Expression,
    pub ranking: Option<i64>,
}

/// FETCH statement
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FetchStatement {
    pub fetch_type: FetchType,
    pub space: Option<String>,
    pub vids: Vec<Expression>,
    pub tags: Vec<String>,
    pub yield_clause: Option<YieldClause>,
    /// `$var.col` variable reference as VID source (compound statement support)
    pub src_var: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FetchType {
    Vertex,
    Edge,
    EdgeProp,
}

/// FIND statement
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindStatement {
    pub find_type: FindType,
    pub from_vid: Expression,
    pub to_vid: Expression,
    pub over_edge: String,
    pub weight_prop: Option<String>,
    pub upto_steps: Option<u32>,
    pub where_clause: Option<Expression>,
    pub yield_clause: Option<YieldClause>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FindType {
    Path,
    ShortestPath,
}

/// MATCH statement (Cypher-like)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchStatement {
    pub pattern: Pattern,
    pub where_clause: Option<Expression>,
    /// OPTIONAL MATCH clauses — each optional pattern is tried for every row
    /// from the main MATCH; if it yields nothing the row is kept with NULLs.
    pub optional_patterns: Vec<Pattern>,
    pub return_clause: Option<ReturnClause>,
    pub group_by: Option<Vec<Expression>>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Pattern {
    Path(PathPattern),
    Multiple(Vec<Pattern>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathPattern {
    pub start: NodePattern,
    pub edges: Vec<EdgePattern>,
    /// Node patterns for each hop destination.
    /// `nodes[i]` is the filter for the node reached via `edges[i]`.
    /// Length must equal `edges.len()`.
    pub nodes: Vec<NodePattern>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodePattern {
    pub variable: Option<String>,
    pub labels: Vec<String>,
    pub props: HashMap<String, Expression>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgePattern {
    pub variable: Option<String>,
    pub edge_types: Vec<String>,
    pub direction: EdgeDirection,
    pub props: HashMap<String, Expression>,
    pub range: Option<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EdgeDirection {
    Outgoing,
    Incoming,
    Undirected,
}

/// GO statement (graph traversal)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoStatement {
    pub from_clause: FromClause,
    pub over_edges: Vec<String>,
    pub direction: EdgeDirection,
    pub to_clause: ToClause,
    pub where_clause: Option<Expression>,
    pub yield_clause: YieldClause,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FromClause {
    pub vids: Vec<Expression>,
    pub src: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToClause {
    pub variable: String,
    pub steps: StepClause,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StepClause {
    Exactly(u32),
    Range(u32, u32),
    Upto(u32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YieldClause {
    pub columns: Vec<YieldColumn>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YieldColumn {
    pub expression: Expression,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReturnClause {
    pub columns: Vec<YieldColumn>,
}

/// LOOKUP statement
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookupStatement {
    pub lookup_type: LookupType,
    pub where_clause: Option<Expression>,
    pub yield_clause: YieldClause,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LookupType {
    Tag(String),
    Edge(String),
}
