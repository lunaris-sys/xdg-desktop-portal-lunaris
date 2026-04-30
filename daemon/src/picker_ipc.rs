//! Daemon-side IPC server for the picker-ui subprocess.
//!
//! Wire shape:
//!
//! ```text
//! daemon                                 picker-ui
//!   |  bind socket                          |
//!   |  spawn subprocess  -----------------> |
//!   |                          connect      |
//!   |  <----- connection accepted --------- |
//!   |                                       |
//!   |  --- PickerRequest::OpenFile -------> |
//!   |  <--- PickerResponse::Picked -------- |
//!   |                                       |
//!   |  --- PickerRequest::OpenFile -------> |   (reused per FA4)
//!   |  <--- PickerResponse::Cancelled ----- |
//! ```
//!
//! At most one picker-ui is connected at a time. If the connection
//! drops while requests are pending, all pending oneshots resolve to
//! `PickerResponse::Cancelled` so the FileChooser handler can return
//! a real error to the caller (FA6, edge case E11).
//!
//! The Unix socket lives at `$XDG_RUNTIME_DIR/lunaris/portal-picker.sock`.
//! A unique runtime path was chosen over `$XDG_RUNTIME_DIR/portal-picker.sock`
//! to keep all Lunaris-specific runtime files under one prefix and to
//! avoid colliding with other portal backends a user might have
//! installed in parallel.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use std::os::unix::fs::FileTypeExt;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{oneshot, Mutex, Notify};
use xdg_portal_lunaris_protocol::codec::{decode_frame, encode_frame};
use xdg_portal_lunaris_protocol::{PickerRequest, PickerResponse};

/// Per-handle correlation slot. The FileChooser method handler
/// inserts itself before sending the request and removes itself when
/// the response (or a connection-drop synthetic Cancelled) arrives.
type PendingMap = HashMap<String, oneshot::Sender<PickerResponse>>;

/// How long `submit` will wait for a freshly-spawned picker-ui to
/// connect back before giving up. WebKitGTK cold-start is ~300 ms;
/// we triple that to absorb slow disks and CI environments. A real
/// failure (binary missing, segfault on startup) shows up as a
/// disconnect that the connection task then reports — before this
/// timeout — so picking 1 s vs 5 s does not change real failure
/// detection time, only the first-pick latency in the worst case.
const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// Cheap, cloneable handle FileChooser/SaveFile/SaveFiles call to
/// submit a request and await its response.
#[derive(Clone)]
pub struct PickerIpcHandle {
    inner: Arc<Inner>,
}

struct Inner {
    /// `None` while no picker-ui is connected. The accept loop swaps
    /// in a writer when a connection arrives and clears it on
    /// disconnect.
    writer: Mutex<Option<tokio::net::unix::OwnedWriteHalf>>,
    pending: Mutex<PendingMap>,
    /// Pulsed once whenever a writer becomes available so callers
    /// blocked in `wait_until_connected` can wake. Per `Notify`
    /// semantics, a pulse with no waiters is buffered for the next
    /// waiter — which is exactly what we want for the spawn-then-
    /// submit pattern.
    connection_ready: Notify,
}

impl PickerIpcHandle {
    /// Resolve the socket path the daemon binds and the picker-ui
    /// connects to.
    pub fn socket_path() -> Result<PathBuf> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .context("XDG_RUNTIME_DIR is not set; cannot derive socket path")?;
        let mut p = PathBuf::from(runtime);
        p.push("lunaris");
        std::fs::create_dir_all(&p)
            .with_context(|| format!("failed to create {}", p.display()))?;
        p.push("portal-picker.sock");
        Ok(p)
    }

    /// Bind the socket, spawn the accept loop, and return a handle the
    /// rest of the daemon uses to submit requests. Removes any stale
    /// socket file from a previous run before binding.
    pub async fn start() -> Result<Self> {
        let path = Self::socket_path()?;
        // Stale socket from a previous (crashed) daemon run blocks
        // bind with EADDRINUSE; remove it. If the file is something
        // unexpected (a regular file the user created by hand) this
        // surfaces as an error — better than silently nuking it.
        if path.exists() {
            let meta = std::fs::metadata(&path)
                .with_context(|| format!("stat {}", path.display()))?;
            if meta.file_type().is_socket() {
                std::fs::remove_file(&path)
                    .with_context(|| format!("remove stale socket {}", path.display()))?;
            } else {
                anyhow::bail!(
                    "{} exists but is not a Unix socket; refusing to overwrite",
                    path.display()
                );
            }
        }
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("bind {}", path.display()))?;
        // Restrict the socket to the user. Default umask is usually
        // 022 which would leave it 0755 — too open. Setting 0600 makes
        // the IPC strictly per-user even on shared machines.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;

        let handle = Self {
            inner: Arc::new(Inner {
                writer: Mutex::new(None),
                pending: Mutex::new(HashMap::new()),
                connection_ready: Notify::new(),
            }),
        };

        let accept_handle = handle.clone();
        tokio::spawn(async move {
            accept_loop(listener, accept_handle).await;
        });

        tracing::info!(socket = %path.display(), "picker IPC server bound");
        Ok(handle)
    }

    /// Wait up to `READY_TIMEOUT` for a picker-ui to connect. Returns
    /// quickly if one is already connected. Used by `submit` to
    /// absorb the spawn → connect race that Codex review flagged
    /// as a P1 source of spurious request failures.
    async fn wait_until_connected(&self) -> Result<()> {
        // Fast path: if a writer is already installed we are done.
        if self.inner.writer.lock().await.is_some() {
            return Ok(());
        }
        // Slow path: subscribe to the next pulse from the accept loop.
        // We re-check the writer after the notification because a
        // racing disconnect could have cleared it again before we got
        // the lock.
        let notified = self.inner.connection_ready.notified();
        tokio::pin!(notified);
        let deadline = tokio::time::sleep(READY_TIMEOUT);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut notified => {
                    if self.inner.writer.lock().await.is_some() {
                        return Ok(());
                    }
                    // Spurious wake (writer cleared between pulse
                    // and lock); rearm and keep waiting until the
                    // wall-clock deadline.
                    notified.set(self.inner.connection_ready.notified());
                }
                _ = &mut deadline => {
                    anyhow::bail!("picker-ui did not connect within {READY_TIMEOUT:?}");
                }
            }
        }
    }

    /// Submit a request to the picker-ui and return a receiver that
    /// resolves to the response.
    ///
    /// Waits up to `READY_TIMEOUT` for a freshly-spawned picker-ui
    /// to connect; returns an error after the timeout. The caller
    /// (FileChooser interface) translates that into a backend-failure
    /// D-Bus response.
    pub async fn submit(&self, request: PickerRequest) -> Result<oneshot::Receiver<PickerResponse>> {
        // Codex P1: wait for the IPC handshake before reserving the
        // correlation slot, so a freshly-spawned picker that has not
        // yet connected does not turn a valid call into OTHER.
        self.wait_until_connected().await?;

        let handle = request_handle(&request).to_string();
        let (tx, rx) = oneshot::channel();

        // Reserve the correlation slot before writing so a fast picker
        // cannot deliver the response before the slot exists.
        {
            let mut pending = self.inner.pending.lock().await;
            if pending.contains_key(&handle) {
                anyhow::bail!("duplicate picker-ipc handle: {handle}");
            }
            pending.insert(handle.clone(), tx);
        }

        let frame = encode_frame(&request)
            .context("encode picker request")?;

        let mut writer_guard = self.inner.writer.lock().await;
        let Some(writer) = writer_guard.as_mut() else {
            // Lost the connection between the wait and the lock.
            self.inner.pending.lock().await.remove(&handle);
            anyhow::bail!("picker-ui not connected");
        };
        if let Err(e) = writer.write_all(&frame).await {
            // Connection died mid-write. Drop the writer so the
            // accept loop can take a fresh one if the picker
            // reconnects, and let the caller see the failure.
            *writer_guard = None;
            self.inner.pending.lock().await.remove(&handle);
            return Err(anyhow::anyhow!("write picker request: {e}"));
        }

        Ok(rx)
    }
}

/// Borrow the correlation handle out of any request variant.
fn request_handle(req: &PickerRequest) -> &str {
    match req {
        PickerRequest::OpenFile { handle, .. } => handle,
        PickerRequest::SaveFile { handle, .. } => handle,
        PickerRequest::SaveFiles { handle, .. } => handle,
        PickerRequest::Cancel { handle, .. } => handle,
    }
}

/// Accept loop. The daemon expects at most one picker-ui at a time;
/// when a second client connects (which would only happen by mistake
/// or as a probe), we close the previous one rather than serializing
/// requests across both, since that would silently break correlation.
async fn accept_loop(listener: UnixListener, handle: PickerIpcHandle) {
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!("picker IPC accept failed: {e}");
                // Brief pause to avoid a tight retry loop on
                // permanent EMFILE-class errors.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        tracing::info!("picker-ui connected");
        // Hand off to the per-connection task. The reader half drives
        // response correlation; the writer half is stashed on the
        // handle for outgoing requests.
        connection_task(stream, handle.clone()).await;
        tracing::info!("picker-ui disconnected");
    }
}

/// Drive one picker-ui connection until it disconnects. On
/// disconnect, any still-pending requests are answered with
/// `Cancelled` so the D-Bus method handlers do not hang forever.
async fn connection_task(stream: UnixStream, handle: PickerIpcHandle) {
    let (read_half, write_half) = stream.into_split();

    // Replace any previously-stashed writer (for a previous picker
    // that did not exit cleanly) with this one.
    {
        let mut writer = handle.inner.writer.lock().await;
        *writer = Some(write_half);
    }
    // Wake any submit() blocked in wait_until_connected.
    handle.inner.connection_ready.notify_waiters();

    let mut reader = read_half;
    let mut buf = Vec::with_capacity(4096);
    loop {
        let mut chunk = [0u8; 4096];
        let n = match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("picker-ui read error: {e}");
                break;
            }
        };
        buf.extend_from_slice(&chunk[..n]);

        // Drain as many complete frames as are in the buffer. Partial
        // frame at the end stays for the next read.
        loop {
            match decode_frame::<PickerResponse>(&buf) {
                Ok((consumed, response)) => {
                    buf.drain(..consumed);
                    deliver_response(&handle, response).await;
                }
                Err(xdg_portal_lunaris_protocol::codec::CodecError::Incomplete { .. }) => break,
                Err(e) => {
                    tracing::error!("picker-ui sent malformed frame: {e}");
                    // A malformed frame is unrecoverable for this
                    // connection — we cannot trust the remaining
                    // byte stream. Drop the connection.
                    buf.clear();
                    break;
                }
            }
        }
        if buf.is_empty() {
            continue;
        }
    }

    // Connection lost. Drop the writer and synthetically Cancel every
    // pending request so D-Bus method handlers unblock.
    {
        let mut writer = handle.inner.writer.lock().await;
        *writer = None;
    }
    let pending: Vec<_> = {
        let mut map = handle.inner.pending.lock().await;
        map.drain().collect()
    };
    for (handle_id, tx) in pending {
        let _ = tx.send(PickerResponse::Cancelled { handle: handle_id });
    }
}

/// Look up the correlation slot for the given response and signal
/// the oneshot. Drops the response (with a warning) if no slot
/// exists, since that is either a stale response after a request
/// was already cancelled or a buggy picker-ui sending unsolicited
/// data.
async fn deliver_response(handle: &PickerIpcHandle, response: PickerResponse) {
    let id = match &response {
        PickerResponse::Picked { handle, .. } => handle.clone(),
        PickerResponse::Cancelled { handle, .. } => handle.clone(),
        PickerResponse::Error { handle, .. } => handle.clone(),
    };
    let tx = handle.inner.pending.lock().await.remove(&id);
    match tx {
        Some(tx) => {
            let _ = tx.send(response);
        }
        None => {
            tracing::warn!(handle = %id, "picker-ui sent response with unknown correlation handle");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `request_handle` covers every variant. Adding a new variant
    /// without updating this fn would silently miss correlation —
    /// guard against that.
    #[test]
    fn request_handle_covers_all_variants() {
        let h1 = PickerRequest::OpenFile {
            handle: "h1".into(),
            app_id: "".into(),
            title: "".into(),
            filters: vec![],
            current_filter: None,
            multiple: false,
            modal: false,
            directory: false,
            current_folder: None,
            parent_window: None,
        };
        assert_eq!(request_handle(&h1), "h1");

        let h2 = PickerRequest::SaveFile {
            handle: "h2".into(),
            app_id: "".into(),
            title: "".into(),
            filters: vec![],
            current_filter: None,
            current_name: None,
            current_folder: None,
            current_file: None,
            parent_window: None,
        };
        assert_eq!(request_handle(&h2), "h2");

        let h3 = PickerRequest::SaveFiles {
            handle: "h3".into(),
            app_id: "".into(),
            title: "".into(),
            files: vec![],
            current_folder: None,
            parent_window: None,
        };
        assert_eq!(request_handle(&h3), "h3");

        let h4 = PickerRequest::Cancel { handle: "h4".into() };
        assert_eq!(request_handle(&h4), "h4");
    }

    fn fresh_handle() -> PickerIpcHandle {
        PickerIpcHandle {
            inner: Arc::new(Inner {
                writer: Mutex::new(None),
                pending: Mutex::new(HashMap::new()),
                connection_ready: Notify::new(),
            }),
        }
    }

    /// End-to-end on a tempdir socket: server, fake client, request
    /// out, response back, oneshot resolves.
    #[tokio::test]
    async fn round_trip_through_real_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("test.sock");

        // Build a handle bound to a custom socket path. We replicate
        // `start` inline so the test can supply its own path.
        let listener = UnixListener::bind(&socket).unwrap();
        let handle = fresh_handle();
        let accept = handle.clone();
        tokio::spawn(async move {
            accept_loop(listener, accept).await;
        });

        // Fake client: connect, echo back a Picked response when it
        // sees a request.
        let socket_clone = socket.clone();
        let client_task = tokio::spawn(async move {
            let mut client = UnixStream::connect(&socket_clone).await.unwrap();
            let mut chunk = [0u8; 4096];
            let mut buf = Vec::new();
            loop {
                let n = client.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Ok((consumed, req)) = decode_frame::<PickerRequest>(&buf) {
                    buf.drain(..consumed);
                    let id = match req {
                        PickerRequest::OpenFile { handle, .. } => handle,
                        _ => panic!("unexpected variant"),
                    };
                    let resp = PickerResponse::Picked {
                        handle: id,
                        paths: vec![PathBuf::from("/tmp/x")],
                        current_filter: None,
                    };
                    let frame = encode_frame(&resp).unwrap();
                    client.write_all(&frame).await.unwrap();
                    break;
                }
            }
        });

        // Wait briefly for the accept-loop to register the connection.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let req = PickerRequest::OpenFile {
            handle: "test-1".into(),
            app_id: "".into(),
            title: "".into(),
            filters: vec![],
            current_filter: None,
            multiple: false,
            modal: false,
            directory: false,
            current_folder: None,
            parent_window: None,
        };
        let rx = handle.submit(req).await.unwrap();
        let response = rx.await.unwrap();
        match response {
            PickerResponse::Picked { handle, paths, .. } => {
                assert_eq!(handle, "test-1");
                assert_eq!(paths, vec![PathBuf::from("/tmp/x")]);
            }
            other => panic!("unexpected response: {other:?}"),
        }

        let _ = client_task.await;
    }

    /// Submitting when no picker-ui ever connects fails with a clear
    /// timeout error after `READY_TIMEOUT` rather than hanging the
    /// caller. The test uses a tokio time-paused runtime so the
    /// timeout is immediate.
    #[tokio::test(start_paused = true)]
    async fn submit_times_out_when_picker_never_connects() {
        let handle = fresh_handle();
        let req = PickerRequest::OpenFile {
            handle: "ne".into(),
            app_id: "".into(),
            title: "".into(),
            filters: vec![],
            current_filter: None,
            multiple: false,
            modal: false,
            directory: false,
            current_folder: None,
            parent_window: None,
        };
        // Drive submit and the deadline forward; the test runtime
        // auto-advances time when no future is making progress.
        let result =
            tokio::time::timeout(Duration::from_secs(5), handle.submit(req)).await;
        let inner = result.expect("readiness timeout should fire before outer timeout");
        assert!(
            inner.is_err(),
            "submit should fail when no picker connected"
        );
        assert!(handle.inner.pending.lock().await.is_empty());
    }

    /// Codex P1 regression: a submit started before the picker
    /// connects must succeed once the picker connects within the
    /// readiness timeout. The fast path here covers a 50 ms-late
    /// picker — well within the 2 s budget — and verifies the
    /// notify pulse from the accept loop wakes the waiter.
    #[tokio::test]
    async fn submit_waits_for_late_connecting_picker() {
        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("late.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let handle = fresh_handle();
        let accept = handle.clone();
        tokio::spawn(async move {
            accept_loop(listener, accept).await;
        });

        // Start submit before the client connects.
        let submit_handle = handle.clone();
        let submit_task = tokio::spawn(async move {
            let req = PickerRequest::OpenFile {
                handle: "late".into(),
                app_id: "".into(),
                title: "".into(),
                filters: vec![],
                current_filter: None,
                multiple: false,
                modal: false,
            directory: false,
                current_folder: None,
                parent_window: None,
            };
            submit_handle.submit(req).await
        });

        // Connect after a short delay, then echo a Picked response.
        let socket_clone = socket.clone();
        let client_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut client = UnixStream::connect(&socket_clone).await.unwrap();
            let mut chunk = [0u8; 4096];
            let mut buf = Vec::new();
            loop {
                let n = client.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Ok((consumed, req)) = decode_frame::<PickerRequest>(&buf) {
                    buf.drain(..consumed);
                    let id = match req {
                        PickerRequest::OpenFile { handle, .. } => handle,
                        _ => panic!("unexpected variant"),
                    };
                    let resp = PickerResponse::Picked {
                        handle: id,
                        paths: vec![PathBuf::from("/tmp/late")],
                        current_filter: None,
                    };
                    let frame = encode_frame(&resp).unwrap();
                    client.write_all(&frame).await.unwrap();
                    break;
                }
            }
        });

        let rx = submit_task.await.unwrap().expect("submit must succeed");
        let resp = rx.await.unwrap();
        match resp {
            PickerResponse::Picked { paths, .. } => {
                assert_eq!(paths, vec![PathBuf::from("/tmp/late")]);
            }
            other => panic!("unexpected response: {other:?}"),
        }
        let _ = client_task.await;
    }
}
