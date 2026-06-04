// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, MetaError>;

#[derive(Error, Debug)]
pub enum MetaError {
    #[error("Space not found: {0}")]
    SpaceNotFound(String),

    #[error("Space already exists: {0}")]
    SpaceAlreadyExists(String),

    #[error("Tag not found: {0}")]
    TagNotFound(String),

    #[error("Tag already exists: {0}")]
    TagAlreadyExists(String),

    #[error("Edge not found: {0}")]
    EdgeNotFound(String),

    #[error("Edge already exists: {0}")]
    EdgeAlreadyExists(String),

    #[error("Index not found: {0}")]
    IndexNotFound(String),

    #[error("Index already exists: {0}")]
    IndexAlreadyExists(String),

    #[error("Field not found: {0}")]
    FieldNotFound(String),

    #[error("Field already exists: {0}")]
    FieldAlreadyExists(String),

    #[error("Invalid ALTER operation: {0}")]
    InvalidAlterOperation(String),

    #[error("User not found: {0}")]
    UserNotFound(String),

    #[error("User already exists: {0}")]
    UserAlreadyExists(String),

    #[error("Host not found: {0}:{1}")]
    HostNotFound(String, u32),

    #[error("Invalid partition: {0}")]
    InvalidPartition(String),

    #[error("Invalid partition strategy: {0}")]
    InvalidPartitionStrategy(String),

    #[error("Partition not found: space {0}, part {1}")]
    PartitionNotFound(u32, u32),

    #[error("KVStore error: {0}")]
    KVStore(String),

    #[error("Codec error: {0}")]
    Codec(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
}

impl From<byoridb_kvstore::KVStoreError> for MetaError {
    fn from(err: byoridb_kvstore::KVStoreError) -> Self {
        MetaError::KVStore(err.to_string())
    }
}

impl From<byoridb_codec::CodecError> for MetaError {
    fn from(err: byoridb_codec::CodecError) -> Self {
        MetaError::Codec(err.to_string())
    }
}

impl From<serde_json::Error> for MetaError {
    fn from(err: serde_json::Error) -> Self {
        MetaError::Serialization(err.to_string())
    }
}
