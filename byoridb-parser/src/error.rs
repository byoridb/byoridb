// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, ParseError>;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    #[error("Unexpected token: {0}")]
    UnexpectedToken(String),

    #[error("Unexpected end of input")]
    UnexpectedEOF,

    #[error("Invalid syntax: {0}")]
    InvalidSyntax(String),

    #[error("Unknown keyword: {0}")]
    UnknownKeyword(String),

    #[error("Invalid identifier: {0}")]
    InvalidIdentifier(String),

    #[error("Invalid literal: {0}")]
    InvalidLiteral(String),

    #[error("Type mismatch: expected {expected}, found {found}")]
    TypeMismatch { expected: String, found: String },

    #[error("Duplicate identifier: {0}")]
    DuplicateIdentifier(String),

    #[error("Lexer error: {0}")]
    LexerError(String),
}
