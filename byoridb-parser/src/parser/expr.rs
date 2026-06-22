// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Expression parsing
//!
//! Handles: expressions, data types, property specs, identifier lists

use super::Parser;
use crate::ast::*;
use crate::error::*;
use crate::lexer::Token;

impl Parser {
    // ===== Property and type parsing =====

    pub(crate) fn parse_property_specs(&mut self) -> Result<Vec<PropertySpec>> {
        let mut props = Vec::new();

        loop {
            if self.peek_token()? == Token::RParen {
                break;
            }

            let name = self.consume_identifier()?;
            // nGQL doesn't use colon between name and type

            let data_type = self.parse_data_type()?;
            let nullable = self.match_token(Token::Null);

            let default = if self.match_token(Token::Default) {
                Some(self.parse_default_literal()?)
            } else {
                None
            };

            props.push(PropertySpec {
                name,
                data_type,
                nullable,
                default,
            });

            if !self.match_token(Token::Comma) {
                break;
            }
        }

        Ok(props)
    }

    /// Parse a literal expression used as a column DEFAULT value.
    ///
    /// Only accepts primitive literals (string, integer, float, bool, NULL)
    /// with an optional leading `-` for numerics. Function calls and
    /// identifiers are rejected because the downstream schema layer only
    /// understands literal defaults.
    fn parse_default_literal(&mut self) -> Result<Expression> {
        let negate = self.match_token(Token::Minus);
        let token = self.peek_token()?;

        let literal = match token {
            Token::Integer(i) => {
                self.advance();
                Literal::Int(if negate { -i } else { i })
            }
            Token::FloatLiteral(f) => {
                self.advance();
                Literal::Float(if negate { -f } else { f })
            }
            Token::StringLiteral(s) | Token::SingleQuotedString(s) => {
                if negate {
                    return Err(ParseError::InvalidSyntax(
                        "Cannot apply unary minus to a string literal".to_string(),
                    ));
                }
                self.advance();
                Literal::String(super::unquote(&s))
            }
            Token::True => {
                if negate {
                    return Err(ParseError::InvalidSyntax(
                        "Cannot apply unary minus to a boolean literal".to_string(),
                    ));
                }
                self.advance();
                Literal::Bool(true)
            }
            Token::False => {
                if negate {
                    return Err(ParseError::InvalidSyntax(
                        "Cannot apply unary minus to a boolean literal".to_string(),
                    ));
                }
                self.advance();
                Literal::Bool(false)
            }
            Token::Null => {
                if negate {
                    return Err(ParseError::InvalidSyntax(
                        "Cannot apply unary minus to NULL".to_string(),
                    ));
                }
                self.advance();
                Literal::Null
            }
            _ => {
                return Err(ParseError::InvalidSyntax(format!(
                    "DEFAULT value must be a literal, found {:?}",
                    token
                )))
            }
        };

        Ok(Expression::Literal(literal))
    }

    pub(crate) fn parse_data_type(&mut self) -> Result<DataType> {
        let token = self.peek_token()?;

        Ok(match token {
            Token::Bool => {
                self.advance();
                DataType::Bool
            }
            Token::Int8 => {
                self.advance();
                DataType::Int8
            }
            Token::Int16 => {
                self.advance();
                DataType::Int16
            }
            Token::Int32 => {
                self.advance();
                DataType::Int32
            }
            Token::Int64 => {
                self.advance();
                DataType::Int64
            }
            Token::Float => {
                self.advance();
                DataType::Float
            }
            Token::Double => {
                self.advance();
                DataType::Double
            }
            Token::String => {
                self.advance();
                DataType::String
            }
            Token::Timestamp => {
                self.advance();
                DataType::Timestamp
            }
            Token::Date => {
                self.advance();
                DataType::Date
            }
            Token::Time => {
                self.advance();
                DataType::Time
            }
            Token::DateTime => {
                self.advance();
                DataType::DateTime
            }
            _ => {
                return Err(ParseError::InvalidSyntax(format!(
                    "Expected data type, found {:?}",
                    token
                )))
            }
        })
    }

    pub(crate) fn parse_identifier_list(&mut self) -> Result<Vec<String>> {
        let mut identifiers = Vec::new();

        loop {
            if self.peek_token()? == Token::RParen {
                break;
            }

            identifiers.push(self.consume_identifier()?);

            // Support optional string-length hint: field(30) → store only "field".
            if self.peek_token()? == Token::LParen {
                self.consume_token(Token::LParen)?;
                // Consume the length argument (integer) and closing paren.
                if let Ok(Token::Integer(_)) = self.peek_token() {
                    self.advance();
                }
                self.consume_token(Token::RParen)?;
            }

            if !self.match_token(Token::Comma) {
                break;
            }
        }

        Ok(identifiers)
    }

    // ===== Expression parsing =====

    /// Parse a simple expression (literal, identifier, or basic operations)
    pub(crate) fn parse_expression(&mut self) -> Result<Expression> {
        self.parse_or_expression()
    }

    fn parse_or_expression(&mut self) -> Result<Expression> {
        let mut left = self.parse_and_expression()?;

        while self.match_token(Token::Or) {
            let right = self.parse_and_expression()?;
            left = Expression::BinaryOp {
                op: BinaryOperator::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_and_expression(&mut self) -> Result<Expression> {
        let mut left = self.parse_comparison_expression()?;

        while self.match_token(Token::And) {
            let right = self.parse_comparison_expression()?;
            left = Expression::BinaryOp {
                op: BinaryOperator::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_comparison_expression(&mut self) -> Result<Expression> {
        let left = self.parse_additive_expression()?;

        // Single-token comparison operators
        let token = self.peek_token()?;
        let op_single = match token {
            Token::Eq | Token::EqEq => Some(BinaryOperator::Eq),
            Token::Neq => Some(BinaryOperator::Neq),
            Token::Lt => Some(BinaryOperator::Lt),
            Token::Lte => Some(BinaryOperator::Lte),
            Token::Gt => Some(BinaryOperator::Gt),
            Token::Gte => Some(BinaryOperator::Gte),
            Token::RegexMatch => Some(BinaryOperator::Regex),
            Token::Contains => Some(BinaryOperator::Contains),
            _ => None,
        };

        if let Some(op) = op_single {
            self.advance();
            let right = self.parse_additive_expression()?;
            return Ok(Expression::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            });
        }

        // Multi-token operators: NOT CONTAINS, STARTS WITH, ENDS WITH
        match self.peek_token()? {
            Token::Not => {
                self.advance();
                if matches!(self.peek_token(), Ok(Token::Contains)) {
                    self.advance();
                    let right = self.parse_additive_expression()?;
                    return Ok(Expression::BinaryOp {
                        op: BinaryOperator::NotContains,
                        left: Box::new(left),
                        right: Box::new(right),
                    });
                }
                // NOT alone without CONTAINS — backtrack is not possible; return left
            }
            Token::Starts => {
                self.advance();
                if self.match_token(Token::With) {
                    let right = self.parse_additive_expression()?;
                    return Ok(Expression::BinaryOp {
                        op: BinaryOperator::StartsWith,
                        left: Box::new(left),
                        right: Box::new(right),
                    });
                }
            }
            Token::Ends => {
                self.advance();
                if self.match_token(Token::With) {
                    let right = self.parse_additive_expression()?;
                    return Ok(Expression::BinaryOp {
                        op: BinaryOperator::EndsWith,
                        left: Box::new(left),
                        right: Box::new(right),
                    });
                }
            }
            _ => {}
        }

        Ok(left)
    }

    fn parse_additive_expression(&mut self) -> Result<Expression> {
        let mut left = self.parse_multiplicative_expression()?;

        loop {
            let token = self.peek_token()?;
            let op = match token {
                Token::Plus => Some(BinaryOperator::Add),
                Token::Minus => Some(BinaryOperator::Sub),
                _ => None,
            };

            if let Some(op) = op {
                self.advance();
                let right = self.parse_multiplicative_expression()?;
                left = Expression::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_multiplicative_expression(&mut self) -> Result<Expression> {
        let mut left = self.parse_unary_expression()?;

        loop {
            let token = self.peek_token()?;
            let op = match token {
                Token::Star => Some(BinaryOperator::Mul),
                Token::Slash => Some(BinaryOperator::Div),
                Token::Percent => Some(BinaryOperator::Mod),
                _ => None,
            };

            if let Some(op) = op {
                self.advance();
                let right = self.parse_unary_expression()?;
                left = Expression::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_unary_expression(&mut self) -> Result<Expression> {
        let token = self.peek_token()?;

        match token {
            Token::Minus => {
                self.advance();
                let operand = self.parse_unary_expression()?;
                Ok(Expression::UnaryOp {
                    op: UnaryOperator::Neg,
                    operand: Box::new(operand),
                })
            }
            Token::NotOp | Token::Not => {
                self.advance();
                let operand = self.parse_unary_expression()?;
                Ok(Expression::UnaryOp {
                    op: UnaryOperator::Not,
                    operand: Box::new(operand),
                })
            }
            _ => self.parse_primary_expression(),
        }
    }

    fn parse_primary_expression(&mut self) -> Result<Expression> {
        let token = self.peek_token()?;

        match token {
            // COUNT(*) wildcard — treated as a named identifier so aggregate
            // functions can recognise it and count all rows.
            Token::Star => {
                self.advance();
                Ok(Expression::Identifier("*".to_string()))
            }
            Token::Integer(i) => {
                self.advance();
                Ok(Expression::Literal(Literal::Int(i)))
            }
            Token::FloatLiteral(f) => {
                self.advance();
                Ok(Expression::Literal(Literal::Float(f)))
            }
            Token::StringLiteral(s) => {
                self.advance();
                let unquoted = super::unquote(&s);
                Ok(Expression::Literal(Literal::String(unquoted)))
            }
            Token::SingleQuotedString(s) => {
                self.advance();
                let unquoted = super::unquote(&s);
                Ok(Expression::Literal(Literal::String(unquoted)))
            }
            Token::True => {
                self.advance();
                Ok(Expression::Literal(Literal::Bool(true)))
            }
            Token::False => {
                self.advance();
                Ok(Expression::Literal(Literal::Bool(false)))
            }
            Token::Null => {
                self.advance();
                Ok(Expression::Literal(Literal::Null))
            }
            Token::Dollar => {
                // `$$.<tag>.<prop>` — destination vertex property reference
                self.advance();
                self.consume_token(Token::Dollar)?;
                self.consume_token(Token::Dot)?;
                let tag = self.consume_identifier()?;
                self.consume_token(Token::Dot)?;
                let prop = self.consume_identifier()?;
                Ok(Expression::DstVertexProp { tag, prop })
            }
            Token::Identifier(name) => {
                self.advance();
                if self.peek_token()? == Token::LParen {
                    // Function call: name(args)
                    self.consume_token(Token::LParen)?;
                    let args = self.parse_expression_list()?;
                    self.consume_token(Token::RParen)?;
                    Ok(Expression::FunctionCall { name, args })
                } else if self.match_token(Token::Dot) {
                    let first = self.consume_identifier()?;
                    // Three-level: var.tag.prop (e.g. `n.person.name`)
                    // → PropRef { object: "n.person", prop: "name" }
                    if self.match_token(Token::Dot) {
                        let prop = self.consume_identifier()?;
                        Ok(Expression::PropRef {
                            object: format!("{}.{}", name, first),
                            prop,
                        })
                    } else {
                        Ok(Expression::PropRef {
                            object: name,
                            prop: first,
                        })
                    }
                } else {
                    Ok(Expression::Identifier(name))
                }
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expression()?;
                self.consume_token(Token::RParen)?;
                Ok(expr)
            }
            Token::LBracket => {
                // List literal: `[e1, e2, ...]` (empty `[]` allowed). Used for
                // embedding vectors, e.g. `INSERT VERTEX p(vec) VALUES 1:([0.1, 0.2])`.
                self.advance();
                let mut items = Vec::new();
                if self.peek_token()? != Token::RBracket {
                    loop {
                        items.push(self.parse_expression()?);
                        if !self.match_token(Token::Comma) {
                            break;
                        }
                    }
                }
                self.consume_token(Token::RBracket)?;
                Ok(Expression::List(items))
            }
            _ => {
                // Allow keywords to be used as identifiers in expressions
                if let Some(keyword_str) = self.keyword_to_string(&token) {
                    self.advance();
                    if self.peek_token()? == Token::LParen {
                        // Keyword used as function name
                        self.consume_token(Token::LParen)?;
                        let args = self.parse_expression_list()?;
                        self.consume_token(Token::RParen)?;
                        Ok(Expression::FunctionCall {
                            name: keyword_str,
                            args,
                        })
                    } else if self.match_token(Token::Dot) {
                        let first = self.consume_identifier()?;
                        if self.match_token(Token::Dot) {
                            let prop = self.consume_identifier()?;
                            Ok(Expression::PropRef {
                                object: format!("{}.{}", keyword_str, first),
                                prop,
                            })
                        } else {
                            Ok(Expression::PropRef {
                                object: keyword_str,
                                prop: first,
                            })
                        }
                    } else {
                        Ok(Expression::Identifier(keyword_str))
                    }
                } else {
                    Err(ParseError::UnexpectedToken(format!(
                        "Expected expression, found {:?}",
                        token
                    )))
                }
            }
        }
    }

    /// Parse a comma-separated list of expressions
    pub(crate) fn parse_expression_list(&mut self) -> Result<Vec<Expression>> {
        let mut expressions = Vec::new();

        // Check for empty list
        if let Ok(Token::RParen) = self.peek_token() {
            return Ok(expressions);
        }

        loop {
            expressions.push(self.parse_expression()?);

            if !self.match_token(Token::Comma) {
                break;
            }
        }

        Ok(expressions)
    }
}
