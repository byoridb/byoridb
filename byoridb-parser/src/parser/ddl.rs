// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! DDL (Data Definition Language) parsing
//!
//! Handles: SHOW, USE, CREATE, ALTER, DROP statements

use super::Parser;
use crate::ast::*;
use crate::error::*;
use crate::lexer::Token;
use crate::parser::ParseResult;

impl Parser {
    // ===== SHOW statements =====

    pub(crate) fn parse_show(&mut self) -> ParseResult {
        self.consume_token(Token::Show)?;

        let token = self.peek_token()?;
        let stmt = match token {
            Token::Space | Token::Spaces => {
                self.advance();
                ShowStatement::Spaces
            }
            // SHOW TAGS / SHOW TAG / SHOW TAG INDEX[ES] [STATUS]
            Token::Tag | Token::Tags => {
                self.advance();
                if matches!(self.peek_token(), Ok(Token::Index) | Ok(Token::Indexes)) {
                    self.advance();
                    if matches!(self.peek_token(), Ok(Token::Status)) {
                        self.advance();
                        ShowStatement::TagIndexStatuses
                    } else {
                        ShowStatement::TagIndexes
                    }
                } else {
                    ShowStatement::Tags
                }
            }
            // SHOW EDGES / SHOW EDGE / SHOW EDGE INDEX[ES] [STATUS]
            Token::Edge | Token::Edges => {
                self.advance();
                if matches!(self.peek_token(), Ok(Token::Index) | Ok(Token::Indexes)) {
                    self.advance();
                    if matches!(self.peek_token(), Ok(Token::Status)) {
                        self.advance();
                        ShowStatement::EdgeIndexStatuses
                    } else {
                        ShowStatement::EdgeIndexes
                    }
                } else {
                    ShowStatement::Edges
                }
            }
            Token::User => {
                self.advance();
                ShowStatement::Users
            }
            Token::Parts => {
                self.advance();
                ShowStatement::Parts
            }
            Token::Hosts => {
                self.advance();
                ShowStatement::Hosts
            }
            // SHOW STATS
            Token::Stats => {
                self.advance();
                ShowStatement::Stats
            }
            // SHOW SESSIONS
            Token::Sessions => {
                self.advance();
                ShowStatement::Sessions
            }
            // SHOW CREATE TAG <name> / SHOW CREATE EDGE <name>
            Token::Create => {
                self.advance();
                let kind = self.peek_token()?;
                match kind {
                    Token::Tag => {
                        self.advance();
                        let name = self.consume_identifier()?;
                        ShowStatement::CreateTag(name)
                    }
                    Token::Edge => {
                        self.advance();
                        let name = self.consume_identifier()?;
                        ShowStatement::CreateEdge(name)
                    }
                    other => {
                        return Err(ParseError::UnexpectedToken(format!(
                            "Expected TAG or EDGE after SHOW CREATE, got {:?}",
                            other
                        )))
                    }
                }
            }
            _ => return Err(ParseError::UnexpectedToken(format!("{:?}", token))),
        };

        Ok(Statement::Show(stmt))
    }

    // ===== DESCRIBE / DESC statements =====

    /// Parse `DESCRIBE TAG|EDGE|SPACE <name>` (also accepts `DESC`).
    pub(crate) fn parse_describe(&mut self) -> ParseResult {
        // Consume DESCRIBE or DESC
        let token = self.peek_token()?;
        match token {
            Token::Describe | Token::Desc => self.advance(),
            _ => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected DESCRIBE or DESC, got {:?}",
                    token
                )))
            }
        }

        let target = self.peek_token()?;
        let stmt = match target {
            Token::Tag => {
                self.advance();
                // DESCRIBE TAG INDEX <name>
                if matches!(self.peek_token(), Ok(Token::Index)) {
                    self.advance();
                    let name = self.consume_identifier()?;
                    DescribeStatement::TagIndex(name)
                } else {
                    let name = self.consume_identifier()?;
                    DescribeStatement::Tag(name)
                }
            }
            Token::Edge => {
                self.advance();
                // DESCRIBE EDGE INDEX <name>
                if matches!(self.peek_token(), Ok(Token::Index)) {
                    self.advance();
                    let name = self.consume_identifier()?;
                    DescribeStatement::EdgeIndex(name)
                } else {
                    let name = self.consume_identifier()?;
                    DescribeStatement::Edge(name)
                }
            }
            Token::Space => {
                self.advance();
                let name = self.consume_identifier()?;
                DescribeStatement::Space(name)
            }
            other => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected TAG, EDGE, or SPACE after DESCRIBE, got {:?}",
                    other
                )))
            }
        };

        Ok(Statement::Describe(stmt))
    }

    // ===== USE statement =====

    pub(crate) fn parse_use(&mut self) -> ParseResult {
        self.consume_token(Token::Use)?;
        let space_name = self.consume_identifier()?;
        Ok(Statement::Use(UseStatement { space: space_name }))
    }

    // ===== CREATE statements =====

    pub(crate) fn parse_create(&mut self) -> ParseResult {
        self.consume_token(Token::Create)?;

        let token = self.peek_token()?;
        match token {
            Token::Space => self.parse_create_space(),
            Token::Tag => {
                // `CREATE TAG INDEX …` vs `CREATE TAG name(…)`
                let next = self
                    .tokens
                    .get(self.pos + 1)
                    .map(|t| t.token.clone())
                    .unwrap_or(Token::Comment);
                if next == Token::Index {
                    self.advance(); // consume TAG
                    self.consume_token(Token::Index)?; // consume INDEX
                    self.parse_tag_index_body()
                } else {
                    self.parse_create_tag()
                }
            }
            Token::Edge => {
                // `CREATE EDGE INDEX …` vs `CREATE EDGE name(…)`
                let next = self
                    .tokens
                    .get(self.pos + 1)
                    .map(|t| t.token.clone())
                    .unwrap_or(Token::Comment);
                if next == Token::Index {
                    self.advance(); // consume EDGE
                    self.consume_token(Token::Index)?; // consume INDEX
                    self.parse_edge_index_body()
                } else {
                    self.parse_create_edge()
                }
            }
            Token::Index => self.parse_create_index(),
            Token::User => self.parse_create_user(),
            _ => Err(ParseError::UnexpectedToken(format!("{:?}", token))),
        }
    }

    fn parse_create_space(&mut self) -> ParseResult {
        self.consume_token(Token::Space)?;
        let if_not_exists = self.parse_if_not_exists()?;

        let name = self.consume_identifier()?;

        // Parse space options: (partition_num=N, replica_factor=N, vid_type=TYPE)
        let (partition_num, replica_factor, vid_type) = if self.match_token(Token::LParen) {
            let options = self.parse_space_options()?;
            self.consume_token(Token::RParen)?;
            options
        } else {
            (None, None, None)
        };

        // Parse optional PARTITION BY clause
        let partition_strategy = if self.match_token(Token::Partition) {
            self.consume_token(Token::By)?;
            Some(self.parse_partition_strategy()?)
        } else {
            None
        };

        Ok(Statement::Create(CreateStatement::Space(
            CreateSpaceStatement {
                if_not_exists,
                name,
                partition_num,
                replica_factor,
                vid_type,
                partition_strategy,
            },
        )))
    }

    /// Parse space options: partition_num=N, replica_factor=N, vid_type=TYPE
    fn parse_space_options(&mut self) -> Result<(Option<u32>, Option<u32>, Option<VidType>)> {
        let mut partition_num = None;
        let mut replica_factor = None;
        let mut vid_type = None;

        loop {
            if self.peek_token()? == Token::RParen {
                break;
            }

            let option_name = self.consume_identifier()?;
            self.consume_token(Token::Eq)?;

            match option_name.to_lowercase().as_str() {
                "partition_num" => {
                    partition_num = Some(self.consume_integer()? as u32);
                }
                "replica_factor" => {
                    replica_factor = Some(self.consume_integer()? as u32);
                }
                "vid_type" => {
                    vid_type = Some(self.parse_vid_type()?);
                }
                _ => {
                    return Err(ParseError::InvalidSyntax(format!(
                        "Unknown space option: {}",
                        option_name
                    )));
                }
            }

            if !self.match_token(Token::Comma) {
                break;
            }
        }

        Ok((partition_num, replica_factor, vid_type))
    }

    /// Parse vid_type: INT64 | FIXED_STRING(N)
    fn parse_vid_type(&mut self) -> Result<VidType> {
        let token = self.peek_token()?;
        match token {
            Token::Int64 => {
                self.advance();
                Ok(VidType::Int64)
            }
            Token::FixedString => {
                self.advance();
                self.consume_token(Token::LParen)?;
                let len = self.consume_integer()? as usize;
                self.consume_token(Token::RParen)?;
                Ok(VidType::FixedString(len))
            }
            _ => Err(ParseError::UnexpectedToken(format!(
                "Expected INT64 or FIXED_STRING, found {:?}",
                token
            ))),
        }
    }

    /// Parse partition strategy: HASH | RANGE(boundaries...) | MODULO
    fn parse_partition_strategy(&mut self) -> Result<PartitionStrategySpec> {
        let token = self.peek_token()?;
        match token {
            Token::Hash => {
                self.advance();
                Ok(PartitionStrategySpec::Hash)
            }
            Token::Modulo => {
                self.advance();
                Ok(PartitionStrategySpec::Modulo)
            }
            Token::Identifier(ref s) if s.to_uppercase() == "RANGE" => {
                self.advance();
                self.consume_token(Token::LParen)?;
                let boundaries = self.parse_integer_list()?;
                self.consume_token(Token::RParen)?;
                Ok(PartitionStrategySpec::Range { boundaries })
            }
            _ => Err(ParseError::UnexpectedToken(format!(
                "Expected HASH, RANGE, or MODULO, found {:?}",
                token
            ))),
        }
    }

    /// Parse a comma-separated list of integers
    fn parse_integer_list(&mut self) -> Result<Vec<i64>> {
        let mut values = Vec::new();

        loop {
            if self.peek_token()? == Token::RParen {
                break;
            }

            values.push(self.consume_integer()?);

            if !self.match_token(Token::Comma) {
                break;
            }
        }

        Ok(values)
    }

    fn parse_create_tag(&mut self) -> ParseResult {
        self.consume_token(Token::Tag)?;
        let if_not_exists = self.parse_if_not_exists()?;
        let name = self.consume_identifier()?;

        self.consume_token(Token::LParen)?;
        let props = self.parse_property_specs()?;
        self.consume_token(Token::RParen)?;

        Ok(Statement::Create(CreateStatement::Tag(
            CreateTagStatement {
                if_not_exists,
                name,
                props,
            },
        )))
    }

    fn parse_create_edge(&mut self) -> ParseResult {
        self.consume_token(Token::Edge)?;
        let if_not_exists = self.parse_if_not_exists()?;
        let name = self.consume_identifier()?;

        self.consume_token(Token::LParen)?;
        let props = self.parse_property_specs()?;
        self.consume_token(Token::RParen)?;

        Ok(Statement::Create(CreateStatement::Edge(
            CreateEdgeStatement {
                if_not_exists,
                name,
                props,
            },
        )))
    }

    fn parse_create_index(&mut self) -> ParseResult {
        self.consume_token(Token::Index)?;

        let token = self.peek_token()?;
        match token {
            Token::Tag => self.parse_create_tag_index(),
            Token::Edge => self.parse_create_edge_index(),
            _ => Err(ParseError::UnexpectedToken(format!("{:?}", token))),
        }
    }

    /// Parse tag index body.  Call sites must have already consumed all
    /// keywords that precede the index name:
    ///   `CREATE INDEX TAG …`  → caller consumed INDEX; this fn consumes TAG
    ///   `CREATE TAG INDEX …`  → caller consumed TAG+INDEX; pass tag_already_consumed=true
    fn parse_create_tag_index(&mut self) -> ParseResult {
        self.consume_token(Token::Tag)?;
        self.parse_tag_index_body()
    }

    fn parse_tag_index_body(&mut self) -> ParseResult {
        let if_not_exists = self.parse_if_not_exists()?;
        let index_name = self.consume_identifier()?;
        self.consume_token(Token::On)?;
        // Optional `TAG` keyword (`ON TAG name(...)`). Skip it only when it is a
        // real keyword prefix; if the tag is literally named `Tag` (e.g. LDBC's
        // Tag class), `ON Tag(...)` has `(` right after — leave it for
        // consume_identifier to read as the tag name instead of swallowing it.
        if self.peek_token()? == Token::Tag
            && self.tokens.get(self.pos + 1).map(|t| &t.token) != Some(&Token::LParen)
        {
            self.advance();
        }
        let tag_name = self.consume_identifier()?;

        self.consume_token(Token::LParen)?;
        let props = self.parse_identifier_list()?;
        self.consume_token(Token::RParen)?;

        Ok(Statement::Create(CreateStatement::TagIndex(
            CreateTagIndexStatement {
                if_not_exists,
                index_name,
                tag_name,
                props,
            },
        )))
    }

    fn parse_create_edge_index(&mut self) -> ParseResult {
        self.consume_token(Token::Edge)?;
        self.parse_edge_index_body()
    }

    fn parse_edge_index_body(&mut self) -> ParseResult {
        let if_not_exists = self.parse_if_not_exists()?;
        let index_name = self.consume_identifier()?;
        self.consume_token(Token::On)?;
        // Optional `EDGE` keyword (`ON EDGE name(...)`). Skip it only when it is
        // a real keyword prefix; if the edge is literally named `Edge`,
        // `ON Edge(...)` has `(` right after — leave it for consume_identifier.
        if self.peek_token()? == Token::Edge
            && self.tokens.get(self.pos + 1).map(|t| &t.token) != Some(&Token::LParen)
        {
            self.advance();
        }
        let edge_name = self.consume_identifier()?;

        self.consume_token(Token::LParen)?;
        let props = self.parse_identifier_list()?;
        self.consume_token(Token::RParen)?;

        Ok(Statement::Create(CreateStatement::EdgeIndex(
            CreateEdgeIndexStatement {
                if_not_exists,
                index_name,
                edge_name,
                props,
            },
        )))
    }

    fn parse_create_user(&mut self) -> ParseResult {
        self.consume_token(Token::User)?;
        let if_not_exists = self.parse_if_not_exists()?;
        let username = self.consume_identifier()?;

        self.consume_token(Token::With)?;
        self.consume_token(Token::Password)?;
        let password = self.consume_string_literal()?;

        // Optional ROLE clause
        let role = if self.match_token(Token::Role) {
            Some(self.consume_identifier()?)
        } else {
            None
        };

        Ok(Statement::Create(CreateStatement::User(
            CreateUserStatement {
                if_not_exists,
                username,
                password,
                role,
            },
        )))
    }

    // ===== ALTER statements =====

    /// Parse: ALTER TAG/EDGE/USER ...
    pub(crate) fn parse_alter(&mut self) -> ParseResult {
        self.consume_token(Token::Alter)?;

        let token = self.peek_token()?;
        match token {
            Token::Tag => self.parse_alter_tag(),
            Token::Edge => self.parse_alter_edge(),
            Token::User => self.parse_alter_user(),
            _ => Err(ParseError::UnexpectedToken(format!(
                "Expected TAG, EDGE, or USER, found {:?}",
                token
            ))),
        }
    }

    fn parse_alter_tag(&mut self) -> ParseResult {
        self.consume_token(Token::Tag)?;
        let name = self.consume_identifier()?;
        let operations = self.parse_alter_operations()?;

        Ok(Statement::Alter(AlterStatement::Tag(AlterTagStatement {
            name,
            operations,
        })))
    }

    fn parse_alter_edge(&mut self) -> ParseResult {
        self.consume_token(Token::Edge)?;
        let name = self.consume_identifier()?;
        let operations = self.parse_alter_operations()?;

        Ok(Statement::Alter(AlterStatement::Edge(AlterEdgeStatement {
            name,
            operations,
        })))
    }

    /// Parse: ALTER USER username WITH PASSWORD 'newpass'
    fn parse_alter_user(&mut self) -> ParseResult {
        self.consume_token(Token::User)?;
        let username = self.consume_identifier()?;

        self.consume_token(Token::With)?;
        self.consume_token(Token::Password)?;
        let new_password = self.consume_string_literal()?;

        Ok(Statement::Alter(AlterStatement::User(AlterUserStatement {
            username,
            new_password: Some(new_password),
        })))
    }

    /// Parse ALTER operations: ADD/DROP/CHANGE (...)
    ///
    /// Syntax:
    ///   ADD (col TYPE [NULL] [DEFAULT val], ...)
    ///   DROP (col, ...)
    ///   CHANGE (col NEW_TYPE [NULL] [DEFAULT val], ...)
    fn parse_alter_operations(&mut self) -> Result<Vec<AlterOperation>> {
        let mut operations = Vec::new();

        let op_token = self.peek_token()?;
        match op_token {
            Token::Add => {
                self.advance();
                self.consume_token(Token::LParen)?;
                loop {
                    if self.peek_token()? == Token::RParen {
                        break;
                    }
                    let name = self.consume_identifier()?;
                    let data_type = self.parse_data_type()?;
                    let nullable = if self.match_token(Token::Not) {
                        self.consume_token(Token::Null)?;
                        false
                    } else {
                        self.match_token(Token::Null);
                        true
                    };
                    let default = if self.match_token(Token::Default) {
                        Some(self.parse_expression()?)
                    } else {
                        None
                    };
                    if !nullable && default.is_none() {
                        return Err(ParseError::InvalidSyntax(format!(
                            "Column '{}' is NOT NULL but has no DEFAULT value",
                            name
                        )));
                    }
                    operations.push(AlterOperation::AddColumn(PropertySpec {
                        name,
                        data_type,
                        nullable,
                        default,
                    }));
                    if !self.match_token(Token::Comma) {
                        break;
                    }
                }
                self.consume_token(Token::RParen)?;
            }
            Token::Drop => {
                self.advance();
                self.consume_token(Token::LParen)?;
                loop {
                    if self.peek_token()? == Token::RParen {
                        break;
                    }
                    let name = self.consume_identifier()?;
                    operations.push(AlterOperation::DropColumn(name));
                    if !self.match_token(Token::Comma) {
                        break;
                    }
                }
                self.consume_token(Token::RParen)?;
            }
            Token::Change => {
                self.advance();
                self.consume_token(Token::LParen)?;
                loop {
                    if self.peek_token()? == Token::RParen {
                        break;
                    }
                    let name = self.consume_identifier()?;
                    let data_type = self.parse_data_type()?;
                    let nullable = if self.match_token(Token::Not) {
                        self.consume_token(Token::Null)?;
                        false
                    } else {
                        self.match_token(Token::Null);
                        true
                    };
                    let default = if self.match_token(Token::Default) {
                        Some(self.parse_expression()?)
                    } else {
                        None
                    };
                    operations.push(AlterOperation::ChangeColumn(PropertySpec {
                        name,
                        data_type,
                        nullable,
                        default,
                    }));
                    if !self.match_token(Token::Comma) {
                        break;
                    }
                }
                self.consume_token(Token::RParen)?;
            }
            _ => {
                return Err(ParseError::InvalidSyntax(format!(
                    "Expected ADD, DROP, or CHANGE after ALTER TAG/EDGE, got {:?}",
                    op_token
                )));
            }
        }

        Ok(operations)
    }

    // ===== DROP statements =====

    pub(crate) fn parse_drop(&mut self) -> ParseResult {
        self.consume_token(Token::Drop)?;

        let token = self.peek_token()?;
        match token {
            Token::Space => self.parse_drop_space(),
            Token::Tag => self.parse_drop_tag(),
            Token::Edge => self.parse_drop_edge(),
            Token::Index => self.parse_drop_index(),
            Token::User => self.parse_drop_user(),
            _ => Err(ParseError::UnexpectedToken(format!("{:?}", token))),
        }
    }

    fn parse_drop_space(&mut self) -> ParseResult {
        self.consume_token(Token::Space)?;
        let if_exists = self.parse_if_exists()?;
        let name = self.consume_identifier()?;

        Ok(Statement::Drop(DropStatement::Space(DropSpaceStatement {
            if_exists,
            name,
        })))
    }

    fn parse_drop_tag(&mut self) -> ParseResult {
        self.consume_token(Token::Tag)?;
        let if_exists = self.parse_if_exists()?;
        let name = self.consume_identifier()?;

        Ok(Statement::Drop(DropStatement::Tag(DropTagStatement {
            if_exists,
            name,
        })))
    }

    fn parse_drop_edge(&mut self) -> ParseResult {
        self.consume_token(Token::Edge)?;
        let if_exists = self.parse_if_exists()?;
        let name = self.consume_identifier()?;

        Ok(Statement::Drop(DropStatement::Edge(DropEdgeStatement {
            if_exists,
            name,
        })))
    }

    fn parse_drop_index(&mut self) -> ParseResult {
        self.consume_token(Token::Index)?;

        let token = self.peek_token()?;
        match token {
            Token::Tag => self.parse_drop_tag_index(),
            Token::Edge => self.parse_drop_edge_index(),
            _ => Err(ParseError::UnexpectedToken(format!("{:?}", token))),
        }
    }

    fn parse_drop_tag_index(&mut self) -> ParseResult {
        self.consume_token(Token::Tag)?;
        let if_exists = self.parse_if_exists()?;
        let index_name = self.consume_identifier()?;

        Ok(Statement::Drop(DropStatement::TagIndex(
            DropTagIndexStatement {
                if_exists,
                index_name,
            },
        )))
    }

    fn parse_drop_edge_index(&mut self) -> ParseResult {
        self.consume_token(Token::Edge)?;
        let if_exists = self.parse_if_exists()?;
        let index_name = self.consume_identifier()?;

        Ok(Statement::Drop(DropStatement::EdgeIndex(
            DropEdgeIndexStatement {
                if_exists,
                index_name,
            },
        )))
    }

    fn parse_drop_user(&mut self) -> ParseResult {
        self.consume_token(Token::User)?;
        let if_exists = self.parse_if_exists()?;
        let username = self.consume_identifier()?;

        Ok(Statement::Drop(DropStatement::User(DropUserStatement {
            if_exists,
            username,
        })))
    }

    // ===== GRANT/REVOKE statements =====

    /// Parse: GRANT ROLE role TO user
    pub(crate) fn parse_grant(&mut self) -> ParseResult {
        self.consume_token(Token::Grant)?;
        self.consume_token(Token::Role)?;
        let role = self.consume_identifier()?;
        self.consume_token(Token::To)?;
        let username = self.consume_identifier()?;

        Ok(Statement::Grant(GrantStatement { role, username }))
    }

    /// Parse: REVOKE ROLE role FROM user
    pub(crate) fn parse_revoke(&mut self) -> ParseResult {
        self.consume_token(Token::Revoke)?;
        self.consume_token(Token::Role)?;
        let role = self.consume_identifier()?;
        self.consume_token(Token::From)?;
        let username = self.consume_identifier()?;

        Ok(Statement::Revoke(RevokeStatement { role, username }))
    }

    // ===== BALANCE statements =====

    /// Parse: BALANCE LEADER | DATA | STATUS | STOP | RESET
    pub(crate) fn parse_balance(&mut self) -> ParseResult {
        self.consume_token(Token::Balance)?;

        let token = self.peek_token()?;
        let stmt = match token {
            Token::Leader => {
                self.advance();
                BalanceStatement::Leader
            }
            Token::Data => {
                self.advance();
                BalanceStatement::Data
            }
            Token::Status => {
                self.advance();
                BalanceStatement::Status
            }
            Token::Stop => {
                self.advance();
                BalanceStatement::Stop
            }
            Token::Reset => {
                self.advance();
                BalanceStatement::Reset
            }
            _ => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected LEADER, DATA, STATUS, STOP, or RESET, found {:?}",
                    token
                )))
            }
        };

        Ok(Statement::Balance(stmt))
    }

    // ===== Helper methods =====

    pub(crate) fn parse_if_not_exists(&mut self) -> Result<bool> {
        if self.match_token(Token::If) {
            self.consume_token(Token::Not)?;
            self.consume_token(Token::Exists)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(crate) fn parse_if_exists(&mut self) -> Result<bool> {
        if self.match_token(Token::If) {
            self.consume_token(Token::Exists)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
