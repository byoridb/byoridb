// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! DQL (Data Query Language) parsing
//!
//! Handles: FETCH, FIND, MATCH, GO, LOOKUP statements

use super::Parser;
use crate::ast::*;
use crate::error::*;
use crate::lexer::Token;
use crate::parser::ParseResult;
use std::collections::HashMap;

impl Parser {
    // ===== FETCH statement =====

    /// Parse: `FETCH PROP ON tag_name vid1, vid2, ...` (vertex fetch)
    /// or:    `FETCH PROP ON edge_type src1->dst1, src2->dst2, ...` (edge fetch)
    pub(crate) fn parse_fetch(&mut self) -> ParseResult {
        self.consume_token(Token::Fetch)?;
        self.consume_token(Token::Prop)?;
        self.consume_token(Token::On)?;

        // Tag/edge type name or *
        let tags = if self.match_token(Token::Star) {
            Vec::new()
        } else {
            vec![self.consume_identifier()?]
        };

        // $var.col as VID source — compound statement support
        if matches!(self.peek_token(), Ok(Token::Dollar)) {
            self.advance(); // consume $
            let var_name = self.consume_identifier()?;
            let qualified = if self.match_token(Token::Dot) {
                let col = self.consume_identifier()?;
                format!("{}.{}", var_name, col)
            } else {
                var_name
            };
            let yield_clause = if self.match_token(Token::Yield) {
                Some(self.parse_yield_clause()?)
            } else {
                None
            };
            return Ok(Statement::Fetch(FetchStatement {
                fetch_type: FetchType::Vertex,
                space: None,
                vids: Vec::new(),
                tags,
                yield_clause,
                src_var: Some(qualified),
            }));
        }

        // Detect edge fetch: peek for `int ->` pattern
        let is_edge = self.peek_is_edge_ref();

        let (fetch_type, vids) = if is_edge {
            // Parse `src->dst [, src->dst ...]`
            let mut edge_exprs: Vec<Expression> = Vec::new();
            loop {
                let src = self.consume_integer()?;
                self.consume_token(Token::Arrow)?;
                let dst = self.consume_integer()?;
                // Encode as two consecutive integer literals; plan builder pairs them up
                edge_exprs.push(Expression::Literal(Literal::Int(src)));
                edge_exprs.push(Expression::Literal(Literal::Int(dst)));
                if !self.match_token(Token::Comma) {
                    break;
                }
            }
            (FetchType::EdgeProp, edge_exprs)
        } else {
            (FetchType::Vertex, self.parse_expression_list()?)
        };

        let yield_clause = if self.match_token(Token::Yield) {
            Some(self.parse_yield_clause()?)
        } else {
            None
        };

        Ok(Statement::Fetch(FetchStatement {
            fetch_type,
            space: None,
            vids,
            tags,
            yield_clause,
            src_var: None,
        }))
    }

    /// Return true when the upcoming tokens look like `<int> ->` (edge ref).
    fn peek_is_edge_ref(&self) -> bool {
        let is_int = matches!(
            self.tokens.get(self.pos).map(|t| &t.token),
            Some(Token::Integer(_))
        );
        let is_arrow = matches!(
            self.tokens.get(self.pos + 1).map(|t| &t.token),
            Some(Token::Arrow)
        );
        is_int && is_arrow
    }

    // ===== FIND statement =====

    /// Parse: FIND SHORTEST PATH FROM vid TO vid OVER edge [WHERE ...] [YIELD ...]
    pub(crate) fn parse_find(&mut self) -> ParseResult {
        self.consume_token(Token::Find)?;

        let find_type = if self.match_token(Token::Shortest) {
            self.consume_token(Token::Path)?;
            FindType::ShortestPath
        } else {
            self.consume_token(Token::Path)?;
            FindType::Path
        };

        self.consume_token(Token::From)?;
        let from_vid = self.parse_expression()?;

        self.consume_token(Token::To)?;
        let to_vid = self.parse_expression()?;

        self.consume_token(Token::Over)?;
        let over_edge = if self.match_token(Token::Star) {
            "*".to_string()
        } else {
            self.consume_identifier()?
        };

        let weight_prop = if self.match_token(Token::Weight) {
            self.consume_token(Token::By)?;
            Some(self.consume_identifier()?)
        } else {
            None
        };

        // Optional: BIDIRECT, REVERSELY, or UPTO steps parsing could go here

        let where_clause = if self.match_token(Token::Where) {
            Some(self.parse_expression()?)
        } else {
            None
        };

        let yield_clause = if self.match_token(Token::Yield) {
            Some(self.parse_yield_clause()?)
        } else {
            None
        };

        Ok(Statement::Find(FindStatement {
            find_type,
            from_vid,
            to_vid,
            over_edge,
            weight_prop,
            upto_steps: None,
            where_clause,
            yield_clause,
        }))
    }

    // ===== MATCH statement =====

    /// Parse: MATCH (n:Tag)-[e:Edge]->(m) [WHERE condition] RETURN columns
    pub(crate) fn parse_match(&mut self) -> ParseResult {
        self.consume_token(Token::Match)?;

        // Parse pattern(s). Multiple comma-separated patterns
        // (`MATCH (a)-[]->(b), (a)-[]->(c)`) are joined on shared variables.
        // Previously the parser stopped at the comma, silently dropping the
        // trailing pattern AND the subsequent WHERE/RETURN/LIMIT clauses
        // (H-6). Collect all comma-separated patterns into Pattern::Multiple.
        let mut patterns = vec![self.parse_match_pattern()?];
        while self.match_token(Token::Comma) {
            patterns.push(self.parse_match_pattern()?);
        }
        let pattern = if patterns.len() == 1 {
            patterns.pop().unwrap()
        } else {
            Pattern::Multiple(patterns)
        };

        // OPTIONAL MATCH clauses — zero or more
        let mut optional_patterns = Vec::new();
        while matches!(self.peek_token(), Ok(Token::Optional)) {
            self.advance(); // consume OPTIONAL
            self.consume_token(Token::Match)?;
            optional_patterns.push(self.parse_match_pattern()?);
        }

        // Optional WHERE clause
        let where_clause = if self.match_token(Token::Where) {
            Some(self.parse_expression()?)
        } else {
            None
        };

        // RETURN clause
        let return_clause = if self.match_token(Token::Return) {
            Some(self.parse_return_clause()?)
        } else {
            None
        };

        // GROUP BY clause
        let group_by = if self.match_token(Token::Group) {
            self.consume_token(Token::By)?;
            Some(self.parse_expression_list()?)
        } else {
            None
        };

        // ORDER BY — consume and discard (sorting not yet implemented, but
        // must be parsed so LIMIT can still be reached).
        // Handles: ORDER BY col [ASC|DESC] [, col2 [ASC|DESC] ...]
        if self.match_token(Token::Order) {
            let _ = self.consume_token(Token::By);
            loop {
                // Parse sort expression
                if self.parse_expression().is_err() {
                    break;
                }
                // Optional direction: DESC token or ASC identifier
                if self.match_token(Token::Desc) {
                    // consumed
                } else if let Ok(Token::Identifier(ref s)) = self.peek_token() {
                    if s.eq_ignore_ascii_case("ASC") {
                        self.advance();
                    }
                }
                // Continue if comma
                if !self.match_token(Token::Comma) {
                    break;
                }
            }
        }

        let limit = if self.match_token(Token::Limit) {
            match self.peek_token()? {
                Token::Integer(n) => {
                    self.advance();
                    Some(n as usize)
                }
                _ => None,
            }
        } else {
            None
        };

        let offset = if self.match_token(Token::Offset) {
            match self.peek_token()? {
                Token::Integer(n) => {
                    self.advance();
                    Some(n as usize)
                }
                _ => None,
            }
        } else {
            None
        };

        Ok(Statement::Match(MatchStatement {
            pattern,
            where_clause,
            optional_patterns,
            return_clause,
            group_by,
            limit,
            offset,
        }))
    }

    /// Parse a MATCH pattern like (n:Person)-[e:KNOWS]->(m:Person)
    fn parse_match_pattern(&mut self) -> Result<Pattern> {
        let start = self.parse_node_pattern()?;
        let mut edges = Vec::new();
        let mut nodes: Vec<NodePattern> = Vec::new();

        // Parse chain of edges and nodes
        loop {
            // Check for edge pattern: -[...]-> or <-[...]-
            let direction = if self.match_token(Token::Minus) {
                if self.match_token(Token::LBracket) {
                    // Parse edge details
                    let edge_pattern = self.parse_edge_pattern_inner()?;
                    self.consume_token(Token::RBracket)?;

                    // Direction suffix: `->` (Arrow) or `-` + optional `>`.
                    let direction = if self.match_token(Token::Arrow) {
                        EdgeDirection::Outgoing
                    } else {
                        self.consume_token(Token::Minus)?;
                        if self.match_token(Token::Gt) {
                            EdgeDirection::Outgoing
                        } else {
                            EdgeDirection::Undirected
                        }
                    };
                    edges.push(EdgePattern {
                        direction,
                        ..edge_pattern
                    });
                    nodes.push(self.parse_node_pattern()?);
                    continue;
                } else if self.match_token(Token::Gt) {
                    // Simple edge: ->
                    edges.push(EdgePattern {
                        variable: None,
                        edge_types: Vec::new(),
                        direction: EdgeDirection::Outgoing,
                        props: HashMap::new(),
                        range: None,
                    });
                    nodes.push(self.parse_node_pattern()?);
                    continue;
                } else {
                    break;
                }
            } else if self.match_token(Token::ReverseArrow) {
                // <-
                if self.match_token(Token::LBracket) {
                    let edge_pattern = self.parse_edge_pattern_inner()?;
                    self.consume_token(Token::RBracket)?;
                    self.consume_token(Token::Minus)?;
                    edges.push(EdgePattern {
                        direction: EdgeDirection::Incoming,
                        ..edge_pattern
                    });
                    nodes.push(self.parse_node_pattern()?);
                    continue;
                } else {
                    EdgeDirection::Incoming
                }
            } else {
                break;
            };

            // Parse next node (used only for the unbracketed edge branches)
            let next_node = self.parse_node_pattern()?;
            nodes.push(next_node);
            edges.push(EdgePattern {
                variable: None,
                edge_types: Vec::new(),
                direction,
                props: HashMap::new(),
                range: None,
            });
        }

        Ok(Pattern::Path(PathPattern {
            start,
            edges,
            nodes,
        }))
    }

    /// Parse node pattern: (variable:Label1:Label2 {prop: value})
    fn parse_node_pattern(&mut self) -> Result<NodePattern> {
        self.consume_token(Token::LParen)?;

        let mut variable = None;
        let mut labels = Vec::new();

        // Check for variable or label
        if let Ok(Token::Identifier(name)) = self.peek_token() {
            self.advance();
            variable = Some(name);
        }

        // Parse labels after colon
        while self.match_token(Token::Colon) {
            labels.push(self.consume_identifier()?);
        }

        // Parse optional property filter: { key: literal, ... }
        let props = if self.peek_token()? == Token::LBrace {
            self.parse_pattern_properties()?
        } else {
            HashMap::new()
        };

        self.consume_token(Token::RParen)?;

        Ok(NodePattern {
            variable,
            labels,
            props,
        })
    }

    /// Parse a property filter map used inside a node or edge pattern:
    /// `{ key: literal [, key: literal ...] }`.
    ///
    /// Only literal values are accepted for now — complex expressions are
    /// rejected so that downstream matching can compare against stored
    /// property values without needing a full evaluator.
    fn parse_pattern_properties(&mut self) -> Result<HashMap<String, Expression>> {
        self.consume_token(Token::LBrace)?;

        let mut props = HashMap::new();

        if self.peek_token()? == Token::RBrace {
            self.consume_token(Token::RBrace)?;
            return Ok(props);
        }

        loop {
            let key = self.consume_identifier()?;
            self.consume_token(Token::Colon)?;
            let value = self.parse_pattern_property_value()?;

            if props.insert(key.clone(), value).is_some() {
                return Err(ParseError::InvalidSyntax(format!(
                    "Duplicate property key in pattern: {}",
                    key
                )));
            }

            if !self.match_token(Token::Comma) {
                break;
            }
        }

        self.consume_token(Token::RBrace)?;
        Ok(props)
    }

    /// Parse a single literal value used inside a pattern property filter.
    ///
    /// Supports integer, float, string, bool, NULL, with optional leading
    /// unary minus for numerics. Mirrors the default-literal restriction in
    /// `parse_property_specs`.
    fn parse_pattern_property_value(&mut self) -> Result<Expression> {
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
                Literal::String(s[1..s.len() - 1].to_string())
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
                    "Pattern property value must be a literal, found {:?}",
                    token
                )))
            }
        };

        Ok(Expression::Literal(literal))
    }

    /// Parse edge pattern inner: [variable:EdgeType*1..3 {prop: value}]
    fn parse_edge_pattern_inner(&mut self) -> Result<EdgePattern> {
        let mut variable = None;
        let mut edge_types = Vec::new();
        let mut range = None;

        // Parse variable
        if let Ok(Token::Identifier(name)) = self.peek_token() {
            self.advance();
            variable = Some(name);
        }

        // Parse edge types after colon
        while self.match_token(Token::Colon) {
            edge_types.push(self.consume_identifier()?);
        }

        // Parse range *1..3
        if self.match_token(Token::Star) {
            if let Ok(Token::Integer(min)) = self.peek_token() {
                self.advance();
                if self.match_token(Token::Dot) {
                    self.consume_token(Token::Dot)?;
                    if let Ok(Token::Integer(max)) = self.peek_token() {
                        self.advance();
                        range = Some((min as u64, max as u64));
                    }
                }
            }
        }

        // Parse optional property filter: { key: literal, ... }
        let props = if self.peek_token()? == Token::LBrace {
            self.parse_pattern_properties()?
        } else {
            HashMap::new()
        };

        Ok(EdgePattern {
            variable,
            edge_types,
            direction: EdgeDirection::Outgoing, // Will be set by caller
            props,
            range,
        })
    }

    /// Parse RETURN clause: RETURN col1, col2 AS alias, ...
    pub(crate) fn parse_return_clause(&mut self) -> Result<ReturnClause> {
        let mut columns = Vec::new();

        loop {
            let expression = self.parse_expression()?;
            let alias = if self.match_token(Token::As) {
                Some(self.consume_identifier()?)
            } else {
                None
            };

            columns.push(YieldColumn { expression, alias });

            if !self.match_token(Token::Comma) {
                break;
            }
        }

        Ok(ReturnClause { columns })
    }

    /// Parse YIELD clause: YIELD col1, col2 AS alias, ...
    pub(crate) fn parse_yield_clause(&mut self) -> Result<YieldClause> {
        let mut columns = Vec::new();

        loop {
            let expression = self.parse_expression()?;
            let alias = if self.match_token(Token::As) {
                Some(self.consume_identifier()?)
            } else {
                None
            };

            columns.push(YieldColumn { expression, alias });

            if !self.match_token(Token::Comma) {
                break;
            }
        }

        Ok(YieldClause { columns })
    }

    // ===== GO statement =====

    /// Parse: GO [N STEPS] FROM vid1, vid2 OVER edge_type [WHERE condition] [YIELD columns]
    pub(crate) fn parse_go(&mut self) -> ParseResult {
        self.consume_token(Token::Go)?;

        // Optional step count: "N STEPS" or "N..M STEPS"
        let steps = if let Ok(Token::Integer(n)) = self.peek_token() {
            self.advance();
            if self.match_token(Token::Dot) {
                // Range: N..M
                self.consume_token(Token::Dot)?;
                if let Ok(Token::Integer(m)) = self.peek_token() {
                    self.advance();
                    self.match_token(Token::Steps);
                    self.match_token(Token::Step);
                    StepClause::Range(n as u32, m as u32)
                } else {
                    StepClause::Exactly(n as u32)
                }
            } else {
                self.match_token(Token::Steps);
                self.match_token(Token::Step);
                StepClause::Exactly(n as u32)
            }
        } else {
            StepClause::Exactly(1)
        };

        // FROM clause — either a list of literal VIDs / expressions or a
        // variable reference like `$var` / `$var.column` that resolves to a
        // VID column from a previous compound-statement result.
        self.consume_token(Token::From)?;
        let (vids, src_var) = if matches!(self.peek_token(), Ok(Token::Dollar)) {
            self.advance(); // consume `$`
            let var_name = self.consume_identifier()?;
            let qualified = if self.match_token(Token::Dot) {
                let col = self.consume_identifier()?;
                format!("{}.{}", var_name, col)
            } else {
                var_name
            };
            (Vec::new(), Some(qualified))
        } else {
            (self.parse_expression_list()?, None)
        };

        // OVER clause — "*" means all edge types (pass empty vec to executor)
        self.consume_token(Token::Over)?;
        let over_edges = if self.match_token(Token::Star) {
            Vec::new()
        } else {
            let mut edges = Vec::new();
            loop {
                edges.push(self.consume_identifier()?);
                if !self.match_token(Token::Comma) {
                    break;
                }
            }
            edges
        };

        let direction = if self.match_token(Token::Reversely) {
            EdgeDirection::Incoming
        } else if self.match_token(Token::Bidirect) {
            EdgeDirection::Undirected
        } else {
            EdgeDirection::Outgoing
        };

        // Optional WHERE clause
        let where_clause = if self.match_token(Token::Where) {
            Some(self.parse_expression()?)
        } else {
            None
        };

        // Optional YIELD clause
        let yield_clause = if self.match_token(Token::Yield) {
            self.parse_yield_clause()?
        } else {
            YieldClause {
                columns: Vec::new(),
            }
        };

        Ok(Statement::Go(GoStatement {
            from_clause: FromClause { vids, src: src_var },
            over_edges,
            direction,
            to_clause: ToClause {
                variable: String::new(),
                steps,
            },
            where_clause,
            yield_clause,
        }))
    }

    // ===== LOOKUP statement =====

    /// Parse: LOOKUP ON tag_name/edge_name [WHERE condition] [YIELD columns]
    pub(crate) fn parse_lookup(&mut self) -> ParseResult {
        self.consume_token(Token::Lookup)?;
        self.consume_token(Token::On)?;

        let name = self.consume_identifier()?;

        // Determine if it's a tag or edge lookup (we assume tag by default)
        let lookup_type = LookupType::Tag(name);

        // Optional WHERE clause
        let where_clause = if self.match_token(Token::Where) {
            Some(self.parse_expression()?)
        } else {
            None
        };

        // Optional YIELD clause
        let yield_clause = if self.match_token(Token::Yield) {
            self.parse_yield_clause()?
        } else {
            YieldClause {
                columns: Vec::new(),
            }
        };

        let limit = if self.match_token(Token::Limit) {
            match self.peek_token()? {
                Token::Integer(n) => {
                    self.advance();
                    Some(n as usize)
                }
                _ => None,
            }
        } else {
            None
        };

        let offset = if self.match_token(Token::Offset) {
            match self.peek_token()? {
                Token::Integer(n) => {
                    self.advance();
                    Some(n as usize)
                }
                _ => None,
            }
        } else {
            None
        };

        Ok(Statement::Lookup(LookupStatement {
            lookup_type,
            where_clause,
            yield_clause,
            limit,
            offset,
        }))
    }
}
