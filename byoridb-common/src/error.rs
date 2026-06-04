// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("Null value")]
    NullValue,

    #[error("Bad data: {0}")]
    BadData(String),

    #[error("Bad type: expected {expected}, found {found}")]
    BadType { expected: String, found: String },

    #[error("Overflow")]
    Overflow,

    #[error("Unknown property: {0}")]
    UnknownProp(String),

    #[error("Division by zero")]
    DivByZero,

    #[error("Out of range")]
    OutOfRange,

    #[error("IO error: {0}")]
    Io(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Serialization(err.to_string())
    }
}
