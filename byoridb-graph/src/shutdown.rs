// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Readiness / graceful-shutdown coordination.
//!
//! A single [`ShutdownState`] is shared by the gRPC and HTTP servers (each
//! owns its own `GraphService`) and by the server binary's signal handler:
//!
//! 1. On SIGTERM/SIGINT the binary calls [`ShutdownState::stop_accepting`].
//! 2. `/ready` starts returning 503, so Kubernetes pulls the pod out of the
//!    Service endpoints and stops routing new connections.
//! 3. `GraphService::execute` rejects new queries with a clear error.
//! 4. The binary drains: it waits until [`ShutdownState::active_queries`]
//!    reaches zero (queries already running finish normally) or a timeout
//!    expires, and only then tears the servers down.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Shared readiness flag + in-flight query counter.
///
/// The counter is maintained by `GraphService`'s active-query RAII guard, so
/// it covers every exit path (success, error, panic-unwind drop).
#[derive(Debug)]
pub struct ShutdownState {
    accepting: AtomicBool,
    active_queries: AtomicU64,
}

impl ShutdownState {
    pub fn new() -> Self {
        Self {
            accepting: AtomicBool::new(true),
            active_queries: AtomicU64::new(0),
        }
    }

    /// Whether new queries are accepted (readiness).
    pub fn is_accepting(&self) -> bool {
        self.accepting.load(Ordering::Acquire)
    }

    /// Flip readiness off: `/ready` → 503, new queries rejected.
    pub fn stop_accepting(&self) {
        self.accepting.store(false, Ordering::Release);
    }

    /// Number of queries currently executing across all services sharing
    /// this state.
    pub fn active_queries(&self) -> u64 {
        self.active_queries.load(Ordering::Acquire)
    }

    pub(crate) fn query_started(&self) {
        self.active_queries.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn query_finished(&self) {
        self.active_queries.fetch_sub(1, Ordering::AcqRel);
    }

    /// Wait until no queries are in flight, polling every `poll` interval,
    /// up to `timeout`. Returns `true` if fully drained, `false` on timeout
    /// (remaining queries will be cut when the servers stop).
    pub async fn drain(&self, timeout: std::time::Duration, poll: std::time::Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = self.active_queries();
            if remaining == 0 {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(poll).await;
        }
    }
}

impl Default for ShutdownState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn accepting_flag_flips_once() {
        let s = ShutdownState::new();
        assert!(s.is_accepting());
        s.stop_accepting();
        assert!(!s.is_accepting());
    }

    #[tokio::test]
    async fn drain_returns_immediately_when_idle() {
        let s = ShutdownState::new();
        assert!(
            s.drain(Duration::from_secs(1), Duration::from_millis(10))
                .await
        );
    }

    #[tokio::test]
    async fn drain_times_out_with_active_queries() {
        let s = ShutdownState::new();
        s.query_started();
        assert!(
            !s.drain(Duration::from_millis(50), Duration::from_millis(10))
                .await
        );
        s.query_finished();
        assert!(
            s.drain(Duration::from_secs(1), Duration::from_millis(10))
                .await
        );
    }
}
