// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Graph service implementation

use super::auth::{AuthManager, Permission, User};
use super::error::{GraphError, Result};
use super::metrics::{QueryTimer, QueryType};
use super::session::SessionManager;
use byoridb_common::DataSet;
use byoridb_executor::key::USER_KEY_PREFIX;
use byoridb_kvstore::KVStore;
use dashmap::DashMap;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tracing::{debug, info};

/// A query currently executing, exposed via the diagnostics endpoint so an
/// operator can see what the server is working on (and whether work continues
/// after an HTTP client timed out).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RunningQuery {
    pub id: u64,
    #[serde(skip_serializing)]
    pub session_id: i64,
    pub query_type: &'static str,
    pub query: String,
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

/// Public errors use the protocol's invalid-session sentinel instead of
/// reflecting a caller-supplied bearer credential into gRPC/HTTP responses.
const REDACTED_SESSION_ID: i64 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthorizationRequirement {
    Permission(Permission),
    Admin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthorizationCheck {
    requirement: AuthorizationRequirement,
    space: Option<String>,
}

impl AuthorizationCheck {
    fn permission(permission: Permission, space: &str) -> Self {
        Self {
            requirement: AuthorizationRequirement::Permission(permission),
            space: Some(space.to_string()),
        }
    }

    fn admin() -> Self {
        Self {
            requirement: AuthorizationRequirement::Admin,
            space: None,
        }
    }
}

/// Redact a credential-bearing query before it reaches logs, metrics, or
/// diagnostics. Once a standalone `PASSWORD` keyword is seen, the remainder is
/// intentionally hidden. This conservative rule also covers malformed input
/// and comments between `PASSWORD` and its value.
pub fn redact_sensitive_query(query: &str) -> String {
    redact_sensitive_query_with_flag(query).0
}

fn redact_sensitive_query_with_flag(query: &str) -> (String, bool) {
    const KEYWORD: &[u8] = b"PASSWORD";
    let bytes = query.as_bytes();
    if bytes.len() < KEYWORD.len() {
        return (query.to_string(), false);
    }

    for start in 0..=bytes.len() - KEYWORD.len() {
        let end = start + KEYWORD.len();
        let before_is_identifier = start > 0 && is_identifier_byte(bytes[start - 1]);
        let after_is_identifier = end < bytes.len() && is_identifier_byte(bytes[end]);
        if !before_is_identifier
            && !after_is_identifier
            && bytes[start..end].eq_ignore_ascii_case(KEYWORD)
        {
            let mut redacted = query[..end].to_string();
            redacted.push_str(" <redacted>");
            return (redacted, true);
        }
    }

    (query.to_string(), false)
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Graph service
pub struct GraphService {
    session_manager: Arc<SessionManager>,
    auth_manager: Arc<AuthManager>,
    kvstore: Arc<dyn KVStore>,
    /// Queries currently executing, keyed by a monotonic id.
    active_queries: Arc<DashMap<u64, RunningQuery>>,
    query_seq: Arc<AtomicU64>,
    /// Readiness flag + in-flight drain counter, shared with the server
    /// binary's signal handler (and across the gRPC/HTTP service instances).
    shutdown: Arc<crate::shutdown::ShutdownState>,
    /// Coordinates durable user hydration/authentication with user mutations so
    /// a concurrent DROP/ALTER cannot be undone by a stale pre-mutation read.
    /// Read-side operations may proceed concurrently; mutations are exclusive.
    user_auth_lock: Arc<tokio::sync::RwLock<()>>,
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
            shutdown: Arc::new(crate::shutdown::ShutdownState::new()),
            user_auth_lock: Arc::new(tokio::sync::RwLock::new(())),
        }
    }

    /// Construct a [`GraphService`] with a caller-supplied [`AuthManager`].
    ///
    /// Embedded / in-process callers use this to inject an auth manager built
    /// with a known root password (e.g. [`AuthManager::with_config`]) so they
    /// can authenticate without depending on the `BYORIDB_ROOT_PASSWORD` env var
    /// or the random password generated by [`GraphService::new`]. The server
    /// path keeps using [`GraphService::new`] unchanged.
    pub fn with_auth(kvstore: Arc<dyn KVStore>, auth_manager: AuthManager) -> Self {
        GraphService {
            session_manager: Arc::new(SessionManager::new()),
            auth_manager: Arc::new(auth_manager),
            kvstore,
            active_queries: Arc::new(DashMap::new()),
            query_seq: Arc::new(AtomicU64::new(1)),
            shutdown: Arc::new(crate::shutdown::ShutdownState::new()),
            user_auth_lock: Arc::new(tokio::sync::RwLock::new(())),
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
        session_id: i64,
        query_type: QueryType,
        query: &str,
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
                session_id,
                query_type: query_type.as_str(),
                query: query.to_string(),
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
        let auth_weak = Arc::downgrade(&self.auth_manager);
        let session_weak = Arc::downgrade(&self.session_manager);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // Skip the immediate first tick.
            ticker.tick().await;

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
                let session_removed = sessions.cleanup_expired();

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
        let username = AuthManager::normalize_username(&username);
        info!(user = %username, "Authentication request");

        // Keep the durable user record, in-memory auth cache, and both session
        // registries in one serialization domain with CREATE/ALTER/DROP/GRANT/
        // REVOKE. Root is deliberately excluded from KV hydration: its env
        // credential is the sole source of truth.
        let _user_auth_guard = self.user_auth_lock.read().await;
        if !username.eq_ignore_ascii_case("root") {
            self.sync_user_from_store(&username, true)
                .await
                .map_err(|error| {
                    debug!(err = %error, "Durable user hydration failed");
                    GraphError::AuthFailed("Authentication failed".to_string())
                })?;
        }

        let session_id = self.auth_manager.authenticate(&username, &password).await?;

        // Register the same bearer in the graph-side space tracker. Never
        // overwrite an existing graph session if an ID collision occurs.
        if !self
            .session_manager
            .create_session_with_id(session_id, username.clone())
            .await
        {
            let _ = self.auth_manager.sign_out(session_id).await;
            return Err(GraphError::InternalError(
                "Unable to register authenticated session".to_string(),
            ));
        }

        debug!(user = %username, "Created authenticated session");

        Ok(session_id)
    }

    /// Return whether `session_id` is a live GOD/ADMIN session. The bearer is
    /// intentionally not logged or included in diagnostics output.
    pub async fn is_admin_session(&self, session_id: i64) -> bool {
        let _user_auth_guard = self.user_auth_lock.read().await;
        self.session_manager.get_session(session_id).await.is_some()
            && self.auth_manager.is_admin_session(session_id).await
    }

    /// Sign out a session.
    ///
    /// `caller_session_id` is the session making the request. A session can
    /// only sign out itself unless the caller has GOD/ADMIN role.
    pub async fn sign_out(&self, caller_session_id: i64, target_session_id: i64) {
        info!("Sign out request");
        let _user_auth_guard = self.user_auth_lock.read().await;

        // Ownership check: caller must own the target session or be an admin.
        if caller_session_id != target_session_id {
            let is_admin = self
                .auth_manager
                .get_session_roles(caller_session_id)
                .await
                .map(|roles| roles.iter().any(|r| r == "GOD" || r == "ADMIN"))
                .unwrap_or(false);

            if !is_admin {
                tracing::warn!("Sign out denied");
                return;
            }
        }

        let _ = self.auth_manager.sign_out(target_session_id).await;
        self.session_manager.remove_session(target_session_id).await;
    }

    /// Execute a query statement
    pub async fn execute(&self, session_id: i64, stmt: String) -> Result<DataSet> {
        let (safe_stmt, contains_password) = redact_sensitive_query_with_flag(&stmt);
        debug!(query = %safe_stmt, "Executing query");

        // Graceful shutdown: once the signal handler flips readiness off, new
        // queries fail fast and clearly; queries already past this gate drain
        // to completion before the servers stop.
        if !self.shutdown.is_accepting() {
            return Err(GraphError::InvalidOperation(
                "server is shutting down; not accepting new queries".to_string(),
            ));
        }

        // Reject obviously invalid bearers before spending work in the parser.
        // The session is revalidated under `user_auth_lock` below to close the
        // authorization race with concurrent role/password/user changes.
        if self.session_manager.get_session(session_id).await.is_none() {
            return Err(GraphError::SessionNotFound(REDACTED_SESSION_ID));
        }

        // Parse query
        let mut statement = byoridb_parser::parse(&stmt).map_err(|error| {
            if contains_password {
                GraphError::ParseError("invalid credential statement".to_string())
            } else {
                GraphError::ParseError(error.to_string())
            }
        })?;
        Self::normalize_security_roles(&mut statement);

        // Every authorization check shares this lock with authentication and
        // user mutations. Read-side work can proceed concurrently; user
        // mutations retain an exclusive guard through durable reconciliation.
        let mutates_users = Self::contains_user_mutation(&statement);
        let user_auth_write_guard = if mutates_users {
            Some(self.user_auth_lock.write().await)
        } else {
            None
        };
        let user_auth_read_guard = if mutates_users {
            None
        } else {
            Some(self.user_auth_lock.read().await)
        };

        let session = match self.session_manager.get_session(session_id).await {
            Some(session) => session,
            None => return Err(GraphError::SessionNotFound(REDACTED_SESSION_ID)),
        };
        let space = session
            .space
            .clone()
            .unwrap_or_else(|| "default".to_string());

        // RBAC: recursively authorize every executable statement before any
        // compound/profile clause can mutate state.
        self.authorize_statement(session_id, &statement, &space)
            .await?;
        drop(user_auth_read_guard);

        // Determine query type for metrics
        let query_type = Self::get_query_type(&statement);

        // Start metrics timer (carries the query text for the slow-query log)
        let timer = QueryTimer::new(query_type, &space)
            .with_slow_threshold(1000) // 1 second threshold
            .with_query(&safe_stmt);

        // Track as in-flight for the lifetime of this call. The guard removes
        // the entry and decrements the gauge on every exit path.
        let _active_guard = self.register_active_query(session_id, query_type, &safe_stmt, &space);

        // Create context
        let mut context = crate::context::ExecutionContext::new(session_id);
        if let Some(space) = session.space {
            context = context.with_space(space);
        }
        // Propagate caller roles for executor-level RBAC (CREATE USER, GRANT, REVOKE)
        if let Some(roles) = self.auth_manager.get_session_roles(session_id).await {
            context = context.with_caller_roles(roles);
        }

        // Plan
        let planner =
            crate::planner::Planner::new(self.session_manager.clone(), self.kvstore.clone());
        let executor = planner.plan(statement.clone(), context.clone())?;
        // Shared flag the executor sets if it falls back to a full scan.
        let full_scan_flag = executor.full_scan_flag();

        // Execute
        let mut result = executor.execute().await;

        // Reconcile even when a compound statement fails: compound execution
        // is sequential without rollback, so earlier user clauses may already
        // have committed. If reconciliation itself fails, evict affected users
        // and sessions to fail closed.
        if mutates_users {
            if let Err(sync_error) = self.sync_auth_manager(&statement).await {
                self.evict_affected_users(&statement).await;
                if result.is_ok() {
                    result = Err(sync_error);
                } else {
                    debug!(err = %sync_error, "User cache reconciliation failed after query error");
                }
            }
        }
        drop(user_auth_write_guard);

        // Record metrics
        match &result {
            Ok(dataset) => {
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
            Err(e) => {
                timer.finish_with_error(&e.to_string());
            }
        }

        result
    }

    async fn load_stored_user(&self, username: &str) -> Result<Option<User>> {
        let username = AuthManager::normalize_username(username);
        if username.eq_ignore_ascii_case("root") {
            return Ok(None);
        }

        let key = format!("{USER_KEY_PREFIX}{username}");
        let Some(bytes) = self
            .kvstore
            .get(key.as_bytes())
            .await
            .map_err(|error| GraphError::Storage(error.to_string()))?
        else {
            return Ok(None);
        };

        let user: User = serde_json::from_slice(&bytes).map_err(|error| {
            GraphError::InternalError(format!("Invalid durable user record: {error}"))
        })?;
        if AuthManager::normalize_username(&user.username) != username {
            return Err(GraphError::InternalError(
                "Durable user key and record do not match".to_string(),
            ));
        }
        Ok(Some(user))
    }

    /// Reconcile one durable user into the authentication cache. Root never
    /// enters this path: environment-backed root credentials are authoritative.
    async fn sync_user_from_store(&self, username: &str, remove_if_missing: bool) -> Result<()> {
        let username = AuthManager::normalize_username(username);
        if username.eq_ignore_ascii_case("root") {
            return Ok(());
        }

        match self.load_stored_user(&username).await? {
            Some(user) => {
                let invalidated = self.auth_manager.upsert_user(user).await?;
                if !invalidated.is_empty() {
                    // A user mutation holds `user_auth_lock` exclusively, so
                    // username-wide cleanup cannot race with a fresh login.
                    self.session_manager
                        .remove_sessions_for_user(&username)
                        .await;
                }
            }
            None if remove_if_missing => {
                self.auth_manager
                    .remove_cached_user_if_present(&username)
                    .await?;
                self.session_manager
                    .remove_sessions_for_user(&username)
                    .await;
            }
            None => {}
        }
        Ok(())
    }

    /// Re-read the final durable state after any user mutation. Reading final
    /// state (rather than replaying AST operations) handles IF EXISTS, compound
    /// create/drop sequences, and password hashing consistently.
    async fn sync_auth_manager(&self, stmt: &byoridb_parser::Statement) -> Result<()> {
        for username in Self::affected_usernames(stmt) {
            self.sync_user_from_store(&username, true).await?;
        }
        Ok(())
    }

    async fn evict_affected_users(&self, stmt: &byoridb_parser::Statement) {
        for username in Self::affected_usernames(stmt) {
            if username.eq_ignore_ascii_case("root") {
                continue;
            }
            if let Err(error) = self
                .auth_manager
                .remove_cached_user_if_present(&username)
                .await
            {
                debug!(user = %username, err = %error, "Failed to evict user after sync error");
            }
            self.session_manager
                .remove_sessions_for_user(&username)
                .await;
        }
    }

    fn affected_usernames(stmt: &byoridb_parser::Statement) -> Vec<String> {
        fn collect(stmt: &byoridb_parser::Statement, users: &mut Vec<String>) {
            use byoridb_parser::ast::{AlterStatement, CreateStatement, DropStatement};
            use byoridb_parser::Statement;

            match stmt {
                Statement::Create(CreateStatement::User(user)) => {
                    users.push(AuthManager::normalize_username(&user.username));
                }
                Statement::Alter(AlterStatement::User(user)) => {
                    users.push(AuthManager::normalize_username(&user.username));
                }
                Statement::Drop(DropStatement::User(user)) => {
                    users.push(AuthManager::normalize_username(&user.username));
                }
                Statement::Grant(grant) => {
                    users.push(AuthManager::normalize_username(&grant.username));
                }
                Statement::Revoke(revoke) => {
                    users.push(AuthManager::normalize_username(&revoke.username));
                }
                Statement::Compound(clauses) => {
                    for clause in clauses {
                        collect(&clause.stmt, users);
                    }
                }
                Statement::Explain {
                    profile: true,
                    statement,
                } => collect(statement, users),
                _ => {}
            }
        }

        let mut users = Vec::new();
        collect(stmt, &mut users);
        let mut seen = HashSet::new();
        users.retain(|username| seen.insert(username.clone()));
        users
    }

    fn contains_user_mutation(stmt: &byoridb_parser::Statement) -> bool {
        !Self::affected_usernames(stmt).is_empty()
    }

    fn normalize_security_roles(stmt: &mut byoridb_parser::Statement) {
        use byoridb_parser::ast::CreateStatement;
        use byoridb_parser::Statement;

        match stmt {
            Statement::Create(CreateStatement::User(user)) => {
                if let Some(role) = &mut user.role {
                    *role = AuthManager::normalize_role(role);
                }
            }
            Statement::Grant(grant) => {
                grant.role = AuthManager::normalize_role(&grant.role);
            }
            Statement::Revoke(revoke) => {
                revoke.role = AuthManager::normalize_role(&revoke.role);
            }
            Statement::Compound(clauses) => {
                for clause in clauses {
                    Self::normalize_security_roles(&mut clause.stmt);
                }
            }
            Statement::Explain { statement, .. } => {
                Self::normalize_security_roles(statement);
            }
            _ => {}
        }
    }

    async fn authorize_statement(
        &self,
        session_id: i64,
        stmt: &byoridb_parser::Statement,
        current_space: &str,
    ) -> Result<()> {
        let mut checks = Vec::new();
        Self::collect_authorization_checks(stmt, current_space, &mut checks);

        for check in checks {
            match check.requirement {
                AuthorizationRequirement::Admin => {
                    if !self.auth_manager.is_admin_session(session_id).await {
                        return Err(GraphError::AuthFailed(
                            "GOD or ADMIN role is required".to_string(),
                        ));
                    }
                }
                AuthorizationRequirement::Permission(permission) => {
                    let target_space = check.space.as_deref().unwrap_or(current_space);
                    if !self
                        .auth_manager
                        .check_permission(session_id, target_space, permission)
                        .await?
                    {
                        return Err(GraphError::AuthFailed(format!(
                            "Permission denied: {permission:?} on space {target_space}"
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    fn collect_authorization_checks(
        stmt: &byoridb_parser::Statement,
        current_space: &str,
        checks: &mut Vec<AuthorizationCheck>,
    ) {
        use byoridb_parser::ast::{
            AlterStatement, CreateStatement, DescribeStatement, DropStatement, ShowStatement,
        };
        use byoridb_parser::Statement;

        match stmt {
            Statement::Compound(clauses) => {
                let mut effective_space = current_space.to_string();
                for clause in clauses {
                    Self::collect_authorization_checks(&clause.stmt, &effective_space, checks);
                    // Only a direct USE changes the context of later sibling
                    // clauses. USE nested in PROFILE/Compound has an inner scope
                    // in the executor and must not escape it.
                    if let Statement::Use(use_stmt) = clause.stmt.as_ref() {
                        effective_space = use_stmt.space.clone();
                    }
                }
            }
            Statement::Explain {
                profile: true,
                statement,
            } => Self::collect_authorization_checks(statement, current_space, checks),
            Statement::Explain { profile: false, .. } => checks.push(
                AuthorizationCheck::permission(Permission::Read, current_space),
            ),
            Statement::Use(use_stmt) => checks.push(AuthorizationCheck::permission(
                Permission::Read,
                &use_stmt.space,
            )),
            Statement::Insert(insert) => checks.push(AuthorizationCheck::permission(
                Permission::Write,
                insert.space.as_deref().unwrap_or(current_space),
            )),
            Statement::Update(update) => checks.push(AuthorizationCheck::permission(
                Permission::Write,
                update.space.as_deref().unwrap_or(current_space),
            )),
            Statement::Delete(delete) => checks.push(AuthorizationCheck::permission(
                Permission::Write,
                delete.space.as_deref().unwrap_or(current_space),
            )),
            Statement::Fetch(fetch) => checks.push(AuthorizationCheck::permission(
                Permission::Read,
                fetch.space.as_deref().unwrap_or(current_space),
            )),
            Statement::Describe(DescribeStatement::Space(space)) => {
                checks.push(AuthorizationCheck::permission(Permission::Read, space))
            }
            Statement::Create(CreateStatement::Space(space)) => checks.push(
                AuthorizationCheck::permission(Permission::Create, &space.name),
            ),
            Statement::Drop(DropStatement::Space(space)) => checks.push(
                AuthorizationCheck::permission(Permission::Drop, &space.name),
            ),
            Statement::Show(
                ShowStatement::Users | ShowStatement::Roles | ShowStatement::Sessions,
            )
            | Statement::Create(CreateStatement::User(_))
            | Statement::Alter(AlterStatement::User(_))
            | Statement::Drop(DropStatement::User(_))
            | Statement::Grant(_)
            | Statement::Revoke(_)
            | Statement::Balance(_) => checks.push(AuthorizationCheck::admin()),
            _ => checks.push(AuthorizationCheck::permission(
                Self::statement_permission(stmt),
                current_space,
            )),
        }
    }

    /// Map a non-recursive statement to its minimum ordinary permission.
    fn statement_permission(stmt: &byoridb_parser::Statement) -> Permission {
        use byoridb_parser::Statement;
        match stmt {
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
            | Statement::CheckShape
            | Statement::Use(_)
            | Statement::Compound(_)
            | Statement::Explain { .. } => Permission::Read,
            Statement::Insert(_) | Statement::Update(_) | Statement::Delete(_) => Permission::Write,
            Statement::Create(_) | Statement::Alter(_) | Statement::Balance(_) => {
                Permission::Create
            }
            Statement::Drop(_) => Permission::Drop,
            Statement::Grant(_) | Statement::Revoke(_) => Permission::Create,
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
            Statement::Alter(_) => QueryType::Create, // ALTER uses Create metrics
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
        let dataset = self.execute(session_id, stmt).await?;
        serde_json::to_string(&dataset).map_err(|e| GraphError::ExecutionError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byoridb_kvstore::MemoryKVStore;
    use byoridb_parser::ast::{CreateStatement, ShowStatement};

    fn authorization_checks(query: &str, current_space: &str) -> Vec<AuthorizationCheck> {
        let statement = byoridb_parser::parse(query).expect("query should parse");
        let mut checks = Vec::new();
        GraphService::collect_authorization_checks(&statement, current_space, &mut checks);
        checks
    }

    #[test]
    fn redactor_handles_short_and_non_sensitive_queries() {
        assert_eq!(redact_sensitive_query(""), "");
        assert_eq!(redact_sensitive_query("SHOW"), "SHOW");
        assert_eq!(
            redact_sensitive_query("SHOW password_hash"),
            "SHOW password_hash"
        );
    }

    #[test]
    fn redactor_hides_quoted_unquoted_malformed_and_commented_passwords() {
        let inputs = [
            r#"CREATE USER alice WITH PASSWORD "double-secret" ROLE USER"#,
            "CREATE USER alice WITH PASSWORD 'single-secret' ROLE USER",
            "ALTER USER alice WITH PASSWORD bare-secret",
            r#"ALTER USER alice WITH PASSWORD /* ignored */ "comment-secret""#,
            "CREATE USER alice WITH PASSWORD 'unterminated-secret",
            r#"CREATE USER a WITH PASSWORD "first"; CREATE USER b WITH PASSWORD "second""#,
        ];

        for query in inputs {
            let redacted = redact_sensitive_query(query);
            assert!(redacted.ends_with("PASSWORD <redacted>"));
            for secret in [
                "double-secret",
                "single-secret",
                "bare-secret",
                "comment-secret",
                "unterminated-secret",
                "first",
                "second",
                "ignored",
            ] {
                assert!(!redacted.contains(secret));
            }
        }
    }

    #[test]
    fn running_query_serialization_omits_bearer_session_id() {
        let running = RunningQuery {
            id: 1,
            session_id: 9_876_543_210,
            query_type: "show",
            query: "SHOW SPACES".to_string(),
            space: "default".to_string(),
            started_at_ms: 1,
        };
        let json = serde_json::to_string(&running).expect("running query should serialize");
        assert!(!json.contains("session_id"));
        assert!(!json.contains("9876543210"));
    }

    #[test]
    fn compound_use_changes_authorization_space_for_following_clause() {
        let checks = authorization_checks(
            "USE secret; INSERT VERTEX person(name) VALUES 1:('alice')",
            "default",
        );
        assert_eq!(
            checks,
            vec![
                AuthorizationCheck::permission(Permission::Read, "secret"),
                AuthorizationCheck::permission(Permission::Write, "secret"),
            ]
        );
    }

    #[test]
    fn profile_uses_inner_mutating_permission() {
        let checks = authorization_checks("PROFILE DROP SPACE victim", "default");
        assert_eq!(
            checks,
            vec![AuthorizationCheck::permission(Permission::Drop, "victim")]
        );
    }

    #[test]
    fn sensitive_show_and_user_management_require_admin() {
        for query in [
            "SHOW USER",
            "SHOW SESSIONS",
            "GRANT ROLE USER TO alice",
            "REVOKE ROLE USER FROM alice",
            "BALANCE STATUS",
        ] {
            assert_eq!(
                authorization_checks(query, "default"),
                vec![AuthorizationCheck::admin()],
                "query should be admin-only: {query}"
            );
        }
        let roles = byoridb_parser::Statement::Show(ShowStatement::Roles);
        let mut checks = Vec::new();
        GraphService::collect_authorization_checks(&roles, "default", &mut checks);
        assert_eq!(checks, vec![AuthorizationCheck::admin()]);
    }

    #[test]
    fn explicit_dml_space_is_used_for_authorization() {
        let mut statement = byoridb_parser::parse("INSERT VERTEX person(name) VALUES 1:('alice')")
            .expect("insert should parse");
        let byoridb_parser::Statement::Insert(insert) = &mut statement else {
            panic!("expected insert statement");
        };
        insert.space = Some("explicit".to_string());

        let mut checks = Vec::new();
        GraphService::collect_authorization_checks(&statement, "default", &mut checks);
        assert_eq!(
            checks,
            vec![AuthorizationCheck::permission(
                Permission::Write,
                "explicit"
            )]
        );
    }

    #[test]
    fn user_helpers_recurse_only_through_executing_profile() {
        let profile =
            byoridb_parser::parse(r#"PROFILE CREATE USER alice WITH PASSWORD "secret" ROLE USER"#)
                .expect("profile create user should parse");
        assert!(GraphService::contains_user_mutation(&profile));
        assert_eq!(GraphService::affected_usernames(&profile), vec!["alice"]);

        let explain =
            byoridb_parser::parse(r#"EXPLAIN CREATE USER alice WITH PASSWORD "secret" ROLE USER"#)
                .expect("explain create user should parse");
        assert!(!GraphService::contains_user_mutation(&explain));
        assert!(GraphService::affected_usernames(&explain).is_empty());
    }

    #[test]
    fn role_normalization_recurses() {
        let mut statement = byoridb_parser::parse(
            r#"SHOW SPACES; CREATE USER alice WITH PASSWORD "secret" ROLE USER"#,
        )
        .expect("compound create user should parse");
        GraphService::normalize_security_roles(&mut statement);

        let byoridb_parser::Statement::Compound(clauses) = statement else {
            panic!("expected compound statement");
        };
        let byoridb_parser::Statement::Create(CreateStatement::User(user)) =
            clauses[1].stmt.as_ref()
        else {
            panic!("expected create user clause");
        };
        assert_eq!(user.role.as_deref(), Some("USER"));
    }

    #[tokio::test]
    async fn authentication_hydrates_and_refreshes_durable_user() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let first = User {
            username: "alice".to_string(),
            password_hash: byoridb_common::crypto::hash_password("old-password")
                .expect("password should hash"),
            roles: vec!["USER".to_string()],
            enabled: true,
        };
        kvstore
            .put(
                format!("{USER_KEY_PREFIX}alice").as_bytes(),
                &serde_json::to_vec(&first).expect("user should serialize"),
            )
            .await
            .expect("durable user should be stored");

        let service = GraphService::with_auth(
            kvstore.clone(),
            AuthManager::with_config("root-password", Duration::from_secs(60)),
        );
        let old_session = service
            .authenticate("alice".to_string(), "old-password".to_string())
            .await
            .expect("durable user should authenticate");

        let changed = User {
            password_hash: byoridb_common::crypto::hash_password("new-password")
                .expect("password should hash"),
            ..first
        };
        kvstore
            .put(
                format!("{USER_KEY_PREFIX}alice").as_bytes(),
                &serde_json::to_vec(&changed).expect("user should serialize"),
            )
            .await
            .expect("durable user should update");

        assert!(service
            .authenticate("alice".to_string(), "old-password".to_string())
            .await
            .is_err());
        assert!(matches!(
            service
                .execute(old_session, "SHOW SPACES".to_string())
                .await,
            Err(GraphError::SessionNotFound(_))
        ));
        service
            .authenticate("alice".to_string(), "new-password".to_string())
            .await
            .expect("new password should authenticate");
    }

    #[tokio::test]
    async fn root_authentication_never_hydrates_kv_root_record() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let forged_root = User {
            username: "root".to_string(),
            password_hash: byoridb_common::crypto::hash_password("forged-password")
                .expect("password should hash"),
            roles: vec!["GOD".to_string()],
            enabled: true,
        };
        kvstore
            .put(
                format!("{USER_KEY_PREFIX}root").as_bytes(),
                &serde_json::to_vec(&forged_root).expect("user should serialize"),
            )
            .await
            .expect("forged root should be stored");

        let service = GraphService::with_auth(
            kvstore,
            AuthManager::with_config("env-password", Duration::from_secs(60)),
        );
        service
            .authenticate("root".to_string(), "env-password".to_string())
            .await
            .expect("configured root password should remain authoritative");
        assert!(service
            .authenticate("root".to_string(), "forged-password".to_string())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn create_user_persists_and_authenticates_from_durable_store() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let service = GraphService::with_auth(
            kvstore.clone(),
            AuthManager::with_config("root-password", Duration::from_secs(60)),
        );
        let root_session = service
            .authenticate("root".to_string(), "root-password".to_string())
            .await
            .expect("root should authenticate");
        service
            .execute(
                root_session,
                r#"CREATE USER durable_guest WITH PASSWORD "guest-password" ROLE GUEST"#
                    .to_string(),
            )
            .await
            .expect("create user should succeed");

        let entries = kvstore
            .scan_prefix(USER_KEY_PREFIX.as_bytes())
            .await
            .expect("user prefix should scan");
        let keys: Vec<String> = entries
            .iter()
            .map(|(key, _)| String::from_utf8_lossy(key).into_owned())
            .collect();
        assert_eq!(keys, vec![format!("{USER_KEY_PREFIX}durable_guest")]);
        service
            .authenticate("durable_guest".to_string(), "guest-password".to_string())
            .await
            .expect("created user should authenticate");
    }

    #[tokio::test]
    async fn authentication_uses_a_shared_user_auth_guard() {
        let service = GraphService::with_auth(
            Arc::new(MemoryKVStore::new()),
            AuthManager::with_config("root-password", Duration::from_secs(60)),
        );
        let read_guard = service.user_auth_lock.read().await;

        let session = tokio::time::timeout(
            Duration::from_secs(2),
            service.authenticate("root".to_string(), "root-password".to_string()),
        )
        .await
        .expect("a concurrent read guard must not serialize authentication")
        .expect("root should authenticate");

        drop(read_guard);
        service.sign_out(session, session).await;
    }

    #[tokio::test]
    async fn concurrent_revocation_blocks_queries_diagnostics_and_admin_sign_out() {
        let kvstore = Arc::new(MemoryKVStore::new());
        let service = GraphService::with_auth(
            kvstore.clone(),
            AuthManager::with_config("root-password", Duration::from_secs(60)),
        );
        let root_session = service
            .authenticate("root".to_string(), "root-password".to_string())
            .await
            .expect("root should authenticate");
        service
            .execute(
                root_session,
                r#"CREATE USER race_admin WITH PASSWORD "admin-password" ROLE ADMIN"#.to_string(),
            )
            .await
            .expect("admin should be created");
        let admin_session = service
            .authenticate("race_admin".to_string(), "admin-password".to_string())
            .await
            .expect("admin should authenticate");

        // Model a user mutation between its durable write and cache/session
        // reconciliation. Every stale-admin entry point must wait at this lock.
        let mutation_guard = service.user_auth_lock.write().await;
        let mut query =
            Box::pin(service.execute(admin_session, "CREATE SPACE stale_admin_race".to_string()));
        let mut diagnostics = Box::pin(service.is_admin_session(admin_session));
        let mut sign_out = Box::pin(service.sign_out(admin_session, root_session));
        let wait = Duration::from_millis(20);
        assert!(tokio::time::timeout(wait, &mut query).await.is_err());
        assert!(tokio::time::timeout(wait, &mut diagnostics).await.is_err());
        assert!(tokio::time::timeout(wait, &mut sign_out).await.is_err());

        service
            .auth_manager
            .remove_cached_user_if_present("race_admin")
            .await
            .expect("revocation should evict the cached admin");
        service
            .session_manager
            .remove_sessions_for_user("race_admin")
            .await;
        drop(mutation_guard);

        let query_result = tokio::time::timeout(Duration::from_secs(1), query)
            .await
            .expect("query should finish after reconciliation");
        assert!(matches!(
            query_result,
            Err(GraphError::SessionNotFound(REDACTED_SESSION_ID))
        ));
        assert!(!tokio::time::timeout(Duration::from_secs(1), diagnostics)
            .await
            .expect("diagnostics check should finish after reconciliation"));
        tokio::time::timeout(Duration::from_secs(1), sign_out)
            .await
            .expect("sign out should finish after reconciliation");

        assert!(service.is_admin_session(root_session).await);
        assert!(kvstore
            .get(&byoridb_executor::key::SchemaKey::space("stale_admin_race"))
            .await
            .expect("space lookup should succeed")
            .is_none());
    }

    #[test]
    fn ordinary_show_remains_read_only() {
        let statement = byoridb_parser::Statement::Show(ShowStatement::Spaces);
        let mut checks = Vec::new();
        GraphService::collect_authorization_checks(&statement, "default", &mut checks);
        assert_eq!(
            checks,
            vec![AuthorizationCheck::permission(Permission::Read, "default")]
        );
    }
}
