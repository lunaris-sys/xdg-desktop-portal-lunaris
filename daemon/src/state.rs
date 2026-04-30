//! Shared daemon state.
//!
//! Carries the open-request counter that gates the idle-timeout (FA10,
//! E12). Cloning is cheap — internally an `Arc`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Cheaply-cloneable handle to the daemon's runtime state.
#[derive(Clone)]
pub struct DaemonState {
    inner: Arc<Inner>,
}

struct Inner {
    /// Number of in-flight portal requests. Incremented when a method
    /// handler enters and decremented when it returns. Idle-timeout
    /// must NOT fire while this is > 0 — see edge case E12.
    open_requests: AtomicUsize,
}

impl DaemonState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                open_requests: AtomicUsize::new(0),
            }),
        }
    }

    /// Returns a guard that increments the counter on creation and
    /// decrements it on drop. The guard pattern is what protects against
    /// counter leaks if a method handler panics or short-circuits.
    pub fn track_request(&self) -> RequestGuard {
        self.inner.open_requests.fetch_add(1, Ordering::SeqCst);
        RequestGuard {
            state: self.clone(),
        }
    }

    /// True if at least one method is currently in flight.
    pub fn has_open_requests(&self) -> bool {
        self.inner.open_requests.load(Ordering::SeqCst) > 0
    }

    /// Test-only accessor for the counter value.
    #[cfg(test)]
    pub fn open_request_count(&self) -> usize {
        self.inner.open_requests.load(Ordering::SeqCst)
    }
}

impl Default for DaemonState {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard. Drop it to release the slot.
pub struct RequestGuard {
    state: DaemonState,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.state
            .inner
            .open_requests
            .fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counter increments and decrements via the guard.
    #[test]
    fn guard_increments_and_decrements() {
        let s = DaemonState::new();
        assert_eq!(s.open_request_count(), 0);
        let g = s.track_request();
        assert_eq!(s.open_request_count(), 1);
        let g2 = s.track_request();
        assert_eq!(s.open_request_count(), 2);
        drop(g);
        assert_eq!(s.open_request_count(), 1);
        drop(g2);
        assert_eq!(s.open_request_count(), 0);
    }

    /// `has_open_requests` reads the same counter as the guard touches.
    #[test]
    fn has_open_requests_reflects_counter() {
        let s = DaemonState::new();
        assert!(!s.has_open_requests());
        let _g = s.track_request();
        assert!(s.has_open_requests());
    }
}
