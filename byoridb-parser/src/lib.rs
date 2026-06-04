// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! nGQL (Graph Query Language) parser
//!
//! This module provides parsing for nGQL queries:
//! - DDL: CREATE SPACE, TAG, EDGE, INDEX
//! - DML: INSERT, UPDATE, DELETE, UPSERT
//! - DQL: MATCH, GO, FETCH, LOOKUP, FIND
//! - User management: CREATE USER, GRANT, REVOKE

pub mod ast;
pub mod error;
pub mod lexer;
pub mod parser;

pub use ast::*;
pub use error::{ParseError, Result};
pub use parser::{parse, ParseResult, Parser};
