// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, ExecutionError>;

#[derive(Error, Debug)]
pub enum ExecutionError {
    #[error("Parse error: {0}")]
    Parse(#[from] byoridb_parser::ParseError),

    #[error("Storage error: {0}")]
    Storage(#[from] byoridb_storage::StorageError),

    #[error("Space not found: {0}")]
    SpaceNotFound(String),

    #[error("Tag not found: {0}")]
    TagNotFound(String),

    #[error("Edge not found: {0}")]
    EdgeNotFound(String),

    #[error("Index not found: {0}")]
    IndexNotFound(String),

    #[error("Vertex not found: {0}")]
    VertexNotFound(String),

    #[error("Type mismatch: {0}")]
    TypeMismatch(String),

    #[error("Constraint violation: {0}")]
    ConstraintViolation(String),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Resource exhausted: {0}")]
    ResourceExhausted(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[source] Box<serde_json::Error>),
}

impl From<serde_json::Error> for ExecutionError {
    fn from(err: serde_json::Error) -> Self {
        ExecutionError::Serialization(Box::new(err))
    }
}

impl From<byoridb_kvstore::KVStoreError> for ExecutionError {
    fn from(err: byoridb_kvstore::KVStoreError) -> Self {
        // Preserve error chain: KVStoreError -> StorageError -> ExecutionError
        ExecutionError::Storage(byoridb_storage::StorageError::from(err))
    }
}
