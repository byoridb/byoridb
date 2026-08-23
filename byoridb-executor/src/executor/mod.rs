// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Query executor for nGQL

use crate::context::ExecutionContext;
use crate::error::Result;
use crate::plan::ExecutionPlan;
use std::sync::Arc;

pub(crate) const DISTRIBUTED_LOOKUP_FULL_SCAN_UNSUPPORTED: &str =
    "Distributed LOOKUP full scans are not supported yet";
pub(crate) const DISTRIBUTED_LOOKUP_RANGE_UNSUPPORTED: &str =
    "Distributed LOOKUP range predicates are not supported yet";

/// Result of query execution
#[derive(Debug, Clone)]
pub struct ExecutorResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<byoridb_common::Value>>,
    pub latency_ms: u64,
}

impl ExecutorResult {
    /// Create an empty result (for DDL operations)
    fn empty() -> Self {
        ExecutorResult {
            columns: vec![],
            rows: vec![],
            latency_ms: 0,
        }
    }

    /// Create a success message result
    fn success_message(message: String) -> Self {
        ExecutorResult {
            columns: vec!["Altered".to_string()],
            rows: vec![vec![byoridb_common::Value::String(message)]],
            latency_ms: 0,
        }
    }
}

/// Query executor
pub struct Executor {
    ctx: Arc<ExecutionContext>,
}

impl Executor {
    pub fn new(ctx: Arc<ExecutionContext>) -> Self {
        Self { ctx }
    }

    /// Borrow the execution context (e.g. to inspect `took_full_scan()` after a run).
    pub fn ctx(&self) -> &Arc<ExecutionContext> {
        &self.ctx
    }

    /// Execute a query plan
    pub async fn execute(&self, plan: ExecutionPlan) -> Result<ExecutorResult> {
        let start = std::time::Instant::now();

        let result = match plan {
            ExecutionPlan::Show(show_plan) => self.execute_show(show_plan).await?,
            ExecutionPlan::Describe(desc_plan) => self.execute_describe(desc_plan).await?,
            ExecutionPlan::Use(use_plan) => self.execute_use(use_plan).await?,
            ExecutionPlan::Create(create_plan) => self.execute_create(create_plan).await?,
            ExecutionPlan::Alter(alter_plan) => self.execute_alter(alter_plan).await?,
            ExecutionPlan::Drop(drop_plan) => self.execute_drop(drop_plan).await?,
            ExecutionPlan::Insert(insert_plan) => self.execute_insert(insert_plan).await?,
            ExecutionPlan::Update(update_plan) => self.execute_update(update_plan).await?,
            ExecutionPlan::Delete(delete_plan) => self.execute_delete(delete_plan).await?,
            ExecutionPlan::DeleteEdge(plan) => self.execute_delete_edge(plan).await?,
            ExecutionPlan::Fetch(fetch_plan) => self.execute_fetch(fetch_plan).await?,
            ExecutionPlan::Go(go_plan) => self.execute_go(go_plan).await?,
            ExecutionPlan::Lookup(lookup_plan) => self.execute_lookup(lookup_plan).await?,
            ExecutionPlan::Recommend(rec_plan) => self.execute_recommend(rec_plan).await?,
            ExecutionPlan::CheckConsistency => self.execute_check_consistency().await?,
            ExecutionPlan::CheckShape => self.execute_check_shape().await?,
            ExecutionPlan::ExplainInference {
                src,
                dst,
                edge_type,
            } => self.explain_inference(src, &edge_type, dst).await?,
            ExecutionPlan::Match(match_plan) => {
                use crate::match_impl::MatchExecutor;
                let match_executor = MatchExecutor::new(self.ctx.clone());
                match_executor.execute_match(match_plan).await?
            }
            ExecutionPlan::Find(find_plan) => self.execute_find(find_plan).await?,
            ExecutionPlan::Grant(grant_plan) => self.execute_grant(grant_plan).await?,
            ExecutionPlan::Revoke(revoke_plan) => self.execute_revoke(revoke_plan).await?,
            ExecutionPlan::Balance(balance_plan) => self.execute_balance(balance_plan).await?,
            ExecutionPlan::Compound(clauses) => self.execute_compound(clauses).await?,
            ExecutionPlan::Explain { profile, plan } => {
                self.execute_explain(profile, *plan).await?
            }
        };

        let latency = start.elapsed().as_millis() as u64;

        Ok(ExecutorResult {
            columns: result.columns,
            rows: result.rows,
            latency_ms: latency,
        })
    }

    /// Execute a compound query: run each clause in order, binding the
    /// assignment clauses to `ctx.vars` so later clauses can reference
    /// them as `$name` / `$name.col`.
    ///
    /// The final non-assignment clause's result is returned as the query
    /// output. If every clause is an assignment, the result is empty.
    /// Errors short-circuit (no rollback semantics — compound execution is
    /// best-effort sequential, matching nGQL's documented behavior).
    fn execute_compound(
        &self,
        clauses: Vec<super::plan::CompoundPlanClause>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ExecutorResult>> + Send + '_>>
    {
        Box::pin(async move {
            let mut last_result = ExecutorResult::empty();
            // Working context for the clauses. A `USE` clause swaps it for a
            // sibling context on the new space so *subsequent* clauses see the
            // switch. `execute_use` alone cannot: `ctx.space` is set once per
            // request from the session and a session-level switch only takes
            // effect on the *next* request — so `USE nexprice; MATCH ...` would
            // otherwise run the MATCH on the stale/unset space (the cause of a
            // "data looks empty" incident). `$var` bindings are preserved
            // because `vars` is shared via `Arc` across derived contexts.
            let mut ctx = self.ctx.clone();
            for clause in clauses {
                if let ExecutionPlan::Use(use_plan) = clause.plan.as_ref() {
                    // Validate the target exists (mirrors `execute_use`) so a
                    // typo errors instead of silently misrouting later clauses.
                    let space_key = crate::key::SchemaKey::space(&use_plan.space);
                    if ctx.kvstore.get(&space_key).await?.is_none() {
                        return Err(crate::error::ExecutionError::SpaceNotFound(
                            use_plan.space.clone(),
                        ));
                    }
                    ctx = Arc::new(ctx.derive_with_space(use_plan.space.clone()));
                    last_result = ExecutorResult::empty();
                    continue;
                }
                let exec = Executor::new(ctx.clone());
                let result = exec.execute(*clause.plan).await?;
                if let Some(var) = clause.var {
                    ctx.bind_var(var, result.clone());
                } else {
                    last_result = result;
                }
            }
            Ok(last_result)
        })
    }

    /// EXPLAIN / PROFILE.
    ///
    /// - EXPLAIN (`profile = false`): derive the logical operator tree and
    ///   statically resolve each scan's access path (index / tag-vid / full
    ///   scan); never executes.
    /// - PROFILE (`profile = true`): build the tree, then execute the inner
    ///   plan with a profile collector attached and overlay the per-operator
    ///   row counts / timings collected at the instrumentation sites.
    async fn execute_explain(
        &self,
        profile: bool,
        plan: super::plan::ExecutionPlan,
    ) -> Result<ExecutorResult> {
        if !profile {
            let tree = crate::explain::build_plan_tree(&self.ctx, &plan).await;
            return Ok(crate::explain::render(&tree, false));
        }

        // Build the tree before flipping profiling on so the access-path probes
        // (tag-vid prefix scan, index listing) don't pollute the records.
        let mut tree = crate::explain::build_plan_tree(&self.ctx, &plan).await;

        let collector = self.ctx.enable_profile();
        let start = std::time::Instant::now();
        // Box the recursive execute() call to break the async-fn size cycle
        // (execute → execute_explain → execute).
        let exec_result = Box::pin(self.execute(plan)).await;
        let total_us = start.elapsed().as_micros() as u64;
        self.ctx.disable_profile();

        let result = exec_result?;
        let records = collector.snapshot();
        crate::explain::overlay_profile(&mut tree, &records, result.rows.len() as u64, total_us);
        Ok(crate::explain::render(&tree, true))
    }
}

/// Return true when an expression tree contains any ordered comparison.
/// Distributed LOOKUP range execution is not implemented, so callers must
/// detect comparisons nested under AND/OR/NOT instead of checking only the
/// top-level predicate and accidentally falling back to coordinator-local IO.
pub(crate) fn expression_contains_ordered_range(
    expression: &byoridb_parser::ast::Expression,
) -> bool {
    use byoridb_parser::ast::{BinaryOperator, Expression};

    match expression {
        Expression::BinaryOp { op, left, right } => {
            matches!(
                op,
                BinaryOperator::Lt | BinaryOperator::Lte | BinaryOperator::Gt | BinaryOperator::Gte
            ) || expression_contains_ordered_range(left)
                || expression_contains_ordered_range(right)
        }
        Expression::UnaryOp { operand, .. } => expression_contains_ordered_range(operand),
        Expression::FunctionCall { args, .. } | Expression::List(args) => {
            args.iter().any(expression_contains_ordered_range)
        }
        Expression::Map(values) => values.values().any(expression_contains_ordered_range),
        Expression::Literal(_)
        | Expression::Identifier(_)
        | Expression::PropRef { .. }
        | Expression::DstVertexProp { .. } => false,
    }
}

/// Convert a LOOKUP range literal to the indexed field's physical key type.
/// Returning `None` deliberately routes unsupported or ambiguous comparisons
/// through the full predicate evaluator, preserving cross-type correctness.
pub(crate) async fn range_index_boundary(
    ctx: &ExecutionContext,
    space: &str,
    tag: &str,
    field: &str,
    value: &byoridb_common::Value,
) -> Option<byoridb_storage::key::IndexValue> {
    use byoridb_common::Value;
    use byoridb_storage::key::IndexValue;

    let schema = ctx
        .kvstore
        .get(&crate::key::SchemaKey::tag(space, tag))
        .await
        .ok()??;
    let schema: serde_json::Value = serde_json::from_slice(&schema).ok()?;
    let data_type = schema
        .get("properties")?
        .as_array()?
        .iter()
        .find(|property| property.get("name").and_then(|name| name.as_str()) == Some(field))?
        .get("data_type")?
        .as_str()?;

    match (data_type, value) {
        ("Bool", Value::Bool(value)) => Some(IndexValue::Bool(*value)),
        ("Int8" | "Int16" | "Int32" | "Int64", Value::Int(value)) => Some(IndexValue::Int(*value)),
        // Float/Double columns accept both Int and Float Values, while DML
        // currently indexes the Value's physical variant without canonicalizing
        // it. Those variants occupy separate key-marker domains, so one ordered
        // range scan cannot be complete. Keep the predicate-scan path until
        // writes canonicalize numeric index keys or the executor can merge both
        // domains safely.
        ("Float" | "Double", _) => None,
        _ => None,
    }
}

mod auth_exec;
mod class_ddl;
mod consistency;
mod ddl;
mod dml;
#[cfg(test)]
mod dogfood_regression;
mod dql;
mod inference;
mod provenance;
mod recommend;
mod sameas;
mod shape;
mod show;
mod temporal;
mod vector_index;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ExecutionConfig, ExecutionContext};
    use crate::error::ExecutionError;
    use crate::key::SchemaKey;
    use crate::plan::{
        AlterColumnOp, AlterOpType, AlterPlan, CreatePlan, DeletePlan, DropPlan, EdgeInsert,
        FetchPlan, FindPlan, FindType, FromClause, GoPlan, InsertPlan, LookupPlan, LookupType,
        PropertyDef, ShowPlan, StepClause, TagData, ToClause, UpdatePlan, VertexInsert, Vid,
        YieldClause, YieldColumn,
    };
    use crate::ExecutionPlanBuilder;
    use byoridb_codec::{
        EdgeData as CodecEdgeData, TagData as CodecTagData, VertexCodec,
        VertexData as CodecVertexData,
    };
    use byoridb_kvstore::store::MemoryKVStore;
    use byoridb_kvstore::KVStore as _;
    use byoridb_parser::ast::DataType;
    use byoridb_parser::ast::{Expression, Literal};
    use byoridb_storage::key::IndexValue;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn create_executor() -> Executor {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kvstore).with_space("default".to_string()));
        Executor::new(ctx)
    }

    // ---- T-4: T-트랙 읽기 표면(FETCH ... AS OF) end-to-end ----

    fn person_blob(name: &str) -> Vec<u8> {
        let v = byoridb_codec::VertexData {
            vid: 1,
            tags: vec![byoridb_codec::TagData {
                name: "person".to_string(),
                properties: HashMap::from([(
                    "name".to_string(),
                    byoridb_common::Value::String(name.to_string()),
                )]),
            }],
        };
        byoridb_codec::VertexCodec::encode_vertex(&v).unwrap()
    }

    fn fetch_as_of_plan(vid: i64, as_of: Option<i64>) -> ExecutionPlan {
        ExecutionPlan::Fetch(FetchPlan {
            space: "default".to_string(),
            vids: vec![vid.into()],
            tags: vec!["person".to_string()],
            yield_clause: None,
            edge_refs: vec![],
            is_edge_fetch: false,
            src_var: None,
            as_of,
        })
    }

    #[tokio::test]
    async fn fetch_as_of_resolves_history() {
        let kv = Arc::new(MemoryKVStore::new());
        let key = b"default:vertex:1".to_vec();
        // 이력: [100,∞)@100="A", [200,∞)@200="B" (v1: valid==tx, open interval)
        kv.put_version(&key, 100, i64::MAX, 100, &person_blob("A"))
            .await
            .unwrap();
        kv.put_version(&key, 200, i64::MAX, 200, &person_blob("B"))
            .await
            .unwrap();
        kv.put(&key, &person_blob("B")).await.unwrap(); // 현재뷰=최신

        let ctx = Arc::new(ExecutionContext::new(kv).with_space("default".to_string()));
        let exec = Executor::new(ctx);

        // 존재 전 시점 → 0행
        let r = exec.execute(fetch_as_of_plan(1, Some(50))).await.unwrap();
        assert_eq!(r.rows.len(), 0, "존재 전 시점");
        // A 시점(150) → name=A
        let ra = exec.execute(fetch_as_of_plan(1, Some(150))).await.unwrap();
        assert_eq!(ra.rows.len(), 1);
        let ra_dbg = format!("{:?}", ra.rows);
        assert!(ra_dbg.contains('A') && !ra_dbg.contains('B'), "150→A");
        // B 시점(250) → name=B
        let rb = exec.execute(fetch_as_of_plan(1, Some(250))).await.unwrap();
        assert_eq!(rb.rows.len(), 1);
        assert!(format!("{:?}", rb.rows).contains('B'), "250→B");
        // 현재뷰(AS OF 없음) → 1행 (무회귀)
        assert_eq!(
            exec.execute(fetch_as_of_plan(1, None))
                .await
                .unwrap()
                .rows
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn fetch_as_of_tombstone_absent() {
        let kv = Arc::new(MemoryKVStore::new());
        let key = b"default:vertex:2".to_vec();
        kv.put_version(&key, 100, i64::MAX, 100, &person_blob("A"))
            .await
            .unwrap();
        kv.put_version(&key, 300, i64::MAX, 300, &[]).await.unwrap(); // tombstone
        let ctx = Arc::new(ExecutionContext::new(kv).with_space("default".to_string()));
        let exec = Executor::new(ctx);
        // 삭제 전(150) → 1행, 삭제 후(400) → 0행
        assert_eq!(
            exec.execute(fetch_as_of_plan(2, Some(150)))
                .await
                .unwrap()
                .rows
                .len(),
            1
        );
        assert_eq!(
            exec.execute(fetch_as_of_plan(2, Some(400)))
                .await
                .unwrap()
                .rows
                .len(),
            0,
            "tombstone 이후"
        );
    }

    async fn insert_test_edge(
        executor: &Executor,
        src: i64,
        dst: i64,
        edge_type: &str,
        ranking: i64,
    ) {
        let key = format!("default:edge:{}:{}:{}:{}", src, edge_type, dst, ranking);
        let edge = CodecEdgeData {
            src_vid: src,
            dst_vid: dst,
            edge_type: edge_type.to_string(),
            ranking,
            properties: std::collections::HashMap::new(),
        };
        let data = VertexCodec::encode_edge(&edge).unwrap();
        executor
            .ctx
            .kvstore
            .put(key.as_bytes(), &data)
            .await
            .unwrap();
        // Mirror the production INSERT EDGE write of the reverse-edge index so
        // reverse traversal (get_incoming_neighbors) finds this edge.
        let in_key = crate::key::SchemaKey::in_edge_data("default", dst, edge_type, src, ranking);
        executor.ctx.kvstore.put(&in_key, &data).await.unwrap();
    }

    async fn insert_weighted_test_edge(
        executor: &Executor,
        src: i64,
        dst: i64,
        edge_type: &str,
        ranking: i64,
        cost: f64,
    ) {
        let key = format!("default:edge:{}:{}:{}:{}", src, edge_type, dst, ranking);
        let mut properties = std::collections::HashMap::new();
        properties.insert("cost".to_string(), byoridb_common::Value::Float(cost));
        let edge = CodecEdgeData {
            src_vid: src,
            dst_vid: dst,
            edge_type: edge_type.to_string(),
            ranking,
            properties,
        };
        let data = VertexCodec::encode_edge(&edge).unwrap();
        executor
            .ctx
            .kvstore
            .put(key.as_bytes(), &data)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_alter_tag_success() {
        let executor = create_executor();

        // Setup: Create a tag manually in KVStore
        let tag_key = "space:default:tag:player";
        let initial_tag = serde_json::json!({
            "name": "player",
            "properties": [
                {"name": "id", "data_type": "Int64", "nullable": false}
            ],
            "version": 1
        });
        executor
            .ctx
            .kvstore
            .put(
                tag_key.as_bytes(),
                serde_json::to_vec(&initial_tag).unwrap().as_slice(),
            )
            .await
            .unwrap();

        // Execute ALTER ADD COLUMN
        let plan = AlterPlan::Tag {
            name: "player".to_string(),
            operations: vec![AlterColumnOp {
                op_type: AlterOpType::AddColumn,
                prop: PropertyDef {
                    name: "age".to_string(),
                    data_type: DataType::Int32,
                    nullable: true,
                    default_value: None,
                },
            }],
        };

        executor.execute_alter(plan).await.unwrap();

        // Verify update
        let data = executor
            .ctx
            .kvstore
            .get(tag_key.as_bytes())
            .await
            .unwrap()
            .unwrap();
        let tag: serde_json::Value = serde_json::from_slice(&data).unwrap();

        // Check properties
        let props = tag["properties"].as_array().unwrap();
        assert_eq!(props.len(), 2);
        assert_eq!(props[1]["name"], "age");
        assert_eq!(props[1]["nullable"], true);

        // Check version increment
        assert_eq!(tag["version"], 2);
    }

    #[tokio::test]
    async fn test_alter_tag_duplicate_column() {
        let executor = create_executor();

        // Setup: Tag with 'age'
        let tag_key = "space:default:tag:player_dup";
        let initial_tag = serde_json::json!({
            "name": "player_dup",
            "properties": [
                {"name": "age", "data_type": "Int32", "nullable": true}
            ],
            "version": 1
        });
        executor
            .ctx
            .kvstore
            .put(
                tag_key.as_bytes(),
                serde_json::to_vec(&initial_tag).unwrap().as_slice(),
            )
            .await
            .unwrap();

        // Try adding 'age' again
        let plan = AlterPlan::Tag {
            name: "player_dup".to_string(),
            operations: vec![AlterColumnOp {
                op_type: AlterOpType::AddColumn,
                prop: PropertyDef {
                    name: "age".to_string(),
                    data_type: DataType::Int32,
                    nullable: true,
                    default_value: None,
                },
            }],
        };

        let result = executor.execute_alter(plan).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ExecutionError::InvalidOperation(msg) => assert!(msg.contains("already exists")),
            _ => panic!("Expected InvalidOperation error"),
        }
    }

    #[tokio::test]
    async fn test_alter_tag_validation_non_null_no_default() {
        let executor = create_executor();

        // Setup
        let tag_key = "space:default:tag:player_val";
        let initial_tag = serde_json::json!({
            "name": "player_val",
            "properties": [],
            "version": 1
        });
        executor
            .ctx
            .kvstore
            .put(
                tag_key.as_bytes(),
                serde_json::to_vec(&initial_tag).unwrap().as_slice(),
            )
            .await
            .unwrap();

        // Try adding non-nullable column without default
        let plan = AlterPlan::Tag {
            name: "player_val".to_string(),
            operations: vec![AlterColumnOp {
                op_type: AlterOpType::AddColumn,
                prop: PropertyDef {
                    name: "score".to_string(),
                    data_type: DataType::Int64,
                    nullable: false,
                    default_value: None,
                },
            }],
        };

        let result = executor.execute_alter(plan).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ExecutionError::InvalidOperation(msg) => {
                assert!(msg.contains("must be nullable or have a default"))
            }
            _ => panic!("Expected InvalidOperation error"),
        }
    }

    #[tokio::test]
    async fn test_alter_tag_corrupted_schema() {
        let executor = create_executor();

        // Setup: Tag with missing properties (simulated corruption)
        let tag_key = "space:default:tag:corrupt";
        let initial_tag = serde_json::json!({
            "name": "corrupt",
            // "properties": [] MISSING
            "version": 1
        });
        executor
            .ctx
            .kvstore
            .put(
                tag_key.as_bytes(),
                serde_json::to_vec(&initial_tag).unwrap().as_slice(),
            )
            .await
            .unwrap();

        let plan = AlterPlan::Tag {
            name: "corrupt".to_string(),
            operations: vec![AlterColumnOp {
                op_type: AlterOpType::AddColumn,
                prop: PropertyDef {
                    name: "new_col".to_string(),
                    data_type: DataType::Int32,
                    nullable: true,
                    default_value: None,
                },
            }],
        };

        let result = executor.execute_alter(plan).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ExecutionError::InvalidOperation(msg) => {
                assert!(msg.contains("missing 'properties' array"))
            }
            _ => panic!("Expected InvalidOperation error"),
        }
    }

    // ===== INSERT Tests =====

    #[tokio::test]
    async fn test_insert_vertex() {
        let executor = create_executor();

        // Setup: create tag schema first (required by S-4 schema validation)
        let tag_key = SchemaKey::tag("default", "player");
        let tag_schema = serde_json::json!({
            "name": "player",
            "properties": [{"name": "name", "data_type": "String", "nullable": true}]
        });
        executor
            .ctx
            .kvstore
            .put(
                &tag_key,
                serde_json::to_vec(&tag_schema).unwrap().as_slice(),
            )
            .await
            .unwrap();

        let plan = InsertPlan::Vertex {
            space: "default".to_string(),
            vertices: vec![VertexInsert {
                vid: 100.into(),
                tags: vec![TagData {
                    name: "player".to_string(),
                    props: [(
                        "name".to_string(),
                        byoridb_common::Value::String("Alice".to_string()),
                    )]
                    .into_iter()
                    .collect(),
                }],
            }],
        };

        let result = executor.execute_insert(plan).await.unwrap();
        assert_eq!(result.rows[0][0], byoridb_common::Value::Int(1));

        // Verify the vertex was stored (using VertexCodec to decode Proto format)
        let key = SchemaKey::vertex("default", 100);
        let data = executor.ctx.kvstore.get(&key).await.unwrap().unwrap();
        let vertex = VertexCodec::decode_vertex(&data).unwrap();
        assert_eq!(vertex.vid, 100);
        assert_eq!(vertex.tags.len(), 1);
        assert_eq!(vertex.tags[0].name, "player");
    }

    async fn put_empty_tag_schema(executor: &Executor, name: &str) {
        let schema = serde_json::json!({
            "name": name,
            "properties": []
        });
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::tag("default", name),
                &serde_json::to_vec(&schema).unwrap(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn vertex_overwrite_and_duplicate_rows_replace_tagvid_atomically() {
        let executor = create_executor();
        for tag in ["existing", "intermediate", "final"] {
            put_empty_tag_schema(&executor, tag).await;
        }

        executor
            .execute_insert(InsertPlan::Vertex {
                space: "default".to_string(),
                vertices: vec![VertexInsert {
                    vid: Vid::Int(77),
                    tags: vec![TagData {
                        name: "existing".to_string(),
                        props: HashMap::new(),
                    }],
                }],
            })
            .await
            .unwrap();

        executor
            .execute_insert(InsertPlan::Vertex {
                space: "default".to_string(),
                vertices: vec![
                    VertexInsert {
                        vid: Vid::Int(77),
                        tags: vec![TagData {
                            name: "intermediate".to_string(),
                            props: HashMap::new(),
                        }],
                    },
                    VertexInsert {
                        vid: Vid::Int(77),
                        tags: vec![TagData {
                            name: "final".to_string(),
                            props: HashMap::new(),
                        }],
                    },
                ],
            })
            .await
            .unwrap();

        for removed in ["existing", "intermediate"] {
            assert!(executor
                .ctx
                .kvstore
                .get(&SchemaKey::tagvid("default", removed, 77))
                .await
                .unwrap()
                .is_none());
        }
        assert!(executor
            .ctx
            .kvstore
            .get(&SchemaKey::tagvid("default", "final", 77))
            .await
            .unwrap()
            .is_some());
        let blob = executor
            .ctx
            .kvstore
            .get(&SchemaKey::vertex("default", 77))
            .await
            .unwrap()
            .unwrap();
        let vertex = VertexCodec::decode_vertex(&blob).unwrap();
        assert_eq!(vertex.tags.len(), 1);
        assert_eq!(vertex.tags[0].name, "final");
    }

    async fn fixed_string_executor(max_len: usize) -> Executor {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kvstore)
                .with_space("default".to_string())
                .with_space_id(1),
        );
        ctx.kvstore
            .put(
                &SchemaKey::space("default"),
                &serde_json::to_vec(&serde_json::json!({
                    "id": 1,
                    "name": "default",
                    "vid_type": format!("FIXED_STRING({max_len})")
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        Executor::new(ctx)
    }

    #[tokio::test]
    async fn multirow_validation_and_update_noop_do_not_create_string_mappings() {
        let executor = fixed_string_executor(4).await;
        put_empty_tag_schema(&executor, "person").await;

        let result = executor
            .execute_insert(InsertPlan::Vertex {
                space: "default".to_string(),
                vertices: vec![
                    VertexInsert {
                        vid: Vid::String("good".to_string()),
                        tags: vec![TagData {
                            name: "person".to_string(),
                            props: HashMap::new(),
                        }],
                    },
                    VertexInsert {
                        vid: Vid::String("too-long".to_string()),
                        tags: vec![TagData {
                            name: "person".to_string(),
                            props: HashMap::new(),
                        }],
                    },
                ],
            })
            .await;
        assert!(result.is_err());
        assert!(executor
            .ctx
            .kvstore
            .scan_prefix(b"default:vid-map:")
            .await
            .unwrap()
            .is_empty());
        assert!(executor
            .ctx
            .kvstore
            .scan_prefix(b"default:vid-rev:")
            .await
            .unwrap()
            .is_empty());

        let edge_schema = serde_json::json!({
            "name": "knows",
            "properties": []
        });
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::edge("default", "knows"),
                &serde_json::to_vec(&edge_schema).unwrap(),
            )
            .await
            .unwrap();
        let edge_result = executor
            .execute_insert(InsertPlan::Edge {
                space: "default".to_string(),
                edges: vec![
                    EdgeInsert {
                        src: Vid::String("a".to_string()),
                        dst: Vid::String("b".to_string()),
                        edge_type: "knows".to_string(),
                        ranking: 0,
                        props: HashMap::new(),
                    },
                    EdgeInsert {
                        src: Vid::String("c".to_string()),
                        dst: Vid::String("d".to_string()),
                        edge_type: "missing".to_string(),
                        ranking: 0,
                        props: HashMap::new(),
                    },
                ],
            })
            .await;
        assert!(edge_result.is_err());
        assert!(executor
            .ctx
            .kvstore
            .scan_prefix(b"default:vid-map:")
            .await
            .unwrap()
            .is_empty());
        assert!(executor
            .ctx
            .kvstore
            .scan_prefix(b"default:vid-rev:")
            .await
            .unwrap()
            .is_empty());

        let no_op = executor
            .execute_update(UpdatePlan {
                space: "default".to_string(),
                vid: Vid::String("none".to_string()),
                tag_name: Some("person".to_string()),
                updates: HashMap::new(),
                conditions: Some(Expression::Literal(Literal::Bool(false))),
                yield_clause: None,
                edge: None,
            })
            .await
            .unwrap();
        assert_eq!(no_op.rows, vec![vec![byoridb_common::Value::Int(0)]]);
        assert!(executor
            .ctx
            .kvstore
            .scan_prefix(b"default:vid-map:")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn fixed_string_live_positive_legacy_vids_support_read_traverse_and_delete() {
        let executor = fixed_string_executor(32).await;
        put_empty_tag_schema(&executor, "person").await;
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::edge("default", "knows"),
                &serde_json::to_vec(&serde_json::json!({
                    "name": "knows",
                    "properties": []
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        for vid in [10, 20] {
            let vertex = CodecVertexData {
                vid,
                tags: vec![CodecTagData {
                    name: "person".to_string(),
                    properties: HashMap::new(),
                }],
            };
            executor
                .ctx
                .kvstore
                .put(
                    &SchemaKey::vertex("default", vid),
                    &VertexCodec::encode_vertex(&vertex).unwrap(),
                )
                .await
                .unwrap();
            executor
                .ctx
                .kvstore
                .put(&SchemaKey::tagvid("default", "person", vid), &[])
                .await
                .unwrap();
        }
        let edge = CodecEdgeData {
            src_vid: 10,
            dst_vid: 20,
            edge_type: "knows".to_string(),
            ranking: 0,
            properties: HashMap::new(),
        };
        let edge_blob = VertexCodec::encode_edge(&edge).unwrap();
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::edge_data("default", 10, "knows", 20, 0),
                &edge_blob,
            )
            .await
            .unwrap();
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::in_edge_data("default", 20, "knows", 10, 0),
                &edge_blob,
            )
            .await
            .unwrap();

        let plan = |query: &str| {
            ExecutionPlanBuilder::build(byoridb_parser::parse(query).unwrap()).unwrap()
        };
        let fetched = executor
            .execute(plan("FETCH PROP ON person 10"))
            .await
            .unwrap();
        assert_eq!(fetched.rows[0][0], byoridb_common::Value::Int(10));

        let matched = executor
            .execute(plan("MATCH (n:person) RETURN id(n)"))
            .await
            .unwrap();
        assert_eq!(matched.rows.len(), 2);
        assert!(matched
            .rows
            .iter()
            .all(|row| matches!(row.first(), Some(byoridb_common::Value::Int(_)))));

        let traversed = executor
            .execute(plan("GO FROM 10 OVER knows"))
            .await
            .unwrap();
        assert_eq!(
            traversed.rows,
            vec![vec![
                byoridb_common::Value::Int(10),
                byoridb_common::Value::Int(20)
            ]]
        );

        let edge_deleted = executor
            .execute(plan("DELETE EDGE knows 10->20"))
            .await
            .unwrap();
        assert_eq!(edge_deleted.rows[0][0], byoridb_common::Value::Int(1));
        let vertex_deleted = executor.execute(plan("DELETE VERTEX 20")).await.unwrap();
        assert_eq!(vertex_deleted.rows[0][0], byoridb_common::Value::Int(1));

        let integer_edge_write = executor
            .execute_insert(InsertPlan::Edge {
                space: "default".to_string(),
                edges: vec![EdgeInsert {
                    src: Vid::Int(10),
                    dst: Vid::String("new".to_string()),
                    edge_type: "knows".to_string(),
                    ranking: 0,
                    props: HashMap::new(),
                }],
            })
            .await;
        assert!(integer_edge_write
            .unwrap_err()
            .to_string()
            .contains("read/delete-only legacy data"));

        let integer_update = executor
            .execute_update(UpdatePlan {
                space: "default".to_string(),
                vid: Vid::Int(10),
                tag_name: Some("person".to_string()),
                updates: HashMap::new(),
                conditions: None,
                yield_clause: None,
                edge: None,
            })
            .await;
        assert!(integer_update
            .unwrap_err()
            .to_string()
            .contains("read/delete-only legacy data"));

        let vid_type = crate::vid::space_vid_type(&executor.ctx, "default")
            .await
            .unwrap();
        let mapped_internal = crate::vid::resolve_vid(
            &executor.ctx,
            "default",
            vid_type,
            &Vid::String("mapped".to_string()),
            true,
        )
        .await
        .unwrap()
        .unwrap();

        let raw_negative = executor
            .execute(ExecutionPlan::Fetch(FetchPlan {
                space: "default".to_string(),
                vids: vec![Vid::Int(mapped_internal)],
                tags: vec!["person".to_string()],
                yield_clause: None,
                edge_refs: vec![],
                is_edge_fetch: false,
                src_var: None,
                as_of: None,
            }))
            .await;
        assert!(raw_negative
            .unwrap_err()
            .to_string()
            .contains("raw negative internal VID"));
    }

    #[tokio::test]
    async fn unknown_string_delete_cannot_remove_old_positive_hash_vid() {
        let executor = fixed_string_executor(32).await;
        let external = "unknown-legacy-collision";
        let mut old_positive =
            (byoridb_common::hash::hash_bytes(external.as_bytes()) & i64::MAX as u64) as i64;
        if old_positive == 0 {
            old_positive = 1;
        }
        let vertex = CodecVertexData {
            vid: old_positive,
            tags: vec![CodecTagData {
                name: "person".to_string(),
                properties: HashMap::new(),
            }],
        };
        executor
            .ctx
            .kvstore
            .put(
                &SchemaKey::vertex("default", old_positive),
                &VertexCodec::encode_vertex(&vertex).unwrap(),
            )
            .await
            .unwrap();

        let result = executor
            .execute_delete(DeletePlan {
                space: "default".to_string(),
                vids: vec![Vid::String(external.to_string())],
                conditions: None,
            })
            .await
            .unwrap();
        assert_eq!(result.rows[0][0], byoridb_common::Value::Int(0));
        assert!(executor
            .ctx
            .kvstore
            .get(&SchemaKey::vertex("default", old_positive))
            .await
            .unwrap()
            .is_some());
    }

    // ===== DELETE Tests =====

    #[tokio::test]
    async fn test_delete_vertex() {
        let executor = create_executor();

        // Setup: Insert a vertex first
        let vertex_key = SchemaKey::vertex("default", 200);
        let vertex_data = serde_json::json!({
            "vid": 200,
            "tags": [{"name": "player", "props": {"name": "Bob"}}]
        });
        executor
            .ctx
            .kvstore
            .put(
                &vertex_key,
                serde_json::to_vec(&vertex_data).unwrap().as_slice(),
            )
            .await
            .unwrap();

        // Delete the vertex
        let plan = DeletePlan {
            space: "default".to_string(),
            vids: vec![200.into()],
            conditions: None,
        };

        let result = executor.execute_delete(plan).await.unwrap();
        assert_eq!(result.rows[0][0], byoridb_common::Value::Int(1));

        // Verify the vertex was deleted
        let data = executor.ctx.kvstore.get(&vertex_key).await.unwrap();
        assert!(data.is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_vertex() {
        let executor = create_executor();

        // Try to delete a vertex that doesn't exist
        let plan = DeletePlan {
            space: "default".to_string(),
            vids: vec![999.into()],
            conditions: None,
        };

        let result = executor.execute_delete(plan).await.unwrap();
        assert_eq!(result.rows[0][0], byoridb_common::Value::Int(0));
    }

    // ===== FETCH Tests =====

    #[tokio::test]
    async fn test_fetch_vertex() {
        let executor = create_executor();

        // Setup: Insert a vertex first
        let vertex_key = SchemaKey::vertex("default", 300);
        let vertex_data = serde_json::json!({
            "vid": 300,
            "tags": [{"name": "player", "props": {"name": "Charlie", "age": 25}}]
        });
        executor
            .ctx
            .kvstore
            .put(
                &vertex_key,
                serde_json::to_vec(&vertex_data).unwrap().as_slice(),
            )
            .await
            .unwrap();

        // Fetch the vertex
        let plan = FetchPlan {
            space: "default".to_string(),
            vids: vec![300.into()],
            tags: vec!["player".to_string()],
            yield_clause: None,
            edge_refs: vec![],
            is_edge_fetch: false,
            src_var: None,
            as_of: None,
        };

        let result = executor.execute_fetch(plan).await.unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], byoridb_common::Value::Int(300));
    }

    #[tokio::test]
    async fn test_fetch_multiple_vertices() {
        let executor = create_executor();

        // Setup: Insert multiple vertices
        for vid in [301, 302, 303] {
            let key = SchemaKey::vertex("default", vid);
            let data = serde_json::json!({
                "vid": vid,
                "tags": [{"name": "player", "props": {"id": vid}}]
            });
            executor
                .ctx
                .kvstore
                .put(&key, serde_json::to_vec(&data).unwrap().as_slice())
                .await
                .unwrap();
        }

        // Fetch multiple vertices
        let plan = FetchPlan {
            space: "default".to_string(),
            vids: vec![301.into(), 302.into(), 303.into()],
            tags: vec!["player".to_string()],
            yield_clause: None,
            edge_refs: vec![],
            is_edge_fetch: false,
            src_var: None,
            as_of: None,
        };

        let result = executor.execute_fetch(plan).await.unwrap();
        assert_eq!(result.rows.len(), 3);
    }

    #[tokio::test]
    async fn test_fetch_nonexistent_vertex() {
        let executor = create_executor();

        // Fetch a vertex that doesn't exist
        let plan = FetchPlan {
            space: "default".to_string(),
            vids: vec![888.into()],
            tags: vec!["player".to_string()],
            yield_clause: None,
            edge_refs: vec![],
            is_edge_fetch: false,
            src_var: None,
            as_of: None,
        };

        let result = executor.execute_fetch(plan).await.unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[tokio::test]
    async fn test_fetch_thousand_vids_preserves_input_order_and_skips_missing() {
        let executor = create_executor();
        let missing = [250i64, 750];
        let mut pairs = Vec::new();
        for vid in 1i64..=1000 {
            if missing.contains(&vid) {
                continue;
            }
            let vertex = CodecVertexData {
                vid,
                tags: vec![CodecTagData {
                    name: "item".to_string(),
                    properties: HashMap::from([(
                        "ordinal".to_string(),
                        byoridb_common::Value::Int(vid),
                    )]),
                }],
            };
            pairs.push((
                format!("default:vertex:{vid}").into_bytes(),
                VertexCodec::encode_vertex(&vertex).unwrap(),
            ));
        }
        executor.ctx.kvstore.batch_put(pairs).await.unwrap();

        let result = executor
            .execute_fetch(FetchPlan {
                space: "default".to_string(),
                vids: (1i64..=1000).map(Into::into).collect(),
                tags: vec!["item".to_string()],
                yield_clause: None,
                edge_refs: vec![],
                is_edge_fetch: false,
                src_var: None,
                as_of: None,
            })
            .await
            .unwrap();

        let actual: Vec<i64> = result
            .rows
            .iter()
            .map(|row| match row.first() {
                Some(byoridb_common::Value::Int(vid)) => *vid,
                other => panic!("expected VID in first FETCH column, got {other:?}"),
            })
            .collect();
        let expected: Vec<i64> = (1i64..=1000).filter(|vid| !missing.contains(vid)).collect();
        assert_eq!(actual, expected);
    }

    // ===== GO Tests =====

    #[tokio::test]
    async fn test_go_traversal() {
        let executor = create_executor();

        // Setup: Create vertices
        for vid in [400, 401, 402] {
            let key = SchemaKey::vertex("default", vid);
            let data = serde_json::json!({"vid": vid, "tags": []});
            executor
                .ctx
                .kvstore
                .put(&key, serde_json::to_vec(&data).unwrap().as_slice())
                .await
                .unwrap();
        }

        // Create edges: 400 -> 401, 400 -> 402
        insert_test_edge(&executor, 400, 401, "follow", 0).await;
        insert_test_edge(&executor, 400, 402, "follow", 1).await;

        // GO from 400 over follow
        let plan = GoPlan {
            from_clause: FromClause {
                vids: vec![400.into()],
                src: None,
            },
            over_edges: vec!["follow".to_string()],
            direction: byoridb_parser::ast::EdgeDirection::Outgoing,
            to_clause: ToClause {
                steps: StepClause::Exactly(1),
                variable: "v".to_string(),
            },
            where_clause: None,
            yield_clause: YieldClause { columns: vec![] },
        };

        let result = executor.execute_go(plan).await.unwrap();
        let mut destinations: Vec<i64> = result
            .rows
            .iter()
            .filter_map(|row| match row.get(1) {
                Some(byoridb_common::Value::Int(dst)) => Some(*dst),
                _ => None,
            })
            .collect();
        destinations.sort_unstable();
        assert_eq!(destinations, vec![401, 402]);
    }

    #[tokio::test]
    async fn test_go_destination_projection_batches_multitag_and_null_values() {
        let executor = create_executor();
        let destination_vertices = [
            CodecVertexData {
                vid: 2,
                tags: vec![
                    CodecTagData {
                        name: "person".to_string(),
                        properties: HashMap::from([(
                            "name".to_string(),
                            byoridb_common::Value::String("Bob".to_string()),
                        )]),
                    },
                    CodecTagData {
                        name: "stats".to_string(),
                        properties: HashMap::from([(
                            "score".to_string(),
                            byoridb_common::Value::Int(7),
                        )]),
                    },
                ],
            },
            CodecVertexData {
                vid: 3,
                tags: vec![CodecTagData {
                    name: "person".to_string(),
                    properties: HashMap::from([(
                        "name".to_string(),
                        byoridb_common::Value::String("Cara".to_string()),
                    )]),
                }],
            },
        ];
        executor
            .ctx
            .kvstore
            .batch_put(
                destination_vertices
                    .iter()
                    .map(|vertex| {
                        (
                            format!("default:vertex:{}", vertex.vid).into_bytes(),
                            VertexCodec::encode_vertex(vertex).unwrap(),
                        )
                    })
                    .collect(),
            )
            .await
            .unwrap();
        insert_test_edge(&executor, 1, 2, "follow", 0).await;
        insert_test_edge(&executor, 1, 3, "follow", 0).await;

        let profile = executor.ctx.enable_profile();
        let result = executor
            .execute_go(GoPlan {
                from_clause: FromClause {
                    vids: vec![1.into()],
                    src: None,
                },
                over_edges: vec!["follow".to_string()],
                direction: byoridb_parser::ast::EdgeDirection::Outgoing,
                to_clause: ToClause {
                    steps: StepClause::Exactly(1),
                    variable: "v".to_string(),
                },
                where_clause: None,
                yield_clause: YieldClause {
                    columns: vec![
                        YieldColumn {
                            expression: Expression::PropRef {
                                object: "follow".to_string(),
                                prop: "_dst".to_string(),
                            },
                            alias: Some("dst".to_string()),
                        },
                        YieldColumn {
                            expression: Expression::DstVertexProp {
                                tag: "person".to_string(),
                                prop: "name".to_string(),
                            },
                            alias: Some("name".to_string()),
                        },
                        YieldColumn {
                            expression: Expression::DstVertexProp {
                                tag: "stats".to_string(),
                                prop: "score".to_string(),
                            },
                            alias: Some("score".to_string()),
                        },
                        YieldColumn {
                            expression: Expression::DstVertexProp {
                                tag: "stats".to_string(),
                                prop: "missing".to_string(),
                            },
                            alias: Some("missing".to_string()),
                        },
                    ],
                },
            })
            .await
            .unwrap();

        assert_eq!(
            result.rows,
            vec![
                vec![
                    byoridb_common::Value::Int(2),
                    byoridb_common::Value::String("Bob".to_string()),
                    byoridb_common::Value::Int(7),
                    byoridb_common::Value::Null(byoridb_common::NullType::Null),
                ],
                vec![
                    byoridb_common::Value::Int(3),
                    byoridb_common::Value::String("Cara".to_string()),
                    byoridb_common::Value::Null(byoridb_common::NullType::Null),
                    byoridb_common::Value::Null(byoridb_common::NullType::Null),
                ],
            ]
        );
        let batch_records: Vec<_> = profile
            .snapshot()
            .into_iter()
            .filter(|record| record.op == crate::profile::ProfileOp::GetVertices)
            .collect();
        assert_eq!(batch_records.len(), 1);
        assert!(batch_records[0].detail.contains("2 unique vid(s), 2 found"));
        executor.ctx.disable_profile();
    }

    #[tokio::test]
    async fn test_go_reversely_traversal() {
        let executor = create_executor();

        insert_test_edge(&executor, 431, 430, "follow", 0).await;
        insert_test_edge(&executor, 432, 430, "follow", 1).await;
        insert_test_edge(&executor, 430, 433, "follow", 0).await;

        let plan = GoPlan {
            from_clause: FromClause {
                vids: vec![430.into()],
                src: None,
            },
            over_edges: vec!["follow".to_string()],
            direction: byoridb_parser::ast::EdgeDirection::Incoming,
            to_clause: ToClause {
                steps: StepClause::Exactly(1),
                variable: "v".to_string(),
            },
            where_clause: None,
            yield_clause: YieldClause { columns: vec![] },
        };

        let result = executor.execute_go(plan).await.unwrap();
        let mut sources: Vec<i64> = result
            .rows
            .iter()
            .filter_map(|row| match row.get(1) {
                Some(byoridb_common::Value::Int(src)) => Some(*src),
                _ => None,
            })
            .collect();
        sources.sort_unstable();
        assert_eq!(sources, vec![431, 432]);
    }

    #[tokio::test]
    async fn test_go_traversal_exactly_two_steps_uses_next_frontier() {
        let executor = create_executor();

        for vid in [410, 411, 412, 413] {
            let key = SchemaKey::vertex("default", vid);
            let data = serde_json::json!({"vid": vid, "tags": []});
            executor
                .ctx
                .kvstore
                .put(&key, serde_json::to_vec(&data).unwrap().as_slice())
                .await
                .unwrap();
        }

        insert_test_edge(&executor, 410, 411, "follow", 0).await;
        insert_test_edge(&executor, 411, 412, "follow", 0).await;
        insert_test_edge(&executor, 410, 413, "follow", 1).await;

        let plan = GoPlan {
            from_clause: FromClause {
                vids: vec![410.into()],
                src: None,
            },
            over_edges: vec!["follow".to_string()],
            direction: byoridb_parser::ast::EdgeDirection::Outgoing,
            to_clause: ToClause {
                steps: StepClause::Exactly(2),
                variable: "v".to_string(),
            },
            where_clause: None,
            yield_clause: YieldClause { columns: vec![] },
        };

        let result = executor.execute_go(plan).await.unwrap();
        assert_eq!(
            result.rows,
            vec![vec![
                byoridb_common::Value::Int(410),
                byoridb_common::Value::Int(412)
            ]]
        );
    }

    #[tokio::test]
    async fn test_go_traversal_deduplicates_destinations_per_source() {
        let executor = create_executor();

        insert_test_edge(&executor, 420, 421, "follow", 0).await;
        insert_test_edge(&executor, 420, 422, "follow", 1).await;
        insert_test_edge(&executor, 421, 423, "follow", 0).await;
        insert_test_edge(&executor, 422, 423, "follow", 0).await;

        let plan = GoPlan {
            from_clause: FromClause {
                vids: vec![420.into()],
                src: None,
            },
            over_edges: vec!["follow".to_string()],
            direction: byoridb_parser::ast::EdgeDirection::Outgoing,
            to_clause: ToClause {
                steps: StepClause::Exactly(2),
                variable: "v".to_string(),
            },
            where_clause: None,
            yield_clause: YieldClause { columns: vec![] },
        };

        let result = executor.execute_go(plan).await.unwrap();
        assert_eq!(
            result.rows,
            vec![vec![
                byoridb_common::Value::Int(420),
                byoridb_common::Value::Int(423)
            ]]
        );
    }

    #[tokio::test]
    async fn test_find_shortest_path_with_weight_uses_dijkstra() {
        let executor = create_executor();

        insert_weighted_test_edge(&executor, 1, 2, "follow", 0, 10.0).await;
        insert_weighted_test_edge(&executor, 1, 3, "follow", 1, 1.0).await;
        insert_weighted_test_edge(&executor, 3, 2, "follow", 0, 1.0).await;

        let plan = FindPlan {
            find_type: FindType::ShortestPath,
            from_vid: Expression::Literal(Literal::Int(1)),
            to_vid: Expression::Literal(Literal::Int(2)),
            over_edge: "follow".to_string(),
            weight_prop: Some("cost".to_string()),
            bidirect: false,
            upto_steps: None,
            where_clause: None,
            yield_clause: None,
        };

        let result = executor.execute_find(plan).await.unwrap();
        assert_eq!(result.rows, vec![vec![path_value(&[1, 3, 2])]]);
    }

    #[tokio::test]
    async fn test_compound_go_via_var_resolves_through_ctx_vars() {
        // `$a = GO FROM 1 OVER follow` binds a one-row result `[2]`, and the
        // second clause `GO FROM $a.dst OVER follow` resolves `$a.dst` to 2
        // so we expect the trailing result to include `[3]` from 2->3.
        let executor = create_executor();

        insert_test_edge(&executor, 1, 2, "follow", 0).await;
        insert_test_edge(&executor, 2, 3, "follow", 0).await;

        let stmt = byoridb_parser::parse("$a = GO FROM 1 OVER follow; GO FROM $a.dst OVER follow")
            .unwrap();
        let plan = ExecutionPlanBuilder::build(stmt).unwrap();

        let result = executor.execute(plan).await.unwrap();

        // First-clause binding shape: ["src", "dst"], rows=[[1,2]].
        let bound = executor.ctx.lookup_var("a").expect("$a must be bound");
        assert_eq!(bound.columns, vec!["src".to_string(), "dst".to_string()]);
        assert_eq!(
            bound.rows,
            vec![vec![
                byoridb_common::Value::Int(1),
                byoridb_common::Value::Int(2)
            ]]
        );

        // Final clause: with $a.dst resolving to 2, GO FROM 2 OVER follow
        // produces [[2, 3]] (one row, src=2 dst=3).
        assert_eq!(
            result.rows,
            vec![vec![
                byoridb_common::Value::Int(2),
                byoridb_common::Value::Int(3)
            ]]
        );
    }

    #[tokio::test]
    async fn test_compound_use_switches_space_for_subsequent_clauses() {
        // Regression (production incident): `USE other; <stmt>` in ONE request
        // must route <stmt> to `other`, not the session's prior/unset space.
        // A fully-populated space looked empty because the MATCH after `USE`
        // ran on the stale space. The executor's default space here is "default".
        let executor = create_executor();

        // Create `other` so USE existence-validation passes. The default space
        // ("default") need not exist — the negative assertion below only checks
        // that nothing was written under it.
        let stmt = byoridb_parser::parse("CREATE SPACE other").unwrap();
        let plan = ExecutionPlanBuilder::build(stmt).unwrap();
        executor.execute(plan).await.unwrap();

        // One compound: switch to `other`, then create a tag. With the fix the
        // tag is created in `other`; without it, in the stale "default".
        let stmt = byoridb_parser::parse("USE other; CREATE TAG person()").unwrap();
        let plan = ExecutionPlanBuilder::build(stmt).unwrap();
        executor.execute(plan).await.unwrap();

        let in_other = executor
            .ctx
            .kvstore
            .get(&SchemaKey::tag("other", "person"))
            .await
            .unwrap();
        let in_default = executor
            .ctx
            .kvstore
            .get(&SchemaKey::tag("default", "person"))
            .await
            .unwrap();
        assert!(
            in_other.is_some(),
            "USE other must route the subsequent CREATE TAG to `other`"
        );
        assert!(
            in_default.is_none(),
            "CREATE TAG must NOT land in the default space after USE other"
        );
    }

    #[tokio::test]
    async fn test_compound_use_unknown_space_errors() {
        // A `USE` of a non-existent space inside a compound must error (not
        // silently misroute the following clauses).
        let executor = create_executor();
        let stmt = byoridb_parser::parse("USE nonexistent; CREATE TAG t()").unwrap();
        let plan = ExecutionPlanBuilder::build(stmt).unwrap();
        let err = executor.execute(plan).await.unwrap_err();
        assert!(
            matches!(err, ExecutionError::SpaceNotFound(_)),
            "expected SpaceNotFound, got {err:?}"
        );
    }

    #[tokio::test]
    async fn test_compound_undefined_var_errors_clearly() {
        let executor = create_executor();
        let stmt = byoridb_parser::parse("GO FROM $missing.dst OVER follow").unwrap();
        let plan = ExecutionPlanBuilder::build(stmt).unwrap();
        let err = executor.execute(plan).await.unwrap_err();
        match err {
            ExecutionError::InvalidOperation(msg) => {
                assert!(
                    msg.contains("missing"),
                    "error should name the var: {}",
                    msg
                );
            }
            other => panic!("expected InvalidOperation, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_find_shortest_path_weight_query_executes_end_to_end() {
        let executor = create_executor();

        insert_weighted_test_edge(&executor, 1, 2, "follow", 0, 10.0).await;
        insert_weighted_test_edge(&executor, 1, 3, "follow", 1, 1.0).await;
        insert_weighted_test_edge(&executor, 3, 2, "follow", 0, 1.0).await;

        let statement =
            byoridb_parser::parse("FIND SHORTEST PATH FROM 1 TO 2 OVER follow WEIGHT BY cost")
                .unwrap();
        let plan = ExecutionPlanBuilder::build(statement).unwrap();

        let result = executor.execute(plan).await.unwrap();
        assert_eq!(result.rows, vec![vec![path_value(&[1, 3, 2])]]);
    }

    /// Expected FIND PATH row value: a list of vids.
    fn path_value(vids: &[i64]) -> byoridb_common::Value {
        byoridb_common::Value::List(byoridb_common::datatypes::list::List::with_values(
            vids.iter()
                .map(|&v| byoridb_common::Value::Int(v))
                .collect(),
        ))
    }

    #[tokio::test]
    async fn test_find_all_shortest_paths_end_to_end() {
        let executor = create_executor();

        // Diamond 1 -> {2, 3} -> 4 plus a longer detour 1->5->6->4.
        insert_test_edge(&executor, 1, 2, "follow", 0).await;
        insert_test_edge(&executor, 1, 3, "follow", 0).await;
        insert_test_edge(&executor, 2, 4, "follow", 0).await;
        insert_test_edge(&executor, 3, 4, "follow", 0).await;
        insert_test_edge(&executor, 1, 5, "follow", 0).await;
        insert_test_edge(&executor, 5, 6, "follow", 0).await;
        insert_test_edge(&executor, 6, 4, "follow", 0).await;

        let statement =
            byoridb_parser::parse("FIND ALL SHORTEST PATHS FROM 1 TO 4 OVER follow UPTO 5 STEPS")
                .unwrap();
        let plan = ExecutionPlanBuilder::build(statement).unwrap();

        let result = executor.execute(plan).await.unwrap();
        assert_eq!(result.columns, vec!["path".to_string()]);
        let mut rows = result.rows;
        rows.sort_by_key(|r| format!("{:?}", r));
        assert_eq!(
            rows,
            vec![vec![path_value(&[1, 2, 4])], vec![path_value(&[1, 3, 4])]]
        );
    }

    #[tokio::test]
    async fn test_find_upto_exceeding_max_go_steps_errors() {
        let executor = create_executor();

        let statement =
            byoridb_parser::parse("FIND SHORTEST PATH FROM 1 TO 2 OVER follow UPTO 999 STEPS")
                .unwrap();
        let plan = ExecutionPlanBuilder::build(statement).unwrap();

        let err = executor.execute(plan).await.unwrap_err();
        match err {
            ExecutionError::InvalidOperation(msg) => {
                assert!(msg.contains("UPTO"), "unexpected message: {}", msg)
            }
            other => panic!("expected InvalidOperation, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_find_weight_by_rejects_bidirect() {
        let executor = create_executor();

        let statement = byoridb_parser::parse(
            "FIND SHORTEST PATH FROM 1 TO 2 OVER follow WEIGHT BY cost BIDIRECT",
        )
        .unwrap();
        let plan = ExecutionPlanBuilder::build(statement).unwrap();

        let err = executor.execute(plan).await.unwrap_err();
        assert!(matches!(err, ExecutionError::InvalidOperation(_)));
    }

    // ===== LOOKUP Tests =====

    async fn lookup_vids(executor: &Executor, query: &str) -> Vec<i64> {
        let statement = byoridb_parser::parse(query).unwrap();
        let plan = ExecutionPlanBuilder::build(statement).unwrap();
        executor
            .execute(plan)
            .await
            .unwrap()
            .rows
            .iter()
            .filter_map(|row| match row.first() {
                Some(byoridb_common::Value::Int(vid)) => Some(*vid),
                _ => None,
            })
            .collect()
    }

    async fn put_lookup_schema(kvstore: &Arc<MemoryKVStore>, tag: &str, fields: &[(&str, &str)]) {
        let properties: Vec<_> = fields
            .iter()
            .map(|(name, data_type)| serde_json::json!({"name": name, "data_type": data_type}))
            .collect();
        kvstore
            .put(
                &SchemaKey::tag("default", tag),
                &serde_json::to_vec(&serde_json::json!({
                    "name": tag,
                    "properties": properties
                }))
                .unwrap(),
            )
            .await
            .unwrap();
    }

    async fn put_lookup_vertex(
        kvstore: &Arc<MemoryKVStore>,
        tag: &str,
        vid: i64,
        properties: HashMap<String, byoridb_common::Value>,
    ) {
        let vertex = CodecVertexData {
            vid,
            tags: vec![CodecTagData {
                name: tag.to_string(),
                properties,
            }],
        };
        kvstore
            .put(
                format!("default:vertex:{vid}").as_bytes(),
                &VertexCodec::encode_vertex(&vertex).unwrap(),
            )
            .await
            .unwrap();
    }

    #[test]
    fn distributed_runtime_range_guard_recurses_through_logical_predicates() {
        for query in [
            "LOOKUP ON person WHERE person.age > 30 AND person.enabled == true",
            "LOOKUP ON person WHERE person.enabled == true OR person.age <= 30",
            "LOOKUP ON person WHERE NOT (person.age > 30)",
        ] {
            let statement = byoridb_parser::parse(query).unwrap();
            let plan = ExecutionPlanBuilder::build(statement).unwrap();
            let ExecutionPlan::Lookup(plan) = plan else {
                panic!("expected LOOKUP plan");
            };
            assert!(
                expression_contains_ordered_range(plan.where_clause.as_ref().unwrap()),
                "query: {query}"
            );
        }

        let statement = byoridb_parser::parse(
            "LOOKUP ON person WHERE person.age == 30 AND person.enabled == true",
        )
        .unwrap();
        let plan = ExecutionPlanBuilder::build(statement).unwrap();
        let ExecutionPlan::Lookup(plan) = plan else {
            panic!("expected LOOKUP plan");
        };
        assert!(!expression_contains_ordered_range(
            plan.where_clause.as_ref().unwrap()
        ));
    }

    #[tokio::test]
    async fn test_lookup_by_scan() {
        let executor = create_executor();

        // Setup: Create vertices
        for (vid, name) in [(500i64, "Eve"), (501, "Frank"), (502, "Grace")] {
            let key = SchemaKey::vertex("default", vid);
            let data = serde_json::json!({
                "vid": vid,
                "tags": [{"name": "player", "props": {"name": name}}]
            });
            executor
                .ctx
                .kvstore
                .put(&key, serde_json::to_vec(&data).unwrap().as_slice())
                .await
                .unwrap();
        }

        // LOOKUP on player tag (no WHERE clause - returns all)
        let plan = LookupPlan {
            lookup_type: LookupType::Tag("player".to_string()),
            where_clause: None,
            yield_clause: YieldClause { columns: vec![] },
            limit: None,
            offset: None,
        };

        let result = executor.execute_lookup(plan).await.unwrap();
        // Should find at least the 3 vertices we created
        assert!(result.rows.len() >= 3);
    }

    #[tokio::test]
    async fn test_lookup_uses_single_field_range_index() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kvstore.clone())
                .with_space("default".to_string())
                .with_space_id(1),
        );
        kvstore
            .put(
                &SchemaKey::tag("default", "person"),
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

        for (vid, age) in [(1i64, 10i64), (2, 30), (3, 31), (4, 50)] {
            let vertex = CodecVertexData {
                vid,
                tags: vec![CodecTagData {
                    name: "person".to_string(),
                    properties: HashMap::from([(
                        "age".to_string(),
                        byoridb_common::Value::Int(age),
                    )]),
                }],
            };
            kvstore
                .put(
                    format!("default:vertex:{vid}").as_bytes(),
                    &VertexCodec::encode_vertex(&vertex).unwrap(),
                )
                .await
                .unwrap();
            index_manager
                .insert_tag_index(1, index_id, &[IndexValue::Int(age)], vid)
                .await
                .unwrap();
        }

        let executor = Executor::new(ctx);
        for (predicate, expected) in [
            ("person.age > 30", vec![3, 4]),
            ("person.age >= 30", vec![2, 3, 4]),
            ("person.age < 30", vec![1]),
            ("person.age <= 30", vec![1, 2]),
            ("30 < person.age", vec![3, 4]),
            // Cross-type numeric comparison remains correct by using the
            // predicate fallback rather than probing the wrong key type.
            ("person.age > 30.5", vec![3, 4]),
            ("30.5 < person.age", vec![3, 4]),
        ] {
            let vids = lookup_vids(&executor, &format!("LOOKUP ON person WHERE {predicate}")).await;
            assert_eq!(vids, expected, "predicate: {predicate}");
        }

        assert_eq!(
            lookup_vids(
                &executor,
                "LOOKUP ON person WHERE person.age > 0 LIMIT 2 OFFSET 1",
            )
            .await,
            vec![2, 3]
        );
    }

    #[tokio::test]
    async fn float_schema_range_falls_back_for_mixed_index_key_domains() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kvstore.clone())
                .with_space("default".to_string())
                .with_space_id(1),
        );
        put_lookup_schema(&kvstore, "metric", &[("score", "Double")]).await;
        let index_manager = ctx.index_manager.as_ref().unwrap().clone();
        let index_id = index_manager
            .create_tag_index(
                1,
                "metric_score_idx".to_string(),
                10,
                "metric".to_string(),
                vec!["score".to_string()],
                vec![0],
            )
            .await
            .unwrap();
        let executor = Executor::new(ctx);

        // Double columns accept both literal variants. DML intentionally keeps
        // their physical Value variants, which produces separate Int and Float
        // index marker domains until numeric index writes are canonicalized.
        let statement =
            byoridb_parser::parse("INSERT VERTEX metric(score) VALUES 1:(2), 2:(2.5)").unwrap();
        let plan = ExecutionPlanBuilder::build(statement).unwrap();
        executor.execute(plan).await.unwrap();

        let index = index_manager.get_index_by_id(1, index_id).await.unwrap();
        assert_eq!(
            index_manager
                .lookup_tag(1, &index, &[IndexValue::Int(2)], 10)
                .await
                .unwrap(),
            vec![1]
        );
        assert_eq!(
            index_manager
                .lookup_tag(1, &index, &[IndexValue::Float(2.5)], 10)
                .await
                .unwrap(),
            vec![2]
        );

        for query in [
            "LOOKUP ON metric WHERE metric.score > 1",
            "LOOKUP ON metric WHERE metric.score > 1.0",
        ] {
            assert_eq!(lookup_vids(&executor, query).await, vec![1, 2], "{query}");
        }
        assert!(executor.ctx().took_full_scan());
    }

    #[tokio::test]
    async fn lookup_indexes_are_scoped_to_the_requested_tag() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kvstore.clone())
                .with_space("default".to_string())
                .with_space_id(1),
        );
        put_lookup_schema(&kvstore, "person", &[("age", "Int64")]).await;
        put_lookup_schema(&kvstore, "car", &[("age", "Int64")]).await;

        let index_manager = ctx.index_manager.as_ref().unwrap();
        let person_index = index_manager
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
        let car_index = index_manager
            .create_tag_index(
                1,
                "car_age_idx".to_string(),
                20,
                "car".to_string(),
                vec!["age".to_string()],
                vec![0],
            )
            .await
            .unwrap();

        for (vid, age) in [(1, 10), (2, 20)] {
            put_lookup_vertex(
                &kvstore,
                "person",
                vid,
                HashMap::from([("age".to_string(), byoridb_common::Value::Int(age))]),
            )
            .await;
            index_manager
                .insert_tag_index(1, person_index, &[IndexValue::Int(age)], vid)
                .await
                .unwrap();
        }
        // Stale index entries sort before the live rows. Candidate scanning
        // must not apply the result LIMIT before missing/mismatched vertices
        // are decoded and filtered, or both range and equality under-return.
        put_lookup_vertex(
            &kvstore,
            "person",
            0,
            HashMap::from([("age".to_string(), byoridb_common::Value::Int(-1))]),
        )
        .await;
        index_manager
            .insert_tag_index(1, person_index, &[IndexValue::Int(1)], 0)
            .await
            .unwrap();
        index_manager
            .insert_tag_index(1, person_index, &[IndexValue::Int(2)], -1)
            .await
            .unwrap();
        // The same live VID can be reachable through both an old stale key and
        // its current key. A broad range must return that vertex only once.
        index_manager
            .insert_tag_index(1, person_index, &[IndexValue::Int(5)], 1)
            .await
            .unwrap();
        index_manager
            .insert_tag_index(1, person_index, &[IndexValue::Int(10)], 0)
            .await
            .unwrap();
        for (vid, age) in [(101, 100), (102, 200)] {
            put_lookup_vertex(
                &kvstore,
                "car",
                vid,
                HashMap::from([("age".to_string(), byoridb_common::Value::Int(age))]),
            )
            .await;
            index_manager
                .insert_tag_index(1, car_index, &[IndexValue::Int(age)], vid)
                .await
                .unwrap();
        }

        let executor = Executor::new(ctx);
        assert_eq!(
            lookup_vids(&executor, "LOOKUP ON person WHERE person.age > 0 LIMIT 2",).await,
            vec![1, 2]
        );
        assert_eq!(
            lookup_vids(&executor, "LOOKUP ON car WHERE car.age > 0").await,
            vec![101, 102]
        );
        assert_eq!(
            lookup_vids(&executor, "LOOKUP ON person WHERE person.age == 10 LIMIT 1",).await,
            vec![1]
        );
        assert_eq!(
            lookup_vids(&executor, "LOOKUP ON car WHERE car.age == 100").await,
            vec![101]
        );
    }

    #[tokio::test]
    async fn lookup_limit_stops_chunked_vertex_decode_early() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kvstore.clone())
                .with_space("default".to_string())
                .with_space_id(1),
        );
        put_lookup_schema(
            &kvstore,
            "person",
            &[("age", "Int64"), ("payload", "String")],
        )
        .await;
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

        put_lookup_vertex(
            &kvstore,
            "person",
            1,
            HashMap::from([
                ("age".to_string(), byoridb_common::Value::Int(10)),
                (
                    "payload".to_string(),
                    byoridb_common::Value::String("x".repeat(256 * 1024)),
                ),
            ]),
        )
        .await;
        index_manager
            .insert_tag_index(1, index_id, &[IndexValue::Int(10)], 1)
            .await
            .unwrap();

        // More than one fetch chunk of stale candidates follows the first
        // live row. The malformed row at the end proves LIMIT 1 neither fetches
        // nor decodes later chunks after the requested row has been collected.
        for vid in 2..=300 {
            index_manager
                .insert_tag_index(1, index_id, &[IndexValue::Int(10)], vid)
                .await
                .unwrap();
        }
        kvstore
            .put(b"default:vertex:400", b"not-json")
            .await
            .unwrap();
        index_manager
            .insert_tag_index(1, index_id, &[IndexValue::Int(10)], 400)
            .await
            .unwrap();

        let executor = Executor::new(ctx);
        assert_eq!(
            lookup_vids(&executor, "LOOKUP ON person WHERE person.age == 10 LIMIT 1",).await,
            vec![1]
        );
        assert_eq!(
            lookup_vids(&executor, "LOOKUP ON person WHERE person.age > 0 LIMIT 1",).await,
            vec![1]
        );
    }

    #[tokio::test]
    async fn lookup_window_and_bool_order_match_index_and_scan_paths() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kvstore.clone())
                .with_space("default".to_string())
                .with_space_id(1)
                .with_config(ExecutionConfig {
                    max_scan_limit: 2,
                    ..ExecutionConfig::default()
                }),
        );
        for tag in ["indexed_item", "scanned_item"] {
            put_lookup_schema(&kvstore, tag, &[("score", "Int64"), ("enabled", "Bool")]).await;
        }

        let index_manager = ctx.index_manager.as_ref().unwrap();
        let score_index = index_manager
            .create_tag_index(
                1,
                "indexed_item_score_idx".to_string(),
                10,
                "indexed_item".to_string(),
                vec!["score".to_string()],
                vec![0],
            )
            .await
            .unwrap();
        let enabled_index = index_manager
            .create_tag_index(
                1,
                "indexed_item_enabled_idx".to_string(),
                10,
                "indexed_item".to_string(),
                vec!["enabled".to_string()],
                vec![1],
            )
            .await
            .unwrap();

        for (tag, base_vid) in [("indexed_item", 0), ("scanned_item", 100)] {
            for score in 1..=4i64 {
                let vid = base_vid + score;
                let enabled = score >= 3;
                put_lookup_vertex(
                    &kvstore,
                    tag,
                    vid,
                    HashMap::from([
                        ("score".to_string(), byoridb_common::Value::Int(score)),
                        ("enabled".to_string(), byoridb_common::Value::Bool(enabled)),
                    ]),
                )
                .await;
                if tag == "indexed_item" {
                    index_manager
                        .insert_tag_index(1, score_index, &[IndexValue::Int(score)], vid)
                        .await
                        .unwrap();
                    index_manager
                        .insert_tag_index(1, enabled_index, &[IndexValue::Bool(enabled)], vid)
                        .await
                        .unwrap();
                }
            }
        }

        let executor = Executor::new(ctx);
        assert_eq!(
            lookup_vids(
                &executor,
                "LOOKUP ON indexed_item WHERE indexed_item.score > 0 OFFSET 1",
            )
            .await,
            vec![2, 3]
        );
        // Unrelated vertices sort before this tag's rows. LIMIT must count
        // predicate matches, not raw prefix entries.
        assert_eq!(
            lookup_vids(
                &executor,
                "LOOKUP ON scanned_item WHERE scanned_item.score > 0 LIMIT 2",
            )
            .await,
            vec![101, 102]
        );
        assert_eq!(
            lookup_vids(
                &executor,
                "LOOKUP ON scanned_item WHERE scanned_item.score > 0 OFFSET 1",
            )
            .await,
            vec![102, 103]
        );
        assert_eq!(
            lookup_vids(
                &executor,
                "LOOKUP ON indexed_item WHERE indexed_item.enabled > false",
            )
            .await,
            vec![3, 4]
        );
        assert_eq!(
            lookup_vids(
                &executor,
                "LOOKUP ON scanned_item WHERE scanned_item.enabled > false",
            )
            .await,
            vec![103, 104]
        );
    }

    #[tokio::test]
    async fn lookup_range_collects_every_local_partition_before_windowing() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let mut context = ExecutionContext::new(kvstore.clone())
            .with_space("default".to_string())
            .with_space_id(1);
        context.partition_num = Some(2);
        let ctx = Arc::new(context);
        put_lookup_schema(&kvstore, "person", &[("age", "Int64")]).await;
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

        for (part_id, vid, age) in [(1, 1, 10), (2, 2, 20)] {
            put_lookup_vertex(
                &kvstore,
                "person",
                vid,
                HashMap::from([("age".to_string(), byoridb_common::Value::Int(age))]),
            )
            .await;
            index_manager
                .insert_tag_index(part_id, index_id, &[IndexValue::Int(age)], vid)
                .await
                .unwrap();
        }

        let executor = Executor::new(ctx);
        assert_eq!(
            lookup_vids(&executor, "LOOKUP ON person WHERE person.age > 0").await,
            vec![1, 2]
        );
    }

    #[tokio::test]
    async fn test_lookup_explicit_limit_overrides_default_scan_limit() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kvstore.clone())
                .with_space("default".to_string())
                .with_config(ExecutionConfig {
                    max_scan_limit: 2,
                    ..ExecutionConfig::default()
                }),
        );
        let executor = Executor::new(ctx);

        for vid in 1..=3 {
            let vertex = CodecVertexData {
                vid,
                tags: vec![CodecTagData {
                    name: "player".to_string(),
                    properties: HashMap::new(),
                }],
            };
            let key = format!("default:vertex:{}", vid);
            kvstore
                .put(
                    key.as_bytes(),
                    &VertexCodec::encode_vertex(&vertex).unwrap(),
                )
                .await
                .unwrap();
        }

        let capped = executor
            .execute_lookup(LookupPlan {
                lookup_type: LookupType::Tag("player".to_string()),
                where_clause: None,
                yield_clause: YieldClause { columns: vec![] },
                limit: None,
                offset: None,
            })
            .await
            .unwrap();
        assert_eq!(capped.rows.len(), 2);

        let explicit = executor
            .execute_lookup(LookupPlan {
                lookup_type: LookupType::Tag("player".to_string()),
                where_clause: None,
                yield_clause: YieldClause { columns: vec![] },
                limit: Some(3),
                offset: None,
            })
            .await
            .unwrap();
        assert_eq!(explicit.rows.len(), 3);
    }

    #[tokio::test]
    async fn match_projection_respects_max_memory_mb() {
        // A 1MB result-memory cap: projecting a wide column over enough rows
        // must fail with ResourceExhausted instead of OOMing (the systematic
        // OOM guard — PLAN.md G-11 max_memory_mb).
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kvstore)
                .with_space("default".to_string())
                .with_config(ExecutionConfig {
                    max_memory_mb: 1, // 1 MB
                    ..ExecutionConfig::default()
                }),
        );
        let executor = Executor::new(ctx);
        let run = |q: String| {
            let stmt = byoridb_parser::parse(&q).expect("parse");
            ExecutionPlanBuilder::build(stmt).expect("plan")
        };

        executor
            .execute(run("CREATE TAG t(big string)".to_string()))
            .await
            .unwrap();
        // ~30KB string × 60 rows ⟹ projecting n.big ≈ 1.8MB > the 1MB cap.
        let big = "x".repeat(30_000);
        for i in 1..=60 {
            executor
                .execute(run(format!("INSERT VERTEX t(big) VALUES {i}:(\"{big}\")")))
                .await
                .unwrap();
        }

        let err = executor
            .execute(run("MATCH (n:t) RETURN n.big".to_string()))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ExecutionError::ResourceExhausted(_)),
            "expected ResourceExhausted, got {err:?}"
        );

        // A small projection under the cap still succeeds.
        let ok = executor
            .execute(run("MATCH (n:t) RETURN id(n) LIMIT 5".to_string()))
            .await
            .unwrap();
        assert_eq!(ok.rows.len(), 5);
    }

    #[tokio::test]
    async fn test_match_count_uses_unlimited_count_path() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kvstore.clone())
                .with_space("default".to_string())
                .with_config(ExecutionConfig {
                    max_scan_limit: 1,
                    ..ExecutionConfig::default()
                }),
        );
        let executor = Executor::new(ctx);

        for vid in 1..=3 {
            let vertex = CodecVertexData {
                vid,
                tags: vec![CodecTagData {
                    name: "post".to_string(),
                    properties: HashMap::new(),
                }],
            };
            let key = format!("default:vertex:{}", vid);
            kvstore
                .put(
                    key.as_bytes(),
                    &VertexCodec::encode_vertex(&vertex).unwrap(),
                )
                .await
                .unwrap();
            let tagvid = format!("default:tagvid:post:{}", vid);
            kvstore.put(tagvid.as_bytes(), &[]).await.unwrap();
        }

        let stmt = byoridb_parser::parse("MATCH (n:post) RETURN count(n)").unwrap();
        let plan = ExecutionPlanBuilder::build(stmt).unwrap();
        let result = executor.execute(plan).await.unwrap();
        assert_eq!(result.rows, vec![vec![byoridb_common::Value::Int(3)]]);
    }

    // ===== INDEX operations without MetaClient =====

    #[tokio::test]
    async fn test_create_tag_index_without_meta_client_uses_local_index_manager() {
        let executor = create_executor();
        let plan = CreatePlan::TagIndex {
            name: "person_name_idx".to_string(),
            tag_name: "person".to_string(),
            props: vec!["name".to_string()],
        };

        executor.execute_create(plan).await.unwrap();
        let indexes = executor
            .ctx
            .index_manager
            .as_ref()
            .unwrap()
            .list_tag_indexes(1)
            .await;
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].index_name, "person_name_idx");
    }

    #[tokio::test]
    async fn test_create_edge_index_without_meta_client_uses_local_index_manager() {
        let executor = create_executor();
        let plan = CreatePlan::EdgeIndex {
            name: "knows_since_idx".to_string(),
            edge_name: "knows".to_string(),
            props: vec!["since".to_string()],
        };

        executor.execute_create(plan).await.unwrap();
        let indexes = executor
            .ctx
            .index_manager
            .as_ref()
            .unwrap()
            .list_edge_indexes(1)
            .await;
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].index_name, "knows_since_idx");
    }

    #[tokio::test]
    async fn test_drop_tag_index_without_meta_client_if_exists_is_ok() {
        let executor = create_executor();
        let plan = DropPlan::TagIndex {
            name: "any_idx".to_string(),
            if_exists: true,
        };

        executor.execute_drop(plan).await.unwrap();
    }

    #[tokio::test]
    async fn test_drop_edge_index_without_meta_client_missing_errors() {
        let executor = create_executor();
        let plan = DropPlan::EdgeIndex {
            name: "any_idx".to_string(),
            if_exists: false,
        };

        let err = executor.execute_drop(plan).await.unwrap_err();
        match err {
            ExecutionError::InvalidOperation(msg) => {
                assert!(msg.contains("not found"), "msg was: {}", msg);
            }
            e => panic!("Expected InvalidOperation, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_show_tag_indexes_without_meta_client_returns_empty() {
        let executor = create_executor();
        // With no meta_client and no index_manager, returns empty results (not error)
        let result = executor.execute_show(ShowPlan::TagIndexes).await.unwrap();
        assert_eq!(result.columns, vec!["Index Name", "On Tag", "Fields"]);
    }

    #[tokio::test]
    async fn test_show_edge_indexes_without_meta_client_returns_empty() {
        let executor = create_executor();
        let result = executor.execute_show(ShowPlan::EdgeIndexes).await.unwrap();
        assert_eq!(result.columns, vec!["Index Name", "On Edge", "Fields"]);
    }

    /// Without a configured meta client, SHOW HOSTS must return the correct
    /// column set with zero rows (no hardcoded localhost:9779 placeholder).
    #[tokio::test]
    async fn test_show_hosts_without_meta_client_returns_empty() {
        let executor = create_executor();

        let result = executor.execute_show(ShowPlan::Hosts).await.unwrap();

        assert_eq!(
            result.columns,
            vec![
                "Host".to_string(),
                "Port".to_string(),
                "Status".to_string(),
                "Leader Count".to_string(),
                "Part Count".to_string(),
            ]
        );
        assert!(
            result.rows.is_empty(),
            "expected empty rows when meta client is missing, got {:?}",
            result.rows
        );
    }

    /// Without a configured meta client, SHOW PARTS must return the correct
    /// column set with zero rows instead of the legacy placeholder row.
    #[tokio::test]
    async fn test_show_parts_without_meta_client_returns_empty() {
        let executor = create_executor();

        let result = executor.execute_show(ShowPlan::Parts).await.unwrap();

        assert_eq!(
            result.columns,
            vec![
                "Part ID".to_string(),
                "Leader".to_string(),
                "Hosts".to_string(),
            ]
        );
        assert!(
            result.rows.is_empty(),
            "expected empty rows when meta client is missing, got {:?}",
            result.rows
        );
    }

    // ===== resolve_local_partition_num — Item 16 of MOCK_REMEDIATION_PLAN =====

    /// When `ExecutionContext::partition_num` is populated (e.g. via
    /// `with_distributed_mode` or a test harness), LOOKUP must respect that
    /// value and iterate every partition instead of the old `part_id = 1`
    /// shortcut.
    #[tokio::test]
    async fn test_resolve_local_partition_num_uses_ctx_value() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let mut ctx = ExecutionContext::new(kvstore).with_space("default".to_string());
        ctx.partition_num = Some(7);
        let executor = Executor::new(Arc::new(ctx));

        let resolved = executor.resolve_local_partition_num("default").await;
        assert_eq!(resolved, 7);
    }

    /// With no meta client and no ctx.partition_num, resolve_local_partition_num
    /// must fall back to 1 (with a warn logged) rather than pretending to know.
    /// The caller then scans only part_id=1, documented in Item 16.
    #[tokio::test]
    async fn test_resolve_local_partition_num_falls_back_to_one() {
        let executor = create_executor();

        let resolved = executor.resolve_local_partition_num("default").await;
        assert_eq!(resolved, 1);
    }

    /// A stored partition count of 0 would cause the caller's 1..=n loop to
    /// evaluate to an empty range, missing every partition. Clamp to at
    /// least 1 so the loop always touches part 1.
    #[tokio::test]
    async fn test_resolve_local_partition_num_clamps_zero_to_one() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let mut ctx = ExecutionContext::new(kvstore).with_space("default".to_string());
        ctx.partition_num = Some(0);
        let executor = Executor::new(Arc::new(ctx));

        let resolved = executor.resolve_local_partition_num("default").await;
        assert_eq!(resolved, 1, "partition_num must never resolve to 0");
    }

    // ===== H-series bug regression tests =====

    #[tokio::test]
    async fn test_h1_show_spaces_returns_distinct_ids() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kvstore.clone()));
        let executor = Executor::new(ctx);

        // Create two spaces
        let r1 = executor
            .execute(ExecutionPlan::Create(CreatePlan::Space {
                name: "space_alpha".to_string(),
                if_not_exists: false,
                partition_num: 1,
                replica_factor: 1,
                vid_type: "INT64".to_string(),
                partition_strategy: byoridb_common::PartitionStrategy::default(),
            }))
            .await
            .unwrap();
        assert!(r1.rows.is_empty());

        let r2 = executor
            .execute(ExecutionPlan::Create(CreatePlan::Space {
                name: "space_beta".to_string(),
                if_not_exists: false,
                partition_num: 1,
                replica_factor: 1,
                vid_type: "INT64".to_string(),
                partition_strategy: byoridb_common::PartitionStrategy::default(),
            }))
            .await
            .unwrap();
        assert!(r2.rows.is_empty());

        // SHOW SPACES
        let show = executor
            .execute(ExecutionPlan::Show(ShowPlan::Spaces))
            .await
            .unwrap();

        assert_eq!(show.columns[0], "ID");
        assert_eq!(show.rows.len(), 2, "expected 2 spaces");

        let ids: Vec<i64> = show
            .rows
            .iter()
            .map(|r| match &r[0] {
                byoridb_common::Value::Int(i) => *i,
                _ => -1,
            })
            .collect();

        // IDs must be distinct and non-zero
        assert!(
            ids.iter().all(|&id| id > 0),
            "all IDs must be > 0, got {:?}",
            ids
        );
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 2, "IDs must be distinct, got {:?}", ids);
    }

    #[tokio::test]
    async fn test_h1_space_id_persists_across_recreate() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kvstore.clone()));
        let executor = Executor::new(ctx);

        executor
            .execute(ExecutionPlan::Create(CreatePlan::Space {
                name: "persistent".to_string(),
                if_not_exists: false,
                partition_num: 1,
                replica_factor: 1,
                vid_type: "INT64".to_string(),
                partition_strategy: byoridb_common::PartitionStrategy::default(),
            }))
            .await
            .unwrap();

        let show = executor
            .execute(ExecutionPlan::Show(ShowPlan::Spaces))
            .await
            .unwrap();

        let id = match &show.rows[0][0] {
            byoridb_common::Value::Int(i) => *i,
            _ => panic!("expected int ID"),
        };
        assert!(id > 0, "space ID must be > 0");
    }

    #[tokio::test]
    async fn test_h4_lookup_proto_encoded_vertex() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kvstore.clone()).with_space("demo".to_string()));
        let executor = Executor::new(ctx);

        // Write schema
        let tag_key = SchemaKey::tag("demo", "person");
        let schema = serde_json::json!({
            "name": "person",
            "properties": [{"name": "name"}]
        });
        kvstore
            .put(&tag_key, serde_json::to_vec(&schema).unwrap().as_slice())
            .await
            .unwrap();

        // Insert a proto-encoded vertex
        let v_key = format!("demo:vertex:{}", 42i64).into_bytes();
        let v_data = VertexCodec::encode_vertex(&CodecVertexData {
            vid: 42,
            tags: vec![CodecTagData {
                name: "person".to_string(),
                properties: {
                    let mut m = std::collections::HashMap::new();
                    m.insert(
                        "name".to_string(),
                        byoridb_common::Value::String("Alice".to_string()),
                    );
                    m
                },
            }],
        })
        .unwrap();
        kvstore.put(&v_key, &v_data).await.unwrap();

        // LOOKUP should return vid=42, not 0
        let result = executor
            .execute(ExecutionPlan::Lookup(LookupPlan {
                lookup_type: LookupType::Tag("person".to_string()),
                where_clause: Some(byoridb_parser::ast::Expression::BinaryOp {
                    op: byoridb_parser::ast::BinaryOperator::Eq,
                    left: Box::new(byoridb_parser::ast::Expression::Identifier(
                        "name".to_string(),
                    )),
                    right: Box::new(byoridb_parser::ast::Expression::Literal(
                        byoridb_parser::ast::Literal::String("Alice".to_string()),
                    )),
                }),
                yield_clause: YieldClause { columns: vec![] },
                limit: None,
                offset: None,
            }))
            .await
            .unwrap();

        assert_eq!(result.rows.len(), 1, "expected 1 row");
        assert_eq!(
            result.rows[0][0],
            byoridb_common::Value::Int(42),
            "vid must be 42, not 0"
        );
    }

    #[tokio::test]
    async fn test_h5_fetch_edge_returns_edge_data() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kvstore.clone()).with_space("social".to_string()));
        let executor = Executor::new(ctx);

        // Write edge schema
        let edge_schema_key = SchemaKey::edge("social", "follows");
        let schema = serde_json::json!({"name": "follows", "properties": []});
        kvstore
            .put(
                &edge_schema_key,
                serde_json::to_vec(&schema).unwrap().as_slice(),
            )
            .await
            .unwrap();

        // Insert a proto-encoded edge 1->2
        let e_key = b"social:edge:1:follows:2:0".to_vec();
        let e_data = VertexCodec::encode_edge(&CodecEdgeData {
            src_vid: 1,
            dst_vid: 2,
            edge_type: "follows".to_string(),
            ranking: 0,
            properties: std::collections::HashMap::new(),
        })
        .unwrap();
        kvstore.put(&e_key, &e_data).await.unwrap();

        // FETCH PROP ON follows 1->2
        let result = executor
            .execute(ExecutionPlan::Fetch(FetchPlan {
                space: "social".to_string(),
                vids: vec![],
                tags: vec!["follows".to_string()],
                yield_clause: None,
                edge_refs: vec![(1.into(), 2.into())],
                is_edge_fetch: true,
                src_var: None,
                as_of: None,
            }))
            .await
            .unwrap();

        assert_eq!(result.rows.len(), 1, "expected 1 edge row");
        assert_eq!(result.rows[0][0], byoridb_common::Value::Int(1), "src=1");
        assert_eq!(result.rows[0][1], byoridb_common::Value::Int(2), "dst=2");
        // 3rd column is the properties JSON
        assert!(
            matches!(&result.rows[0][2], byoridb_common::Value::String(_)),
            "properties should be a JSON string"
        );
    }

    // ===== H-2 / H-3 regression tests (AKS 재배포 재검증) =====

    /// H-2: SHOW TAGS in space A must not include tags from space B.
    /// Root cause hypothesis: schemaKey prefix "space:{name}:tag:" is name-based,
    /// so two different spaces cannot produce a collision. Verify here.
    #[tokio::test]
    async fn test_h2_show_tags_no_cross_space_leak() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kvstore.clone()).with_space("demo".to_string()));
        let executor = Executor::new(ctx);

        // Write tag schema for `demo` space
        let demo_tag_key = crate::key::SchemaKey::tag("demo", "person");
        let demo_schema = serde_json::json!({"name": "person", "properties": []});
        kvstore
            .put(
                &demo_tag_key,
                serde_json::to_vec(&demo_schema).unwrap().as_slice(),
            )
            .await
            .unwrap();

        // Write tag schema for `social` space — must NOT appear in demo's SHOW TAGS
        let social_tag_key = crate::key::SchemaKey::tag("social", "person");
        kvstore
            .put(
                &social_tag_key,
                serde_json::to_vec(&demo_schema).unwrap().as_slice(),
            )
            .await
            .unwrap();

        // Also add a tag_index entry to verify it's filtered out
        let idx_key = b"space:demo:tag_index:person_name_idx".to_vec();
        kvstore
            .put(
                &idx_key,
                serde_json::to_vec(&serde_json::json!({}))
                    .unwrap()
                    .as_slice(),
            )
            .await
            .unwrap();

        let result = executor
            .execute(ExecutionPlan::Show(ShowPlan::Tags))
            .await
            .unwrap();

        assert_eq!(
            result.rows.len(),
            1,
            "SHOW TAGS must return exactly 1 row (no cross-space leak, no index entries), got {:?}",
            result.rows
        );
        assert_eq!(
            result.rows[0][0],
            byoridb_common::Value::String("person".to_string()),
            "tag name must be 'person'"
        );
    }

    /// H-2 (edges): Same check for SHOW EDGES.
    #[tokio::test]
    async fn test_h2_show_edges_no_cross_space_leak() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kvstore.clone()).with_space("demo".to_string()));
        let executor = Executor::new(ctx);

        // `demo` space has edge `follows`
        let demo_edge_key = crate::key::SchemaKey::edge("demo", "follows");
        let edge_schema = serde_json::json!({"name": "follows", "properties": []});
        kvstore
            .put(
                &demo_edge_key,
                serde_json::to_vec(&edge_schema).unwrap().as_slice(),
            )
            .await
            .unwrap();

        // `social` space also has `follows` — must NOT appear in demo's SHOW EDGES
        let social_edge_key = crate::key::SchemaKey::edge("social", "follows");
        kvstore
            .put(
                &social_edge_key,
                serde_json::to_vec(&edge_schema).unwrap().as_slice(),
            )
            .await
            .unwrap();

        let result = executor
            .execute(ExecutionPlan::Show(ShowPlan::Edges))
            .await
            .unwrap();

        assert_eq!(
            result.rows.len(),
            1,
            "SHOW EDGES must return exactly 1 row, got {:?}",
            result.rows
        );
    }

    /// H-3: GO FROM src OVER edge_type must return only correctly stored neighbors.
    /// Verifies that multi-digit src VIDs don't collide with single-digit prefix,
    /// and that proto-encoded edges decode with correct dst_vid (not 0).
    #[tokio::test]
    async fn test_h3_go_returns_correct_dst_no_zero() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kvstore.clone()).with_space("social".to_string()));
        let executor = Executor::new(ctx);

        // Insert edges: 1->2 (ranking=0) and 1->3 (ranking=1).
        // Edge identity is (src, edge_type, ranking) — different rankings required
        // for two distinct edges from the same source.
        for (dst, ranking) in [(2i64, 0i64), (3, 1)] {
            let key = format!("social:edge:1:follows:{}", ranking).into_bytes();
            let edge = CodecEdgeData {
                src_vid: 1,
                dst_vid: dst,
                edge_type: "follows".to_string(),
                ranking,
                properties: HashMap::new(),
            };
            let data = VertexCodec::encode_edge(&edge).unwrap();
            kvstore.put(&key, &data).await.unwrap();
        }

        // Also insert edge from src=10 to verify no prefix collision with src=1
        let key10 = b"social:edge:10:follows:0".to_vec();
        let edge10 = CodecEdgeData {
            src_vid: 10,
            dst_vid: 99,
            edge_type: "follows".to_string(),
            ranking: 0,
            properties: HashMap::new(),
        };
        kvstore
            .put(&key10, &VertexCodec::encode_edge(&edge10).unwrap())
            .await
            .unwrap();

        // GO FROM 1 OVER follows
        let result = executor
            .execute(ExecutionPlan::Go(GoPlan {
                from_clause: FromClause {
                    vids: vec![1.into()],
                    src: None,
                },
                over_edges: vec!["follows".to_string()],
                direction: byoridb_parser::ast::EdgeDirection::Outgoing,
                to_clause: ToClause {
                    variable: String::new(),
                    steps: StepClause::Exactly(1),
                },
                where_clause: None,
                yield_clause: YieldClause { columns: vec![] },
            }))
            .await
            .unwrap();

        let dsts: Vec<i64> = result
            .rows
            .iter()
            .map(|r| match &r[1] {
                byoridb_common::Value::Int(i) => *i,
                _ => -1,
            })
            .collect();

        // Must contain exactly dst=2 and dst=3
        assert!(
            dsts.contains(&2),
            "GO result must include dst=2, got {:?}",
            dsts
        );
        assert!(
            dsts.contains(&3),
            "GO result must include dst=3, got {:?}",
            dsts
        );
        // Must NOT contain dst=0 (proto default leak) or dst=99 (src=10's edge)
        assert!(
            !dsts.contains(&0),
            "GO result must NOT contain dst=0, got {:?}",
            dsts
        );
        assert!(
            !dsts.contains(&99),
            "GO result must NOT contain dst=99 (src=10 edge), got {:?}",
            dsts
        );
        assert_eq!(
            dsts.len(),
            2,
            "Exactly 2 edges expected (1->2, 1->3), got {:?}",
            dsts
        );
    }

    // ===== 배치 INSERT EDGE 회귀 테스트 =====

    /// 같은 src에서 다른 dst로의 배치 INSERT 시 마지막 것만 남는 버그 회귀 테스트.
    /// 키에 dst를 포함하지 않으면 1→2, 1→3, 1→10이 모두 같은 키를 써서 마지막만 남음.
    #[tokio::test]
    async fn test_batch_insert_edge_different_dst_all_survive() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx =
            Arc::new(ExecutionContext::new(kvstore.clone()).with_space("default".to_string()));
        let executor = Executor::new(ctx);

        // Create edge schema
        let edge_key = SchemaKey::edge("default", "follows");
        let schema = serde_json::json!({"name": "follows", "properties": [
            {"name": "since", "data_type": "Int64", "nullable": true}
        ]});
        kvstore
            .put(&edge_key, serde_json::to_vec(&schema).unwrap().as_slice())
            .await
            .unwrap();

        // Batch INSERT EDGE follows 1->2, 1->3, 1->10 (all ranking=0)
        let plan = crate::plan::InsertPlan::Edge {
            space: "default".to_string(),
            edges: vec![
                crate::plan::EdgeInsert {
                    src: 1.into(),
                    dst: 2.into(),
                    edge_type: "follows".to_string(),
                    ranking: 0,
                    props: std::collections::HashMap::from([(
                        "since".to_string(),
                        byoridb_common::Value::Int(2021),
                    )]),
                },
                crate::plan::EdgeInsert {
                    src: 1.into(),
                    dst: 3.into(),
                    edge_type: "follows".to_string(),
                    ranking: 0,
                    props: std::collections::HashMap::from([(
                        "since".to_string(),
                        byoridb_common::Value::Int(2022),
                    )]),
                },
                crate::plan::EdgeInsert {
                    src: 1.into(),
                    dst: 10.into(),
                    edge_type: "follows".to_string(),
                    ranking: 0,
                    props: std::collections::HashMap::from([(
                        "since".to_string(),
                        byoridb_common::Value::Int(2023),
                    )]),
                },
            ],
        };
        executor.execute_insert(plan).await.unwrap();

        // All 3 edges must be present in kvstore
        let key_2 = "default:edge:1:follows:2:0";
        let key_3 = "default:edge:1:follows:3:0";
        let key_10 = "default:edge:1:follows:10:0";

        assert!(
            kvstore.get(key_2.as_bytes()).await.unwrap().is_some(),
            "edge 1->2 must exist"
        );
        assert!(
            kvstore.get(key_3.as_bytes()).await.unwrap().is_some(),
            "edge 1->3 must exist"
        );
        assert!(
            kvstore.get(key_10.as_bytes()).await.unwrap().is_some(),
            "edge 1->10 must exist"
        );
    }

    // ===== LOOKUP 태그 필터 회귀 테스트 =====

    /// LOOKUP ON <tag>가 다른 태그를 가진 버텍스를 반환하는 버그 회귀 테스트.
    /// filter_fn에 태그 이름 필터가 없으면 전체 버텍스를 반환함.
    #[tokio::test]
    async fn test_lookup_on_tag_filters_by_tag_name() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx =
            Arc::new(ExecutionContext::new(kvstore.clone()).with_space("default".to_string()));
        let executor = Executor::new(ctx);

        // Insert two vertices: one `person`, one `company`
        let person_key = SchemaKey::vertex("default", 100);
        let person_data = VertexCodec::encode_vertex(&CodecVertexData {
            vid: 100,
            tags: vec![CodecTagData {
                name: "person".to_string(),
                properties: std::collections::HashMap::from([(
                    "name".to_string(),
                    byoridb_common::Value::String("Alice".to_string()),
                )]),
            }],
        })
        .unwrap();
        kvstore.put(&person_key, &person_data).await.unwrap();

        let company_key = SchemaKey::vertex("default", 200);
        let company_data = VertexCodec::encode_vertex(&CodecVertexData {
            vid: 200,
            tags: vec![CodecTagData {
                name: "company".to_string(),
                properties: std::collections::HashMap::from([(
                    "name".to_string(),
                    byoridb_common::Value::String("Acme".to_string()),
                )]),
            }],
        })
        .unwrap();
        kvstore.put(&company_key, &company_data).await.unwrap();

        // LOOKUP ON person — must return only vid=100, not vid=200
        let plan = LookupPlan {
            lookup_type: LookupType::Tag("person".to_string()),
            where_clause: None,
            yield_clause: YieldClause { columns: vec![] },
            limit: None,
            offset: None,
        };
        let result = executor.execute_lookup(plan).await.unwrap();

        let vids: Vec<i64> = result
            .rows
            .iter()
            .filter_map(|r| {
                if let byoridb_common::Value::Int(v) = r[0] {
                    Some(v)
                } else {
                    None
                }
            })
            .collect();

        assert!(
            vids.contains(&100),
            "person vertex (vid=100) must be returned"
        );
        assert!(
            !vids.contains(&200),
            "company vertex (vid=200) must NOT be returned by LOOKUP ON person"
        );
    }
}
