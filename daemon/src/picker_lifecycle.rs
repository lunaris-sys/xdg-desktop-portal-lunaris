//! Picker-UI subprocess lifecycle (FA3, FA5, edge cases E11, E25).
//!
//! The daemon spawns the picker-ui process when its own D-Bus name is
//! bound (pre-warm per FA5) and re-spawns on demand if the process
//! exits or fails to connect to the IPC socket. The picker-ui itself
//! handles hidden-window-reuse across pick requests inside one
//! lifetime (FA4); when it exits — voluntarily after its own idle
//! timer or on memory-leak rotation (E25) or unexpectedly (E11) — we
//! spawn a fresh one.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Path to the picker-ui binary. In production this is
/// `/usr/lib/lunaris/libexec/xdg-desktop-portal-lunaris-picker`. In
/// development the dev-script overrides it via the
/// `LUNARIS_PORTAL_PICKER_BIN` environment variable so the daemon
/// finds the picker built at `picker-ui/src-tauri/target/debug/...`
/// without an install step.
const PROD_PICKER_PATH: &str = "/usr/lib/lunaris/libexec/xdg-desktop-portal-lunaris-picker";
const ENV_OVERRIDE: &str = "LUNARIS_PORTAL_PICKER_BIN";

/// Resolve the picker-ui binary path.
fn picker_binary_path() -> PathBuf {
    if let Some(custom) = std::env::var_os(ENV_OVERRIDE) {
        return PathBuf::from(custom);
    }
    PathBuf::from(PROD_PICKER_PATH)
}

/// Cheaply-cloneable handle to the picker subprocess slot.
#[derive(Clone)]
pub struct PickerLifecycle {
    inner: Arc<Mutex<Option<Child>>>,
}

impl PickerLifecycle {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    /// Spawn a picker-ui subprocess if none is currently running.
    /// Returns immediately; the picker connects back to the daemon's
    /// IPC socket asynchronously and the FileChooser handler waits
    /// for that connection.
    ///
    /// Idempotent: if a process is already running and has not
    /// exited, this is a no-op.
    pub async fn ensure_running(&self) -> Result<()> {
        let mut slot = self.inner.lock().await;

        // If we have a child handle, check whether it is still alive.
        // `try_wait` returns Ok(Some(_)) once the child exits; we
        // clear the slot and fall through to a fresh spawn.
        if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    tracing::info!(
                        ?status,
                        "previous picker-ui exited, spawning fresh process"
                    );
                    *slot = None;
                }
                Ok(None) => {
                    // Still running — nothing to do.
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!("try_wait on picker-ui child failed: {e}");
                    // Be conservative: drop the handle and respawn.
                    *slot = None;
                }
            }
        }

        let bin = picker_binary_path();
        tracing::info!(binary = %bin.display(), "spawning picker-ui");
        let child = Command::new(&bin)
            // Inherit stderr so picker-ui logs land in the same
            // journal stream as the daemon. stdin closed.
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawn picker-ui from {}", bin.display()))?;

        *slot = Some(child);
        Ok(())
    }

    /// True if a picker-ui process is currently tracked. Reserved
    /// for the idle-timeout interaction in F2.5; left here so the
    /// matching test in this module continues to compile.
    #[allow(dead_code)]
    pub async fn is_running(&self) -> bool {
        let mut slot = self.inner.lock().await;
        match slot.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Terminate the picker-ui if it is running. Best-effort; failures
    /// are logged but not propagated since the daemon is shutting
    /// down anyway when this is called.
    pub async fn shutdown(&self) {
        let mut slot = self.inner.lock().await;
        if let Some(mut child) = slot.take() {
            if let Err(e) = child.kill().await {
                tracing::warn!("kill picker-ui on shutdown: {e}");
            }
        }
    }
}

impl Default for PickerLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Override env var resolves to the requested path even if the
    /// production path exists.
    #[test]
    fn env_override_takes_precedence() {
        let prev = std::env::var_os(ENV_OVERRIDE);
        std::env::set_var(ENV_OVERRIDE, "/tmp/custom-picker");
        let path = picker_binary_path();
        assert_eq!(path, PathBuf::from("/tmp/custom-picker"));
        match prev {
            Some(v) => std::env::set_var(ENV_OVERRIDE, v),
            None => std::env::remove_var(ENV_OVERRIDE),
        }
    }

    /// Without the env var, falls back to the production path.
    #[test]
    fn falls_back_to_prod_path() {
        let prev = std::env::var_os(ENV_OVERRIDE);
        std::env::remove_var(ENV_OVERRIDE);
        let path = picker_binary_path();
        assert_eq!(path, PathBuf::from(PROD_PICKER_PATH));
        if let Some(v) = prev {
            std::env::set_var(ENV_OVERRIDE, v);
        }
    }

    /// `is_running` is `false` for a fresh handle.
    #[tokio::test]
    async fn fresh_handle_not_running() {
        let h = PickerLifecycle::new();
        assert!(!h.is_running().await);
    }
}
