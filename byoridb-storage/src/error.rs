// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, StorageError>;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Key not found")]
    KeyNotFound,

    #[error("Space not found: {0}")]
    SpaceNotFound(u32),

    #[error("Partition not found: space={0}, part={1}")]
    PartitionNotFound(u32, u32),

    #[error("KVStore error: {0}")]
    KVStore(#[from] byoridb_kvstore::KVStoreError),

    #[error("Codec error: {0}")]
    Codec(#[from] byoridb_codec::CodecError),

    #[error("Invalid schema: {0}")]
    InvalidSchema(String),

    #[error("Index error: {0}")]
    Index(String),

    #[error("Transaction error: {0}")]
    Transaction(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Encoding error: {0}")]
    EncodingError(String),

    #[error("Decoding error: {0}")]
    DecodingError(String),

    #[error("Store error: {0}")]
    StoreError(String),
}
