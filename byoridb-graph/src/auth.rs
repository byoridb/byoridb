// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Authentication and authorization for ByoriDB
//!
//! This module provides:
//! - User authentication
//! - Role-based access control (RBAC)
//! - Permission management
//! - Space-level access control

use super::error::{GraphError, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

/// Default session TTL: 24 hours
pub const DEFAULT_SESSION_TTL_SECS: u64 = 24 * 60 * 60;

/// Environment variable that sets the root user's password at startup.
/// If unset, a cryptographically random ephemeral password is generated. The
/// value is deliberately never logged; operators must set this variable to a
/// retrievable secret before starting a production server.
pub const ROOT_PASSWORD_ENV: &str = "BYORIDB_ROOT_PASSWORD";

/// Deliberately generic so callers cannot distinguish an unknown, disabled,
/// locked, or wrong-password account from the response text.
const AUTH_FAILED_MESSAGE: &str = "Authentication failed";

/// Maximum consecutive failed login attempts before lockout.
pub const MAX_FAILED_ATTEMPTS: u32 = 5;
/// Lockout duration after exceeding MAX_FAILED_ATTEMPTS.
pub const LOCKOUT_DURATION_SECS: u64 = 300; // 5 minutes

/// Per-username failed login tracking entry.
struct FailedAttempt {
    count: u32,
    locked_until: Option<SystemTime>,
}

/// Authentication manager
pub struct AuthManager {
    users: Arc<RwLock<HashMap<String, User>>>,
    roles: Arc<RwLock<HashMap<String, Role>>>,
    sessions: Arc<RwLock<HashMap<i64, Session>>>,
    /// Brute-force protection: tracks consecutive failures per username.
    failed_attempts: Arc<RwLock<HashMap<String, FailedAttempt>>>,
    session_ttl: Duration,
}

impl AuthManager {
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_secs(DEFAULT_SESSION_TTL_SECS))
    }

    /// Construct with a custom session TTL. Root password is resolved from
    /// [`ROOT_PASSWORD_ENV`] or a randomly generated value.
    pub fn with_ttl(session_ttl: Duration) -> Self {
        // Determine root password:
        //   1. BYORIDB_ROOT_PASSWORD env var (explicit, preferred for production)
        //   2. Cryptographically random, deliberately undisclosed password
        let (root_password, generated) = match std::env::var(ROOT_PASSWORD_ENV) {
            Ok(p) if !p.trim().is_empty() => (p, false),
            _ => (Self::generate_random_password(), true),
        };

        if generated {
            tracing::warn!(
                "No {} env var set; root received an undisclosed ephemeral password. \
                Set {} to a managed secret and restart before accepting clients.",
                ROOT_PASSWORD_ENV,
                ROOT_PASSWORD_ENV
            );
        } else {
            tracing::info!("Using root password from {} env var", ROOT_PASSWORD_ENV);
        }

        Self::with_config(&root_password, session_ttl)
    }

    /// Construct with an explicit root password and session TTL.
    ///
    /// Prefer [`AuthManager::new`] or [`AuthManager::with_ttl`] in production;
    /// this constructor is useful for tests and for environments that resolve
    /// the root password out-of-band (e.g. secrets managers).
    pub fn with_config(root_password: &str, session_ttl: Duration) -> Self {
        let mut users = HashMap::new();
        let root_password = if root_password.trim().is_empty() {
            tracing::error!(
                "Empty root password rejected; root received an undisclosed ephemeral password"
            );
            Self::generate_random_password()
        } else {
            root_password.to_string()
        };

        match Self::hash_password(&root_password) {
            Ok(root_hash) => {
                let root = User {
                    username: "root".to_string(),
                    password_hash: root_hash,
                    roles: vec!["GOD".to_string()],
                    enabled: true,
                };
                users.insert("root".to_string(), root);
            }
            Err(error) => {
                // Keep the manager usable but fail closed: without a valid hash
                // no root authentication is possible.
                tracing::error!(err = %error, "Failed to initialize root credentials");
            }
        }

        let mut roles = HashMap::new();

        // Create default roles
        roles.insert("GOD".to_string(), Role::god());
        roles.insert("ADMIN".to_string(), Role::admin());
        roles.insert("DBA".to_string(), Role::dba());
        roles.insert("USER".to_string(), Role::user());
        roles.insert("GUEST".to_string(), Role::guest());

        AuthManager {
            users: Arc::new(RwLock::new(users)),
            roles: Arc::new(RwLock::new(roles)),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            failed_attempts: Arc::new(RwLock::new(HashMap::new())),
            session_ttl,
        }
    }

    /// Generate a cryptographically secure random password (24 bytes, hex-encoded → 48 chars).
    fn generate_random_password() -> String {
        use argon2::password_hash::rand_core::{OsRng, RngCore};
        let mut bytes = [0u8; 24];
        OsRng.fill_bytes(&mut bytes);
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Canonical username used by the in-memory authentication cache.
    pub fn normalize_username(username: &str) -> String {
        username.trim().to_string()
    }

    /// Canonical role name used by permission checks.
    pub fn normalize_role(role: &str) -> String {
        role.trim().to_ascii_uppercase()
    }

    fn random_session_id() -> i64 {
        use argon2::password_hash::rand_core::{OsRng, RngCore};

        loop {
            let candidate = (OsRng.next_u64() >> 1) as i64;
            if candidate > 0 {
                return candidate;
            }
        }
    }

    fn allocate_session_id_with<F>(sessions: &HashMap<i64, Session>, mut generate: F) -> i64
    where
        F: FnMut() -> i64,
    {
        loop {
            let candidate = generate();
            if candidate > 0 && !sessions.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    fn allocate_session_id(sessions: &HashMap<i64, Session>) -> i64 {
        Self::allocate_session_id_with(sessions, Self::random_session_id)
    }

    /// Authenticate a user
    pub async fn authenticate(&self, username: &str, password: &str) -> Result<i64> {
        self.authenticate_with_verifier(username, password, Self::verify_password)
            .await
    }

    async fn authenticate_with_verifier<F>(
        &self,
        username: &str,
        password: &str,
        verify: F,
    ) -> Result<i64>
    where
        F: Fn(&str, &str) -> bool,
    {
        let username = Self::normalize_username(username);
        if username.is_empty() || password.trim().is_empty() {
            return Err(GraphError::AuthFailed(AUTH_FAILED_MESSAGE.to_string()));
        }

        // Always perform a password verification, including for unknown users,
        // using the root hash as dummy material. This narrows the externally
        // observable timing difference without ever authenticating the dummy.
        let (user_snapshot, password_hash) = {
            let users = self.users.read().await;
            if let Some(user) = users.get(&username) {
                (Some(user.clone()), Some(user.password_hash.clone()))
            } else {
                (
                    None,
                    users.get("root").map(|root| root.password_hash.clone()),
                )
            }
        };
        let password_valid = password_hash
            .as_deref()
            .map(|hash| verify(password, hash))
            .unwrap_or(false);

        let credentials_valid = user_snapshot
            .as_ref()
            .is_some_and(|user| user.enabled && password_valid);
        if !credentials_valid {
            // Record failures only for enabled, real accounts. Unknown names are
            // not retained, preventing an unauthenticated memory-amplification
            // attack with arbitrary usernames.
            if let Some(snapshot) = user_snapshot.as_ref().filter(|user| user.enabled) {
                // Recheck the snapshot under the user read lock. A concurrent
                // password/role/state mutation either clears this counter after
                // acquiring the write lock or makes this attempt irrelevant to
                // the new security state.
                let users = self.users.read().await;
                if users.get(&username) != Some(snapshot) {
                    return Err(GraphError::AuthFailed(AUTH_FAILED_MESSAGE.to_string()));
                }
                let now = SystemTime::now();
                let lockout_duration = Duration::from_secs(LOCKOUT_DURATION_SECS);
                let mut attempts = self.failed_attempts.write().await;
                let entry = attempts.entry(username.clone()).or_insert(FailedAttempt {
                    count: 0,
                    locked_until: None,
                });
                let currently_locked = entry
                    .locked_until
                    .map(|locked_until| now < locked_until)
                    .unwrap_or(false);
                if !currently_locked {
                    entry.count = entry.count.saturating_add(1);
                    if entry.count >= MAX_FAILED_ATTEMPTS {
                        entry.locked_until = Some(now + lockout_duration);
                        tracing::warn!(
                            username = %username,
                            failed_attempts = entry.count,
                            "Account temporarily locked after repeated authentication failures"
                        );
                    }
                }
            }
            return Err(GraphError::AuthFailed(AUTH_FAILED_MESSAGE.to_string()));
        }

        let Some(user_snapshot) = user_snapshot else {
            return Err(GraphError::AuthFailed(AUTH_FAILED_MESSAGE.to_string()));
        };

        // Revalidate after the deliberately expensive password verification.
        // Keep the read lock until insertion so a concurrent password, role,
        // enable-state, or delete mutation cannot slip between validation and
        // session creation. Once this lock is released, the mutator obtains its
        // write lock and revokes the newly inserted session.
        let users = self.users.read().await;
        if users.get(&username) != Some(&user_snapshot) {
            return Err(GraphError::AuthFailed(AUTH_FAILED_MESSAGE.to_string()));
        }

        let now = SystemTime::now();
        let mut sessions = self.sessions.write().await;
        let session_id = Self::allocate_session_id(&sessions);
        let session = Session {
            id: session_id,
            username: username.clone(),
            roles: user_snapshot.roles,
            created_at: now,
            expires_at: now + self.session_ttl,
        };
        sessions.insert(session_id, session);
        drop(sessions);
        drop(users);

        // Success: clear failure counter.
        self.failed_attempts.write().await.remove(&username);

        tracing::info!(username = %username, "User authenticated");

        Ok(session_id)
    }

    async fn normalize_and_validate_user(&self, mut user: User) -> Result<User> {
        user.username = Self::normalize_username(&user.username);
        if user.username.is_empty() {
            return Err(GraphError::InvalidOperation(
                "Username must not be empty".to_string(),
            ));
        }

        if argon2::password_hash::PasswordHash::new(&user.password_hash).is_err() {
            return Err(GraphError::InvalidOperation(format!(
                "User {} has an invalid password hash",
                user.username
            )));
        }

        let known_roles = self.roles.read().await;
        let mut seen = HashSet::new();
        let mut normalized_roles = Vec::with_capacity(user.roles.len());
        for role in user.roles {
            let role = Self::normalize_role(&role);
            if !known_roles.contains_key(&role) {
                return Err(GraphError::InvalidOperation(format!(
                    "Unknown role {} for user {}",
                    role, user.username
                )));
            }
            if seen.insert(role.clone()) {
                normalized_roles.push(role);
            }
        }
        drop(known_roles);
        normalized_roles.sort();
        user.roles = normalized_roles;

        if user.username.eq_ignore_ascii_case("root")
            && (!user.enabled || !user.roles.iter().any(|role| role == "GOD"))
        {
            return Err(GraphError::InvalidOperation(
                "Root user must remain enabled with GOD role".to_string(),
            ));
        }

        Ok(user)
    }

    async fn validate_role(&self, role: &str) -> Result<String> {
        let role = Self::normalize_role(role);
        if role.is_empty() || !self.roles.read().await.contains_key(&role) {
            return Err(GraphError::InvalidOperation(format!(
                "Unknown role {}",
                role
            )));
        }
        Ok(role)
    }

    /// Insert or replace a user loaded from the durable user store.
    ///
    /// The replacement is validated and normalized before the cache is
    /// modified. If password, roles, or enabled state changed, all existing
    /// authentication sessions for the user are revoked. The removed session
    /// IDs are returned so the graph-layer [`crate::session::SessionManager`]
    /// can remove its corresponding space-tracking sessions as well.
    pub async fn upsert_user(&self, user: User) -> Result<Vec<i64>> {
        let user = self.normalize_and_validate_user(user).await?;
        let username = user.username.clone();
        if username.eq_ignore_ascii_case("root") {
            return Err(GraphError::InvalidOperation(format!(
                "Root credentials are managed by {} and cannot be replaced from the user store",
                ROOT_PASSWORD_ENV
            )));
        }

        let security_state_changed = {
            let mut users = self.users.write().await;
            let changed = users.get(&username).is_some_and(|existing| {
                existing.password_hash != user.password_hash
                    || existing.roles != user.roles
                    || existing.enabled != user.enabled
            });
            users.insert(username.clone(), user);
            changed
        };

        self.failed_attempts.write().await.remove(&username);
        if security_state_changed {
            Ok(self.invalidate_user_sessions(&username).await)
        } else {
            Ok(Vec::new())
        }
    }

    async fn remove_cached_user_internal(&self, username: &str) -> Result<(bool, Vec<i64>)> {
        let username = Self::normalize_username(username);
        if username.eq_ignore_ascii_case("root") {
            return Err(GraphError::InvalidOperation(
                "Cannot delete root user".to_string(),
            ));
        }

        let existed = self.users.write().await.remove(&username).is_some();
        self.failed_attempts.write().await.remove(&username);
        let removed = self.invalidate_user_sessions(&username).await;
        if existed {
            tracing::info!(username = %username, "Deleted user");
        }
        Ok((existed, removed))
    }

    /// Remove a user from the in-memory cache and revoke all of their sessions.
    /// Root removal is rejected. Removed session IDs are returned for graph
    /// session cleanup by the caller. A missing user remains an error for
    /// compatibility with the existing delete-user API.
    pub async fn remove_cached_user(&self, username: &str) -> Result<Vec<i64>> {
        let normalized = Self::normalize_username(username);
        let (existed, removed) = self.remove_cached_user_internal(&normalized).await?;
        if !existed {
            return Err(GraphError::InvalidOperation(format!(
                "User {} not found",
                normalized
            )));
        }
        Ok(removed)
    }

    /// Idempotent cache removal for `DROP USER IF EXISTS` and durable-store
    /// cache reconciliation. Missing users succeed with an empty session list.
    pub async fn remove_cached_user_if_present(&self, username: &str) -> Result<Vec<i64>> {
        let (_, removed) = self.remove_cached_user_internal(username).await?;
        Ok(removed)
    }

    /// Revoke every authentication session owned by `username`.
    pub async fn invalidate_user_sessions(&self, username: &str) -> Vec<i64> {
        let username = Self::normalize_username(username);
        let mut sessions = self.sessions.write().await;
        let mut removed = Vec::new();
        sessions.retain(|session_id, session| {
            let keep = session.username != username;
            if !keep {
                removed.push(*session_id);
            }
            keep
        });
        removed.sort_unstable();
        removed
    }

    /// Return whether a live session has GOD or ADMIN role.
    pub async fn is_admin_session(&self, session_id: i64) -> bool {
        let mut sessions = self.sessions.write().await;
        let Some(expired) = sessions.get(&session_id).map(Session::is_expired) else {
            return false;
        };
        if expired {
            sessions.remove(&session_id);
            return false;
        }
        sessions
            .get(&session_id)
            .into_iter()
            .flat_map(|session| session.roles.iter())
            .map(|role| Self::normalize_role(role))
            .any(|role| role == "GOD" || role == "ADMIN")
    }

    /// Create a new user
    pub async fn create_user(
        &self,
        username: &str,
        password: &str,
        roles: Vec<String>,
    ) -> Result<()> {
        let user = self
            .normalize_and_validate_user(User {
                username: username.to_string(),
                password_hash: Self::hash_password(password)?,
                roles,
                enabled: true,
            })
            .await?;
        let username = user.username.clone();
        if username.eq_ignore_ascii_case("root") {
            return Err(GraphError::InvalidOperation(format!(
                "Root credentials are managed by {}",
                ROOT_PASSWORD_ENV
            )));
        }
        let mut users = self.users.write().await;

        if users.contains_key(&username) {
            return Err(GraphError::InvalidOperation(format!(
                "User {} already exists",
                username
            )));
        }

        users.insert(username.clone(), user);

        tracing::info!(username = %username, "Created user");

        Ok(())
    }

    /// Delete a user
    pub async fn delete_user(&self, username: &str) -> Result<()> {
        self.remove_cached_user(username).await?;
        Ok(())
    }

    /// Grant a role to a user
    pub async fn grant_role(&self, username: &str, role: &str) -> Result<()> {
        let username = Self::normalize_username(username);
        let role = self.validate_role(role).await?;
        let mut users = self.users.write().await;

        let user = users
            .get_mut(&username)
            .ok_or_else(|| GraphError::InvalidOperation(format!("User {} not found", username)))?;

        let changed = if !user.roles.contains(&role) {
            user.roles.push(role.clone());
            user.roles.sort();
            true
        } else {
            false
        };
        drop(users);

        if changed {
            self.invalidate_user_sessions(&username).await;
        }

        tracing::info!(role = %role, username = %username, "Granted role to user");

        Ok(())
    }

    /// Revoke a role from a user
    pub async fn revoke_role(&self, username: &str, role: &str) -> Result<()> {
        let username = Self::normalize_username(username);
        let role = self.validate_role(role).await?;
        if username.eq_ignore_ascii_case("root") && role == "GOD" {
            return Err(GraphError::InvalidOperation(
                "Cannot revoke GOD role from root user".to_string(),
            ));
        }

        let mut users = self.users.write().await;

        let user = users
            .get_mut(&username)
            .ok_or_else(|| GraphError::InvalidOperation(format!("User {} not found", username)))?;

        let original_len = user.roles.len();
        user.roles.retain(|existing| existing != &role);
        let changed = user.roles.len() != original_len;
        drop(users);

        if changed {
            self.invalidate_user_sessions(&username).await;
        }

        tracing::info!(role = %role, username = %username, "Revoked role from user");

        Ok(())
    }

    /// Enable or disable a user. Disabled users cannot authenticate.
    /// The root user cannot be disabled.
    pub async fn set_user_enabled(&self, username: &str, enabled: bool) -> Result<()> {
        let username = Self::normalize_username(username);
        if username.eq_ignore_ascii_case("root") && !enabled {
            return Err(GraphError::InvalidOperation(
                "Cannot disable root user".to_string(),
            ));
        }

        let mut users = self.users.write().await;
        let user = users
            .get_mut(&username)
            .ok_or_else(|| GraphError::InvalidOperation(format!("User {} not found", username)))?;

        let changed = user.enabled != enabled;
        user.enabled = enabled;
        drop(users);

        if changed {
            self.invalidate_user_sessions(&username).await;
        }

        tracing::info!(
            username = %username,
            enabled,
            "User enabled state changed"
        );

        Ok(())
    }

    /// Check if a session has permission
    pub async fn check_permission(
        &self,
        session_id: i64,
        space: &str,
        permission: Permission,
    ) -> Result<bool> {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(&session_id)
            .ok_or_else(|| GraphError::AuthFailed("Invalid session".to_string()))?;

        if session.is_expired() {
            return Err(GraphError::AuthFailed("Session expired".to_string()));
        }

        let roles = self.roles.read().await;

        // Check if any role has the required permission
        for role_name in &session.roles {
            if let Some(role) = roles.get(role_name) {
                if role.has_permission(space, permission) {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Get the roles associated with a session (for RBAC propagation to executor).
    pub async fn get_session_roles(&self, session_id: i64) -> Option<Vec<String>> {
        let mut sessions = self.sessions.write().await;
        let expired = sessions
            .get(&session_id)
            .map(Session::is_expired)
            .unwrap_or(false);
        if expired {
            sessions.remove(&session_id);
            return None;
        }
        sessions
            .get(&session_id)
            .map(|session| session.roles.clone())
    }

    /// Remove all expired sessions. Returns the number of sessions removed.
    pub async fn cleanup_expired_sessions(&self) -> usize {
        let now = SystemTime::now();
        let mut sessions = self.sessions.write().await;
        let expired: Vec<i64> = sessions
            .iter()
            .filter(|(_, s)| now >= s.expires_at)
            .map(|(id, _)| *id)
            .collect();

        let count = expired.len();
        for id in expired {
            sessions.remove(&id);
        }
        count
    }

    /// Sign out a session
    pub async fn sign_out(&self, session_id: i64) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(&session_id).ok_or_else(|| {
            GraphError::InvalidOperation(format!("Session {} not found", session_id))
        })?;

        tracing::info!("Session signed out");

        Ok(())
    }

    /// Hash password using argon2 (delegates to byoridb_common::crypto)
    fn hash_password(password: &str) -> Result<String> {
        if password.trim().is_empty() {
            return Err(GraphError::InvalidOperation(
                "Password must not be empty".to_string(),
            ));
        }
        byoridb_common::crypto::hash_password(password)
            .map_err(|e| GraphError::InternalError(e.to_string()))
    }

    /// Verify password against stored hash (delegates to byoridb_common::crypto)
    fn verify_password(password: &str, hash: &str) -> bool {
        byoridb_common::crypto::verify_password(password, hash)
    }
}

/// User information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub username: String,
    pub password_hash: String,
    pub roles: Vec<String>,
    pub enabled: bool,
}

/// Role with permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    pub permissions: Vec<PermissionEntry>,
}

impl Role {
    /// Create GOD role (all permissions)
    pub fn god() -> Self {
        Role {
            name: "GOD".to_string(),
            permissions: vec![
                PermissionEntry::any(Permission::Read),
                PermissionEntry::any(Permission::Write),
                PermissionEntry::any(Permission::Create),
                PermissionEntry::any(Permission::Delete),
                PermissionEntry::any(Permission::Alter),
                PermissionEntry::any(Permission::Drop),
            ],
        }
    }

    /// Create ADMIN role (admin permissions)
    pub fn admin() -> Self {
        Role {
            name: "ADMIN".to_string(),
            permissions: vec![
                PermissionEntry::any(Permission::Read),
                PermissionEntry::any(Permission::Write),
                PermissionEntry::any(Permission::Create),
                PermissionEntry::any(Permission::Delete),
                PermissionEntry::any(Permission::Alter),
                PermissionEntry::any(Permission::Drop),
            ],
        }
    }

    /// Create DBA role (database administrator - no drop)
    pub fn dba() -> Self {
        Role {
            name: "DBA".to_string(),
            permissions: vec![
                PermissionEntry::any(Permission::Read),
                PermissionEntry::any(Permission::Write),
                PermissionEntry::any(Permission::Create),
                PermissionEntry::any(Permission::Alter),
            ],
        }
    }

    /// Create USER role (basic permissions)
    pub fn user() -> Self {
        Role {
            name: "USER".to_string(),
            permissions: vec![
                PermissionEntry::any(Permission::Read),
                PermissionEntry::any(Permission::Write),
            ],
        }
    }

    /// Create GUEST role (read-only)
    pub fn guest() -> Self {
        Role {
            name: "GUEST".to_string(),
            permissions: vec![PermissionEntry::any(Permission::Read)],
        }
    }

    /// Check if role has a specific permission
    pub fn has_permission(&self, space: &str, permission: Permission) -> bool {
        self.permissions
            .iter()
            .any(|p| (p.space == "*" || p.space == space) && p.permission == permission)
    }
}

/// Permission entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionEntry {
    pub space: String,
    pub permission: Permission,
}

impl PermissionEntry {
    pub fn any(permission: Permission) -> Self {
        PermissionEntry {
            space: "*".to_string(),
            permission,
        }
    }

    pub fn space(space: &str, permission: Permission) -> Self {
        PermissionEntry {
            space: space.to_string(),
            permission,
        }
    }
}

/// Permission types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    Read,
    Write,
    Create,
    Delete,
    Alter,
    Drop,
}

/// Session information
#[derive(Debug, Clone)]
pub struct Session {
    pub id: i64,
    pub username: String,
    pub roles: Vec<String>,
    pub created_at: SystemTime,
    pub expires_at: SystemTime,
}

impl Session {
    /// Returns true if the session has passed its expiration time.
    pub fn is_expired(&self) -> bool {
        SystemTime::now() >= self.expires_at
    }
}

impl Default for AuthManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PW: &str = "test-password-xyz";

    fn make_manager() -> AuthManager {
        AuthManager::with_config(TEST_PW, Duration::from_secs(3600))
    }

    fn make_manager_with_ttl(ttl: Duration) -> AuthManager {
        AuthManager::with_config(TEST_PW, ttl)
    }

    #[tokio::test]
    async fn test_authenticate_root_success() {
        let mgr = make_manager();
        let session_id = mgr.authenticate("root", TEST_PW).await.unwrap();
        assert!(session_id >= 1);
    }

    #[tokio::test]
    async fn test_empty_explicit_root_password_is_not_accepted() {
        let mgr = AuthManager::with_config("", Duration::from_secs(3600));
        let err = mgr.authenticate("root", "").await.unwrap_err();
        assert!(matches!(err, GraphError::AuthFailed(_)));
    }

    #[tokio::test]
    async fn test_blank_passwords_are_rejected() {
        let mgr = AuthManager::with_config("   ", Duration::from_secs(3600));
        assert!(mgr.authenticate("root", "   ").await.is_err());

        let error = mgr
            .create_user("blank-user", "\t\n", vec!["USER".to_string()])
            .await
            .expect_err("blank user passwords must be rejected");
        assert!(matches!(error, GraphError::InvalidOperation(_)));

        let legacy_blank_hash = byoridb_common::crypto::hash_password("")
            .expect("the legacy hash fixture should be valid Argon2 material");
        mgr.upsert_user(User {
            username: "legacy-blank".to_string(),
            password_hash: legacy_blank_hash,
            roles: vec!["USER".to_string()],
            enabled: true,
        })
        .await
        .expect("the legacy user fixture should load");
        let legacy_error = mgr
            .authenticate("legacy-blank", "")
            .await
            .expect_err("legacy blank-password records must not authenticate");
        assert!(matches!(legacy_error, GraphError::AuthFailed(_)));
    }

    #[tokio::test]
    async fn test_authenticate_wrong_password_fails() {
        let mgr = make_manager();
        let err = mgr.authenticate("root", "wrong").await.unwrap_err();
        assert!(matches!(err, GraphError::AuthFailed(_)));
    }

    #[tokio::test]
    async fn test_authenticate_unknown_user_fails() {
        let mgr = make_manager();
        let err = mgr.authenticate("ghost", TEST_PW).await.unwrap_err();
        assert!(matches!(err, GraphError::AuthFailed(_)));
    }

    #[tokio::test]
    async fn test_authentication_failures_use_same_message() {
        let mgr = make_manager();
        mgr.create_user("disabled", "pw", vec!["USER".to_string()])
            .await
            .unwrap();
        mgr.set_user_enabled("disabled", false).await.unwrap();

        let unknown = mgr.authenticate("ghost", TEST_PW).await.unwrap_err();
        let wrong = mgr.authenticate("root", "wrong").await.unwrap_err();
        let disabled = mgr.authenticate("disabled", "pw").await.unwrap_err();

        assert_eq!(unknown.to_string(), wrong.to_string());
        assert_eq!(wrong.to_string(), disabled.to_string());
        assert!(wrong.to_string().contains(AUTH_FAILED_MESSAGE));
    }

    #[tokio::test]
    async fn test_unknown_user_dummy_hash_never_authenticates() {
        let mgr = make_manager();
        // Unknown-user verification uses the root hash as dummy material. Even
        // when the supplied password matches that hash, the user must not pass.
        let err = mgr.authenticate("ghost", TEST_PW).await.unwrap_err();
        assert!(matches!(err, GraphError::AuthFailed(_)));
        assert!(mgr.failed_attempts.read().await.get("ghost").is_none());
    }

    #[tokio::test]
    async fn test_create_and_authenticate_user() {
        let mgr = make_manager();
        mgr.create_user("alice", "alice-pw", vec!["USER".to_string()])
            .await
            .unwrap();

        let session_id = mgr.authenticate("alice", "alice-pw").await.unwrap();
        assert!(session_id >= 1);
    }

    #[tokio::test]
    async fn test_user_and_roles_are_normalized_without_changing_case_semantics() {
        let mgr = make_manager();
        mgr.create_user(
            "  Alice  ",
            "pw",
            vec![" user ".to_string(), "USER".to_string()],
        )
        .await
        .unwrap();

        assert!(mgr.authenticate(" Alice ", "pw").await.is_ok());
        assert!(mgr.authenticate("alice", "pw").await.is_err());
        let users = mgr.users.read().await;
        assert_eq!(users.get("Alice").unwrap().roles, vec!["USER"]);
    }

    #[tokio::test]
    async fn test_create_duplicate_user_fails() {
        let mgr = make_manager();
        mgr.create_user("alice", "pw", vec!["USER".to_string()])
            .await
            .unwrap();
        let err = mgr
            .create_user("alice", "pw2", vec!["USER".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, GraphError::InvalidOperation(_)));
    }

    #[tokio::test]
    async fn test_delete_user() {
        let mgr = make_manager();
        mgr.create_user("alice", "pw", vec!["USER".to_string()])
            .await
            .unwrap();
        mgr.delete_user("alice").await.unwrap();

        let err = mgr.authenticate("alice", "pw").await.unwrap_err();
        assert!(matches!(err, GraphError::AuthFailed(_)));
    }

    #[tokio::test]
    async fn test_cannot_delete_root_user() {
        let mgr = make_manager();
        let err = mgr.delete_user("root").await.unwrap_err();
        assert!(matches!(err, GraphError::InvalidOperation(_)));
        let alias_err = mgr.delete_user(" ROOT ").await.unwrap_err();
        assert!(matches!(alias_err, GraphError::InvalidOperation(_)));
    }

    #[tokio::test]
    async fn test_remove_cached_user_if_present_is_idempotent() {
        let mgr = make_manager();
        assert!(mgr
            .remove_cached_user_if_present("missing")
            .await
            .unwrap()
            .is_empty());

        mgr.create_user("alice", "pw", vec!["USER".to_string()])
            .await
            .unwrap();
        let session = mgr.authenticate("alice", "pw").await.unwrap();
        assert_eq!(
            mgr.remove_cached_user_if_present("alice").await.unwrap(),
            vec![session]
        );
        assert!(mgr
            .remove_cached_user_if_present("alice")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_upsert_user_replaces_security_state_and_invalidates_sessions() {
        let mgr = make_manager();
        mgr.create_user("Alice", "old-pw", vec!["USER".to_string()])
            .await
            .unwrap();
        let old_session = mgr.authenticate("Alice", "old-pw").await.unwrap();
        let new_hash = AuthManager::hash_password("new-pw").unwrap();

        let removed = mgr
            .upsert_user(User {
                username: " Alice ".to_string(),
                password_hash: new_hash,
                roles: vec![" admin ".to_string(), "ADMIN".to_string()],
                enabled: true,
            })
            .await
            .unwrap();

        assert_eq!(removed, vec![old_session]);
        assert!(mgr.get_session_roles(old_session).await.is_none());
        assert!(mgr.authenticate("Alice", "old-pw").await.is_err());
        let new_session = mgr.authenticate("Alice", "new-pw").await.unwrap();
        assert!(mgr.is_admin_session(new_session).await);
        assert_eq!(
            mgr.users.read().await.get("Alice").unwrap().roles,
            vec!["ADMIN"]
        );
    }

    #[tokio::test]
    async fn test_upsert_user_rejects_invalid_hash_without_replacing_cache() {
        let mgr = make_manager();
        mgr.create_user("alice", "pw", vec!["USER".to_string()])
            .await
            .unwrap();

        let result = mgr
            .upsert_user(User {
                username: "alice".to_string(),
                password_hash: "not-a-password-hash".to_string(),
                roles: vec!["ADMIN".to_string()],
                enabled: true,
            })
            .await;

        assert!(result.is_err());
        assert!(mgr.authenticate("alice", "pw").await.is_ok());
    }

    #[tokio::test]
    async fn test_upsert_user_cannot_override_env_backed_root() {
        let mgr = make_manager();
        let attacker_hash = AuthManager::hash_password("attacker-pw").unwrap();
        let result = mgr
            .upsert_user(User {
                username: " ROOT ".to_string(),
                password_hash: attacker_hash,
                roles: vec!["GOD".to_string()],
                enabled: true,
            })
            .await;

        assert!(result.is_err());
        assert!(mgr.authenticate("root", TEST_PW).await.is_ok());
        assert!(mgr.authenticate("root", "attacker-pw").await.is_err());
    }

    #[tokio::test]
    async fn test_grant_and_revoke_role() {
        let mgr = make_manager();
        mgr.create_user("alice", "pw", vec!["USER".to_string()])
            .await
            .unwrap();

        mgr.grant_role("alice", "DBA").await.unwrap();
        {
            let users = mgr.users.read().await;
            assert!(users
                .get("alice")
                .unwrap()
                .roles
                .contains(&"DBA".to_string()));
        }

        mgr.revoke_role("alice", "USER").await.unwrap();
        {
            let users = mgr.users.read().await;
            let user = users.get("alice").unwrap();
            assert!(!user.roles.contains(&"USER".to_string()));
            assert!(user.roles.contains(&"DBA".to_string()));
        }
    }

    #[tokio::test]
    async fn test_grant_role_unknown_user_fails() {
        let mgr = make_manager();
        let err = mgr.grant_role("ghost", "USER").await.unwrap_err();
        assert!(matches!(err, GraphError::InvalidOperation(_)));
    }

    #[tokio::test]
    async fn test_security_mutations_invalidate_existing_sessions() {
        let mgr = make_manager();
        mgr.create_user("alice", "pw", vec!["USER".to_string()])
            .await
            .unwrap();

        let before_grant = mgr.authenticate("alice", "pw").await.unwrap();
        mgr.grant_role(" alice ", " admin ").await.unwrap();
        assert!(mgr.get_session_roles(before_grant).await.is_none());

        let before_revoke = mgr.authenticate("alice", "pw").await.unwrap();
        assert!(mgr.is_admin_session(before_revoke).await);
        mgr.revoke_role("alice", "admin").await.unwrap();
        assert!(mgr.get_session_roles(before_revoke).await.is_none());

        let before_disable = mgr.authenticate("alice", "pw").await.unwrap();
        mgr.set_user_enabled("alice", false).await.unwrap();
        assert!(mgr.get_session_roles(before_disable).await.is_none());

        mgr.set_user_enabled("alice", true).await.unwrap();
        let before_drop = mgr.authenticate("alice", "pw").await.unwrap();
        mgr.delete_user("alice").await.unwrap();
        assert!(mgr.get_session_roles(before_drop).await.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_security_mutation_during_password_verification_fails_closed() {
        let mgr = std::sync::Arc::new(make_manager());
        mgr.create_user("alice", "pw", vec!["USER".to_string()])
            .await
            .unwrap();

        let verification_started = std::sync::Arc::new(std::sync::Barrier::new(2));
        let resume_verification = std::sync::Arc::new(std::sync::Barrier::new(2));
        let auth_mgr = std::sync::Arc::clone(&mgr);
        let auth_started = std::sync::Arc::clone(&verification_started);
        let auth_resume = std::sync::Arc::clone(&resume_verification);
        let authentication = tokio::spawn(async move {
            auth_mgr
                .authenticate_with_verifier("alice", "pw", move |_, _| {
                    auth_started.wait();
                    auth_resume.wait();
                    true
                })
                .await
        });

        verification_started.wait();
        mgr.set_user_enabled("alice", false).await.unwrap();
        resume_verification.wait();

        let result = authentication.await.unwrap();
        assert!(matches!(result, Err(GraphError::AuthFailed(_))));
        assert!(mgr.sessions.read().await.is_empty());
    }

    #[tokio::test]
    async fn test_invalidate_user_sessions_only_removes_target_user() {
        let mgr = make_manager();
        mgr.create_user("alice", "pw", vec!["USER".to_string()])
            .await
            .unwrap();
        mgr.create_user("bob", "pw", vec!["USER".to_string()])
            .await
            .unwrap();
        let alice_1 = mgr.authenticate("alice", "pw").await.unwrap();
        let alice_2 = mgr.authenticate("alice", "pw").await.unwrap();
        let bob = mgr.authenticate("bob", "pw").await.unwrap();

        let mut expected = vec![alice_1, alice_2];
        expected.sort_unstable();
        assert_eq!(mgr.invalidate_user_sessions(" alice ").await, expected);
        assert!(mgr.get_session_roles(alice_1).await.is_none());
        assert!(mgr.get_session_roles(alice_2).await.is_none());
        assert!(mgr.get_session_roles(bob).await.is_some());
    }

    #[tokio::test]
    async fn test_is_admin_session_rejects_non_admin_invalid_and_expired() {
        let mgr = make_manager();
        mgr.create_user("guest", "pw", vec!["GUEST".to_string()])
            .await
            .unwrap();
        let guest = mgr.authenticate("guest", "pw").await.unwrap();
        let root = mgr.authenticate("root", TEST_PW).await.unwrap();

        assert!(mgr.is_admin_session(root).await);
        assert!(!mgr.is_admin_session(guest).await);
        assert!(!mgr.is_admin_session(i64::MAX).await);
        mgr.sessions
            .write()
            .await
            .get_mut(&root)
            .unwrap()
            .expires_at = SystemTime::UNIX_EPOCH;
        assert!(!mgr.is_admin_session(root).await);
    }

    #[tokio::test]
    async fn test_sign_out_removes_session() {
        let mgr = make_manager();
        let sid = mgr.authenticate("root", TEST_PW).await.unwrap();
        mgr.sign_out(sid).await.unwrap();

        // Permission check on signed-out session should fail
        let err = mgr
            .check_permission(sid, "any_space", Permission::Read)
            .await
            .unwrap_err();
        assert!(matches!(err, GraphError::AuthFailed(_)));
    }

    #[tokio::test]
    async fn test_sign_out_unknown_session_fails() {
        let mgr = make_manager();
        let err = mgr.sign_out(9999).await.unwrap_err();
        assert!(matches!(err, GraphError::InvalidOperation(_)));
    }

    #[tokio::test]
    async fn test_session_expires() {
        let mgr = make_manager_with_ttl(Duration::from_millis(50));
        let sid = mgr.authenticate("root", TEST_PW).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        let err = mgr
            .check_permission(sid, "any_space", Permission::Read)
            .await
            .unwrap_err();
        assert!(matches!(err, GraphError::AuthFailed(_)));
        assert!(err.to_string().to_lowercase().contains("expired"));
    }

    #[tokio::test]
    async fn test_cleanup_expired_sessions() {
        let mgr = make_manager_with_ttl(Duration::from_millis(50));
        let _s1 = mgr.authenticate("root", TEST_PW).await.unwrap();
        mgr.create_user("alice", "pw", vec!["USER".to_string()])
            .await
            .unwrap();
        let _s2 = mgr.authenticate("alice", "pw").await.unwrap();

        assert_eq!(mgr.sessions.read().await.len(), 2);

        tokio::time::sleep(Duration::from_millis(100)).await;

        let removed = mgr.cleanup_expired_sessions().await;
        assert_eq!(removed, 2);
        assert_eq!(mgr.sessions.read().await.len(), 0);
    }

    #[tokio::test]
    async fn test_check_permission_root_has_all() {
        let mgr = make_manager();
        let sid = mgr.authenticate("root", TEST_PW).await.unwrap();

        for p in [
            Permission::Read,
            Permission::Write,
            Permission::Create,
            Permission::Delete,
            Permission::Alter,
            Permission::Drop,
        ] {
            assert!(mgr.check_permission(sid, "any_space", p).await.unwrap());
        }
    }

    #[tokio::test]
    async fn test_check_permission_guest_is_read_only() {
        let mgr = make_manager();
        mgr.create_user("g", "pw", vec!["GUEST".to_string()])
            .await
            .unwrap();
        let sid = mgr.authenticate("g", "pw").await.unwrap();

        assert!(mgr
            .check_permission(sid, "any_space", Permission::Read)
            .await
            .unwrap());
        assert!(!mgr
            .check_permission(sid, "any_space", Permission::Write)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_set_user_enabled_blocks_auth() {
        let mgr = make_manager();
        mgr.create_user("alice", "pw", vec!["USER".to_string()])
            .await
            .unwrap();

        mgr.set_user_enabled("alice", false).await.unwrap();
        let err = mgr.authenticate("alice", "pw").await.unwrap_err();
        assert!(matches!(err, GraphError::AuthFailed(_)));
        assert!(err.to_string().contains(AUTH_FAILED_MESSAGE));

        mgr.set_user_enabled("alice", true).await.unwrap();
        assert!(mgr.authenticate("alice", "pw").await.is_ok());
    }

    #[tokio::test]
    async fn test_cannot_disable_root() {
        let mgr = make_manager();
        let err = mgr.set_user_enabled("root", false).await.unwrap_err();
        assert!(matches!(err, GraphError::InvalidOperation(_)));
        // Root is still usable
        assert!(mgr.authenticate("root", TEST_PW).await.is_ok());
    }

    #[test]
    fn test_generate_random_password_is_unique_and_long() {
        let a = AuthManager::generate_random_password();
        let b = AuthManager::generate_random_password();
        assert_eq!(a.len(), 48);
        assert_eq!(b.len(), 48);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_session_is_expired() {
        let now = SystemTime::now();
        let fresh = Session {
            id: 1,
            username: "u".to_string(),
            roles: vec![],
            created_at: now,
            expires_at: now + Duration::from_secs(60),
        };
        assert!(!fresh.is_expired());

        let stale = Session {
            id: 2,
            username: "u".to_string(),
            roles: vec![],
            created_at: now - Duration::from_secs(120),
            expires_at: now - Duration::from_secs(60),
        };
        assert!(stale.is_expired());
    }

    #[test]
    fn test_session_id_allocation_skips_nonpositive_and_colliding_candidates() {
        let now = SystemTime::now();
        let sessions = HashMap::from([(
            7,
            Session {
                id: 7,
                username: "user".to_string(),
                roles: vec!["USER".to_string()],
                created_at: now,
                expires_at: now + Duration::from_secs(60),
            },
        )]);
        let mut candidates = [0, -1, 7, 9].into_iter();

        let allocated =
            AuthManager::allocate_session_id_with(&sessions, || candidates.next().unwrap_or(9));
        assert_eq!(allocated, 9);
    }

    #[tokio::test]
    async fn test_correct_password_succeeds_during_lockout() {
        let mgr = make_manager();

        // Fail MAX_FAILED_ATTEMPTS times
        for _ in 0..MAX_FAILED_ATTEMPTS {
            let err = mgr.authenticate("root", "wrong").await.unwrap_err();
            assert!(matches!(err, GraphError::AuthFailed(_)));
        }

        assert!(mgr
            .failed_attempts
            .read()
            .await
            .get("root")
            .and_then(|entry| entry.locked_until)
            .is_some());

        // A correct secret must recover the account and clear the counter;
        // unauthenticated failures alone cannot deny the legitimate operator.
        assert!(mgr.authenticate("root", TEST_PW).await.is_ok());
        assert!(mgr.failed_attempts.read().await.get("root").is_none());
    }

    #[tokio::test]
    async fn test_brute_force_counter_resets_on_success() {
        let mgr = make_manager();

        // Fail a few times (below threshold)
        for _ in 0..MAX_FAILED_ATTEMPTS - 1 {
            let _ = mgr.authenticate("root", "wrong").await;
        }

        // Successful login clears counter
        assert!(mgr.authenticate("root", TEST_PW).await.is_ok());

        // A second below-threshold sequence must also permit the valid secret;
        // without the reset, these failures would cross the lockout threshold.
        for _ in 0..MAX_FAILED_ATTEMPTS - 1 {
            let _ = mgr.authenticate("root", "wrong").await;
        }
        assert!(mgr.authenticate("root", TEST_PW).await.is_ok());
    }
}
