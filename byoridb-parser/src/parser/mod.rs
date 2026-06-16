// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! nGQL parser
//!
//! This module is split into submodules:
//! - `ddl`: CREATE, DROP, ALTER, SHOW, USE statements
//! - `dml`: INSERT, UPDATE, DELETE statements
//! - `dql`: MATCH, GO, FETCH, LOOKUP, FIND statements
//! - `expr`: Expression parsing

mod ddl;
mod dml;
mod dql;
mod expr;

#[cfg(test)]
mod tests;

use crate::lexer::{Lexer, LocatedToken, Token};
use crate::{ast::*, error::*};

pub type ParseResult = Result<Statement>;

/// Parser for nGQL
pub struct Parser {
    pub(crate) tokens: Vec<LocatedToken>,
    pub(crate) pos: usize,
}

impl Parser {
    pub fn new(input: &str) -> Self {
        let lexer = Lexer::new(input);
        let tokens = lexer.tokenize().unwrap_or_default();
        Parser { tokens, pos: 0 }
    }

    /// Entry point. Parses either a single statement or a compound query
    /// (`$var = stmt; stmt2; ...`) — compound is detected when the first
    /// token is `$` (an assignment leads the input) or when a `;` separates
    /// multiple statements.
    pub fn parse(&mut self) -> ParseResult {
        if self.is_at_end() {
            return Err(ParseError::UnexpectedEOF);
        }

        let first = self.peek_token()?;
        // Fast-path: a leading `$` always means compound (assignment leads).
        if matches!(first, Token::Dollar) {
            return self.parse_compound();
        }

        let stmt = self.parse_single_statement()?;

        // If a `;` follows with more content, escalate to compound.
        if self.match_token(Token::SemiColon) {
            // Allow trailing `;` with nothing after — return the single stmt.
            if self.is_at_end() || matches!(self.peek_token(), Ok(Token::Comment)) {
                return Ok(stmt);
            }
            let mut clauses = vec![CompoundClause {
                var: None,
                stmt: Box::new(stmt),
            }];
            loop {
                let next = self.parse_compound_clause()?;
                clauses.push(next);
                if !self.match_token(Token::SemiColon) {
                    break;
                }
                if self.is_at_end() || matches!(self.peek_token(), Ok(Token::Comment)) {
                    break;
                }
            }
            return Ok(Statement::Compound(clauses));
        }

        Ok(stmt)
    }

    fn parse_compound(&mut self) -> ParseResult {
        let mut clauses = Vec::new();
        loop {
            clauses.push(self.parse_compound_clause()?);
            if !self.match_token(Token::SemiColon) {
                break;
            }
            if self.is_at_end() || matches!(self.peek_token(), Ok(Token::Comment)) {
                break;
            }
        }
        Ok(Statement::Compound(clauses))
    }

    /// Parse one clause of a compound query: either `$name = stmt` or a
    /// bare statement. The assignment form binds the statement's
    /// `ExecutorResult` to `$name` for downstream references.
    fn parse_compound_clause(&mut self) -> Result<CompoundClause> {
        let var = if matches!(self.peek_token(), Ok(Token::Dollar)) {
            self.advance();
            let name = self.consume_identifier()?;
            self.consume_token(Token::Eq)?;
            Some(name)
        } else {
            None
        };
        let stmt = self.parse_single_statement()?;
        Ok(CompoundClause {
            var,
            stmt: Box::new(stmt),
        })
    }

    fn parse_single_statement(&mut self) -> ParseResult {
        let token = self.peek_token()?;
        match token {
            // DDL
            Token::Show => self.parse_show(),
            Token::Describe | Token::Desc => self.parse_describe(),
            Token::Use => self.parse_use(),
            Token::Create => self.parse_create(),
            Token::Alter => self.parse_alter(),
            Token::Drop => self.parse_drop(),
            // User management
            Token::Grant => self.parse_grant(),
            Token::Revoke => self.parse_revoke(),
            // Admin commands
            Token::Balance => self.parse_balance(),
            // REBUILD TAG/EDGE INDEX <name>
            Token::Rebuild => {
                self.advance();
                let kind = self.peek_token()?;
                let is_tag = match kind {
                    Token::Tag => {
                        self.advance();
                        true
                    }
                    Token::Edge => {
                        self.advance();
                        false
                    }
                    other => {
                        return Err(ParseError::UnexpectedToken(format!(
                            "Expected TAG or EDGE after REBUILD, got {:?}",
                            other
                        )))
                    }
                };
                self.consume_token(Token::Index)?;
                let _name = self.consume_identifier()?;
                // Map to SHOW INDEX STATUS so the executor can report back
                Ok(Statement::Show(if is_tag {
                    crate::ast::ShowStatement::TagIndexStatuses
                } else {
                    crate::ast::ShowStatement::EdgeIndexStatuses
                }))
            }
            // DML
            Token::Insert => self.parse_insert(),
            Token::Update => self.parse_update(),
            Token::Delete => self.parse_delete(),
            // DQL
            Token::Fetch => self.parse_fetch(),
            Token::Find => self.parse_find(),
            Token::Match => self.parse_match(),
            Token::Go => self.parse_go(),
            Token::Lookup => self.parse_lookup(),
            Token::Recommend => self.parse_recommend(),
            // EXPLAIN / PROFILE
            tok @ (Token::Explain | Token::Profile) => {
                let profile = matches!(tok, Token::Profile);
                self.advance();
                let inner = self.parse_single_statement()?;
                Ok(Statement::Explain {
                    profile,
                    statement: Box::new(inner),
                })
            }
            _ => Err(ParseError::UnexpectedToken(format!("{:?}", token))),
        }
    }

    // ===== Helper methods (pub(crate) for submodules) =====

    pub(crate) fn peek_token(&self) -> Result<Token> {
        if self.pos < self.tokens.len() {
            Ok(self.tokens[self.pos].token.clone())
        } else if self.pos == self.tokens.len() {
            Ok(Token::Comment) // Placeholder for EOF
        } else {
            Err(ParseError::UnexpectedEOF)
        }
    }

    pub(crate) fn advance(&mut self) {
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
    }

    pub(crate) fn match_token(&mut self, expected: Token) -> bool {
        if let Ok(token) = self.peek_token() {
            if std::mem::discriminant(&token) == std::mem::discriminant(&expected) {
                self.advance();
                return true;
            }
        }
        false
    }

    /// Human-readable position of the current token, for error messages.
    pub(crate) fn err_location(&self) -> String {
        match self.tokens.get(self.pos) {
            Some(t) => format!("line {}, column {}", t.line, t.column),
            None => "end of input".to_string(),
        }
    }

    pub(crate) fn consume_token(&mut self, expected: Token) -> Result<()> {
        if let Ok(token) = self.peek_token() {
            if std::mem::discriminant(&token) == std::mem::discriminant(&expected) {
                self.advance();
                return Ok(());
            }
            return Err(ParseError::UnexpectedToken(format!(
                "expected {:?}, found {:?} at {}",
                expected,
                token,
                self.err_location()
            )));
        }
        Err(ParseError::UnexpectedEOF)
    }

    pub(crate) fn consume_identifier(&mut self) -> Result<String> {
        let token = self.peek_token()?;
        match token {
            Token::Identifier(s) => {
                self.advance();
                Ok(s)
            }
            // Allow keywords to be used as identifiers (common in SQL)
            _ => {
                if let Some(keyword_str) = self.keyword_to_string(&token) {
                    self.advance();
                    Ok(keyword_str)
                } else {
                    Err(ParseError::UnexpectedToken(format!(
                        "identifier expected, found {:?} at {}",
                        token,
                        self.err_location()
                    )))
                }
            }
        }
    }

    /// Convert keyword tokens to their string representation for use as identifiers
    pub(crate) fn keyword_to_string(&self, token: &Token) -> Option<String> {
        let s = match token {
            Token::Vertex => "vertex",
            Token::Edge => "edge",
            Token::Edges => "edges",
            Token::Tag => "tag",
            Token::Tags => "tags",
            Token::Path => "path",
            Token::Paths => "paths",
            Token::Upto => "upto",
            Token::Class => "class",
            Token::Classes => "classes",
            Token::Subclass => "subclass",
            Token::Of => "of",
            Token::Weight => "weight",
            Token::Space => "space",
            Token::Spaces => "spaces",
            Token::User => "user",
            Token::Values => "values",
            Token::Set => "set",
            Token::All => "all",
            Token::From => "from",
            Token::To => "to",
            Token::On => "on",
            Token::As => "as",
            Token::By => "by",
            Token::Index => "index",
            Token::Date => "date",
            Token::Time => "time",
            Token::Step => "step",
            Token::Steps => "steps",
            // Recommend keywords — `embedding` is a natural property name.
            Token::Recommend => "recommend",
            Token::Similar => "similar",
            Token::Embedding => "embedding",
            // Role keywords (used as identifiers in GRANT/REVOKE)
            Token::Admin => "ADMIN",
            Token::God => "GOD",
            Token::Dba => "DBA",
            Token::Guest => "GUEST",
            Token::Role => "role",
            _ => return None,
        };
        Some(s.to_string())
    }

    pub(crate) fn consume_string_literal(&mut self) -> Result<String> {
        let token = self.peek_token()?;
        match token {
            Token::StringLiteral(s) => {
                self.advance();
                // Remove quotes
                Ok(s[1..s.len() - 1].to_string())
            }
            _ => Err(ParseError::UnexpectedToken(format!(
                "String literal expected, found {:?}",
                token
            ))),
        }
    }

    pub(crate) fn is_at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    /// Consume an integer literal and return its value
    pub(crate) fn consume_integer(&mut self) -> Result<i64> {
        let token = self.peek_token()?;
        match token {
            Token::Integer(n) => {
                self.advance();
                Ok(n)
            }
            _ => Err(ParseError::UnexpectedToken(format!(
                "Integer expected, found {:?}",
                token
            ))),
        }
    }
}

/// Convenience function to parse a query string
pub fn parse(input: &str) -> ParseResult {
    let mut parser = Parser::new(input);
    parser.parse()
}
