use crate::context::ExecutionContext;
use crate::error::Result;
use crate::executor::{ByoriDBExecutorAdapter, Executor, UseExecutor};
use crate::session::SessionManager;
use byoridb_kvstore::KVStore;
use byoridb_parser::ast::Statement;
use std::sync::Arc;

pub struct Planner {
    session_manager: Arc<SessionManager>,
    kvstore: Arc<dyn KVStore>,
}

impl Planner {
    pub fn new(session_manager: Arc<SessionManager>, kvstore: Arc<dyn KVStore>) -> Self {
        Self {
            session_manager,
            kvstore,
        }
    }

    pub fn plan(&self, stmt: Statement, context: ExecutionContext) -> Result<Box<dyn Executor>> {
        match &stmt {
            // USE statement needs session management, keep using local executor
            Statement::Use(use_stmt) => Ok(Box::new(UseExecutor {
                stmt: use_stmt.clone(),
                session_manager: self.session_manager.clone(),
                session_id: context.session_id,
            })),
            // All other statements route to byoridb-executor via adapter
            _ => Ok(Box::new(
                ByoriDBExecutorAdapter::new(stmt, context.space_name, self.kvstore.clone())
                    .with_caller_roles(context.caller_roles)
                    .with_session_manager(self.session_manager.clone()),
            )),
        }
    }
}
