// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Metrics collection for ByoriDB
//!
//! This module provides metrics collection using the `metrics` crate:
//! - Query counters (total, by type)
//! - Query latency histograms
//! - Error counters
//! - Active connections gauge
//! - Storage metrics

use metrics::{counter, gauge, histogram};
// describe_* are only used by init_metrics (server-only).
#[cfg(feature = "server")]
use metrics::{describe_counter, describe_gauge, describe_histogram};
// The Prometheus exporter is server-only. The `metrics` facade above stays
// always-on because the in-process execute path (QueryTimer) records through it.
#[cfg(feature = "server")]
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
#[cfg(feature = "server")]
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Global Prometheus handle for metrics export
#[cfg(feature = "server")]
static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Metric names
pub mod metric_names {
    pub const QUERY_TOTAL: &str = "byoridb_query_total";
    pub const QUERY_LATENCY: &str = "byoridb_query_latency_seconds";
    pub const QUERY_ERRORS: &str = "byoridb_query_errors_total";
    pub const ACTIVE_CONNECTIONS: &str = "byoridb_active_connections";
    pub const ACTIVE_SESSIONS: &str = "byoridb_active_sessions";
    pub const STORAGE_BYTES: &str = "byoridb_storage_bytes";
    pub const SLOW_QUERIES: &str = "byoridb_slow_queries_total";

    // Partition metrics
    pub const PARTITION_REQUESTS: &str = "byoridb_partition_requests_total";
    pub const PARTITION_HOTSPOT_RATIO: &str = "byoridb_partition_hotspot_ratio";
    pub const PARTITION_COUNT: &str = "byoridb_partition_count";
    pub const PARTITION_LEADER_COUNT: &str = "byoridb_partition_leader_count";
}

/// Query types for labeling
#[derive(Debug, Clone, Copy)]
pub enum QueryType {
    Show,
    Use,
    Create,
    Alter,
    Drop,
    Insert,
    Update,
    Delete,
    Fetch,
    Go,
    Match,
    Lookup,
    Find,
    Unknown,
}

impl QueryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            QueryType::Show => "show",
            QueryType::Use => "use",
            QueryType::Create => "create",
            QueryType::Alter => "alter",
            QueryType::Drop => "drop",
            QueryType::Insert => "insert",
            QueryType::Update => "update",
            QueryType::Delete => "delete",
            QueryType::Fetch => "fetch",
            QueryType::Go => "go",
            QueryType::Match => "match",
            QueryType::Lookup => "lookup",
            QueryType::Find => "find",
            QueryType::Unknown => "unknown",
        }
    }
}

/// Initialize the metrics system
#[cfg(feature = "server")]
pub fn init_metrics() -> PrometheusHandle {
    let builder = PrometheusBuilder::new();
    let handle = builder
        .install_recorder()
        .expect("Failed to install Prometheus recorder");

    // Describe all metrics
    describe_counter!(
        metric_names::QUERY_TOTAL,
        "Total number of queries executed"
    );
    describe_histogram!(
        metric_names::QUERY_LATENCY,
        "Query execution latency in seconds"
    );
    describe_counter!(metric_names::QUERY_ERRORS, "Total number of query errors");
    describe_gauge!(
        metric_names::ACTIVE_CONNECTIONS,
        "Number of active connections"
    );
    describe_gauge!(metric_names::ACTIVE_SESSIONS, "Number of active sessions");
    describe_gauge!(metric_names::STORAGE_BYTES, "Total storage size in bytes");
    describe_counter!(metric_names::SLOW_QUERIES, "Total number of slow queries");

    // Partition metrics
    describe_counter!(
        metric_names::PARTITION_REQUESTS,
        "Total partition requests by space, partition, and operation type"
    );
    describe_gauge!(
        metric_names::PARTITION_HOTSPOT_RATIO,
        "Ratio of partition QPS to average QPS (>1 indicates hot partition)"
    );
    describe_gauge!(
        metric_names::PARTITION_COUNT,
        "Total number of partitions per space"
    );
    describe_gauge!(
        metric_names::PARTITION_LEADER_COUNT,
        "Number of partition leaders per host"
    );

    // Store globally
    let _ = PROMETHEUS_HANDLE.set(handle.clone());

    handle
}

/// Get the Prometheus handle for rendering metrics
#[cfg(feature = "server")]
pub fn get_prometheus_handle() -> Option<&'static PrometheusHandle> {
    PROMETHEUS_HANDLE.get()
}

/// Render metrics as Prometheus text format
#[cfg(feature = "server")]
pub fn render_metrics() -> String {
    PROMETHEUS_HANDLE
        .get()
        .map(|h| h.render())
        .unwrap_or_else(|| "# Metrics not initialized\n".to_string())
}

/// Record a query execution
pub fn record_query(query_type: QueryType, space: &str) {
    counter!(
        metric_names::QUERY_TOTAL,
        "type" => query_type.as_str(),
        "space" => space.to_string()
    )
    .increment(1);
}

/// Record query latency
pub fn record_latency(query_type: QueryType, duration: Duration) {
    histogram!(
        metric_names::QUERY_LATENCY,
        "type" => query_type.as_str()
    )
    .record(duration.as_secs_f64());
}

/// Record a query error
pub fn record_error(query_type: QueryType, error_type: &str) {
    counter!(
        metric_names::QUERY_ERRORS,
        "type" => query_type.as_str(),
        "error" => error_type.to_string()
    )
    .increment(1);
}

/// Record a slow query.
///
/// Beyond bumping the per-type counter, this emits a structured `warn` log
/// carrying the query text and whether the query fell back to a full scan —
/// the two things an operator needs to triage a slow query.
pub fn record_slow_query(query_type: QueryType, duration_ms: u64, query: &str, full_scan: bool) {
    counter!(
        metric_names::SLOW_QUERIES,
        "type" => query_type.as_str()
    )
    .increment(1);

    tracing::warn!(
        query_type = query_type.as_str(),
        duration_ms = duration_ms,
        full_scan = full_scan,
        query = query,
        "Slow query detected"
    );
}

/// Update active connections count
pub fn set_active_connections(count: usize) {
    gauge!(metric_names::ACTIVE_CONNECTIONS).set(count as f64);
}

/// Update active sessions count
pub fn set_active_sessions(count: usize) {
    gauge!(metric_names::ACTIVE_SESSIONS).set(count as f64);
}

/// Update storage size
pub fn set_storage_bytes(space: &str, bytes: u64) {
    gauge!(
        metric_names::STORAGE_BYTES,
        "space" => space.to_string()
    )
    .set(bytes as f64);
}

/// Increment active connections
pub fn inc_connections() {
    gauge!(metric_names::ACTIVE_CONNECTIONS).increment(1.0);
}

/// Decrement active connections
pub fn dec_connections() {
    gauge!(metric_names::ACTIVE_CONNECTIONS).decrement(1.0);
}

// ===== Partition metrics =====

/// Record a partition request
pub fn record_partition_request(space_id: u32, part_id: u32, is_write: bool) {
    let op_type = if is_write { "write" } else { "read" };
    counter!(
        metric_names::PARTITION_REQUESTS,
        "space" => space_id.to_string(),
        "partition" => part_id.to_string(),
        "operation" => op_type.to_string()
    )
    .increment(1);
}

/// Update hotspot ratio for a partition
pub fn set_partition_hotspot_ratio(space_id: u32, part_id: u32, ratio: f64) {
    gauge!(
        metric_names::PARTITION_HOTSPOT_RATIO,
        "space" => space_id.to_string(),
        "partition" => part_id.to_string()
    )
    .set(ratio);
}

/// Set the total partition count for a space
pub fn set_partition_count(space_id: u32, count: u32) {
    gauge!(
        metric_names::PARTITION_COUNT,
        "space" => space_id.to_string()
    )
    .set(count as f64);
}

/// Set the leader count for a host
pub fn set_partition_leader_count(host: &str, count: u32) {
    gauge!(
        metric_names::PARTITION_LEADER_COUNT,
        "host" => host.to_string()
    )
    .set(count as f64);
}

/// Query execution timer for automatic latency recording
pub struct QueryTimer {
    query_type: QueryType,
    space: String,
    start: Instant,
    slow_threshold_ms: u64,
    query: String,
}

impl QueryTimer {
    /// Start a new query timer
    pub fn new(query_type: QueryType, space: &str) -> Self {
        Self {
            query_type,
            space: space.to_string(),
            start: Instant::now(),
            slow_threshold_ms: 1000, // Default 1 second
            query: String::new(),
        }
    }

    /// Set the slow query threshold in milliseconds
    pub fn with_slow_threshold(mut self, ms: u64) -> Self {
        self.slow_threshold_ms = ms;
        self
    }

    /// Attach the raw query text so the slow-query log can include it.
    pub fn with_query(mut self, query: &str) -> Self {
        self.query = query.to_string();
        self
    }

    /// Complete the timer and record metrics. `full_scan` indicates whether the
    /// query fell back to an un-indexed full scan (surfaced in the slow log).
    pub fn finish(self, full_scan: bool) -> Duration {
        let duration = self.start.elapsed();
        let duration_ms = duration.as_millis() as u64;

        // Record query count
        record_query(self.query_type, &self.space);

        // Record latency
        record_latency(self.query_type, duration);

        // Check for slow query
        if duration_ms > self.slow_threshold_ms {
            record_slow_query(self.query_type, duration_ms, &self.query, full_scan);
        }

        duration
    }

    /// Complete the timer with an error
    pub fn finish_with_error(self, error_type: &str) -> Duration {
        let duration = self.start.elapsed();

        // Record error
        record_error(self.query_type, error_type);

        // Record latency even for errors
        record_latency(self.query_type, duration);

        duration
    }
}

/// Metrics summary for API responses
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSummary {
    pub total_queries: u64,
    pub error_count: u64,
    pub avg_latency_ms: f64,
    pub slow_query_count: u64,
    pub active_connections: u64,
    pub active_sessions: u64,
}

impl Default for MetricsSummary {
    fn default() -> Self {
        Self {
            total_queries: 0,
            error_count: 0,
            avg_latency_ms: 0.0,
            slow_query_count: 0,
            active_connections: 0,
            active_sessions: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_type_as_str() {
        assert_eq!(QueryType::Show.as_str(), "show");
        assert_eq!(QueryType::Insert.as_str(), "insert");
        assert_eq!(QueryType::Match.as_str(), "match");
    }

    #[test]
    fn test_query_timer() {
        let timer = QueryTimer::new(QueryType::Show, "test_space").with_slow_threshold(100);

        std::thread::sleep(std::time::Duration::from_millis(10));
        let duration = timer.finish(false);

        assert!(duration.as_millis() >= 10);
    }
}
