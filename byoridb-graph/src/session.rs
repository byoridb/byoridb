// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Session management with lock-free concurrent access and expiration

use dashmap::DashMap;
use std::time::{Duration, SystemTime};

/// Default session TTL: 24 hours
pub const DEFAULT_SESSION_TTL_SECS: u64 = 24 * 60 * 60;

/// Graph session
#[derive(Debug, Clone)]
pub struct Session {
    pub id: i64,
    pub username: String,
    pub space: Option<String>,
    /// Session creation time
    pub created_at: SystemTime,
    /// Absolute expiration time
    pub expires_at: SystemTime,
    /// Last access time (sliding window)
    pub last_accessed: SystemTime,
}

impl Session {
    /// Returns true if the session has expired
    pub fn is_expired(&self) -> bool {
        SystemTime::now() >= self.expires_at
    }
}

/// Session manager using lock-free DashMap for concurrent access
pub struct SessionManager {
    sessions: DashMap<i64, Session>,
    /// Default TTL for new sessions
    ttl: Duration,
}

impl SessionManager {
    /// Create a new SessionManager with the default TTL (24 hours)
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_secs(DEFAULT_SESSION_TTL_SECS))
    }

    /// Create a new SessionManager with a custom TTL
    pub fn with_ttl(ttl: Duration) -> Self {
        SessionManager {
            sessions: DashMap::new(),
            ttl,
        }
    }

    /// Create a new session and return its ID (cryptographically random)
    pub async fn create_session(&self, username: String) -> i64 {
        let id = Self::random_session_id();
        let now = SystemTime::now();
        let session = Session {
            id,
            username,
            space: None,
            created_at: now,
            expires_at: now + self.ttl,
            last_accessed: now,
        };

        self.sessions.insert(id, session);
        id
    }

    fn random_session_id() -> i64 {
        use argon2::password_hash::rand_core::{OsRng, RngCore};
        // Cryptographically-random, collision-resistant id. The old
        // SystemTime-nanos + thread-id hash collided under concurrency (a few
        // tokio worker threads share the same coarse nanosecond), which dropped
        // sessions and risked session-id *reuse*. `>> 1` clears the sign bit so
        // the id is non-negative without the `i64::MIN.abs()` overflow hazard.
        (OsRng.next_u64() >> 1) as i64
    }

    /// Create a session with a specific ID (used when AuthManager provides the ID)
    pub async fn create_session_with_id(&self, id: i64, username: String) {
        let now = SystemTime::now();
        let session = Session {
            id,
            username,
            space: None,
            created_at: now,
            expires_at: now + self.ttl,
            last_accessed: now,
        };

        self.sessions.insert(id, session);
    }

    /// Remove a session by ID. Returns true when a session existed.
    pub async fn remove_session(&self, session_id: i64) -> bool {
        self.sessions.remove(&session_id).is_some()
    }

    /// Check if a session exists and is not expired.
    /// Expired sessions are removed as a side effect.
    pub async fn has_session(&self, session_id: i64) -> bool {
        // Scope the read guard so it is released before we attempt `remove`
        // on the same DashMap shard (which would otherwise deadlock).
        let is_expired = {
            match self.sessions.get(&session_id) {
                Some(s) => s.is_expired(),
                None => return false,
            }
        };

        if is_expired {
            self.sessions.remove(&session_id);
            false
        } else {
            true
        }
    }

    /// Get a session by ID (returns a clone). Returns None if expired.
    /// Touches the session to update last_accessed and extends expires_at
    /// (sliding window: each access resets the TTL countdown).
    pub async fn get_session(&self, session_id: i64) -> Option<Session> {
        let mut result = None;
        let mut expired = false;

        if let Some(mut session_ref) = self.sessions.get_mut(&session_id) {
            if session_ref.is_expired() {
                expired = true;
            } else {
                let now = SystemTime::now();
                session_ref.last_accessed = now;
                // Sliding window: push expiry forward on every access
                session_ref.expires_at = now + self.ttl;
                result = Some(session_ref.value().clone());
            }
        }

        if expired {
            self.sessions.remove(&session_id);
        }

        result
    }

    /// Set the active space for a session
    pub async fn set_space(&self, session_id: i64, space: String) -> Result<(), String> {
        if let Some(mut session_ref) = self.sessions.get_mut(&session_id) {
            if session_ref.is_expired() {
                drop(session_ref);
                self.sessions.remove(&session_id);
                return Err("Session expired".to_string());
            }
            session_ref.space = Some(space);
            session_ref.last_accessed = SystemTime::now();
            Ok(())
        } else {
            Err("Session not found".to_string())
        }
    }

    /// Get the number of active sessions (includes expired-but-not-yet-evicted)
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Return a snapshot of all active sessions as (session_id, username, space).
    pub fn list_sessions(&self) -> Vec<(i64, String, Option<String>)> {
        self.sessions
            .iter()
            .map(|e| {
                (
                    *e.key(),
                    e.value().username.clone(),
                    e.value().space.clone(),
                )
            })
            .collect()
    }

    /// Remove every graph-layer session owned by `username`. Authentication
    /// changes revoke the corresponding AuthManager sessions at the same time;
    /// clearing both stores keeps diagnostics and cleanup state consistent.
    pub fn remove_user_sessions(&self, username: &str) -> usize {
        let ids: Vec<i64> = self
            .sessions
            .iter()
            .filter(|entry| entry.value().username == username)
            .map(|entry| *entry.key())
            .collect();
        let count = ids.len();
        for id in ids {
            self.sessions.remove(&id);
        }
        count
    }

    /// Remove all expired sessions. Returns the number of sessions removed.
    /// Call this periodically from a background task.
    pub fn cleanup_expired(&self) -> usize {
        let now = SystemTime::now();
        let expired: Vec<i64> = self
            .sessions
            .iter()
            .filter(|entry| now >= entry.value().expires_at)
            .map(|entry| *entry.key())
            .collect();

        let count = expired.len();
        for id in expired {
            self.sessions.remove(&id);
        }
        count
    }

    /// Clear all sessions (for testing)
    #[cfg(test)]
    pub fn clear(&self) {
        self.sessions.clear();
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_create_session() {
        let manager = SessionManager::new();
        let id1 = manager.create_session("user1".to_string()).await;
        let id2 = manager.create_session("user2".to_string()).await;

        assert!(id1 > 0);
        assert!(id2 > 0);
        assert!(manager.has_session(id1).await);
        assert!(manager.has_session(id2).await);
    }

    #[tokio::test]
    async fn test_remove_session() {
        let manager = SessionManager::new();
        let id = manager.create_session("user".to_string()).await;

        assert!(manager.has_session(id).await);
        assert!(manager.remove_session(id).await);
        assert!(!manager.has_session(id).await);
        assert!(!manager.remove_session(id).await);
    }

    #[tokio::test]
    async fn test_set_space() {
        let manager = SessionManager::new();
        let id = manager.create_session("user".to_string()).await;

        manager.set_space(id, "my_space".to_string()).await.unwrap();

        let session = manager.get_session(id).await.unwrap();
        assert_eq!(session.space, Some("my_space".to_string()));
    }

    #[tokio::test]
    async fn test_session_expires() {
        // Session with 50ms TTL
        let manager = SessionManager::with_ttl(Duration::from_millis(50));
        let id = manager.create_session("user".to_string()).await;

        // Session should exist initially
        assert!(manager.has_session(id).await);

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Session should be expired and evicted
        assert!(!manager.has_session(id).await);
        assert!(manager.get_session(id).await.is_none());
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let manager = SessionManager::with_ttl(Duration::from_millis(50));
        let _id1 = manager.create_session("user1".to_string()).await;
        let _id2 = manager.create_session("user2".to_string()).await;

        assert_eq!(manager.session_count(), 2);

        tokio::time::sleep(Duration::from_millis(100)).await;
        let removed = manager.cleanup_expired();

        assert_eq!(removed, 2);
        assert_eq!(manager.session_count(), 0);
    }

    #[tokio::test]
    async fn test_set_space_on_expired_session_fails() {
        let manager = SessionManager::with_ttl(Duration::from_millis(50));
        let id = manager.create_session("user".to_string()).await;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let result = manager.set_space(id, "s".to_string()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expired"));
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let manager = Arc::new(SessionManager::new());
        let mut handles = vec![];

        // Spawn multiple tasks creating sessions concurrently
        for i in 0..100 {
            let mgr = manager.clone();
            handles.push(tokio::spawn(async move {
                mgr.create_session(format!("user{}", i)).await
            }));
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // All sessions should be created
        assert_eq!(manager.session_count(), 100);
    }
}
