// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Graph service implementation

use super::auth::{AuthManager, User};
use super::error::{GraphError, Result};
use super::metrics::{QueryTimer, QueryType};
use super::session::SessionManager;
use byoridb_common::DataSet;
use byoridb_kvstore::KVStore;
use dashmap::DashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tracing::{debug, info};

/// Detect credential-bearing syntax without retaining or reproducing the raw
/// query. Besides a standalone `PASSWORD` keyword, classify CREATE/ALTER USER
/// intent so a malformed statement cannot expose a secret merely by omitting
/// the expected keyword.
fn contains_sensitive_credential_syntax(query: &str) -> bool {
    let mut user_mutation = false;
    for word in
        query.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
    {
        if word.eq_ignore_ascii_case("PASSWORD") {
            return true;
        }
        if word.eq_ignore_ascii_case("CREATE") || word.eq_ignore_ascii_case("ALTER") {
            user_mutation = true;
        } else if user_mutation && word.eq_ignore_ascii_case("USER") {
            return true;
        }
    }
    false
}

/// Safe metadata for a query currently executing. Bearer session IDs and raw
/// query text are intentionally never retained in this registry.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RunningQuery {
    pub id: u64,
    pub query_type: &'static str,
    pub query_length_bytes: usize,
    pub space: String,
    pub started_at_ms: u64,
}

/// RAII guard: removes the query from the active registry and decrements the
/// in-flight gauge on drop, so every exit path (success, error, early return,
/// panic) cleans up.
struct ActiveQueryGuard {
    registry: Arc<DashMap<u64, RunningQuery>>,
    id: u64,
    /// Shared drain counter for graceful shutdown (see [`crate::shutdown`]).
    shutdown: Arc<crate::shutdown::ShutdownState>,
}

impl Drop for ActiveQueryGuard {
    fn drop(&mut self) {
        self.registry.remove(&self.id);
        crate::metrics::dec_inflight();
        self.shutdown.query_finished();
    }
}

/// Default interval for the background session-cleanup task (60 seconds).
pub const DEFAULT_SESSION_CLEANUP_INTERVAL_SECS: u64 = 60;

/// Graph service
/// Caller-supplied constraints on a single query request.
///
/// Defaults to today's behavior, so an unset field never changes what a caller
/// could already do.
#[derive(Debug, Clone, Copy, Default)]
pub struct QueryOptions {
    /// Refuse the request unless every clause it will execute is a read.
    ///
    /// This is a constraint on one request, not a property of the session or of
    /// the credential. It exists so a caller can pass an untrusted or
    /// model-generated statement through a writable session and still be sure it
    /// cannot mutate anything, without reimplementing statement classification
    /// outside the parser.
    ///
    /// Administrative statements are refused as well. `SHOW USERS`,
    /// `SHOW ROLES`, and `SHOW SESSIONS` only require `Permission::Read`, so
    /// without that clause an administrator's read-only request would still
    /// enumerate users and live sessions — a surprising thing to inherit when the
    /// point of the flag is to sandbox a statement you did not write.
    ///
    /// It is not a tenant boundary. Built-in roles apply to every space, so a
    /// read-only request can still read any space the session could read.
    pub read_only: bool,
}

pub struct GraphService {
    session_manager: Arc<SessionManager>,
    auth_manager: Arc<AuthManager>,
    kvstore: Arc<dyn KVStore>,
    /// Queries currently executing, keyed by a monotonic id.
    active_queries: Arc<DashMap<u64, RunningQuery>>,
    query_seq: Arc<AtomicU64>,
    session_cleanup_started: Arc<AtomicBool>,
    /// Readiness flag + in-flight drain counter, shared with the server
    /// binary's signal handler (and across the gRPC/HTTP service instances).
    shutdown: Arc<crate::shutdown::ShutdownState>,
}

impl GraphService {
    pub fn new(kvstore: Arc<dyn KVStore>) -> Self {
        let auth_manager = Arc::new(AuthManager::new());

        GraphService {
            session_manager: Arc::new(SessionManager::new()),
            auth_manager,
            kvstore,
            active_queries: Arc::new(DashMap::new()),
            query_seq: Arc::new(AtomicU64::new(1)),
            session_cleanup_started: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(crate::shutdown::ShutdownState::new()),
        }
    }

    /// Construct a [`GraphService`] with a caller-supplied [`AuthManager`].
    ///
    /// Embedded / in-process callers use this to inject an auth manager built
    /// with a known root password (e.g. [`AuthManager::with_config`]) so they
    /// can authenticate without depending on the `BYORIDB_ROOT_PASSWORD` env var
    /// or the random password generated by [`GraphService::new`]. Network
    /// launchers should use [`AuthManager::try_with_config`], construct one
    /// service with the resulting manager, and share its [`Arc`] across every
    /// protocol.
    pub fn with_auth(kvstore: Arc<dyn KVStore>, auth_manager: AuthManager) -> Self {
        GraphService {
            session_manager: Arc::new(SessionManager::new()),
            auth_manager: Arc::new(auth_manager),
            kvstore,
            active_queries: Arc::new(DashMap::new()),
            query_seq: Arc::new(AtomicU64::new(1)),
            session_cleanup_started: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(crate::shutdown::ShutdownState::new()),
        }
    }

    /// Replace the readiness/drain state with one shared by the embedding
    /// server binary (both the gRPC and HTTP services should share it so the
    /// drain counter covers every in-flight query).
    pub fn with_shutdown_state(mut self, shutdown: Arc<crate::shutdown::ShutdownState>) -> Self {
        self.shutdown = shutdown;
        self
    }

    /// The readiness/drain state this service reports into.
    pub fn shutdown_state(&self) -> Arc<crate::shutdown::ShutdownState> {
        self.shutdown.clone()
    }

    /// Register a query in the active-query registry and bump the in-flight
    /// gauge. The returned guard removes it on drop (any exit path).
    fn register_active_query(
        &self,
        query_type: QueryType,
        query_length_bytes: usize,
        space: &str,
    ) -> ActiveQueryGuard {
        let id = self.query_seq.fetch_add(1, Ordering::Relaxed);
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.active_queries.insert(
            id,
            RunningQuery {
                id,
                query_type: query_type.as_str(),
                query_length_bytes,
                space: space.to_string(),
                started_at_ms,
            },
        );
        crate::metrics::inc_inflight();
        self.shutdown.query_started();
        ActiveQueryGuard {
            registry: self.active_queries.clone(),
            id,
            shutdown: self.shutdown.clone(),
        }
    }

    /// Snapshot of queries currently executing, oldest first.
    pub fn list_active_queries(&self) -> Vec<RunningQuery> {
        let mut v: Vec<RunningQuery> = self
            .active_queries
            .iter()
            .map(|e| e.value().clone())
            .collect();
        v.sort_by_key(|q| q.started_at_ms);
        v
    }

    /// Load persisted users into the in-memory authentication cache. The root
    /// account is deliberately excluded: its credential is process bootstrap
    /// state supplied by `BYORIDB_ROOT_PASSWORD`, not mutable KV metadata.
    pub async fn hydrate_persisted_users(&self) -> Result<usize> {
        const USER_KEY_PREFIX: &[u8] = b"__user_";
        let entries = self
            .kvstore
            .scan_prefix(USER_KEY_PREFIX)
            .await
            .map_err(|e| GraphError::Storage(e.to_string()))?;

        let mut hydrated = 0;
        for (key, value) in entries {
            let key_username = key
                .strip_prefix(USER_KEY_PREFIX)
                .and_then(|suffix| std::str::from_utf8(suffix).ok())
                .ok_or_else(|| {
                    GraphError::InternalError("Invalid persisted user key".to_string())
                })?;
            let user: User = serde_json::from_slice(&value).map_err(|_| {
                GraphError::InternalError("Invalid persisted user record".to_string())
            })?;
            if key_username != user.username {
                return Err(GraphError::InternalError(
                    "Persisted user key does not match its record".to_string(),
                ));
            }
            if !user.username.eq_ignore_ascii_case("root") {
                self.auth_manager.upsert_persisted_user(user).await?;
                hydrated += 1;
            }
        }
        info!(
            user_count = hydrated,
            "Persisted authentication state loaded"
        );
        Ok(hydrated)
    }

    /// Validate an HTTP bearer session without exposing it to logs.
    pub async fn validate_session(&self, session_id: i64) -> Result<()> {
        self.live_session(session_id).await.map(|_| ())
    }

    /// Check diagnostics/admin API access for a live bearer session.
    pub async fn is_admin_session(&self, session_id: i64) -> Result<bool> {
        let (roles, _) = self.live_session(session_id).await?;
        Ok(roles.iter().any(|role| role == "GOD" || role == "ADMIN"))
    }

    /// Touch both session stores together, with AuthManager authoritative. If
    /// either side is missing/expired, revoke the surviving side so a bearer
    /// cannot create a sliding-TTL zombie in only one store.
    async fn live_session(
        &self,
        session_id: i64,
    ) -> Result<(Vec<String>, crate::session::Session)> {
        let Some(roles) = self.auth_manager.get_session_roles(session_id).await else {
            self.session_manager.remove_session(session_id).await;
            return Err(GraphError::SessionNotFound(session_id));
        };
        let Some(session) = self.session_manager.get_session(session_id).await else {
            let _ = self.auth_manager.sign_out(session_id).await;
            return Err(GraphError::SessionNotFound(session_id));
        };
        Ok((roles, session))
    }

    /// Remove graph sessions with no corresponding live auth bearer, without
    /// refreshing either store's TTL.
    async fn remove_orphan_graph_sessions(&self) -> usize {
        Self::remove_orphan_sessions(&self.auth_manager, &self.session_manager).await
    }

    async fn remove_orphan_sessions(
        auth_manager: &AuthManager,
        session_manager: &SessionManager,
    ) -> usize {
        let mut removed = 0;
        for (session_id, _, _) in session_manager.list_sessions() {
            if !auth_manager
                .has_live_session_without_touch(session_id)
                .await
            {
                session_manager.remove_session(session_id).await;
                removed += 1;
            }
        }
        removed
    }

    /// Map a write query type to its `rows_written` metric label.
    fn rows_written_op(query_type: QueryType) -> Option<&'static str> {
        match query_type {
            QueryType::Insert => Some("insert"),
            QueryType::Update => Some("update"),
            QueryType::Delete => Some("delete"),
            _ => None,
        }
    }

    /// Spawn a background task that periodically evicts expired sessions from
    /// both the [`AuthManager`] and the [`SessionManager`].
    ///
    /// The task holds [`Weak`](std::sync::Weak) references to the managers, so
    /// it exits automatically once the [`GraphService`] (and all clones of its
    /// inner [`Arc`]s) are dropped.
    pub fn spawn_session_cleanup(&self, interval: Duration) -> JoinHandle<()> {
        if self
            .session_cleanup_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // Preserve the historical JoinHandle API while making startup
            // idempotent when HTTP and gRPC share this service.
            return tokio::spawn(async {});
        }

        let auth_weak = Arc::downgrade(&self.auth_manager);
        let session_weak = Arc::downgrade(&self.session_manager);

        tokio::spawn(async move {
            let first_cleanup = tokio::time::Instant::now() + interval;
            let mut ticker = tokio::time::interval_at(first_cleanup, interval);

            loop {
                ticker.tick().await;

                let (auth, sessions) = match (auth_weak.upgrade(), session_weak.upgrade()) {
                    (Some(a), Some(s)) => (a, s),
                    _ => {
                        debug!("Session cleanup task exiting: service dropped");
                        break;
                    }
                };

                let auth_removed = auth.cleanup_expired_sessions().await;
                let mut session_removed = sessions.cleanup_expired();
                session_removed += Self::remove_orphan_sessions(&auth, &sessions).await;

                if auth_removed > 0 || session_removed > 0 {
                    debug!(
                        "Session cleanup: removed {} auth sessions, {} graph sessions",
                        auth_removed, session_removed
                    );
                }
            }
        })
    }

    /// Authenticate a user and create a session
    pub async fn authenticate(&self, username: String, password: String) -> Result<i64> {
        self.authenticate_from(username, password, None).await
    }

    /// Authenticate with an optional transport peer address for source-based
    /// throttling.
    pub async fn authenticate_from(
        &self,
        username: String,
        password: String,
        source: Option<std::net::IpAddr>,
    ) -> Result<i64> {
        info!("Authentication request received");

        // Use AuthManager for authentication
        let session_id = self
            .auth_manager
            .authenticate_from(&username, &password, source)
            .await?;

        // Also register in session manager for space tracking
        self.session_manager
            .create_session_with_id(session_id, username.clone())
            .await;

        debug!("Authenticated session created");

        Ok(session_id)
    }

    /// Sign out a session.
    ///
    /// `caller_session_id` is the session making the request. A session can
    /// only sign out itself unless the caller has GOD/ADMIN role.
    pub async fn sign_out(&self, caller_session_id: i64, target_session_id: i64) -> Result<()> {
        info!("Sign out request received");

        // A bearer is valid only while both session stores agree that it is
        // live. `live_session` also removes a surviving orphan from either
        // store before returning SessionNotFound.
        let (caller_roles, _) = self.live_session(caller_session_id).await?;

        // Ownership check: caller must own the target session or be an admin.
        // Check authorization before target liveness so a non-admin caller
        // cannot use sign-out as a target-session existence oracle.
        if caller_session_id != target_session_id {
            let is_admin = caller_roles
                .iter()
                .any(|role| role == "GOD" || role == "ADMIN");

            if !is_admin {
                tracing::warn!("Sign out request denied");
                return Err(GraphError::InvalidOperation(
                    "Not authorized to sign out the target session".to_string(),
                ));
            }

            // Cross-session sign-out requires a live target under the same
            // dual-store contract as every query bearer.
            self.live_session(target_session_id).await?;
        }

        // AuthManager is the authoritative removal point. It gives concurrent
        // sign-outs one winner; every path still removes any graph-session
        // residue. A target that disappeared after liveness validation is
        // reported as missing rather than as false success.
        let auth_result = self.auth_manager.sign_out(target_session_id).await;
        self.session_manager.remove_session(target_session_id).await;
        match auth_result {
            Ok(()) => Ok(()),
            Err(GraphError::InvalidOperation(_)) => {
                Err(GraphError::SessionNotFound(target_session_id))
            }
            Err(error) => Err(error),
        }
    }

    /// Execute a query statement.
    pub async fn execute(&self, session_id: i64, stmt: String) -> Result<DataSet> {
        self.execute_with_options(session_id, stmt, QueryOptions::default())
            .await
    }

    /// Execute a query statement under caller-supplied constraints.
    ///
    /// [`QueryOptions::read_only`] lets a caller run an untrusted or
    /// model-generated statement over a session that genuinely has write
    /// permission, without granting that statement the session's authority. It
    /// constrains one request and leaves no residue: the next request on the
    /// same session may write.
    pub async fn execute_with_options(
        &self,
        session_id: i64,
        stmt: String,
        options: QueryOptions,
    ) -> Result<DataSet> {
        // Graceful shutdown: once the signal handler flips readiness off, new
        // queries fail fast and clearly; queries already past this gate drain
        // to completion before the servers stop.
        if !self.shutdown.is_accepting() {
            return Err(GraphError::InvalidOperation(
                "server is shutting down; not accepting new queries".to_string(),
            ));
        }

        // Auth is authoritative and is touched before graph session state, so
        // an expired bearer cannot extend a graph-only sliding TTL entry.
        let (caller_roles, session) = self.live_session(session_id).await?;

        let space = session
            .space
            .clone()
            .unwrap_or_else(|| "default".to_string());

        // Parse query. Parser diagnostics can contain unexpected token text,
        // so credential-bearing failures use a bounded generic message.
        let contains_password = contains_sensitive_credential_syntax(&stmt);
        let statement = byoridb_parser::parse(&stmt).map_err(|error| {
            if contains_password {
                GraphError::ParseError("invalid credential statement".to_string())
            } else {
                GraphError::ParseError(error.to_string())
            }
        })?;

        // A read-only request is refused before the role checks below, so the
        // caller learns the flag rejected the statement rather than that their
        // role was insufficient — their role may well have been sufficient.
        //
        // `required_permissions` has already expanded compound statements and
        // PROFILE recursively, and every statement maps to exactly one
        // permission of which only `Read` is a read. So this single check covers
        // `PROFILE INSERT ...` and `SHOW SPACES; DELETE VERTEX 1` too, and a
        // caller does not have to ban semicolons or comments to stay safe.
        if options.read_only {
            if Self::is_admin_only_statement(&statement) {
                return Err(GraphError::AuthFailed(
                    "read-only request may not run an administrative statement".to_string(),
                ));
            }
            if let Some(required) = Self::required_permissions(&statement)
                .into_iter()
                .find(|permission| *permission != crate::auth::Permission::Read)
            {
                return Err(GraphError::AuthFailed(format!(
                    "read-only request may not run a statement requiring {:?}",
                    required
                )));
            }
        }

        // Administrative statements must be checked at the service boundary as
        // well as in executors so compound/profile statements cannot bypass
        // role checks.
        if Self::is_admin_only_statement(&statement)
            && !caller_roles
                .iter()
                .any(|role| role == "GOD" || role == "ADMIN")
        {
            return Err(GraphError::AuthFailed(
                "GOD or ADMIN role required".to_string(),
            ));
        }

        if matches!(
            &statement,
            byoridb_parser::Statement::Show(byoridb_parser::ast::ShowStatement::Sessions)
        ) {
            self.remove_orphan_graph_sessions().await;
        }

        // RBAC: recursively authorize every clause that will execute. A compound
        // statement is not inherently read-only, and PROFILE executes its inner
        // statement (plain EXPLAIN does not).
        for required in Self::required_permissions(&statement) {
            let allowed = match self
                .auth_manager
                .check_permission(session_id, &space, required)
                .await
            {
                Ok(allowed) => allowed,
                Err(_) => {
                    self.session_manager.remove_session(session_id).await;
                    return Err(GraphError::SessionNotFound(session_id));
                }
            };
            if !allowed {
                return Err(GraphError::AuthFailed(format!(
                    "Permission denied: requires {:?}",
                    required
                )));
            }
        }

        // Determine query type for metrics
        let query_type = Self::get_query_type(&statement);
        debug!(
            query_type = query_type.as_str(),
            query_length_bytes = stmt.len(),
            space = %space,
            "Executing query"
        );

        // The timer retains only query length, never raw statement text.
        let timer = QueryTimer::new(query_type, &space)
            .with_slow_threshold(1000) // 1 second threshold
            .with_query(&stmt);

        // Track as in-flight for the lifetime of this call. The guard removes
        // the entry and decrements the gauge on every exit path.
        let _active_guard = self.register_active_query(query_type, stmt.len(), &space);

        // Create context
        let mut context = crate::context::ExecutionContext::new(session_id);
        if let Some(space) = session.space {
            context = context.with_space(space);
        }
        // Propagate caller roles for executor-level RBAC (CREATE USER, GRANT, REVOKE)
        context = context.with_caller_roles(caller_roles);

        // Plan
        let planner =
            crate::planner::Planner::new(self.session_manager.clone(), self.kvstore.clone());
        let executor = planner.plan(statement.clone(), context.clone())?;
        // Shared flag the executor sets if it falls back to a full scan.
        let full_scan_flag = executor.full_scan_flag();

        // Execute
        let result = executor.execute().await;

        // AUTH-SYNC: reconcile every user touched by the statement against the
        // final KV state, even when execution failed. Compound execution is
        // sequential without rollback, so an earlier clause may already have
        // committed a credential change before a later clause errors.
        let auth_sync_result = self.sync_auth_manager(&statement).await;
        if let Err(error) = &auth_sync_result {
            tracing::error!(
                error_type = Self::error_kind(error),
                "Failed to synchronize authentication state"
            );
        }

        // Record metrics
        match (&result, &auth_sync_result) {
            (Ok(dataset), Ok(())) => {
                // Record write throughput: INSERT/UPDATE/DELETE return a single
                // Int row carrying the affected-row count ("Inserted"/etc).
                if let Some(op) = Self::rows_written_op(query_type) {
                    if let Some(byoridb_common::Value::Int(n)) =
                        dataset.rows.first().and_then(|r| r.first())
                    {
                        crate::metrics::record_rows_written(op, (*n).max(0) as u64);
                    }
                }
                let full_scan = full_scan_flag
                    .map(|f| f.load(std::sync::atomic::Ordering::Relaxed))
                    .unwrap_or(false);
                timer.finish(full_scan);
            }
            (Err(e), _) => {
                timer.finish_with_error(Self::error_kind(e));
            }
            (Ok(_), Err(error)) => {
                timer.finish_with_error(Self::error_kind(error));
            }
        }

        // Preserve the original execution error when both execution and sync
        // fail. If execution succeeded, a sync failure is surfaced because the
        // persisted and live authentication states cannot safely diverge.
        let dataset = result?;
        auth_sync_result?;
        Ok(dataset)
    }

    /// AUTH-SYNC: after a successful user-management statement, sync the
    /// AuthManager in-memory cache so changes take effect without restart.
    async fn sync_auth_manager(&self, stmt: &byoridb_parser::Statement) -> Result<()> {
        for username in Self::affected_usernames(stmt) {
            if username.eq_ignore_ascii_case("root") {
                // The built-in root identity is process bootstrap state and has
                // no mutable KV record to reconcile.
                continue;
            }
            const USER_KEY_PREFIX: &str = "__user_";
            let key = format!("{}{}", USER_KEY_PREFIX, username);
            let value = self
                .kvstore
                .get(key.as_bytes())
                .await
                .map_err(|e| GraphError::Storage(e.to_string()))?;
            if let Some(value) = value {
                let user: User = serde_json::from_slice(&value).map_err(|_| {
                    GraphError::InternalError("Invalid persisted user record".to_string())
                })?;
                if user.username != username {
                    return Err(GraphError::InternalError(
                        "Persisted user key does not match its record".to_string(),
                    ));
                }
                if self.auth_manager.upsert_persisted_user(user).await? {
                    // Revoke only graph sessions whose exact bearer IDs were
                    // removed from AuthManager. A concurrent login with the
                    // new credential must not be deleted merely because it has
                    // the same username.
                    self.remove_orphan_graph_sessions().await;
                    debug!("Authentication cache synchronized");
                }
            } else {
                // This covers DROP, CREATE+DROP in one compound, and failed
                // statements whose target never existed. Converge to final KV
                // state without treating an absent cache entry as an error.
                let _ = self.auth_manager.delete_user(&username).await;
                self.remove_orphan_graph_sessions().await;
            }
        }
        Ok(())
    }

    fn affected_usernames(stmt: &byoridb_parser::Statement) -> std::collections::BTreeSet<String> {
        use byoridb_parser::ast::{AlterStatement, CreateStatement, DropStatement};
        use byoridb_parser::Statement;

        fn collect(stmt: &Statement, usernames: &mut std::collections::BTreeSet<String>) {
            match stmt {
                Statement::Create(CreateStatement::User(user)) => {
                    usernames.insert(user.username.clone());
                }
                Statement::Alter(AlterStatement::User(user)) => {
                    usernames.insert(user.username.clone());
                }
                Statement::Drop(DropStatement::User(user)) => {
                    usernames.insert(user.username.clone());
                }
                Statement::Grant(grant) => {
                    usernames.insert(grant.username.clone());
                }
                Statement::Revoke(revoke) => {
                    usernames.insert(revoke.username.clone());
                }
                Statement::Compound(clauses) => {
                    for clause in clauses {
                        collect(&clause.stmt, usernames);
                    }
                }
                Statement::Explain {
                    profile: true,
                    statement,
                } => collect(statement, usernames),
                _ => {}
            }
        }

        let mut usernames = std::collections::BTreeSet::new();
        collect(stmt, &mut usernames);
        usernames
    }

    fn is_admin_only_statement(stmt: &byoridb_parser::Statement) -> bool {
        use byoridb_parser::ast::{AlterStatement, CreateStatement, DropStatement, ShowStatement};
        use byoridb_parser::Statement;
        match stmt {
            Statement::Show(
                ShowStatement::Users | ShowStatement::Roles | ShowStatement::Sessions,
            )
            | Statement::Create(CreateStatement::User(_))
            | Statement::Alter(AlterStatement::User(_))
            | Statement::Drop(DropStatement::User(_))
            | Statement::Grant(_)
            | Statement::Revoke(_)
            | Statement::Balance(_) => true,
            Statement::Compound(clauses) => clauses
                .iter()
                .any(|clause| Self::is_admin_only_statement(&clause.stmt)),
            Statement::Explain { statement, .. } => Self::is_admin_only_statement(statement),
            _ => false,
        }
    }

    /// Permissions for every statement that will actually execute. `EXPLAIN`
    /// only plans its inner statement, while `PROFILE` executes it.
    fn required_permissions(stmt: &byoridb_parser::Statement) -> Vec<crate::auth::Permission> {
        use crate::auth::Permission;
        use byoridb_parser::Statement;

        fn collect(stmt: &Statement, permissions: &mut Vec<Permission>) {
            match stmt {
                Statement::Compound(clauses) => {
                    if clauses.is_empty() {
                        permissions.push(Permission::Read);
                    } else {
                        for clause in clauses {
                            collect(&clause.stmt, permissions);
                        }
                    }
                }
                Statement::Explain {
                    profile: true,
                    statement,
                } => collect(statement, permissions),
                Statement::Explain { profile: false, .. } => permissions.push(Permission::Read),
                other => permissions.push(GraphService::statement_permission(other)),
            }
        }

        let mut permissions = Vec::new();
        collect(stmt, &mut permissions);
        permissions
    }

    /// Bounded error classification for metrics and logs. Dynamic error text
    /// may contain user data and must never become a Prometheus label.
    pub(crate) fn error_kind(error: &GraphError) -> &'static str {
        match error {
            GraphError::AuthFailed(_) => "auth_failed",
            GraphError::TooManyAttempts { .. } => "too_many_attempts",
            GraphError::SessionNotFound(_) => "session_not_found",
            GraphError::ParseError(_) => "parse_error",
            GraphError::ValidationError(_) => "validation_error",
            GraphError::PlanningError(_) => "planning_error",
            GraphError::ExecutionError(_) => "execution_error",
            GraphError::Storage(_) => "storage_error",
            GraphError::BadSyntax(_) => "bad_syntax",
            GraphError::SemanticError(_) => "semantic_error",
            GraphError::InvalidOperation(_) => "invalid_operation",
            GraphError::InternalError(_) => "internal_error",
        }
    }

    /// Map a statement to the minimum required Permission.
    fn statement_permission(stmt: &byoridb_parser::Statement) -> crate::auth::Permission {
        use crate::auth::Permission;
        use byoridb_parser::Statement;
        match stmt {
            // Read-only
            Statement::Show(_)
            | Statement::Describe(_)
            | Statement::Fetch(_)
            | Statement::Go(_)
            | Statement::Match(_)
            | Statement::Lookup(_)
            | Statement::Find(_)
            | Statement::Recommend(_)
            | Statement::ExplainInference { .. }
            | Statement::CheckConsistency
            | Statement::CheckShape => Permission::Read,
            // Write
            Statement::Insert(_) | Statement::Update(_) | Statement::Delete(_) => Permission::Write,
            // Create / schema
            Statement::Create(_) | Statement::Alter(_) | Statement::Balance(_) => {
                Permission::Create
            }
            // Drop
            Statement::Drop(_) => Permission::Drop,
            // User management — requires GOD/ADMIN (Create covers it; GOD has all)
            Statement::Grant(_) | Statement::Revoke(_) => Permission::Create,
            // USE space — read is sufficient
            Statement::Use(_) => Permission::Read,
            Statement::Compound(_) => Permission::Read, // expanded by required_permissions
            Statement::Explain { .. } => Permission::Read,
        }
    }

    /// Determine query type from statement for metrics
    fn get_query_type(stmt: &byoridb_parser::Statement) -> QueryType {
        use byoridb_parser::Statement;
        match stmt {
            Statement::Show(_) => QueryType::Show,
            Statement::Describe(_) => QueryType::Show, // DESCRIBE is metadata, use Show metrics
            Statement::Use(_) => QueryType::Use,
            Statement::Create(_) => QueryType::Create,
            Statement::Alter(_) => QueryType::Alter,
            Statement::Drop(_) => QueryType::Drop,
            Statement::Insert(_) => QueryType::Insert,
            Statement::Update(_) => QueryType::Update,
            Statement::Delete(_) => QueryType::Delete,
            Statement::Fetch(_) => QueryType::Fetch,
            Statement::Go(_) => QueryType::Go,
            Statement::Match(_) => QueryType::Match,
            Statement::Lookup(_) => QueryType::Lookup,
            Statement::Find(_) => QueryType::Find,
            Statement::Recommend(_) => QueryType::Recommend,
            Statement::CheckConsistency => QueryType::Show,
            Statement::CheckShape => QueryType::Show,
            Statement::ExplainInference { .. } => QueryType::Show,
            Statement::Grant(_) => QueryType::Create, // User management uses Create metrics
            Statement::Revoke(_) => QueryType::Create,
            Statement::Balance(_) => QueryType::Show, // Admin commands use Show metrics
            // Compound queries are categorized by their dominant trailing
            // clause for metrics; default to Go since `$var = ...` chains
            // are nearly always GO traversal pipelines today.
            Statement::Compound(_) => QueryType::Go,
            Statement::Explain { .. } => QueryType::Show,
        }
    }

    /// Execute a query with parameters
    pub async fn execute_with_params(
        &self,
        session_id: i64,
        stmt: String,
        _params: std::collections::HashMap<String, String>,
    ) -> Result<DataSet> {
        self.execute(session_id, stmt).await
    }

    /// Execute a query and return JSON result
    pub async fn execute_json(&self, session_id: i64, stmt: String) -> Result<String> {
        self.execute_json_with_options(session_id, stmt, QueryOptions::default())
            .await
    }

    /// [`Self::execute_json`] under caller-supplied constraints. The JSON path
    /// must honor [`QueryOptions`] too: a silently ignored `read_only` would be
    /// worse than one that does not exist, because a caller would believe a
    /// guarantee it never got.
    pub async fn execute_json_with_options(
        &self,
        session_id: i64,
        stmt: String,
        options: QueryOptions,
    ) -> Result<String> {
        let dataset = self.execute_with_options(session_id, stmt, options).await?;
        serde_json::to_string(&dataset).map_err(|e| GraphError::ExecutionError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthManager;
    use byoridb_kvstore::MemoryKVStore;

    const ROOT_PASSWORD: &str = "root-test-password";

    fn test_service(kvstore: Arc<MemoryKVStore>) -> GraphService {
        GraphService::with_auth(
            kvstore,
            AuthManager::with_config(ROOT_PASSWORD, Duration::from_secs(3600)),
        )
    }

    async fn root_session(service: &GraphService) -> i64 {
        service
            .authenticate("root".to_string(), ROOT_PASSWORD.to_string())
            .await
            .unwrap()
    }

    #[test]
    fn sensitive_credential_detection_covers_missing_password_keyword() {
        for query in [
            "CREATE USER alice WITH PASSWORD secret",
            "ALTER USER alice WITH password /* comment */ secret",
            "CREATE USER alice WITH \"secret\"",
            "SHOW SPACES; ALTER USER alice WITH \"secret\"",
            "CREATE /* ; */ USER alice WITH \"secret\"",
            "SHOW PASSWORD",
        ] {
            assert!(
                contains_sensitive_credential_syntax(query),
                "query: {query}"
            );
        }

        for query in ["", "SHOW password_hash", "SHOW myPassword", "PASSWORD2"] {
            assert!(
                !contains_sensitive_credential_syntax(query),
                "query: {query}"
            );
        }
    }

    #[tokio::test]
    async fn malformed_credential_statements_do_not_echo_passwords() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;

        for query in [
            "CREATE USER malformed WITH PASSWORD supersecret123",
            "CREATE USER malformed WITH PaSsWoRd 'supersecret123'",
            "ALTER USER malformed WITH PASSWORD /* comment */ supersecret123",
            "CREATE USER malformed WITH \"supersecret123\"",
            "ALTER USER malformed WITH \"supersecret123\"",
            "SHOW SPACES; CREATE USER malformed WITH \"supersecret123\"",
            "CREATE /* ; */ USER malformed WITH \"supersecret123\"",
        ] {
            let error = service.execute(root, query.to_string()).await.unwrap_err();
            assert!(matches!(error, GraphError::ParseError(_)));
            assert_eq!(
                error.to_string(),
                "Parse error: invalid credential statement"
            );
            assert!(!error.to_string().contains("supersecret123"));
        }
    }

    #[tokio::test]
    async fn user_ddl_rejects_blank_passwords_without_revoking_valid_sessions() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;

        let create_error = service
            .execute(
                root,
                "CREATE USER blank_user WITH PASSWORD \"   \" ROLE USER".to_string(),
            )
            .await
            .unwrap_err();
        assert!(matches!(create_error, GraphError::InvalidOperation(_)));
        assert!(service
            .authenticate("blank_user".to_string(), "   ".to_string())
            .await
            .is_err());

        service
            .execute(
                root,
                "CREATE USER alice WITH PASSWORD \"old-password\" ROLE USER".to_string(),
            )
            .await
            .unwrap();
        let alice = service
            .authenticate("alice".to_string(), "old-password".to_string())
            .await
            .unwrap();
        let alter_error = service
            .execute(root, "ALTER USER alice WITH PASSWORD \" \t \"".to_string())
            .await
            .unwrap_err();
        assert!(matches!(alter_error, GraphError::InvalidOperation(_)));
        assert!(service
            .execute(alice, "SHOW SPACES".to_string())
            .await
            .is_ok());
        assert!(service
            .authenticate("alice".to_string(), "old-password".to_string())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn sign_out_invalidates_both_session_stores() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let session = root_session(&service).await;

        service.sign_out(session, session).await.unwrap();

        assert!(
            !service
                .auth_manager
                .has_live_session_without_touch(session)
                .await
        );
        assert!(!service.session_manager.has_session(session).await);
    }

    #[tokio::test]
    async fn sign_out_expired_session_returns_not_found_and_cleans_both_stores() {
        let service = GraphService::with_auth(
            Arc::new(MemoryKVStore::new()),
            AuthManager::with_config(ROOT_PASSWORD, Duration::from_millis(20)),
        );
        let session = root_session(&service).await;
        tokio::time::sleep(Duration::from_millis(30)).await;

        let error = service.sign_out(session, session).await.unwrap_err();

        assert!(matches!(error, GraphError::SessionNotFound(_)));
        assert!(
            !service
                .auth_manager
                .has_live_session_without_touch(session)
                .await
        );
        assert!(!service.session_manager.has_session(session).await);
    }

    #[tokio::test]
    async fn sign_out_auth_only_session_returns_not_found_and_cleans_auth_store() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let session = root_session(&service).await;
        assert!(service.session_manager.remove_session(session).await);

        let error = service.sign_out(session, session).await.unwrap_err();

        assert!(matches!(error, GraphError::SessionNotFound(_)));
        assert!(
            !service
                .auth_manager
                .has_live_session_without_touch(session)
                .await
        );
        assert!(!service.session_manager.has_session(session).await);
    }

    #[tokio::test]
    async fn sign_out_graph_only_session_returns_not_found_and_cleans_graph_store() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let session = root_session(&service).await;
        service.auth_manager.sign_out(session).await.unwrap();

        let error = service.sign_out(session, session).await.unwrap_err();

        assert!(matches!(error, GraphError::SessionNotFound(_)));
        assert!(
            !service
                .auth_manager
                .has_live_session_without_touch(session)
                .await
        );
        assert!(!service.session_manager.has_session(session).await);
    }

    #[tokio::test]
    async fn sign_out_unknown_session_returns_not_found() {
        let service = test_service(Arc::new(MemoryKVStore::new()));

        let error = service.sign_out(99_999_999, 99_999_999).await.unwrap_err();

        assert!(matches!(error, GraphError::SessionNotFound(_)));
    }

    #[tokio::test]
    async fn non_admin_cannot_sign_out_another_session() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        service
            .auth_manager
            .create_user("alice", "alice-password", vec!["USER".to_string()])
            .await
            .unwrap();
        let caller = service
            .authenticate("alice".to_string(), "alice-password".to_string())
            .await
            .unwrap();
        let target = service
            .authenticate("alice".to_string(), "alice-password".to_string())
            .await
            .unwrap();

        let error = service.sign_out(caller, target).await.unwrap_err();

        assert!(matches!(error, GraphError::InvalidOperation(_)));
        assert!(service.validate_session(target).await.is_ok());
    }

    #[tokio::test]
    async fn admin_can_sign_out_another_session() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;
        service
            .auth_manager
            .create_user("alice", "alice-password", vec!["USER".to_string()])
            .await
            .unwrap();
        let target = service
            .authenticate("alice".to_string(), "alice-password".to_string())
            .await
            .unwrap();

        service.sign_out(root, target).await.unwrap();

        assert!(matches!(
            service.validate_session(target).await,
            Err(GraphError::SessionNotFound(_))
        ));
        assert!(service.validate_session(root).await.is_ok());
    }

    #[tokio::test]
    async fn auth_only_admin_cannot_sign_out_another_session() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;
        service
            .auth_manager
            .create_user("alice", "alice-password", vec!["USER".to_string()])
            .await
            .unwrap();
        let target = service
            .authenticate("alice".to_string(), "alice-password".to_string())
            .await
            .unwrap();
        assert!(service.session_manager.remove_session(root).await);

        let error = service.sign_out(root, target).await.unwrap_err();

        assert!(matches!(error, GraphError::SessionNotFound(_)));
        assert!(
            !service
                .auth_manager
                .has_live_session_without_touch(root)
                .await
        );
        assert!(!service.session_manager.has_session(root).await);
        assert!(service.validate_session(target).await.is_ok());
    }

    #[tokio::test]
    async fn admin_cannot_sign_out_an_auth_only_target() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;
        service
            .auth_manager
            .create_user("alice", "alice-password", vec!["USER".to_string()])
            .await
            .unwrap();
        let target = service
            .authenticate("alice".to_string(), "alice-password".to_string())
            .await
            .unwrap();
        assert!(service.session_manager.remove_session(target).await);

        let error = service.sign_out(root, target).await.unwrap_err();

        assert!(matches!(error, GraphError::SessionNotFound(_)));
        assert!(
            !service
                .auth_manager
                .has_live_session_without_touch(target)
                .await
        );
        assert!(!service.session_manager.has_session(target).await);
        assert!(service.validate_session(root).await.is_ok());
    }

    #[tokio::test]
    async fn admin_cannot_sign_out_a_graph_only_target() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;
        service
            .auth_manager
            .create_user("alice", "alice-password", vec!["USER".to_string()])
            .await
            .unwrap();
        let target = service
            .authenticate("alice".to_string(), "alice-password".to_string())
            .await
            .unwrap();
        service.auth_manager.sign_out(target).await.unwrap();

        let error = service.sign_out(root, target).await.unwrap_err();

        assert!(matches!(error, GraphError::SessionNotFound(_)));
        assert!(
            !service
                .auth_manager
                .has_live_session_without_touch(target)
                .await
        );
        assert!(!service.session_manager.has_session(target).await);
        assert!(service.validate_session(root).await.is_ok());
    }

    #[tokio::test]
    async fn persisted_users_are_hydrated_on_restart() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let first = test_service(kvstore.clone());
        let root = root_session(&first).await;
        first
            .execute(
                root,
                "CREATE USER alice WITH PASSWORD \"alice-password\" ROLE USER".to_string(),
            )
            .await
            .unwrap();

        let restarted = test_service(kvstore);
        assert_eq!(restarted.hydrate_persisted_users().await.unwrap(), 1);
        assert!(restarted
            .authenticate("alice".to_string(), "alice-password".to_string())
            .await
            .is_ok());
    }

    #[test]
    fn alter_statements_use_the_alter_observability_label() {
        let statement =
            byoridb_parser::parse("ALTER USER alice WITH PASSWORD \"replacement-password\"")
                .unwrap();
        assert_eq!(GraphService::get_query_type(&statement).as_str(), "alter");
    }

    #[tokio::test]
    async fn mixed_case_persisted_root_is_never_hydrated_or_listed_twice() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let legacy_root = User {
            username: "ROOT".to_string(),
            password_hash: byoridb_common::crypto::hash_password("legacy-password").unwrap(),
            roles: vec!["GOD".to_string()],
            enabled: true,
        };
        kvstore
            .put(b"__user_ROOT", &serde_json::to_vec(&legacy_root).unwrap())
            .await
            .unwrap();

        let service = test_service(kvstore);
        assert_eq!(service.hydrate_persisted_users().await.unwrap(), 0);
        assert!(service
            .authenticate("ROOT".to_string(), "legacy-password".to_string())
            .await
            .is_err());

        let root = root_session(&service).await;
        let users = service
            .execute(root, "SHOW USERS".to_string())
            .await
            .unwrap();
        let root_rows = users
            .rows
            .iter()
            .filter(|row| {
                matches!(
                    row.first(),
                    Some(byoridb_common::Value::String(username))
                        if username.eq_ignore_ascii_case("root")
                )
            })
            .count();
        assert_eq!(root_rows, 1);
    }

    #[tokio::test]
    async fn persisted_non_root_god_role_fails_hydration() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let legacy_user = User {
            username: "legacy-super".to_string(),
            password_hash: byoridb_common::crypto::hash_password("legacy-password").unwrap(),
            roles: vec!["god".to_string()],
            enabled: true,
        };
        kvstore
            .put(
                b"__user_legacy-super",
                &serde_json::to_vec(&legacy_user).unwrap(),
            )
            .await
            .unwrap();

        let service = test_service(kvstore);
        let error = service.hydrate_persisted_users().await.unwrap_err();
        assert!(matches!(error, GraphError::InvalidOperation(_)));
        assert!(!error.to_string().contains("legacy-super"));
        assert!(service
            .authenticate("legacy-super".to_string(), "legacy-password".to_string())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn mixed_case_root_cannot_be_dropped_or_altered() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;
        for statement in [
            "DROP USER ROOT",
            "ALTER USER RoOt WITH PASSWORD \"replacement\"",
        ] {
            let error = service
                .execute(root, statement.to_string())
                .await
                .unwrap_err();
            assert!(
                matches!(error, GraphError::InvalidOperation(_)),
                "{statement} returned {error:?}"
            );
        }
        assert!(service.validate_session(root).await.is_ok());
        assert!(service
            .authenticate("root".to_string(), ROOT_PASSWORD.to_string())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn alter_user_takes_effect_immediately_and_revokes_old_session() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;
        service
            .execute(
                root,
                "CREATE USER alice WITH PASSWORD \"old-password\" ROLE USER".to_string(),
            )
            .await
            .unwrap();
        let old_session = service
            .authenticate("alice".to_string(), "old-password".to_string())
            .await
            .unwrap();

        service
            .execute(
                root,
                "ALTER USER alice WITH PASSWORD \"new-password\"".to_string(),
            )
            .await
            .unwrap();

        assert!(service.validate_session(old_session).await.is_err());
        assert!(service
            .authenticate("alice".to_string(), "old-password".to_string())
            .await
            .is_err());
        assert!(service
            .authenticate("alice".to_string(), "new-password".to_string())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn role_changes_take_effect_immediately_and_revoke_old_sessions() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;
        service
            .execute(
                root,
                "CREATE USER role_target WITH PASSWORD \"role-password\" ROLE USER".to_string(),
            )
            .await
            .unwrap();

        let before_grant = service
            .authenticate("role_target".to_string(), "role-password".to_string())
            .await
            .unwrap();
        service
            .execute(root, "GRANT ROLE ADMIN TO role_target".to_string())
            .await
            .unwrap();
        assert!(matches!(
            service
                .execute(before_grant, "SHOW SPACES".to_string())
                .await,
            Err(GraphError::SessionNotFound(_))
        ));

        let after_grant = service
            .authenticate("role_target".to_string(), "role-password".to_string())
            .await
            .unwrap();
        service
            .execute(after_grant, "CREATE SPACE role_change_probe".to_string())
            .await
            .unwrap();

        service
            .execute(root, "REVOKE ROLE ADMIN FROM role_target".to_string())
            .await
            .unwrap();
        assert!(matches!(
            service
                .execute(after_grant, "SHOW SPACES".to_string())
                .await,
            Err(GraphError::SessionNotFound(_))
        ));

        let after_revoke = service
            .authenticate("role_target".to_string(), "role-password".to_string())
            .await
            .unwrap();
        assert!(matches!(
            service
                .execute(after_revoke, "DROP SPACE role_change_probe".to_string())
                .await,
            Err(GraphError::AuthFailed(_))
        ));
    }

    #[tokio::test]
    async fn auth_sync_orphan_cleanup_preserves_concurrent_new_login() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;
        service
            .execute(
                root,
                "CREATE USER alice WITH PASSWORD \"old-password\" ROLE USER".to_string(),
            )
            .await
            .unwrap();
        let old_session = service
            .authenticate("alice".to_string(), "old-password".to_string())
            .await
            .unwrap();

        // Reproduce the critical interleaving: auth state is replaced (which
        // revokes old auth IDs), then a new login completes before graph-layer
        // reconciliation runs.
        let replacement = User {
            username: "alice".to_string(),
            password_hash: byoridb_common::crypto::hash_password("new-password").unwrap(),
            roles: vec!["USER".to_string()],
            enabled: true,
        };
        assert!(service
            .auth_manager
            .upsert_persisted_user(replacement)
            .await
            .unwrap());
        let new_session = service
            .authenticate("alice".to_string(), "new-password".to_string())
            .await
            .unwrap();

        assert_eq!(service.remove_orphan_graph_sessions().await, 1);
        assert!(service.validate_session(old_session).await.is_err());
        assert!(service.validate_session(new_session).await.is_ok());
    }

    #[tokio::test]
    async fn expired_auth_token_cannot_extend_graph_session() {
        let service = GraphService::with_auth(
            Arc::new(MemoryKVStore::new()),
            AuthManager::with_config(ROOT_PASSWORD, Duration::from_millis(50)),
        );
        let session = root_session(&service).await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        for _ in 0..2 {
            let error = service
                .execute(session, "SHOW SPACES".to_string())
                .await
                .unwrap_err();
            assert!(matches!(error, GraphError::SessionNotFound(_)));
            assert!(!service.session_manager.has_session(session).await);
        }
    }

    #[tokio::test]
    async fn missing_graph_session_revokes_remaining_auth_session() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let session = root_session(&service).await;
        service.session_manager.remove_session(session).await;

        let error = service
            .execute(session, "SHOW SPACES".to_string())
            .await
            .unwrap_err();
        assert!(matches!(error, GraphError::SessionNotFound(_)));
        assert!(service
            .auth_manager
            .get_session_roles(session)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn account_and_session_enumeration_require_admin_and_hide_tokens() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;
        service
            .execute(
                root,
                "CREATE USER reader WITH PASSWORD \"reader-password\" ROLE USER".to_string(),
            )
            .await
            .unwrap();
        let reader = service
            .authenticate("reader".to_string(), "reader-password".to_string())
            .await
            .unwrap();

        for statement in ["SHOW USERS", "SHOW ROLES", "SHOW SESSIONS"] {
            let error = service
                .execute(reader, statement.to_string())
                .await
                .unwrap_err();
            assert!(
                matches!(error, GraphError::AuthFailed(_)),
                "{statement} returned {error:?}"
            );
        }

        let sessions = service
            .execute(root, "SHOW SESSIONS".to_string())
            .await
            .unwrap();
        assert_eq!(sessions.column_names, vec!["User", "Space"]);
        assert!(sessions
            .rows
            .iter()
            .flatten()
            .all(|value| !matches!(value, byoridb_common::Value::Int(_))));
    }

    #[tokio::test]
    async fn dba_cannot_bypass_admin_only_user_management() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;
        service
            .execute(
                root,
                "CREATE USER target WITH PASSWORD \"old-password\" ROLE USER".to_string(),
            )
            .await
            .unwrap();
        service
            .execute(
                root,
                "CREATE USER dba_user WITH PASSWORD \"dba-password\" ROLE DBA".to_string(),
            )
            .await
            .unwrap();
        let dba = service
            .authenticate("dba_user".to_string(), "dba-password".to_string())
            .await
            .unwrap();

        let error = service
            .execute(
                dba,
                "ALTER USER target WITH PASSWORD \"stolen-password\"".to_string(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, GraphError::AuthFailed(_)));
        assert!(service
            .authenticate("target".to_string(), "old-password".to_string())
            .await
            .is_ok());
        assert!(service
            .authenticate("target".to_string(), "stolen-password".to_string())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn compound_and_profile_authorize_every_executed_statement() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let service = test_service(kvstore.clone());
        let root = root_session(&service).await;
        service
            .execute(root, "CREATE SPACE protected".to_string())
            .await
            .unwrap();
        service
            .execute(
                root,
                "CREATE USER reader WITH PASSWORD \"reader-password\" ROLE USER".to_string(),
            )
            .await
            .unwrap();
        let reader = service
            .authenticate("reader".to_string(), "reader-password".to_string())
            .await
            .unwrap();

        for statement in [
            "SHOW SPACES; DROP SPACE protected",
            "PROFILE DROP SPACE protected",
        ] {
            let error = service
                .execute(reader, statement.to_string())
                .await
                .unwrap_err();
            assert!(
                matches!(error, GraphError::AuthFailed(_)),
                "{statement} returned {error:?}"
            );
            assert!(kvstore
                .get(byoridb_executor::key::SchemaKey::space("protected").as_slice())
                .await
                .unwrap()
                .is_some());
        }
    }

    #[tokio::test]
    async fn failed_compound_reconciles_committed_user_mutation() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;
        let error = service
            .execute(
                root,
                concat!(
                    "CREATE USER partial WITH PASSWORD \"partial-password\" ROLE USER; ",
                    "DROP USER root"
                )
                .to_string(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, GraphError::InvalidOperation(_)));

        // The first clause committed before DROP root failed. The in-memory
        // cache must still converge to that final persisted state.
        assert!(service
            .authenticate("partial".to_string(), "partial-password".to_string())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn create_then_drop_compound_converges_to_absent_user() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let root = root_session(&service).await;
        service
            .execute(
                root,
                concat!(
                    "CREATE USER transient WITH PASSWORD \"transient-password\" ROLE USER; ",
                    "DROP USER transient"
                )
                .to_string(),
            )
            .await
            .unwrap();
        assert!(service
            .authenticate("transient".to_string(), "transient-password".to_string())
            .await
            .is_err());
    }

    #[test]
    fn running_query_serialization_contains_only_safe_metadata() {
        let running = RunningQuery {
            id: 7,
            query_type: "insert",
            query_length_bytes: 123,
            space: "default".to_string(),
            started_at_ms: 42,
        };
        let value = serde_json::to_value(running).unwrap();
        assert!(value.get("session_id").is_none());
        assert!(value.get("query").is_none());
        assert_eq!(value["query_type"], "insert");
        assert_eq!(value["query_length_bytes"], 123);
    }

    #[test]
    fn error_kind_never_returns_dynamic_error_text() {
        let private = GraphError::ParseError(
            "INSERT VERTEX person(email) VALUES 1:(\"private@example.com\")".to_string(),
        );
        assert_eq!(GraphService::error_kind(&private), "parse_error");
    }

    /// A read-only request must refuse every mutating shape, including the two
    /// that a client-side keyword scrubber has the most trouble with: a mutation
    /// hidden behind a read in a compound statement, and one wrapped in PROFILE
    /// (which executes its inner statement, unlike plain EXPLAIN).
    #[tokio::test]
    async fn read_only_request_refuses_mutations_including_compound_and_profile() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let session = service
            .authenticate("root".to_string(), ROOT_PASSWORD.to_string())
            .await
            .unwrap();

        for stmt in [
            "CREATE SPACE ro_probe",
            "INSERT VERTEX person(name) VALUES 1:('ada')",
            r#"UPDATE VERTEX ON person 1 SET name = "grace""#,
            "DELETE VERTEX 1",
            "DROP SPACE ro_probe",
            "CREATE TAG person(name STRING)",
            // A read first, so a scrubber keying on the leading token passes it.
            "SHOW SPACES; DELETE VERTEX 1",
            // PROFILE executes the inner statement.
            "PROFILE INSERT VERTEX person(name) VALUES 1:('ada')",
        ] {
            let error = service
                .execute_with_options(session, stmt.to_string(), QueryOptions { read_only: true })
                .await
                .expect_err(&format!("read-only must refuse: {stmt}"));
            assert!(
                matches!(error, GraphError::AuthFailed(_)),
                "read-only refusal should be an authorization failure for {stmt}, got {error:?}"
            );
        }
    }

    /// Administrative statements require only `Permission::Read`, so without an
    /// explicit clause a root read-only request would still enumerate users and
    /// live sessions.
    #[tokio::test]
    async fn read_only_request_refuses_administrative_reads() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let session = service
            .authenticate("root".to_string(), ROOT_PASSWORD.to_string())
            .await
            .unwrap();

        for stmt in ["SHOW USERS", "SHOW ROLES", "SHOW SESSIONS"] {
            let error = service
                .execute_with_options(session, stmt.to_string(), QueryOptions { read_only: true })
                .await
                .expect_err(&format!("read-only must refuse administrative {stmt}"));
            assert!(
                matches!(error, GraphError::AuthFailed(_)),
                "{stmt}: {error:?}"
            );
        }

        // The same statements still work without the flag, so the refusal comes
        // from the request constraint and not from the session's authority.
        service
            .execute(session, "SHOW USERS".to_string())
            .await
            .expect("root should be able to list users on an unconstrained request");
    }

    /// Reads must still work, and the flag must leave no residue: the very next
    /// request on the same session writes. A byori coordinator reads and writes
    /// over one connection and cannot afford a sticky downgrade.
    #[tokio::test]
    async fn read_only_permits_reads_and_leaves_the_session_writable() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let session = service
            .authenticate("root".to_string(), ROOT_PASSWORD.to_string())
            .await
            .unwrap();

        for stmt in [
            "CREATE SPACE ro_ok",
            "USE ro_ok",
            "CREATE TAG p(name STRING)",
        ] {
            service.execute(session, stmt.to_string()).await.unwrap();
        }

        for stmt in [
            "SHOW SPACES",
            "MATCH (n:p) RETURN n LIMIT 1",
            "LOOKUP ON p WHERE p.name == 'ada'",
            // Read-only compound statements are fine; a caller no longer has to
            // ban semicolons to stay safe.
            "SHOW SPACES; SHOW TAGS",
            // Plain EXPLAIN does not execute its inner statement.
            "EXPLAIN INSERT VERTEX p(name) VALUES 1:('ada')",
        ] {
            service
                .execute_with_options(session, stmt.to_string(), QueryOptions { read_only: true })
                .await
                .unwrap_or_else(|e| panic!("read-only should permit {stmt}: {e:?}"));
        }

        // No residue.
        service
            .execute(
                session,
                "INSERT VERTEX p(name) VALUES 1:('ada')".to_string(),
            )
            .await
            .expect("the session must still be writable after a read-only request");
    }

    /// The JSON entry point must honor the flag too. A silently ignored
    /// `read_only` is worse than none: the caller believes a guarantee it never
    /// received.
    #[tokio::test]
    async fn read_only_is_enforced_on_the_json_entry_point() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let session = service
            .authenticate("root".to_string(), ROOT_PASSWORD.to_string())
            .await
            .unwrap();

        let error = service
            .execute_json_with_options(
                session,
                "CREATE SPACE ro_json".to_string(),
                QueryOptions { read_only: true },
            )
            .await
            .expect_err("the JSON path must refuse a mutation under read_only");
        assert!(matches!(error, GraphError::AuthFailed(_)), "{error:?}");

        service
            .execute_json_with_options(
                session,
                "SHOW SPACES".to_string(),
                QueryOptions { read_only: true },
            )
            .await
            .expect("the JSON path must still allow reads");
    }

    /// Default options are exactly today's behavior.
    #[tokio::test]
    async fn default_options_do_not_constrain_anything() {
        let service = test_service(Arc::new(MemoryKVStore::new()));
        let session = service
            .authenticate("root".to_string(), ROOT_PASSWORD.to_string())
            .await
            .unwrap();

        assert!(!QueryOptions::default().read_only);
        service
            .execute_with_options(
                session,
                "CREATE SPACE ro_default".to_string(),
                QueryOptions::default(),
            )
            .await
            .expect("an unconstrained request must behave as before");
    }
}
