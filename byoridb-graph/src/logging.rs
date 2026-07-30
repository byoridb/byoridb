// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Logging configuration for ByoriDB
//!
//! This module provides structured logging with:
//! - JSON format support
//! - Slow query logging
//! - Error stack traces
//! - Configurable log levels

use tracing::Level;
// `init_logging` (and its tracing-subscriber dependency) is a process-level
// server concern. Embedders configure their own subscriber, so it's gated
// behind `server`. LogConfig / QueryLogger / log_query! stay always-on.
#[cfg(feature = "server")]
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

/// Logging configuration
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: Level,
    /// Enable JSON format
    pub json_format: bool,
    /// Slow query threshold in milliseconds
    pub slow_query_threshold_ms: u64,
    /// Log to file path (None for stdout only)
    pub log_file: Option<String>,
    /// Include span events
    pub with_spans: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: Level::INFO,
            json_format: false,
            slow_query_threshold_ms: 1000,
            log_file: None,
            with_spans: false,
        }
    }
}

impl LogConfig {
    /// Create a new config with info level
    pub fn new() -> Self {
        Self::default()
    }

    /// Set log level
    pub fn with_level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    /// Enable JSON format
    pub fn with_json(mut self) -> Self {
        self.json_format = true;
        self
    }

    /// Set slow query threshold
    pub fn with_slow_query_threshold(mut self, ms: u64) -> Self {
        self.slow_query_threshold_ms = ms;
        self
    }

    /// Enable span events
    pub fn with_spans(mut self) -> Self {
        self.with_spans = true;
        self
    }
}

/// Initialize logging with the given configuration
#[cfg(feature = "server")]
pub fn init_logging(config: &LogConfig) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "byoridb={},byoridb_graph={},byoridb_executor={}",
            config.level, config.level, config.level
        ))
    });

    let span_events = if config.with_spans {
        FmtSpan::CLOSE
    } else {
        FmtSpan::NONE
    };

    if config.json_format {
        // JSON format for production
        let subscriber = tracing_subscriber::registry().with(filter).with(
            fmt::layer()
                .json()
                .with_span_events(span_events)
                .with_current_span(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true),
        );

        subscriber.init();
    } else {
        // Pretty format for development
        let subscriber = tracing_subscriber::registry().with(filter).with(
            fmt::layer()
                .with_span_events(span_events)
                .with_target(true)
                .with_thread_ids(false),
        );

        subscriber.init();
    }
}

/// Return a bounded statement category without retaining or reproducing query
/// text. Unknown/malformed input is deliberately collapsed to `unknown`.
pub fn safe_statement_type(query: &str) -> &'static str {
    let first = query.split_ascii_whitespace().next().unwrap_or_default();
    if first.eq_ignore_ascii_case("SHOW") {
        "show"
    } else if first.eq_ignore_ascii_case("USE") {
        "use"
    } else if first.eq_ignore_ascii_case("CREATE") {
        "create"
    } else if first.eq_ignore_ascii_case("ALTER") {
        "alter"
    } else if first.eq_ignore_ascii_case("DROP") {
        "drop"
    } else if first.eq_ignore_ascii_case("INSERT") {
        "insert"
    } else if first.eq_ignore_ascii_case("UPDATE") {
        "update"
    } else if first.eq_ignore_ascii_case("DELETE") {
        "delete"
    } else if first.eq_ignore_ascii_case("FETCH") {
        "fetch"
    } else if first.eq_ignore_ascii_case("GO") {
        "go"
    } else if first.eq_ignore_ascii_case("MATCH") {
        "match"
    } else if first.eq_ignore_ascii_case("LOOKUP") {
        "lookup"
    } else if first.eq_ignore_ascii_case("FIND") {
        "find"
    } else if first.eq_ignore_ascii_case("RECOMMEND") {
        "recommend"
    } else if first.eq_ignore_ascii_case("GRANT") {
        "grant"
    } else if first.eq_ignore_ascii_case("REVOKE") {
        "revoke"
    } else if first.eq_ignore_ascii_case("EXPLAIN") {
        "explain"
    } else if first.eq_ignore_ascii_case("PROFILE") {
        "profile"
    } else {
        "unknown"
    }
}

/// Log a query execution using metadata only.
#[macro_export]
macro_rules! log_query {
    ($query:expr, $space:expr, $latency_ms:expr) => {
        tracing::info!(
            query_type = $crate::logging::safe_statement_type($query),
            query_length_bytes = $query.len(),
            space = $space,
            latency_ms = $latency_ms,
            "Query executed"
        );
    };
}

/// Log a slow query using metadata only.
#[macro_export]
macro_rules! log_slow_query {
    ($query:expr, $space:expr, $latency_ms:expr, $threshold_ms:expr) => {
        tracing::warn!(
            query_type = $crate::logging::safe_statement_type($query),
            query_length_bytes = $query.len(),
            space = $space,
            latency_ms = $latency_ms,
            threshold_ms = $threshold_ms,
            "Slow query detected"
        );
    };
}

/// Log an error with context
#[macro_export]
macro_rules! log_error {
    ($error:expr, $context:expr) => {
        tracing::error!("Error occurred");
    };
}

/// Query logger that tracks execution and logs appropriately
pub struct QueryLogger {
    query_type: &'static str,
    query_length_bytes: usize,
    space: String,
    start: std::time::Instant,
    slow_threshold_ms: u64,
}

impl QueryLogger {
    /// Create a new query logger
    pub fn new(query: &str, space: &str, slow_threshold_ms: u64) -> Self {
        let query_type = safe_statement_type(query);
        let query_length_bytes = query.len();
        tracing::debug!(
            query_type = query_type,
            query_length_bytes = query_length_bytes,
            space = space,
            "Query started"
        );

        Self {
            query_type,
            query_length_bytes,
            space: space.to_string(),
            start: std::time::Instant::now(),
            slow_threshold_ms,
        }
    }

    /// Log successful completion
    pub fn success(self) {
        let latency_ms = self.start.elapsed().as_millis() as u64;

        if latency_ms > self.slow_threshold_ms {
            tracing::warn!(
                query_type = self.query_type,
                query_length_bytes = self.query_length_bytes,
                space = %self.space,
                latency_ms = latency_ms,
                threshold_ms = self.slow_threshold_ms,
                "Slow query detected"
            );
        } else {
            tracing::info!(
                query_type = self.query_type,
                query_length_bytes = self.query_length_bytes,
                space = %self.space,
                latency_ms = latency_ms,
                "Query completed"
            );
        }
    }

    /// Log error completion
    pub fn error(self, _err: &dyn std::error::Error) {
        let latency_ms = self.start.elapsed().as_millis() as u64;

        tracing::error!(
            query_type = self.query_type,
            query_length_bytes = self.query_length_bytes,
            space = %self.space,
            latency_ms = latency_ms,
            "Query failed"
        );
    }
}

/// Structured log entry for query execution
#[derive(Debug, serde::Serialize)]
pub struct QueryLogEntry {
    pub timestamp: String,
    /// Retained for source/serialization compatibility. This field is always a
    /// constant marker and never contains the submitted statement.
    pub query: String,
    pub query_type: &'static str,
    pub query_length_bytes: usize,
    pub space: String,
    pub latency_ms: u64,
    pub status: QueryStatus,
    pub error: Option<String>,
    pub rows_affected: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub enum QueryStatus {
    Success,
    Error,
    Slow,
}

impl QueryLogEntry {
    /// Create a success entry
    pub fn success(query: &str, space: &str, latency_ms: u64, rows: Option<u64>) -> Self {
        Self {
            timestamp: chrono_timestamp(),
            query: "<redacted>".to_string(),
            query_type: safe_statement_type(query),
            query_length_bytes: query.len(),
            space: space.to_string(),
            latency_ms,
            status: QueryStatus::Success,
            error: None,
            rows_affected: rows,
        }
    }

    /// Create an error entry
    pub fn error(query: &str, space: &str, latency_ms: u64, _error: &str) -> Self {
        Self {
            timestamp: chrono_timestamp(),
            query: "<redacted>".to_string(),
            query_type: safe_statement_type(query),
            query_length_bytes: query.len(),
            space: space.to_string(),
            latency_ms,
            status: QueryStatus::Error,
            error: Some("query_error".to_string()),
            rows_affected: None,
        }
    }

    /// Create a slow query entry
    pub fn slow(query: &str, space: &str, latency_ms: u64, rows: Option<u64>) -> Self {
        Self {
            timestamp: chrono_timestamp(),
            query: "<redacted>".to_string(),
            query_type: safe_statement_type(query),
            query_length_bytes: query.len(),
            space: space.to_string(),
            latency_ms,
            status: QueryStatus::Slow,
            error: None,
            rows_affected: rows,
        }
    }

    /// Log this entry
    pub fn log(&self) {
        match self.status {
            QueryStatus::Success => {
                tracing::info!(
                    query_type = self.query_type,
                    query_length_bytes = self.query_length_bytes,
                    space = %self.space,
                    latency_ms = self.latency_ms,
                    rows = ?self.rows_affected,
                    "Query completed"
                );
            }
            QueryStatus::Error => {
                tracing::error!(
                    query_type = self.query_type,
                    query_length_bytes = self.query_length_bytes,
                    space = %self.space,
                    latency_ms = self.latency_ms,
                    error = ?self.error,
                    "Query failed"
                );
            }
            QueryStatus::Slow => {
                tracing::warn!(
                    query_type = self.query_type,
                    query_length_bytes = self.query_length_bytes,
                    space = %self.space,
                    latency_ms = self.latency_ms,
                    rows = ?self.rows_affected,
                    "Slow query"
                );
            }
        }
    }
}

/// Get current timestamp as ISO 8601 string
fn chrono_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    let secs = duration.as_secs();
    let nanos = duration.subsec_nanos();

    // Simple ISO 8601 format without external dependency
    format!("{}.{:09}Z", secs, nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_config_default() {
        let config = LogConfig::default();
        assert_eq!(config.level, Level::INFO);
        assert!(!config.json_format);
        assert_eq!(config.slow_query_threshold_ms, 1000);
    }

    #[test]
    fn test_log_config_builder() {
        let config = LogConfig::new()
            .with_level(Level::DEBUG)
            .with_json()
            .with_slow_query_threshold(500);

        assert_eq!(config.level, Level::DEBUG);
        assert!(config.json_format);
        assert_eq!(config.slow_query_threshold_ms, 500);
    }

    #[test]
    fn test_query_log_entry() {
        let query = "CREATE USER private@example.com WITH PASSWORD \"secret-value\"";
        let entry = QueryLogEntry::success(query, "default", 50, Some(5));
        assert_eq!(entry.space, "default");
        assert_eq!(entry.query, "<redacted>");
        assert_eq!(entry.query_type, "create");
        assert_eq!(entry.query_length_bytes, query.len());
        assert_eq!(entry.latency_ms, 50);
        assert!(matches!(entry.status, QueryStatus::Success));
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("private@example.com"));
        assert!(!json.contains("secret-value"));
    }

    #[test]
    fn safe_statement_type_is_bounded() {
        assert_eq!(safe_statement_type("INSERT VERTEX person"), "insert");
        assert_eq!(safe_statement_type("private@example.com secret"), "unknown");
    }
}
