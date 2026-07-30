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
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

/// Default session TTL: 24 hours
pub const DEFAULT_SESSION_TTL_SECS: u64 = 24 * 60 * 60;

/// Environment variable that sets the root user's password at startup.
/// If unset, embedded callers receive a cryptographically random password that
/// is deliberately never written to logs. The standalone server requires this
/// variable and fails fast before constructing the authentication manager.
pub const ROOT_PASSWORD_ENV: &str = "BYORIDB_ROOT_PASSWORD";

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
    /// Valid Argon2 hash used to equalize authentication work for unknown or
    /// locked accounts without creating per-unknown-user state.
    dummy_password_hash: String,
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
        //   2. Cryptographically random password (never logged)
        let (root_password, generated) = match std::env::var(ROOT_PASSWORD_ENV) {
            Ok(p) if !p.is_empty() => (p, false),
            _ => (Self::generate_random_password(), true),
        };

        if generated {
            tracing::warn!(
                "No {} env var set; generated an ephemeral root credential that is not logged. \
                Set {} before starting a network server.",
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
    /// Network launchers should resolve the root password out-of-band (for
    /// example, from a secrets manager) and use this constructor. [`Self::new`]
    /// and [`Self::with_ttl`] remain convenient for embedded/test contexts, but
    /// their fallback credential is intentionally undisclosed.
    pub fn with_config(root_password: &str, session_ttl: Duration) -> Self {
        let mut users = HashMap::new();

        let root_hash = Self::hash_password(root_password).expect("Failed to hash root password");
        // Generate the timing sentinel at startup and immediately discard its
        // plaintext. A source-known sentinel could be supplied deliberately,
        // turning the dummy verification into its success path.
        let dummy_password_hash = Self::hash_password(&Self::generate_random_password())
            .expect("Failed to initialize authentication timing sentinel");
        let root = User {
            username: "root".to_string(),
            password_hash: root_hash,
            roles: vec!["GOD".to_string()],
            enabled: true,
        };
        users.insert("root".to_string(), root);

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
            dummy_password_hash,
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

    /// Authenticate a user
    pub async fn authenticate(&self, username: &str, password: &str) -> Result<i64> {
        // Brute-force check: reject if account is locked
        let account_locked = {
            let attempts = self.failed_attempts.read().await;
            attempts
                .get(username)
                .and_then(|entry| entry.locked_until)
                .is_some_and(|locked_until| SystemTime::now() < locked_until)
        };
        if account_locked {
            // Keep locked-account responses computationally comparable to a
            // normal password attempt. Do not reveal this state at transports.
            let _ = Self::verify_password(password, &self.dummy_password_hash);
            return Err(GraphError::AuthFailed(
                "Account locked due to too many failed attempts. Try again later.".to_string(),
            ));
        }

        let users = self.users.read().await;
        let Some(user) = users.get(username) else {
            // Unknown usernames still pay one Argon2 verification, preventing a
            // cheap timing oracle. They are intentionally not added to
            // `failed_attempts`, which would permit unbounded memory growth.
            let _ = Self::verify_password(password, &self.dummy_password_hash);
            return Err(GraphError::AuthFailed("User not found".to_string()));
        };

        // Always perform the expensive verification before reporting disabled
        // state so all credential failures have comparable work.
        let password_valid = Self::verify_password(password, &user.password_hash);
        if !user.enabled {
            return Err(GraphError::AuthFailed("User is disabled".to_string()));
        }

        if !password_valid {
            // Record failure
            let mut attempts = self.failed_attempts.write().await;
            let entry = attempts
                .entry(username.to_string())
                .or_insert(FailedAttempt {
                    count: 0,
                    locked_until: None,
                });
            entry.count += 1;
            if entry.count >= MAX_FAILED_ATTEMPTS {
                entry.locked_until =
                    Some(SystemTime::now() + Duration::from_secs(LOCKOUT_DURATION_SECS));
                tracing::warn!(
                    failed_attempts = entry.count,
                    "Account locked after repeated authentication failures"
                );
            }
            return Err(GraphError::AuthFailed("Invalid password".to_string()));
        }

        // Success: clear failure counter
        self.failed_attempts.write().await.remove(username);

        let now = SystemTime::now();
        let mut sessions = self.sessions.write().await;
        // Cryptographically random 63-bit bearer token. Clear the sign bit
        // instead of calling `i64::abs`, which overflows for `i64::MIN`, and
        // retry the vanishingly unlikely collision rather than replacing an
        // existing session.
        let session_id = loop {
            use argon2::password_hash::rand_core::{OsRng, RngCore};
            let candidate = (OsRng.next_u64() >> 1) as i64;
            if candidate != 0 && !sessions.contains_key(&candidate) {
                break candidate;
            }
        };
        let session = Session {
            id: session_id,
            username: username.to_string(),
            roles: user.roles.clone(),
            created_at: now,
            expires_at: now + self.session_ttl,
        };
        sessions.insert(session_id, session);
        tracing::info!("User authenticated");

        Ok(session_id)
    }

    /// Create a new user
    pub async fn create_user(
        &self,
        username: &str,
        password: &str,
        roles: Vec<String>,
    ) -> Result<()> {
        if username.eq_ignore_ascii_case("root") {
            return Err(GraphError::InvalidOperation(
                "The root identity is reserved".to_string(),
            ));
        }
        if roles.iter().any(|role| role.eq_ignore_ascii_case("GOD")) {
            return Err(GraphError::InvalidOperation(
                "The root-only role cannot be assigned".to_string(),
            ));
        }
        let mut users = self.users.write().await;

        if users.contains_key(username) {
            return Err(GraphError::InvalidOperation(format!(
                "User {} already exists",
                username
            )));
        }

        let user = User {
            username: username.to_string(),
            password_hash: Self::hash_password(password)?,
            roles,
            enabled: true,
        };

        users.insert(username.to_string(), user);
        drop(users);

        tracing::info!("User created");

        Ok(())
    }

    /// Delete a user
    pub async fn delete_user(&self, username: &str) -> Result<()> {
        let mut users = self.users.write().await;

        if username.eq_ignore_ascii_case("root") {
            return Err(GraphError::InvalidOperation(
                "Cannot delete root user".to_string(),
            ));
        }

        users
            .remove(username)
            .ok_or_else(|| GraphError::InvalidOperation(format!("User {} not found", username)))?;
        drop(users);

        self.invalidate_user_sessions(username).await;

        tracing::info!("User deleted");

        Ok(())
    }

    /// Grant a role to a user
    pub async fn grant_role(&self, username: &str, role: &str) -> Result<()> {
        if username.eq_ignore_ascii_case("root") {
            return Err(GraphError::InvalidOperation(
                "Cannot alter root user roles".to_string(),
            ));
        }
        if role.eq_ignore_ascii_case("GOD") {
            return Err(GraphError::InvalidOperation(
                "The root-only role cannot be assigned".to_string(),
            ));
        }
        let mut users = self.users.write().await;

        let user = users
            .get_mut(username)
            .ok_or_else(|| GraphError::InvalidOperation(format!("User {} not found", username)))?;

        if !user.roles.contains(&role.to_string()) {
            user.roles.push(role.to_string());
        }
        drop(users);

        self.invalidate_user_sessions(username).await;

        tracing::info!("Role granted to user");

        Ok(())
    }

    /// Revoke a role from a user
    pub async fn revoke_role(&self, username: &str, role: &str) -> Result<()> {
        if username.eq_ignore_ascii_case("root") {
            return Err(GraphError::InvalidOperation(
                "Cannot alter root user roles".to_string(),
            ));
        }
        let mut users = self.users.write().await;

        let user = users
            .get_mut(username)
            .ok_or_else(|| GraphError::InvalidOperation(format!("User {} not found", username)))?;

        user.roles.retain(|r| r != role);
        drop(users);

        self.invalidate_user_sessions(username).await;

        tracing::info!("Role revoked from user");

        Ok(())
    }

    /// Enable or disable a user. Disabled users cannot authenticate.
    /// The root user cannot be disabled.
    pub async fn set_user_enabled(&self, username: &str, enabled: bool) -> Result<()> {
        if username.eq_ignore_ascii_case("root") {
            return Err(GraphError::InvalidOperation(
                "Cannot alter root user state".to_string(),
            ));
        }

        let mut users = self.users.write().await;
        let user = users
            .get_mut(username)
            .ok_or_else(|| GraphError::InvalidOperation(format!("User {} not found", username)))?;

        user.enabled = enabled;
        drop(users);

        // Authentication state is captured in bearer sessions. Revoke existing
        // sessions whenever account state changes so disable takes effect now.
        self.invalidate_user_sessions(username).await;

        tracing::info!(enabled = enabled, "User enabled state changed");

        Ok(())
    }

    /// Check if a session has permission
    pub async fn check_permission(
        &self,
        session_id: i64,
        space: &str,
        permission: Permission,
    ) -> Result<bool> {
        let session_roles = self
            .get_session_roles(session_id)
            .await
            .ok_or_else(|| GraphError::AuthFailed("Invalid or expired session".to_string()))?;

        let roles = self.roles.read().await;

        // Check if any role has the required permission
        for role_name in &session_roles {
            if let Some(role) = roles.get(role_name) {
                if role.has_permission(space, permission) {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Get the roles associated with a session (for RBAC propagation to executor).
    /// Successful access refreshes the session's sliding TTL.
    pub async fn get_session_roles(&self, session_id: i64) -> Option<Vec<String>> {
        let now = SystemTime::now();
        let mut sessions = self.sessions.write().await;
        match sessions.get_mut(&session_id) {
            Some(session) if now < session.expires_at => {
                session.expires_at = now + self.session_ttl;
                Some(session.roles.clone())
            }
            Some(_) => {
                sessions.remove(&session_id);
                None
            }
            None => None,
        }
    }

    /// Check liveness without refreshing the sliding TTL. Background
    /// reconciliation uses this so maintenance itself cannot keep sessions
    /// alive forever.
    pub async fn has_live_session_without_touch(&self, session_id: i64) -> bool {
        let now = SystemTime::now();
        let mut sessions = self.sessions.write().await;
        match sessions.get(&session_id) {
            Some(session) if now < session.expires_at => true,
            Some(_) => {
                sessions.remove(&session_id);
                false
            }
            None => false,
        }
    }

    /// Whether a live bearer session belongs to a GOD/ADMIN principal.
    /// Missing and expired sessions are authentication failures; a valid
    /// non-admin session returns `Ok(false)` so HTTP can distinguish 401/403.
    pub async fn is_admin_session(&self, session_id: i64) -> Result<bool> {
        let roles = self
            .get_session_roles(session_id)
            .await
            .ok_or_else(|| GraphError::AuthFailed("Invalid or expired session".to_string()))?;
        Ok(roles.iter().any(|role| role == "GOD" || role == "ADMIN"))
    }

    /// Replace a user from the persisted KV representation. Root is owned by
    /// the process bootstrap credential and is never overridden by storage.
    /// Existing sessions are revoked when authentication or role state changes.
    pub async fn upsert_persisted_user(&self, user: User) -> Result<bool> {
        if user.username.eq_ignore_ascii_case("root") {
            return Ok(false);
        }
        if user
            .roles
            .iter()
            .any(|role| role.eq_ignore_ascii_case("GOD"))
        {
            return Err(GraphError::InvalidOperation(
                "Persisted user record contains a root-only role".to_string(),
            ));
        }
        if user.username.is_empty() || user.password_hash.is_empty() {
            return Err(GraphError::InvalidOperation(
                "Persisted user record is incomplete".to_string(),
            ));
        }
        {
            let known_roles = self.roles.read().await;
            if user
                .roles
                .iter()
                .any(|role| !known_roles.contains_key(role))
            {
                return Err(GraphError::InvalidOperation(
                    "Persisted user record contains an unknown role".to_string(),
                ));
            }
        }

        let changed = {
            let mut users = self.users.write().await;
            if users.get(&user.username) == Some(&user) {
                false
            } else {
                users.insert(user.username.clone(), user.clone());
                true
            }
        };
        if changed {
            self.invalidate_user_sessions(&user.username).await;
        }
        Ok(changed)
    }

    /// Change a user's password and revoke all of that user's bearer sessions.
    pub async fn change_password(&self, username: &str, password: &str) -> Result<()> {
        if username.eq_ignore_ascii_case("root") {
            return Err(GraphError::InvalidOperation(
                "Root password is managed by process configuration".to_string(),
            ));
        }
        let password_hash = Self::hash_password(password)?;
        let mut users = self.users.write().await;
        let user = users
            .get_mut(username)
            .ok_or_else(|| GraphError::InvalidOperation(format!("User {} not found", username)))?;
        user.password_hash = password_hash;
        drop(users);
        self.invalidate_user_sessions(username).await;
        tracing::info!("User password changed");
        Ok(())
    }

    /// Revoke every live bearer session owned by a user.
    pub async fn invalidate_user_sessions(&self, username: &str) -> usize {
        let mut sessions = self.sessions.write().await;
        let before = sessions.len();
        sessions.retain(|_, session| session.username != username);
        before - sessions.len()
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
        sessions
            .remove(&session_id)
            .ok_or_else(|| GraphError::InvalidOperation("Session not found".to_string()))?;

        tracing::info!("Session signed out");

        Ok(())
    }

    /// Hash password using argon2 (delegates to byoridb_common::crypto)
    fn hash_password(password: &str) -> Result<String> {
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
        assert!(
            !mgr.failed_attempts.read().await.contains_key("ghost"),
            "unknown usernames must not grow the lockout map"
        );
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
    async fn god_role_is_root_only_across_public_assignment_apis() {
        let mgr = make_manager();

        let create_error = mgr
            .create_user("alice", "pw", vec!["gOd".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(create_error, GraphError::InvalidOperation(_)));
        assert!(!mgr.users.read().await.contains_key("alice"));

        mgr.create_user("alice", "pw", vec!["USER".to_string()])
            .await
            .unwrap();
        let grant_error = mgr.grant_role("alice", "god").await.unwrap_err();
        assert!(matches!(grant_error, GraphError::InvalidOperation(_)));
        assert_eq!(
            mgr.users.read().await.get("alice").unwrap().roles,
            vec!["USER".to_string()]
        );
    }

    #[tokio::test]
    async fn persisted_god_role_is_rejected_without_installing_user() {
        let mgr = make_manager();
        let user = User {
            username: "legacy-admin".to_string(),
            password_hash: byoridb_common::crypto::hash_password("pw").unwrap(),
            roles: vec!["God".to_string()],
            enabled: true,
        };

        let error = mgr.upsert_persisted_user(user).await.unwrap_err();
        assert!(matches!(error, GraphError::InvalidOperation(_)));
        assert!(!mgr.users.read().await.contains_key("legacy-admin"));
    }

    #[tokio::test]
    async fn test_grant_role_unknown_user_fails() {
        let mgr = make_manager();
        let err = mgr.grant_role("ghost", "USER").await.unwrap_err();
        assert!(matches!(err, GraphError::InvalidOperation(_)));
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
    async fn session_role_access_refreshes_sliding_ttl() {
        let mgr = make_manager_with_ttl(Duration::from_millis(200));
        let sid = mgr.authenticate("root", TEST_PW).await.unwrap();

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(mgr.get_session_roles(sid).await.is_some());
        // Total age now exceeds the original TTL, but not the refreshed TTL.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(mgr.get_session_roles(sid).await.is_some());
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
        assert!(err.to_string().to_lowercase().contains("disabled"));

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

    #[tokio::test]
    async fn test_brute_force_lockout() {
        let mgr = make_manager();

        // Fail MAX_FAILED_ATTEMPTS times
        for _ in 0..MAX_FAILED_ATTEMPTS {
            let err = mgr.authenticate("root", "wrong").await.unwrap_err();
            assert!(matches!(err, GraphError::AuthFailed(_)));
        }

        // Next attempt should be locked
        let err = mgr.authenticate("root", TEST_PW).await.unwrap_err();
        assert!(err.to_string().contains("locked"));
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

        // Should be able to fail again without immediate lockout
        let err = mgr.authenticate("root", "wrong").await.unwrap_err();
        assert!(matches!(err, GraphError::AuthFailed(_)));
        assert!(!err.to_string().contains("locked"));
    }

    #[tokio::test]
    async fn persisted_auth_change_revokes_existing_sessions() {
        let mgr = make_manager();
        mgr.create_user("alice", "old-password", vec!["USER".to_string()])
            .await
            .unwrap();
        let old_session = mgr.authenticate("alice", "old-password").await.unwrap();

        let replacement = User {
            username: "alice".to_string(),
            password_hash: byoridb_common::crypto::hash_password("new-password").unwrap(),
            roles: vec!["DBA".to_string()],
            enabled: true,
        };
        assert!(mgr.upsert_persisted_user(replacement).await.unwrap());

        assert!(mgr.get_session_roles(old_session).await.is_none());
        assert!(mgr.authenticate("alice", "old-password").await.is_err());
        assert!(mgr.authenticate("alice", "new-password").await.is_ok());
    }

    #[tokio::test]
    async fn deleting_user_revokes_existing_sessions() {
        let mgr = make_manager();
        mgr.create_user("alice", "password", vec!["USER".to_string()])
            .await
            .unwrap();
        let session = mgr.authenticate("alice", "password").await.unwrap();

        mgr.delete_user("alice").await.unwrap();

        assert!(mgr.get_session_roles(session).await.is_none());
        assert!(mgr
            .check_permission(session, "default", Permission::Read)
            .await
            .is_err());
    }
}
