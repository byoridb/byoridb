// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Lexical analysis for nGQL

use logos::Logos;

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r\n\f]+")]
pub enum Token {
    // Keywords (case-insensitive)
    #[token("CREATE", ignore(case))]
    Create,
    #[token("DROP", ignore(case))]
    Drop,
    #[token("ALTER", ignore(case))]
    Alter,
    #[token("ADD", ignore(case))]
    Add,
    #[token("CHANGE", ignore(case))]
    Change,
    #[token("DESCRIBE", ignore(case))]
    Describe,
    #[token("DESC", ignore(case))]
    Desc,
    #[token("SHOW", ignore(case))]
    Show,
    #[token("USE", ignore(case))]
    Use,
    #[token("SPACE", ignore(case))]
    Space,
    #[token("SPACES", ignore(case))]
    Spaces,
    #[token("TAG", ignore(case))]
    Tag,
    #[token("TAGS", ignore(case))]
    Tags,
    #[token("EDGE", ignore(case))]
    Edge,
    #[token("EDGES", ignore(case))]
    Edges,
    #[token("INDEX", ignore(case))]
    Index,
    #[token("REBUILD", ignore(case))]
    Rebuild,

    // Data manipulation
    #[token("INSERT", ignore(case))]
    Insert,
    #[token("UPDATE", ignore(case))]
    Update,
    #[token("DELETE", ignore(case))]
    Delete,
    #[token("UPSERT", ignore(case))]
    Upsert,
    #[token("FETCH", ignore(case))]
    Fetch,
    #[token("FIND", ignore(case))]
    Find,
    #[token("LOOKUP", ignore(case))]
    Lookup,
    #[token("RECOMMEND", ignore(case))]
    Recommend,
    #[token("SIMILAR", ignore(case))]
    Similar,
    #[token("EMBEDDING", ignore(case))]
    Embedding,
    #[token("BLEND", ignore(case))]
    Blend,
    // Ontology semantic relation types (PLAN.md O-4)
    #[token("TRANSITIVE", ignore(case))]
    Transitive,
    #[token("SYMMETRIC", ignore(case))]
    Symmetric,
    #[token("INVERSE", ignore(case))]
    Inverse,
    #[token("SUBPROPERTY", ignore(case))]
    Subproperty,
    #[token("DOMAIN", ignore(case))]
    Domain,
    #[token("RANGE", ignore(case))]
    Range,
    #[token("DISJOINT", ignore(case))]
    Disjoint,
    #[token("CHECK", ignore(case))]
    Check,
    #[token("CONSISTENCY", ignore(case))]
    Consistency,
    #[token("MATCH", ignore(case))]
    Match,
    #[token("GO", ignore(case))]
    Go,
    #[token("VERTEX", ignore(case))]
    Vertex,
    #[token("VALUES", ignore(case))]
    Values,
    #[token("PROP", ignore(case))]
    Prop,
    #[token("STEPS", ignore(case))]
    Steps,
    #[token("STEP", ignore(case))]
    Step,
    #[token("PATHS", ignore(case))]
    Paths,
    #[token("CLASSES", ignore(case))]
    Classes,
    #[token("CLASS", ignore(case))]
    Class,
    #[token("SUBCLASS", ignore(case))]
    Subclass,
    #[token("OF", ignore(case))]
    Of,
    #[token("PATH", ignore(case))]
    Path,
    #[token("SHORTEST", ignore(case))]
    Shortest,
    #[token("UPTO", ignore(case))]
    Upto,
    #[token("WEIGHT", ignore(case))]
    Weight,
    #[token("ALL", ignore(case))]
    All,
    #[token("NOLOOP", ignore(case))]
    Noloop,
    #[token("REVERSELY", ignore(case))]
    Reversely,
    #[token("BIDIRECT", ignore(case))]
    Bidirect,

    // Clauses
    #[token("FROM", ignore(case))]
    From,
    #[token("TO", ignore(case))]
    To,
    #[token("OVER", ignore(case))]
    Over,
    #[token("WHERE", ignore(case))]
    Where,
    #[token("WHEN", ignore(case))]
    When,
    #[token("YIELD", ignore(case))]
    Yield,
    #[token("RETURN", ignore(case))]
    Return,
    #[token("WITH", ignore(case))]
    With,
    #[token("INDEXES", ignore(case))]
    Indexes,
    #[token("CONTAINS", ignore(case))]
    Contains,
    #[token("STARTS", ignore(case))]
    Starts,
    #[token("ENDS", ignore(case))]
    Ends,
    #[token("SET", ignore(case))]
    Set,
    #[token("ORDER", ignore(case))]
    Order,
    #[token("BY", ignore(case))]
    By,
    #[token("LIMIT", ignore(case))]
    Limit,
    #[token("OFFSET", ignore(case))]
    #[token("SKIP", ignore(case))]
    Offset,
    #[token("GROUP", ignore(case))]
    Group,
    #[token("AS", ignore(case))]
    As,
    #[token("STATS", ignore(case))]
    Stats,
    #[token("SESSIONS", ignore(case))]
    Sessions,
    #[token("EXPLAIN", ignore(case))]
    Explain,
    #[token("PROFILE", ignore(case))]
    Profile,
    #[token("OPTIONAL", ignore(case))]
    Optional,
    #[token("=~")]
    RegexMatch,

    // Types (case-insensitive)
    #[token("BOOL", ignore(case))]
    Bool,
    #[token("INT8", ignore(case))]
    Int8,
    #[token("INT16", ignore(case))]
    Int16,
    #[token("INT32", ignore(case))]
    Int32,
    // `INT` is accepted as an alias for `INT64` (NebulaGraph compatibility).
    #[token("INT64", ignore(case))]
    #[token("INT", ignore(case))]
    Int64,
    #[token("FLOAT", ignore(case))]
    Float,
    #[token("DOUBLE", ignore(case))]
    Double,
    #[token("STRING", ignore(case))]
    String,
    #[token("FIXED_STRING", ignore(case))]
    FixedString,
    #[token("TIMESTAMP", ignore(case))]
    Timestamp,
    #[token("DATE", ignore(case))]
    Date,
    #[token("TIME", ignore(case))]
    Time,
    #[token("DATETIME", ignore(case))]
    DateTime,
    #[token("GEOGRAPHY", ignore(case))]
    Geography,
    #[token("DURATION", ignore(case))]
    Duration,

    // Partition
    #[token("PARTITION", ignore(case))]
    Partition,
    #[token("PARTS", ignore(case))]
    Parts,
    #[token("HASH", ignore(case))]
    Hash,
    #[token("MODULO", ignore(case))]
    Modulo,

    // Admin commands
    #[token("HOSTS", ignore(case))]
    Hosts,
    #[token("BALANCE", ignore(case))]
    Balance,
    #[token("LEADER", ignore(case))]
    Leader,
    #[token("DATA", ignore(case))]
    Data,
    #[token("STATUS", ignore(case))]
    Status,
    #[token("STOP", ignore(case))]
    Stop,
    #[token("RESET", ignore(case))]
    Reset,

    // Properties
    #[token("NULL", ignore(case))]
    Null,
    #[token("NOT", ignore(case))]
    Not,
    #[token("IF", ignore(case))]
    If,
    #[token("EXISTS", ignore(case))]
    Exists,
    #[token("DEFAULT", ignore(case))]
    Default,
    #[token("UNIQUE", ignore(case))]
    Unique,
    #[token("TTL", ignore(case))]
    Ttl,
    #[token("COLLATE", ignore(case))]
    Collate,

    // User management
    #[token("USER", ignore(case))]
    User,
    #[token("PASSWORD", ignore(case))]
    Password,
    #[token("ROLE", ignore(case))]
    Role,
    #[token("GOD", ignore(case))]
    God,
    #[token("ADMIN", ignore(case))]
    Admin,
    #[token("DBA", ignore(case))]
    Dba,
    #[token("GUEST", ignore(case))]
    Guest,
    #[token("GRANT", ignore(case))]
    Grant,
    #[token("REVOKE", ignore(case))]
    Revoke,
    #[token("ON", ignore(case))]
    On,

    // Operators
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("=")]
    Eq,
    #[token("==")]
    EqEq,
    #[token("!=")]
    Neq,
    #[token("<")]
    Lt,
    #[token("<=")]
    Lte,
    #[token(">")]
    Gt,
    #[token(">=")]
    Gte,
    #[token("&&")]
    #[token("AND", ignore(case))]
    And,
    #[token("||")]
    #[token("OR", ignore(case))]
    Or,
    #[token("!")]
    NotOp,

    // Punctuation
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token(",")]
    Comma,
    #[token(".")]
    Dot,
    #[token(":")]
    Colon,
    #[token(";")]
    SemiColon,
    #[token("->")]
    Arrow,
    #[token("<-")]
    ReverseArrow,
    #[token("@")]
    At,
    /// `$` introduces a variable reference in compound statements:
    /// `$var = GO FROM 1 OVER e; GO FROM $var.dst OVER e`.
    #[token("$")]
    Dollar,

    // Literals
    // `\"`/`\\`/`\n` etc. are accepted inside the quotes (the `\\.` alternative),
    // so a quote or backslash can appear in the value. The raw slice (quotes +
    // escapes intact) is stored; `parser::unquote` strips quotes and interprets
    // the escapes. Without `\\.`, a value containing the delimiter quote or a
    // backslash truncated the token (the LDBC-only/integer-VID blind spot).
    #[regex(r#""([^"\\]|\\.)*""#, |lex| lex.slice().to_string())]
    StringLiteral(std::string::String),
    #[regex(r"'([^'\\]|\\.)*'", |lex| lex.slice().to_string())]
    SingleQuotedString(std::string::String),
    #[regex(r"-?[0-9]+", |lex| lex.slice().parse().ok(), priority = 2)]
    Integer(i64),
    #[regex(r"-?[0-9]+\.[0-9]+", |lex| lex.slice().parse().ok())]
    FloatLiteral(f64),
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Identifier(std::string::String),

    // Boolean (case-insensitive)
    #[token("true", ignore(case))]
    True,
    #[token("false", ignore(case))]
    False,

    // Comments
    #[regex(r"//.*", logos::skip)]
    #[regex(r"/\*[^*]*\*+(?:[^/*][^*]*\*+)*/", logos::skip)]
    Comment,
}

/// Token with location information.
///
/// `line` and `column` are 1-based. `column` is the byte offset within the
/// line (plus one), not the number of Unicode graphemes — sufficient for
/// error reporting on ASCII-heavy nGQL sources.
#[derive(Debug, Clone)]
pub struct LocatedToken {
    pub token: Token,
    pub line: usize,
    pub column: usize,
}

/// Lexer for nGQL
pub struct Lexer {
    input: std::string::String,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Lexer {
            input: input.to_string(),
        }
    }

    pub fn tokenize(&self) -> super::error::Result<Vec<LocatedToken>> {
        let line_starts = compute_line_starts(&self.input);
        let mut tokens = Vec::new();
        let mut lex = Token::lexer(&self.input);

        while let Some(token) = lex.next() {
            match token {
                Ok(Token::Comment) => continue,
                Ok(t) => {
                    let span = lex.span();
                    let (line, column) = locate(&line_starts, span.start);
                    tokens.push(LocatedToken {
                        token: t,
                        line,
                        column,
                    });
                }
                Err(_) => {
                    let span = lex.span();
                    let error_msg = self.input[span.start..span.end].to_string();
                    let (line, column) = locate(&line_starts, span.start);
                    return Err(super::ParseError::LexerError(format!(
                        "Unknown token at line {}, column {} ({}..{}): {}",
                        line, column, span.start, span.end, error_msg
                    )));
                }
            }
        }

        Ok(tokens)
    }
}

/// Precompute the byte offset of the start of each line in `input`.
///
/// Index 0 is always `0`. Each subsequent entry is the offset just after a
/// `\n`, so a byte at offset `b` belongs to the line whose index is the
/// largest `i` with `line_starts[i] <= b`.
fn compute_line_starts(input: &str) -> Vec<usize> {
    let mut starts = Vec::with_capacity(16);
    starts.push(0);
    for (i, byte) in input.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// Resolve a byte offset into a 1-based (line, column) tuple using a table
/// built by [`compute_line_starts`].
fn locate(line_starts: &[usize], offset: usize) -> (usize, usize) {
    // partition_point returns the index of the first line_start > offset,
    // which equals the 1-based line number.
    let line = line_starts.partition_point(|&s| s <= offset);
    let line_start = line_starts.get(line - 1).copied().unwrap_or(0);
    let column = offset - line_start + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keywords() {
        let lexer = Lexer::new("CREATE TAG IF NOT EXISTS");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens.len(), 5);
        assert_eq!(tokens[0].token, Token::Create);
        assert_eq!(tokens[1].token, Token::Tag);
    }

    #[test]
    fn test_case_insensitive() {
        let lexer = Lexer::new("create tag Show SPACES");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0].token, Token::Create);
        assert_eq!(tokens[1].token, Token::Tag);
        assert_eq!(tokens[2].token, Token::Show);
        assert_eq!(tokens[3].token, Token::Spaces);
    }

    #[test]
    fn test_literals() {
        let lexer = Lexer::new("123 45.67 \"hello\" 'world'");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0].token, Token::Integer(123));
        assert_eq!(tokens[1].token, Token::FloatLiteral(45.67));
        assert_eq!(
            tokens[2].token,
            Token::StringLiteral("\"hello\"".to_string())
        );
        assert_eq!(
            tokens[3].token,
            Token::SingleQuotedString("'world'".to_string())
        );
    }

    #[test]
    fn test_string_with_escapes_stays_one_token() {
        // A double-quoted value containing an escaped quote, a backslash, and a
        // bare single quote must remain a SINGLE token (the old `[^"]*` regex
        // truncated at the inner quote/backslash — the dogfooding gap).
        let lexer = Lexer::new(r#""a\"b\\c 'x'""#);
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens.len(), 1, "escapes/quotes must not split the token");
        assert_eq!(
            tokens[0].token,
            Token::StringLiteral(r#""a\"b\\c 'x'""#.to_string())
        );
        // Symmetric case for single-quoted strings with an escaped quote.
        let tokens = Lexer::new(r"'it\'s'").tokenize().unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].token,
            Token::SingleQuotedString(r"'it\'s'".to_string())
        );
    }

    #[test]
    fn test_dollar_token_recognized() {
        let lexer = Lexer::new("$var.dst");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0].token, Token::Dollar);
        assert_eq!(tokens[1].token, Token::Identifier("var".to_string()));
        assert_eq!(tokens[2].token, Token::Dot);
        assert_eq!(tokens[3].token, Token::Identifier("dst".to_string()));
    }

    #[test]
    fn test_line_column_single_line() {
        let lexer = Lexer::new("CREATE TAG player");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0].line, 1);
        assert_eq!(tokens[0].column, 1);
        assert_eq!(tokens[1].line, 1);
        assert_eq!(tokens[1].column, 8); // "CREATE " is 7 bytes, next col is 8
        assert_eq!(tokens[2].line, 1);
        assert_eq!(tokens[2].column, 12);
    }

    #[test]
    fn test_line_column_multi_line() {
        let lexer = Lexer::new("CREATE\nTAG\n  player");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!((tokens[0].line, tokens[0].column), (1, 1)); // CREATE
        assert_eq!((tokens[1].line, tokens[1].column), (2, 1)); // TAG
        assert_eq!((tokens[2].line, tokens[2].column), (3, 3)); // player (after "  ")
    }

    #[test]
    fn test_line_column_lexer_error_mentions_position() {
        // Backtick is not a valid token — error should reference a line/col.
        let lexer = Lexer::new("CREATE\nTAG `bad`");
        let err = lexer.tokenize().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("line 2"),
            "expected 'line 2' in error, got: {}",
            msg
        );
    }

    #[test]
    fn test_compute_line_starts_basic() {
        assert_eq!(compute_line_starts(""), vec![0]);
        assert_eq!(compute_line_starts("abc"), vec![0]);
        assert_eq!(compute_line_starts("a\nb"), vec![0, 2]);
        assert_eq!(compute_line_starts("\n\n"), vec![0, 1, 2]);
    }

    #[test]
    fn test_locate_returns_one_based() {
        let starts = vec![0usize, 5, 10];
        assert_eq!(locate(&starts, 0), (1, 1));
        assert_eq!(locate(&starts, 4), (1, 5));
        assert_eq!(locate(&starts, 5), (2, 1));
        assert_eq!(locate(&starts, 9), (2, 5));
        assert_eq!(locate(&starts, 10), (3, 1));
    }
}
