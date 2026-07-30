use crate::error::Result;
use crate::session::SessionManager;
use async_trait::async_trait;
use byoridb_common::DataSet;
use byoridb_kvstore::KVStore;
use byoridb_parser::ast::{Statement, UseStatement};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[async_trait]
pub trait Executor: Send + Sync {
    async fn execute(&self) -> Result<DataSet>;

    /// Shared flag the underlying executor sets when the query falls back to an
    /// un-indexed full scan. `None` for executors that never scan (USE/NoOp).
    /// The caller reads it after `execute()` to enrich the slow-query log.
    fn full_scan_flag(&self) -> Option<Arc<AtomicBool>> {
        None
    }
}

// Executor for USE statement
pub struct UseExecutor {
    pub stmt: UseStatement,
    pub session_manager: Arc<SessionManager>,
    pub session_id: i64,
}

#[async_trait]
impl Executor for UseExecutor {
    async fn execute(&self) -> Result<DataSet> {
        self.session_manager
            .set_space(self.session_id, self.stmt.space.clone())
            .await
            .map_err(crate::error::GraphError::ExecutionError)?;

        Ok(DataSet::new(vec!["Space switched".to_string()]))
    }
}

pub struct NoOpExecutor;

#[async_trait]
impl Executor for NoOpExecutor {
    async fn execute(&self) -> Result<DataSet> {
        Ok(DataSet::new(vec!["NoOp".to_string()]))
    }
}

/// Adapter to bridge byoridb-executor with byoridb-graph's Executor trait
pub struct ByoriDBExecutorAdapter {
    stmt: Statement,
    space_name: Option<String>,
    kvstore: Arc<dyn KVStore>,
    caller_roles: Vec<String>,
    session_manager: Option<Arc<crate::session::SessionManager>>,
    /// Shared with the executor context so the caller can detect a full-scan
    /// fallback after execution (for the slow-query log).
    full_scan: Arc<AtomicBool>,
}

impl ByoriDBExecutorAdapter {
    pub fn new(stmt: Statement, space_name: Option<String>, kvstore: Arc<dyn KVStore>) -> Self {
        Self {
            stmt,
            space_name,
            kvstore,
            caller_roles: vec![],
            session_manager: None,
            full_scan: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_caller_roles(mut self, roles: Vec<String>) -> Self {
        self.caller_roles = roles;
        self
    }

    pub fn with_session_manager(
        mut self,
        session_manager: Arc<crate::session::SessionManager>,
    ) -> Self {
        self.session_manager = Some(session_manager);
        self
    }
}

#[async_trait]
impl Executor for ByoriDBExecutorAdapter {
    async fn execute(&self) -> Result<DataSet> {
        // SHOW SESSIONS — handled here because session data lives in the graph layer
        if matches!(
            &self.stmt,
            Statement::Show(byoridb_parser::ast::ShowStatement::Sessions)
        ) {
            if !self
                .caller_roles
                .iter()
                .any(|role| role == "GOD" || role == "ADMIN")
            {
                return Err(crate::error::GraphError::AuthFailed(
                    "SHOW SESSIONS requires GOD or ADMIN role".to_string(),
                ));
            }
            let rows = if let Some(ref sm) = self.session_manager {
                sm.list_sessions()
                    .into_iter()
                    .map(|(_sid, user, space)| {
                        vec![
                            byoridb_common::Value::String(user),
                            byoridb_common::Value::String(space.unwrap_or_else(|| "-".to_string())),
                        ]
                    })
                    .collect()
            } else {
                vec![]
            };
            let dataset = byoridb_common::DataSet::with_rows(
                vec!["User".to_string(), "Space".to_string()],
                rows,
            );
            return Ok(dataset);
        }

        // Build execution plan from statement
        let plan = byoridb_executor::ExecutionPlanBuilder::build(self.stmt.clone())
            .map_err(crate::adapter::executor_error_to_graph_error)?;

        // Create executor context with caller roles for RBAC. The shared
        // full-scan flag lets the caller observe a full-scan fallback.
        let ctx = crate::adapter::create_executor_context_with_roles(
            self.space_name.clone(),
            self.kvstore.clone(),
            self.caller_roles.clone(),
            self.full_scan.clone(),
        );

        // Execute the plan
        let executor = byoridb_executor::Executor::new(Arc::new(ctx));
        let result = executor
            .execute(plan)
            .await
            .map_err(crate::adapter::executor_error_to_graph_error)?;

        // Convert result to DataSet
        Ok(crate::adapter::executor_result_to_dataset(result))
    }

    fn full_scan_flag(&self) -> Option<Arc<AtomicBool>> {
        Some(self.full_scan.clone())
    }
}
