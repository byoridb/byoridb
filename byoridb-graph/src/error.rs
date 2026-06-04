// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, GraphError>;

#[derive(Error, Debug)]
pub enum GraphError {
    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Session not found: {0}")]
    SessionNotFound(i64),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Planning error: {0}")]
    PlanningError(String),

    #[error("Execution error: {0}")]
    ExecutionError(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Bad syntax: {0}")]
    BadSyntax(String),

    #[error("Semantic error: {0}")]
    SemanticError(String),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

impl From<byoridb_storage::StorageError> for GraphError {
    fn from(err: byoridb_storage::StorageError) -> Self {
        GraphError::Storage(err.to_string())
    }
}
