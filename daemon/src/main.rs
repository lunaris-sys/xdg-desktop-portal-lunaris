//! xdg-desktop-portal-lunaris daemon entry point.
//!
//! Registers `org.freedesktop.impl.portal.desktop.lunaris` on the session
//! bus and serves the `org.freedesktop.impl.portal.FileChooser` and
//! `org.freedesktop.impl.portal.OpenURI` interfaces at
//! `/org/freedesktop/portal/desktop`.
//!
//! Architecture decisions and edge-case handling live in
//! `docs/architecture/xdg-desktop-portal-lunaris.md`. This file is the
//! plumbing: D-Bus bind, IPC server, picker pre-warm, idle loop.

mod document_portal;
mod interfaces;
mod picker_ipc;
mod picker_lifecycle;
mod request;
mod sandbox;
mod state;

use std::time::Duration;

use anyhow::Context;
use zbus::connection;

use crate::interfaces::{file_chooser::FileChooser, open_uri::OpenUri};
use crate::picker_ipc::PickerIpcHandle;
use crate::picker_lifecycle::PickerLifecycle;
use crate::state::DaemonState;

/// Well-known D-Bus name we register on the session bus. The
/// `xdg-desktop-portal` frontend dispatches to whichever backend is
/// declared in `lunaris.portal` for `UseIn=lunaris;`.
const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.lunaris";

/// Object path the FileChooser and OpenURI interfaces are served at.
/// The frontend always queries this exact path.
const OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";

/// Default idle window before the daemon exits when no requests are
/// open. Override via `LUNARIS_PORTAL_IDLE_TIMEOUT_SECS` (handy for
/// dev sessions where 60 s would kick in mid-debug). See FA10 and
/// edge case E12 for the open-request-counter interaction.
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 60;

fn idle_timeout() -> Duration {
    let secs = std::env::var("LUNARIS_PORTAL_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("starting xdg-desktop-portal-lunaris");

    // Bring up the picker-IPC socket FIRST so the picker subprocess
    // we spawn next finds a server to connect to. Order matters: a
    // race where the picker connects before the listener exists
    // would surface as a "connection refused" inside the picker and
    // a needless respawn cycle.
    let picker_ipc = PickerIpcHandle::start()
        .await
        .context("start picker IPC")?;
    let picker_lifecycle = PickerLifecycle::new();

    // Pre-warm: spawn the picker-ui process now so the first
    // FileChooser call only pays the cost of a `.show()`, not the
    // ~300 ms WebKitGTK init (FA5, edge case E24). If the picker
    // binary is missing (devbox without a build), the spawn fails
    // and the FileChooser handler will surface a clear error to
    // callers when one finally arrives.
    if let Err(e) = picker_lifecycle.ensure_running().await {
        tracing::warn!(
            "picker-ui pre-warm failed: {e}. FileChooser calls will fail until the binary is available."
        );
    }

    let state = DaemonState::new(picker_ipc, picker_lifecycle);

    let _conn = connection::Builder::session()
        .context("failed to connect to session bus")?
        .name(BUS_NAME)
        .with_context(|| format!("failed to claim D-Bus name {BUS_NAME}"))?
        .serve_at(OBJECT_PATH, FileChooser::new(state.clone()))
        .with_context(|| format!("failed to serve FileChooser at {OBJECT_PATH}"))?
        .serve_at(OBJECT_PATH, OpenUri::new(state.clone()))
        .with_context(|| format!("failed to serve OpenURI at {OBJECT_PATH}"))?
        .build()
        .await
        .context("failed to build D-Bus connection")?;

    tracing::info!(
        bus_name = BUS_NAME,
        path = OBJECT_PATH,
        "D-Bus interfaces ready"
    );

    // Idle-timeout loop: tick once per second and exit when no requests
    // have been open for IDLE_TIMEOUT. The state's request counter
    // protects against exit-while-pick-open (E12).
    let exit_signal = tokio::signal::ctrl_c();
    tokio::pin!(exit_signal);

    let timeout = idle_timeout();
    tracing::info!(idle_timeout_secs = timeout.as_secs(), "idle loop armed");
    let mut last_active = std::time::Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = &mut exit_signal => {
                tracing::info!("received Ctrl-C, shutting down");
                break;
            }
            _ = tick.tick() => {
                if state.has_open_requests() {
                    last_active = std::time::Instant::now();
                    continue;
                }
                if last_active.elapsed() >= timeout {
                    tracing::info!(
                        idle_for_secs = last_active.elapsed().as_secs(),
                        "idle timeout reached, exiting"
                    );
                    break;
                }
            }
        }
    }

    state.picker_lifecycle.shutdown().await;
    Ok(())
}
