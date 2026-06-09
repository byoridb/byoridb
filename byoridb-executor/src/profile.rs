// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Runtime profiling for `PROFILE <query>`.
//!
//! The executor is imperative (no Volcano-style operator iterators), so there
//! is no natural per-operator boundary to hang timers on. Instead each query
//! type instruments the handful of points where real work happens — index
//! lookups, full scans, neighbour expansion, WHERE filtering — and records one
//! [`ProfileRecord`] per point. [`crate::explain`] later overlays these records
//! onto the logical plan tree by matching [`ProfileOp`].
//!
//! Granularity is honest, not aspirational: operators we don't instrument
//! (projection, LIMIT) show row counts but no wall-clock time, because their
//! cost is not separable from the surrounding scan in the current model.

use parking_lot::Mutex;

/// Operator kind for a profile observation. Used both by the instrumentation
/// sites and by the plan-tree overlay so the two agree on what each record
/// describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileOp {
    /// Tag/edge secondary index lookup producing VIDs (LOOKUP, MATCH).
    IndexScan,
    /// Label-only `{space}:tagvid:{label}:` secondary-index prefix scan.
    TagVidScan,
    /// Full `{space}:vertex:` scan fallback (no usable index).
    FullScan,
    /// Outgoing edge prefix scan (`GO` / MATCH forward expand).
    GetNeighbors,
    /// Incoming edge expansion (reverse traversal) via the reverse-edge index
    /// (`{space}:in-edge:{dst}:` prefix scan) — O(in-degree), not a full scan.
    GetIncoming,
    /// MATCH multi-hop edge expansion, aggregated across hops.
    Expand,
    /// WHERE-clause post-filter (rows in → rows out).
    Filter,
    /// RETURN/YIELD projection of the final rows.
    Project,
    /// COUNT/SUM/AVG/MIN/MAX and GROUP BY reduction.
    Aggregate,
    /// Inner join of comma-separated multi-patterns / OPTIONAL MATCH merge.
    Join,
    /// Point lookup of vertices by VID (FETCH).
    GetVertices,
    /// Edge fetch by `src->dst` (FETCH PROP ON edge).
    GetEdges,
    /// BFS / Dijkstra path finding (FIND).
    PathFind,
}

impl ProfileOp {
    /// Stable display label used in the EXPLAIN/PROFILE operator column.
    pub fn label(self) -> &'static str {
        match self {
            ProfileOp::IndexScan => "IndexScan",
            ProfileOp::TagVidScan => "TagVidIndexScan",
            ProfileOp::FullScan => "FullScan",
            ProfileOp::GetNeighbors => "GetNeighbors",
            ProfileOp::GetIncoming => "GetIncomingNeighbors",
            ProfileOp::Expand => "Expand",
            ProfileOp::Filter => "Filter",
            ProfileOp::Project => "Project",
            ProfileOp::Aggregate => "Aggregate",
            ProfileOp::Join => "Join",
            ProfileOp::GetVertices => "GetVertices",
            ProfileOp::GetEdges => "GetEdges",
            ProfileOp::PathFind => "PathFind",
        }
    }
}

/// One operator-level observation captured during a PROFILE run.
#[derive(Debug, Clone)]
pub struct ProfileRecord {
    pub op: ProfileOp,
    /// Human-readable detail (index name, edge types, scanned counts, …).
    pub detail: String,
    /// Rows produced by this operator.
    pub rows: u64,
    /// Wall-clock time attributed to this operator, in microseconds.
    pub time_us: u64,
    /// Whether this operator performed an un-indexed full scan.
    pub full_scan: bool,
}

/// Thread-safe accumulator of [`ProfileRecord`]s for a single PROFILE query.
#[derive(Debug, Default)]
pub struct ProfileCollector {
    records: Mutex<Vec<ProfileRecord>>,
}

impl ProfileCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, rec: ProfileRecord) {
        self.records.lock().push(rec);
    }

    /// Snapshot the records collected so far (clones — callers usually outlive
    /// the lock release).
    pub fn snapshot(&self) -> Vec<ProfileRecord> {
        self.records.lock().clone()
    }
}
