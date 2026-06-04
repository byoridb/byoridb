// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, CodecError>;

#[derive(Error, Debug)]
pub enum CodecError {
    #[error("Unknown field: {0}")]
    UnknownField(String),

    #[error("Type mismatch: expected {expected}, found {found}")]
    TypeMismatch { expected: String, found: String },

    #[error("Out of range")]
    OutOfRange,

    #[error("Field not nullable: {0}")]
    NotNullable(String),

    #[error("Field unset: {0}")]
    FieldUnset(String),

    #[error("Incorrect value: {0}")]
    IncorrectValue(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Invalid schema version: {0}")]
    InvalidSchemaVersion(i32),

    #[error("Invalid encoding version: {0}")]
    InvalidEncodingVersion(u8),
}

impl From<bincode::Error> for CodecError {
    fn from(err: bincode::Error) -> Self {
        CodecError::Serialization(err.to_string())
    }
}

impl From<byoridb_common::Error> for CodecError {
    fn from(err: byoridb_common::Error) -> Self {
        CodecError::Serialization(err.to_string())
    }
}
