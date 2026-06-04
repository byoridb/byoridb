// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! DML (Data Manipulation Language) parsing
//!
//! Handles: INSERT, UPDATE, DELETE statements

use super::Parser;
use crate::ast::*;
use crate::error::*;
use crate::lexer::Token;
use crate::parser::ParseResult;
use std::collections::HashMap;

impl Parser {
    // ===== INSERT statements =====

    pub(crate) fn parse_insert(&mut self) -> ParseResult {
        self.consume_token(Token::Insert)?;

        let token = self.peek_token()?;
        match token {
            Token::Vertex => self.parse_insert_vertex(),
            Token::Edge => self.parse_insert_edge(),
            _ => Err(ParseError::UnexpectedToken(format!(
                "Expected VERTEX or EDGE, found {:?}",
                token
            ))),
        }
    }

    /// Parse: INSERT VERTEX tag_name (prop1, prop2) VALUES vid:(val1, val2), ...
    fn parse_insert_vertex(&mut self) -> ParseResult {
        self.consume_token(Token::Vertex)?;

        // Parse tag name
        let tag_name = self.consume_identifier()?;

        // Parse property names: (prop1, prop2, ...)
        self.consume_token(Token::LParen)?;
        let prop_names = self.parse_identifier_list()?;
        self.consume_token(Token::RParen)?;

        // VALUES keyword
        self.consume_token(Token::Values)?;

        // Parse vertex values: vid:(val1, val2), ...
        let mut vertices = Vec::new();
        loop {
            let vid = self.parse_expression()?;
            self.consume_token(Token::Colon)?;
            self.consume_token(Token::LParen)?;
            let values = self.parse_expression_list()?;
            self.consume_token(Token::RParen)?;

            // Build property map
            let mut props = HashMap::new();
            for (i, name) in prop_names.iter().enumerate() {
                if i < values.len() {
                    props.insert(name.clone(), values[i].clone());
                }
            }

            vertices.push(VertexInsertSpec {
                vid,
                tags: vec![TagInsertSpec {
                    name: tag_name.clone(),
                    props,
                }],
            });

            if !self.match_token(Token::Comma) {
                break;
            }
        }

        Ok(Statement::Insert(InsertStatement {
            insert_type: InsertType::Vertex,
            space: None,
            vertices,
            edges: Vec::new(),
        }))
    }

    /// Parse: INSERT EDGE edge_name (prop1) VALUES src->dst@rank:(val1), ...
    fn parse_insert_edge(&mut self) -> ParseResult {
        self.consume_token(Token::Edge)?;

        // Parse edge name
        let edge_name = self.consume_identifier()?;

        // Parse property names: (prop1, prop2, ...)
        self.consume_token(Token::LParen)?;
        let prop_names = self.parse_identifier_list()?;
        self.consume_token(Token::RParen)?;

        // VALUES keyword
        self.consume_token(Token::Values)?;

        // Parse edge values: src->dst@ranking:(val1, val2), ...
        let mut edges = Vec::new();
        loop {
            // Parse src vid
            let src_vid = self.parse_expression()?;

            // Expect -> arrow
            self.consume_token(Token::Arrow)?;

            // Parse dst vid
            let dst_vid = self.parse_expression()?;

            // Optional @ranking
            let ranking = if self.match_token(Token::At) {
                if let Ok(Token::Integer(r)) = self.peek_token() {
                    self.advance();
                    Some(r)
                } else {
                    None
                }
            } else {
                None
            };

            // Parse values: :(val1, val2)
            self.consume_token(Token::Colon)?;
            self.consume_token(Token::LParen)?;
            let values = self.parse_expression_list()?;
            self.consume_token(Token::RParen)?;

            // Build property map
            let mut props = HashMap::new();
            for (i, name) in prop_names.iter().enumerate() {
                if i < values.len() {
                    props.insert(name.clone(), values[i].clone());
                }
            }

            edges.push(EdgeInsertSpec {
                src_vid,
                dst_vid,
                ranking,
                edge_name: edge_name.clone(),
                props,
            });

            if !self.match_token(Token::Comma) {
                break;
            }
        }

        Ok(Statement::Insert(InsertStatement {
            insert_type: InsertType::Edge,
            space: None,
            vertices: Vec::new(),
            edges,
        }))
    }

    // ===== UPDATE statements =====

    /// Parse: UPDATE VERTEX ON tag_name vid SET prop = value [WHEN condition] [YIELD columns]
    pub(crate) fn parse_update(&mut self) -> ParseResult {
        self.consume_token(Token::Update)?;

        let token = self.peek_token()?;
        match token {
            Token::Vertex => self.parse_update_vertex(),
            Token::Edge => self.parse_update_edge(),
            _ => Err(ParseError::UnexpectedToken(format!(
                "Expected VERTEX or EDGE, found {:?}",
                token
            ))),
        }
    }

    fn parse_update_vertex(&mut self) -> ParseResult {
        self.consume_token(Token::Vertex)?;
        self.consume_token(Token::On)?;

        // Tag name
        let tag_name = self.consume_identifier()?;

        // Vertex ID
        let vid = self.parse_expression()?;

        // SET clause
        self.consume_token(Token::Set)?;
        let updates = self.parse_set_assignments()?;

        // Optional WHEN clause
        let conditions = if self.match_token(Token::When) {
            Some(self.parse_expression()?)
        } else {
            None
        };

        // Optional YIELD clause
        let yield_clause = if self.match_token(Token::Yield) {
            Some(self.parse_yield_clause()?)
        } else {
            None
        };

        Ok(Statement::Update(UpdateStatement {
            update_type: UpdateType::Vertex,
            space: None,
            vid,
            dst_vid: None,
            ranking: None,
            tag_name: Some(tag_name),
            edge_name: None,
            updates,
            conditions,
            yield_clause,
        }))
    }

    fn parse_update_edge(&mut self) -> ParseResult {
        self.consume_token(Token::Edge)?;
        self.consume_token(Token::On)?;

        // Edge name
        let edge_name = self.consume_identifier()?;

        // Parse src_vid
        let vid = self.parse_expression()?;

        // Expect -> arrow
        self.consume_token(Token::Arrow)?;

        // Parse dst_vid
        let dst_vid = self.parse_expression()?;

        // Optional @ranking
        let ranking = if self.match_token(Token::At) {
            if let Ok(Token::Integer(r)) = self.peek_token() {
                self.advance();
                Some(r)
            } else {
                None
            }
        } else {
            None
        };

        // SET clause
        self.consume_token(Token::Set)?;
        let updates = self.parse_set_assignments()?;

        // Optional WHEN clause
        let conditions = if self.match_token(Token::When) {
            Some(self.parse_expression()?)
        } else {
            None
        };

        // Optional YIELD clause
        let yield_clause = if self.match_token(Token::Yield) {
            Some(self.parse_yield_clause()?)
        } else {
            None
        };

        Ok(Statement::Update(UpdateStatement {
            update_type: UpdateType::Edge,
            space: None,
            vid,
            dst_vid: Some(dst_vid),
            ranking,
            tag_name: None,
            edge_name: Some(edge_name),
            updates,
            conditions,
            yield_clause,
        }))
    }

    pub(crate) fn parse_set_assignments(&mut self) -> Result<HashMap<String, Expression>> {
        let mut updates = HashMap::new();

        loop {
            let name = self.consume_identifier()?;
            self.consume_token(Token::Eq)?;
            let value = self.parse_expression()?;
            updates.insert(name, value);

            if !self.match_token(Token::Comma) {
                break;
            }
        }

        Ok(updates)
    }

    // ===== DELETE statements =====

    /// Parse: DELETE VERTEX vid1, vid2, ... [WHERE condition]
    /// or: DELETE EDGE edge_name src->dst@ranking, ...
    pub(crate) fn parse_delete(&mut self) -> ParseResult {
        self.consume_token(Token::Delete)?;

        let token = self.peek_token()?;
        match token {
            Token::Vertex => {
                self.advance();
                let vids = self.parse_expression_list()?;

                let conditions = if self.match_token(Token::Where) {
                    Some(self.parse_expression()?)
                } else {
                    None
                };

                Ok(Statement::Delete(DeleteStatement {
                    delete_type: DeleteType::Vertex,
                    space: None,
                    vids,
                    edge_refs: Vec::new(),
                    edge_name: None,
                    conditions,
                }))
            }
            Token::Edge => {
                self.advance();

                // Parse edge name
                let edge_name = self.consume_identifier()?;

                // Parse edge references: src->dst@ranking, ...
                let mut edge_refs = Vec::new();
                loop {
                    // Parse src vid
                    let src_vid = self.parse_expression()?;

                    // Expect -> arrow
                    self.consume_token(Token::Arrow)?;

                    // Parse dst vid
                    let dst_vid = self.parse_expression()?;

                    // Optional @ranking
                    let ranking = if self.match_token(Token::At) {
                        if let Ok(Token::Integer(r)) = self.peek_token() {
                            self.advance();
                            Some(r)
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    edge_refs.push(EdgeRef {
                        src_vid,
                        dst_vid,
                        ranking,
                    });

                    if !self.match_token(Token::Comma) {
                        break;
                    }
                }

                let conditions = if self.match_token(Token::Where) {
                    Some(self.parse_expression()?)
                } else {
                    None
                };

                Ok(Statement::Delete(DeleteStatement {
                    delete_type: DeleteType::Edge,
                    space: None,
                    vids: Vec::new(),
                    edge_refs,
                    edge_name: Some(edge_name),
                    conditions,
                }))
            }
            _ => Err(ParseError::UnexpectedToken(format!(
                "Expected VERTEX or EDGE, found {:?}",
                token
            ))),
        }
    }
}
