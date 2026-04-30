//! Picker-UI side of the daemon ↔ picker-ui IPC.
//!
//! Connects to the daemon's Unix socket at
//! `$XDG_RUNTIME_DIR/lunaris/portal-picker.sock`, reads framed
//! `PickerRequest` messages, and sends back `PickerResponse`. The
//! reader spawns one task; the writer is wrapped in a `Mutex` so the
//! `picker_respond` Tauri command can grab it.
//!
//! On daemon disconnect the picker UI exits with code 0. The daemon
//! will spawn a fresh picker process on the next FileChooser call
//! (FA4); a stale connection that survives a daemon restart would
//! correlate handles against the wrong daemon.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use xdg_portal_lunaris_protocol::codec::{decode_frame, encode_frame};
use xdg_portal_lunaris_protocol::{PickerRequest, PickerResponse};

/// Socket the daemon binds. Must match `daemon/src/picker_ipc.rs`.
fn socket_path() -> Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .context("XDG_RUNTIME_DIR is not set")?;
    let mut p = PathBuf::from(runtime);
    p.push("lunaris");
    p.push("portal-picker.sock");
    Ok(p)
}

/// Cheap-clone handle the `picker_respond` Tauri command uses to
/// write replies back to the daemon.
#[derive(Clone)]
pub struct DaemonClient {
    writer: Arc<Mutex<Option<tokio::net::unix::OwnedWriteHalf>>>,
}

impl DaemonClient {
    pub fn new() -> Self {
        Self {
            writer: Arc::new(Mutex::new(None)),
        }
    }

    /// Send a response frame to the daemon. Errors out if the
    /// connection is gone (which means the daemon has crashed or is
    /// shutting down — the picker UI should follow it).
    pub async fn send(&self, response: &PickerResponse) -> Result<()> {
        let frame = encode_frame(response).context("encode picker response")?;
        let mut guard = self.writer.lock().await;
        let writer = guard
            .as_mut()
            .context("daemon connection is not open")?;
        writer
            .write_all(&frame)
            .await
            .context("write picker response")?;
        Ok(())
    }
}

impl Default for DaemonClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Open the socket, install the writer half on `client`, and spawn a
/// background task that reads incoming requests and emits them as
/// `picker:request` Tauri events.
///
/// Returns once the connection is established. The reader runs until
/// the daemon disconnects; on disconnect the function calls
/// `app.exit(0)` so the next request gets a fresh process (FA4).
pub async fn connect(app: AppHandle, client: DaemonClient) -> Result<()> {
    let path = socket_path()?;
    let stream = UnixStream::connect(&path)
        .await
        .with_context(|| format!("connect {}", path.display()))?;
    let (read_half, write_half) = stream.into_split();
    {
        let mut writer = client.writer.lock().await;
        *writer = Some(write_half);
    }
    tracing::info!(socket = %path.display(), "connected to daemon");

    let app_clone = app.clone();
    tokio::spawn(async move {
        if let Err(e) = read_loop(read_half, app_clone.clone()).await {
            tracing::warn!("daemon read loop ended: {e}");
        }
        // Exit so the daemon respawns a fresh picker on next request.
        app_clone.exit(0);
    });

    Ok(())
}

async fn read_loop(mut reader: tokio::net::unix::OwnedReadHalf, app: AppHandle) -> Result<()> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        let n = reader.read(&mut chunk).await.context("read from daemon")?;
        if n == 0 {
            anyhow::bail!("daemon closed the connection");
        }
        buf.extend_from_slice(&chunk[..n]);
        loop {
            match decode_frame::<PickerRequest>(&buf) {
                Ok((consumed, request)) => {
                    buf.drain(..consumed);
                    handle_request(&app, request).await;
                }
                Err(xdg_portal_lunaris_protocol::codec::CodecError::Incomplete { .. }) => break,
                Err(e) => {
                    anyhow::bail!("daemon sent malformed frame: {e}");
                }
            }
        }
    }
}

async fn handle_request(app: &AppHandle, request: PickerRequest) {
    match &request {
        PickerRequest::Cancel { handle } => {
            // Daemon-initiated cancel: hide the window and forward as
            // an event so the UI can clear itself. No response is
            // expected.
            if let Some(w) = app.get_webview_window("picker") {
                let _ = w.hide();
            }
            tracing::info!(handle, "received Cancel from daemon");
            let _ = app.emit("picker:cancel", handle.clone());
        }
        _ => {
            // Show the window and emit the request to the frontend.
            if let Some(w) = app.get_webview_window("picker") {
                if let Err(e) = w.show() {
                    tracing::warn!("show picker window: {e}");
                }
                if let Err(e) = w.set_focus() {
                    tracing::warn!("focus picker window: {e}");
                }
            }
            if let Err(e) = app.emit("picker:request", request) {
                tracing::warn!("emit picker:request event: {e}");
            }
        }
    }
}
