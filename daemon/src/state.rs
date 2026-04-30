//! Shared daemon state.
//!
//! Carries the open-request counter that gates the idle-timeout (FA10,
//! E12) plus handles for the picker-IPC server and the picker-UI
//! subprocess lifecycle.
//!
//! Cloning is cheap — internally an `Arc`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::picker_ipc::PickerIpcHandle;
use crate::picker_lifecycle::PickerLifecycle;

/// Cheaply-cloneable handle to the daemon's runtime state.
#[derive(Clone)]
pub struct DaemonState {
    inner: Arc<Inner>,
    pub picker_ipc: PickerIpcHandle,
    pub picker_lifecycle: PickerLifecycle,
}

struct Inner {
    /// Number of in-flight portal requests. Incremented when a method
    /// handler enters and decremented when it returns. Idle-timeout
    /// must NOT fire while this is > 0 — see edge case E12.
    open_requests: AtomicUsize,
}

impl DaemonState {
    pub fn new(picker_ipc: PickerIpcHandle, picker_lifecycle: PickerLifecycle) -> Self {
        Self {
            inner: Arc::new(Inner {
                open_requests: AtomicUsize::new(0),
            }),
            picker_ipc,
            picker_lifecycle,
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
