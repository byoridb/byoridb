// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Readiness / graceful-shutdown coordination.
//!
//! A single [`ShutdownState`] is shared by the gRPC and HTTP service and by
//! the server binary's signal handler:
//!
//! 1. On SIGTERM/SIGINT the binary calls [`ShutdownState::stop_accepting`].
//! 2. `/ready` starts returning 503, so Kubernetes pulls the pod out of the
//!    Service endpoints and stops routing new connections.
//! 3. `GraphService::execute` rejects new queries with a clear error.
//! 4. The binary drains: it waits until [`ShutdownState::active_queries`]
//!    reaches zero (queries already running finish normally) or a timeout
//!    expires, and only then tears the servers down.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const STOPPING: u64 = 1 << 63;
const ACTIVE_MASK: u64 = STOPPING - 1;

/// Shared readiness flag + in-flight query counter.
///
/// The counter is maintained by `GraphService`'s active-query RAII guard, so
/// it covers every exit path (success, error, panic-unwind drop).
#[derive(Debug)]
pub struct ShutdownState {
    // Admission and shutdown share one atomic word. A request can never pass
    // the accepting check and become invisible to a concurrent drain.
    state: AtomicU64,
}

pub(crate) struct AdmissionGuard(Arc<ShutdownState>);

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        self.0.state.fetch_sub(1, Ordering::AcqRel);
    }
}

impl ShutdownState {
    pub fn new() -> Self {
        Self {
            state: AtomicU64::new(0),
        }
    }

    /// Whether new queries are accepted (readiness).
    pub fn is_accepting(&self) -> bool {
        self.state.load(Ordering::Acquire) & STOPPING == 0
    }

    /// Flip readiness off: `/ready` → 503, new queries rejected.
    pub fn stop_accepting(&self) {
        self.state.fetch_or(STOPPING, Ordering::AcqRel);
    }

    /// Number of queries currently executing across all services sharing
    /// this state.
    pub fn active_queries(&self) -> u64 {
        self.state.load(Ordering::Acquire) & ACTIVE_MASK
    }

    /// Reserve a drain slot before the first request await, including auth.
    pub(crate) fn try_admit(self: &Arc<Self>) -> Option<AdmissionGuard> {
        self.state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state & STOPPING == 0 && state < ACTIVE_MASK).then(|| state + 1)
            })
            .ok()
            .map(|_| AdmissionGuard(self.clone()))
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
        let s = Arc::new(ShutdownState::new());
        let admitted = s.try_admit().unwrap();
        s.stop_accepting();
        assert!(s.try_admit().is_none());
        assert!(
            !s.drain(Duration::from_millis(50), Duration::from_millis(10))
                .await
        );
        drop(admitted);
        assert!(
            s.drain(Duration::from_secs(1), Duration::from_millis(10))
                .await
        );
    }

    #[test]
    fn admission_and_shutdown_are_one_ordered_transition() {
        for _ in 0..100 {
            let state = Arc::new(ShutdownState::new());
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let other_state = state.clone();
            let other_barrier = barrier.clone();
            let request = std::thread::spawn(move || {
                other_barrier.wait();
                other_state.try_admit()
            });
            barrier.wait();
            state.stop_accepting();
            let admitted = request.join().unwrap();
            assert_eq!(state.active_queries(), u64::from(admitted.is_some()));
            assert!(state.try_admit().is_none());
            drop(admitted);
            assert_eq!(state.active_queries(), 0);
        }
    }
}
