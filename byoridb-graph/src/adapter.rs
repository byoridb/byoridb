// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Adapter module to bridge byoridb-graph and byoridb-executor types

use byoridb_common::DataSet;
use byoridb_executor::{ExecutionContext as ByoriExecutionContext, ExecutorResult};
use byoridb_kvstore::KVStore;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::error::GraphError;

/// Convert ExecutorResult from byoridb-executor to DataSet
pub fn executor_result_to_dataset(result: ExecutorResult) -> DataSet {
    DataSet::with_rows(result.columns, result.rows)
}

/// Create byoridb-executor's ExecutionContext from space name and kvstore
pub fn create_executor_context(
    space_name: Option<String>,
    kvstore: Arc<dyn KVStore>,
) -> ByoriExecutionContext {
    let mut ctx = ByoriExecutionContext::new(kvstore);
    if let Some(space) = space_name {
        ctx = ctx.with_space(space);
    }
    ctx
}

/// Create byoridb-executor's ExecutionContext with caller roles for RBAC.
///
/// `full_scan_flag` is a shared flag the executor sets if the query falls back
/// to an un-indexed full scan; the Graph service reads it afterwards to enrich
/// the slow-query log.
pub fn create_executor_context_with_roles(
    space_name: Option<String>,
    kvstore: Arc<dyn KVStore>,
    caller_roles: Vec<String>,
    full_scan_flag: Arc<AtomicBool>,
) -> ByoriExecutionContext {
    let mut ctx = ByoriExecutionContext::new(kvstore)
        .with_caller_roles(caller_roles)
        .with_full_scan_flag(full_scan_flag);
    if let Some(space) = space_name {
        ctx = ctx.with_space(space);
    }
    ctx
}

/// Convert byoridb-executor's ExecutionError to GraphError
pub fn executor_error_to_graph_error(err: byoridb_executor::ExecutionError) -> GraphError {
    use byoridb_executor::ExecutionError;

    match err {
        ExecutionError::SpaceNotFound(s) => {
            GraphError::ExecutionError(format!("Space not found: {}", s))
        }
        ExecutionError::TagNotFound(s) => {
            GraphError::ExecutionError(format!("Tag not found: {}", s))
        }
        ExecutionError::EdgeNotFound(s) => {
            GraphError::ExecutionError(format!("Edge not found: {}", s))
        }
        ExecutionError::IndexNotFound(s) => {
            GraphError::ExecutionError(format!("Index not found: {}", s))
        }
        ExecutionError::VertexNotFound(s) => {
            GraphError::ExecutionError(format!("Vertex not found: {}", s))
        }
        ExecutionError::InvalidOperation(s) => GraphError::InvalidOperation(s),
        ExecutionError::Storage(e) => GraphError::Storage(e.to_string()),
        ExecutionError::Parse(e) => GraphError::ParseError(e.to_string()),
        ExecutionError::TypeMismatch(s) => {
            GraphError::ExecutionError(format!("Type mismatch: {}", s))
        }
        ExecutionError::ConstraintViolation(s) => {
            GraphError::ExecutionError(format!("Constraint violation: {}", s))
        }
        _ => GraphError::ExecutionError(err.to_string()),
    }
}
