// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Execution context for nGQL queries

#[cfg(feature = "distributed")]
use crate::distributed::{DistributedQueryConfig, DistributedQueryExecutor};
use crate::executor::ExecutorResult;
use crate::profile::{ProfileCollector, ProfileOp, ProfileRecord};
#[cfg(feature = "distributed")]
use crate::storage_client::StorageQueryClient;
use byoridb_kvstore::KVStore;
#[cfg(feature = "distributed")]
use byoridb_meta::MetaClient;
use byoridb_storage::IndexManager;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Configuration for query execution
#[derive(Debug, Clone)]
pub struct ExecutionConfig {
    pub timeout_ms: u64,
    /// Soft cap on the estimated memory a single query may accumulate while
    /// materializing results (binding rows + projected output). When exceeded,
    /// the query fails with [`crate::error::ExecutionError::ResourceExhausted`]
    /// instead of letting the process OOM (which on a single node forces a
    /// ~60min redb repair). `0` disables the cap. Overridable via
    /// `BYORIDB_MAX_MEMORY_MB`.
    pub max_memory_mb: usize,
    pub enable_optimization: bool,
    /// Hard cap on the number of vertices BFS/Dijkstra will process before
    /// returning early. Guards against pathological traversals on cycles or
    /// dense graphs. `0` disables the cap.
    pub max_traversal_nodes: usize,
    /// Maximum number of rows returned by a single scan_prefix call.
    /// Prevents OOM from unbounded full-table scans (MATCH fallback, LOOKUP).
    /// `0` disables the cap (not recommended in production).
    pub max_scan_limit: usize,
    /// Maximum number of GO steps allowed per query.
    /// Prevents exponential fanout from large step counts.
    pub max_go_steps: u32,
    /// Maximum number of paths `FIND ALL SHORTEST PATHS` will enumerate.
    /// The shortest-path DAG can hold combinatorially many distinct paths
    /// on dense graphs; enumeration stops at this cap. `0` disables the cap.
    pub max_find_paths: usize,
    /// Minimum vector count before `RECOMMEND ... BY EMBEDDING` builds and uses
    /// a persisted HNSW index (PLAN.md R-2b). At or below this, exact flat KNN
    /// is used (no index overhead, exact results). Above it, the ANN index wins.
    pub vector_index_min: usize,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 30000, // 30 seconds
            // Per-query result-memory cap (OOM guard). Default 1GB; raise via
            // BYORIDB_MAX_MEMORY_MB on nodes with headroom so legitimate large
            // aggregations aren't rejected while runaway materializations still are.
            max_memory_mb: std::env::var("BYORIDB_MAX_MEMORY_MB")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1024),
            enable_optimization: true,
            max_traversal_nodes: 100_000,
            // 100K rows per scan by default; overridable via BYORIDB_MAX_SCAN_LIMIT
            // for accurate global aggregation over large datasets (needs enough
            // memory — this is an OOM guard, so only raise it with headroom).
            max_scan_limit: std::env::var("BYORIDB_MAX_SCAN_LIMIT")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(100_000),
            max_go_steps: 20,
            max_find_paths: 1024,
            vector_index_min: 1000,
        }
    }
}

/// Execution context for a single query
pub struct ExecutionContext {
    pub space: Option<String>,
    pub space_id: Option<u32>,
    pub config: ExecutionConfig,
    pub kvstore: Arc<dyn KVStore>,
    pub index_manager: Option<Arc<IndexManager>>,
    #[cfg(feature = "distributed")]
    pub meta_client: Option<Arc<MetaClient>>,

    // Distributed execution fields
    #[cfg(feature = "distributed")]
    pub storage_client: Option<Arc<StorageQueryClient>>,
    #[cfg(feature = "distributed")]
    pub distributed_mode: bool,
    pub partition_num: Option<u32>,

    /// Roles of the authenticated caller (from session). Used for RBAC checks
    /// inside the executor (e.g. CREATE USER, GRANT, REVOKE require GOD/ADMIN).
    pub caller_roles: Vec<String>,

    /// Variable bindings for compound statements (`$var = GO ...`).
    ///
    /// `Mutex` because the executor holds the context behind an `Arc` for
    /// the duration of a query, and compound execution needs to insert
    /// each clause's `ExecutorResult` for subsequent clauses to read.
    /// Access is synchronous and short — the lock is never held across an
    /// `.await` boundary. Wrapped in `Arc` so a `USE`-derived sibling context
    /// (see [`Self::derive_with_space`]) shares the *same* bindings: a compound
    /// like `$a = GO ...; USE other; ... $a ...` must still resolve `$a`.
    pub vars: Arc<Mutex<HashMap<String, ExecutorResult>>>,

    /// Active profile collector. `Some` only while a `PROFILE <query>` is
    /// executing; instrumentation sites push [`ProfileRecord`]s through it.
    /// Behind a `Mutex<Option<..>>` because the context is shared via `Arc`
    /// and PROFILE flips it on for the duration of the inner execution.
    pub profile: Mutex<Option<Arc<ProfileCollector>>>,

    /// Always-on flag set whenever any query performs an un-indexed full scan
    /// (MATCH/LOOKUP fallback, incoming-edge scan). Read by the Graph service
    /// after execution to enrich the slow-query log, independent of PROFILE.
    /// Behind an `Arc` so the Graph layer can hand in a shared flag, run the
    /// query, and read the result back across the executor boundary.
    pub full_scan: Arc<AtomicBool>,

    /// Cache for the space id resolved from the space *name* (see
    /// [`Self::resolve_space_id`]). Populated once per context; a `USE`-derived
    /// sibling gets a fresh cell so it re-resolves for its own space.
    resolved_space_id: OnceCell<u32>,
}

impl ExecutionContext {
    pub fn new(kvstore: Arc<dyn KVStore>) -> Self {
        Self {
            space: None,
            space_id: None,
            config: ExecutionConfig::default(),
            kvstore: kvstore.clone(),
            index_manager: Some(Arc::new(IndexManager::new(kvstore))),
            #[cfg(feature = "distributed")]
            meta_client: None,
            #[cfg(feature = "distributed")]
            storage_client: None,
            #[cfg(feature = "distributed")]
            distributed_mode: false,
            partition_num: None,
            caller_roles: vec![],
            vars: Arc::new(Mutex::new(HashMap::new())),
            profile: Mutex::new(None),
            full_scan: Arc::new(AtomicBool::new(false)),
            resolved_space_id: OnceCell::new(),
        }
    }

    /// Resolve the effective space id for storage/index operations.
    ///
    /// Prefers an explicitly-set `space_id` (the distributed path sets it from
    /// Meta). Otherwise resolves it from the space *name* via the persisted
    /// space definition (`{"id":...}`) and caches the result. Falls back to `1`
    /// only when no space is set or its definition is missing.
    ///
    /// This is the fix for cross-space index contamination: `space_id` was never
    /// populated on the standalone/embedded path, so every `space_id.unwrap_or(1)`
    /// collapsed all spaces onto id 1 — index definitions, name uniqueness, and
    /// LOOKUP/INSERT index selection all merged across logical spaces.
    pub async fn resolve_space_id(&self) -> u32 {
        if let Some(id) = self.space_id {
            return id;
        }
        *self
            .resolved_space_id
            .get_or_init(|| async {
                let Some(name) = self.space.as_deref() else {
                    return 1;
                };
                match self.kvstore.get(&crate::key::SchemaKey::space(name)).await {
                    Ok(Some(bytes)) => serde_json::from_slice::<serde_json::Value>(&bytes)
                        .ok()
                        .and_then(|v| v.get("id").and_then(|i| i.as_u64()))
                        .map(|i| i as u32)
                        .unwrap_or(1),
                    _ => 1,
                }
            })
            .await
    }

    /// Use a caller-supplied shared full-scan flag so the embedding layer (the
    /// Graph service) can observe whether this query fell back to a full scan.
    pub fn with_full_scan_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.full_scan = flag;
        self
    }

    /// Turn on profile collection and return the fresh collector. Called by
    /// `PROFILE <query>` before executing the inner plan.
    pub fn enable_profile(&self) -> Arc<ProfileCollector> {
        let collector = Arc::new(ProfileCollector::new());
        *self.profile.lock() = Some(collector.clone());
        collector
    }

    /// Turn off profile collection, returning the collector if one was active.
    pub fn disable_profile(&self) -> Option<Arc<ProfileCollector>> {
        self.profile.lock().take()
    }

    /// Record a profile observation when collection is active; cheap no-op
    /// otherwise. Instrumentation sites guard timing with [`Self::profiling`]
    /// so they don't pay for `Instant::now()` on normal queries.
    pub fn record_profile(
        &self,
        op: ProfileOp,
        detail: impl Into<String>,
        rows: u64,
        time_us: u64,
        full_scan: bool,
    ) {
        if let Some(collector) = self.profile.lock().as_ref() {
            collector.record(ProfileRecord {
                op,
                detail: detail.into(),
                rows,
                time_us,
                full_scan,
            });
        }
    }

    /// Whether profile collection is currently active. Instrumentation sites
    /// check this before measuring time, so non-PROFILE queries stay free.
    pub fn profiling(&self) -> bool {
        self.profile.lock().is_some()
    }

    /// Mark that the current query performed an un-indexed full scan.
    pub fn mark_full_scan(&self) {
        self.full_scan.store(true, Ordering::Relaxed);
    }

    /// Whether any full scan has occurred during this context's lifetime.
    pub fn took_full_scan(&self) -> bool {
        self.full_scan.load(Ordering::Relaxed)
    }

    /// Fail the query if `estimated_bytes` of accumulated result memory exceeds
    /// the `max_memory_mb` budget. Executors call this as they materialize rows,
    /// so a runaway query returns [`crate::error::ExecutionError::ResourceExhausted`]
    /// instead of OOM-killing the process. `max_memory_mb == 0` disables the cap.
    pub fn check_result_budget(&self, estimated_bytes: usize) -> crate::error::Result<()> {
        let cap = self.config.max_memory_mb.saturating_mul(1024 * 1024);
        if cap > 0 && estimated_bytes > cap {
            return Err(crate::error::ExecutionError::ResourceExhausted(format!(
                "query exceeded the {} MB result-memory cap (max_memory_mb) while \
                 materializing rows — add a LIMIT or narrow the query",
                self.config.max_memory_mb
            )));
        }
        Ok(())
    }

    /// Set the roles of the authenticated caller for RBAC checks.
    pub fn with_caller_roles(mut self, roles: Vec<String>) -> Self {
        self.caller_roles = roles;
        self
    }

    /// Returns true if the caller has GOD or ADMIN role.
    pub fn is_admin(&self) -> bool {
        self.caller_roles.iter().any(|r| r == "GOD" || r == "ADMIN")
    }

    /// Bind a result to a compound-statement variable.
    pub fn bind_var(&self, name: String, result: ExecutorResult) {
        self.vars.lock().insert(name, result);
    }

    /// Look up a compound-statement variable. Clones because the caller
    /// commonly needs to outlive the lock release.
    pub fn lookup_var(&self, name: &str) -> Option<ExecutorResult> {
        self.vars.lock().get(name).cloned()
    }

    pub fn with_space(mut self, space: String) -> Self {
        self.space = Some(space);
        self
    }

    /// Derive a sibling context bound to a different `space`, sharing the same
    /// kvstore, index manager, variable bindings (`Arc<Mutex>`), profile
    /// collector, full-scan flag, caller roles, and config. Used by compound
    /// execution so a `USE` clause switches the space seen by *subsequent*
    /// clauses in the same request — `execute_use` cannot mutate `space` in
    /// place because the context is shared behind an `Arc`.
    ///
    /// `space_id` is reset to `None` to match the normal per-request path (the
    /// Graph service builds the context with `with_space` only); distributed
    /// reads resolve it on demand from the space name.
    pub fn derive_with_space(&self, space: String) -> Self {
        Self {
            space: Some(space),
            space_id: None,
            config: self.config.clone(),
            kvstore: self.kvstore.clone(),
            index_manager: self.index_manager.clone(),
            #[cfg(feature = "distributed")]
            meta_client: self.meta_client.clone(),
            #[cfg(feature = "distributed")]
            storage_client: self.storage_client.clone(),
            #[cfg(feature = "distributed")]
            distributed_mode: self.distributed_mode,
            partition_num: self.partition_num,
            caller_roles: self.caller_roles.clone(),
            vars: self.vars.clone(),
            profile: Mutex::new(self.profile.lock().clone()),
            full_scan: self.full_scan.clone(),
            // Fresh cell: the derived context resolves space_id for its own space.
            resolved_space_id: OnceCell::new(),
        }
    }

    pub fn with_space_id(mut self, space_id: u32) -> Self {
        self.space_id = Some(space_id);
        self
    }

    pub fn with_config(mut self, config: ExecutionConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_index_manager(mut self, index_manager: Arc<IndexManager>) -> Self {
        self.index_manager = Some(index_manager);
        self
    }

    #[cfg(feature = "distributed")]
    pub fn with_meta_client(mut self, meta_client: Arc<MetaClient>) -> Self {
        self.meta_client = Some(meta_client);
        self
    }

    /// Enable distributed query execution mode
    #[cfg(feature = "distributed")]
    pub fn with_distributed_mode(
        mut self,
        storage_client: Arc<StorageQueryClient>,
        partition_num: u32,
    ) -> Self {
        self.storage_client = Some(storage_client);
        self.distributed_mode = true;
        self.partition_num = Some(partition_num);
        self
    }

    /// Check if distributed mode is enabled and properly configured
    #[cfg(feature = "distributed")]
    pub fn is_distributed(&self) -> bool {
        self.distributed_mode
            && self.storage_client.is_some()
            && self.meta_client.is_some()
            && self.partition_num.is_some()
    }

    /// In a non-distributed (embedded) build there is no remote Storage/Meta,
    /// so every query takes the local path.
    #[cfg(not(feature = "distributed"))]
    pub fn is_distributed(&self) -> bool {
        false
    }

    #[cfg(feature = "distributed")]
    pub fn has_meta_client(&self) -> bool {
        self.meta_client.is_some()
    }

    #[cfg(not(feature = "distributed"))]
    pub fn has_meta_client(&self) -> bool {
        false
    }

    /// Create a distributed query executor if distributed mode is enabled
    #[cfg(feature = "distributed")]
    pub fn get_distributed_executor(&self) -> Option<DistributedQueryExecutor> {
        if self.is_distributed() {
            let storage_client = self.storage_client.as_ref()?.clone();
            let meta_client = self.meta_client.as_ref()?.clone();
            Some(DistributedQueryExecutor::new(storage_client, meta_client))
        } else {
            None
        }
    }

    /// Create a distributed query executor with custom config
    #[cfg(feature = "distributed")]
    pub fn get_distributed_executor_with_config(
        &self,
        config: DistributedQueryConfig,
    ) -> Option<DistributedQueryExecutor> {
        if self.is_distributed() {
            let storage_client = self.storage_client.as_ref()?.clone();
            let meta_client = self.meta_client.as_ref()?.clone();
            Some(DistributedQueryExecutor::with_config(
                storage_client,
                meta_client,
                config,
            ))
        } else {
            None
        }
    }

    /// Get the partition number for the current space
    pub fn get_partition_num(&self) -> Option<u32> {
        self.partition_num
    }
}

// ===== Result-memory estimation (shared by all executors for the OOM guard) =====

/// Cheap, conservative estimate of a materialized [`Value`]'s footprint in
/// bytes. Strings count their length; composite values (vertex/edge/map/list/
/// path/set) use a flat estimate rather than walking their internals — this
/// feeds [`ExecutionContext::check_result_budget`], a safety guard, so a slight
/// under/overestimate is fine.
pub(crate) fn estimate_value_bytes(v: &byoridb_common::Value) -> usize {
    use byoridb_common::Value;
    match v {
        Value::String(s) => 24 + s.len(),
        Value::Edge(_) => 256,
        Value::Vertex(_)
        | Value::Path(_)
        | Value::Map(_)
        | Value::Set(_)
        | Value::List(_)
        | Value::DataSet(_) => 512,
        _ => 16,
    }
}

/// Estimated footprint of one output row.
pub(crate) fn estimate_row_bytes(row: &[byoridb_common::Value]) -> usize {
    24 + row.iter().map(estimate_value_bytes).sum::<usize>()
}
