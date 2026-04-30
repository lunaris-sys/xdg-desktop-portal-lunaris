//! Picker-UI Tauri backend.
//!
//! Connects to the daemon's IPC socket, drives request/response
//! round-trips, and shows/hides the WebviewWindow accordingly.

mod fs_commands;
mod ipc_client;
mod theme;

use std::sync::Arc;

use tauri::{AppHandle, Manager, State};
use xdg_portal_lunaris_protocol::{PickerRequest, PickerResponse};

use ipc_client::{connect, DaemonClient};

/// Shared state surface to Tauri commands.
struct PickerState {
    client: DaemonClient,
}

/// Tauri command: send a response back to the daemon and hide the
/// window. Frontend invokes this from `respond()` in `lib/ipc.ts`.
#[tauri::command]
async fn picker_respond(
    response: PickerResponse,
    app: AppHandle,
    state: State<'_, Arc<PickerState>>,
) -> Result<(), String> {
    state
        .client
        .send(&response)
        .await
        .map_err(|e| format!("send response: {e}"))?;
    if let Some(w) = app.get_webview_window("picker") {
        let _ = w.hide();
    }
    Ok(())
}

/// Tauri command: return the user's resolved Lunaris theme so the
/// frontend can inject matching CSS variables on mount. Falls back
/// to dark defaults on any failure to read appearance.toml.
#[tauri::command]
fn get_theme() -> theme::Theme {
    theme::load_theme()
}

/// Tauri command: route a frontend log line into the daemon's
/// log stream. The picker UI cannot reach DevTools, so frontend
/// diagnostics need a back channel into the journal.
#[tauri::command]
fn frontend_log(level: String, msg: String) {
    match level.as_str() {
        "warn" => tracing::warn!("[picker-ui-frontend] {msg}"),
        "error" => tracing::error!("[picker-ui-frontend] {msg}"),
        _ => tracing::info!("[picker-ui-frontend] {msg}"),
    }
}

/// Tauri command: atomically take the pending picker request (if
/// any). The Svelte frontend invokes this on mount so it can
/// recover requests whose `picker:request` Tauri event fired before
/// the listener was registered.
#[tauri::command]
async fn picker_take_pending(
    state: State<'_, Arc<PickerState>>,
) -> Result<Option<PickerRequest>, String> {
    let pending = state.client.take_pending().await;
    tracing::info!(
        has_pending = pending.is_some(),
        "picker_take_pending invoked from frontend"
    );
    Ok(pending)
}

/// Tauri entry point invoked from `main.rs`.
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let client = DaemonClient::new();
    let state = Arc::new(PickerState {
        client: client.clone(),
    });

    tauri::Builder::default()
        .manage(state)
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let client = client.clone();
            // Connect to the daemon in the background; if it is not
            // running yet, abort with a non-zero exit so systemd /
            // shell scripts see the failure rather than the picker
            // sitting idle forever waiting for a request.
            tauri::async_runtime::spawn(async move {
                if let Err(e) = connect(app_handle.clone(), client).await {
                    tracing::error!("failed to connect to daemon: {e}");
                    app_handle.exit(1);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            picker_respond,
            picker_take_pending,
            frontend_log,
            get_theme,
            fs_commands::list_directory,
            fs_commands::resolve_start_dir,
            fs_commands::parent_dir,
            fs_commands::file_exists,
        ])
        .run(tauri::generate_context!())
        .expect("error while running picker-ui");
}
