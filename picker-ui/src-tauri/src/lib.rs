//! Picker-UI Tauri backend.
//!
//! Connects to the daemon's IPC socket, drives request/response
//! round-trips, and shows/hides the WebviewWindow accordingly.

mod fs_commands;
mod ipc_client;

use std::sync::Arc;

use tauri::{AppHandle, Manager, State};
use xdg_portal_lunaris_protocol::PickerResponse;

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
            fs_commands::list_directory,
            fs_commands::resolve_start_dir,
            fs_commands::parent_dir,
            fs_commands::file_exists,
        ])
        .run(tauri::generate_context!())
        .expect("error while running picker-ui");
}
