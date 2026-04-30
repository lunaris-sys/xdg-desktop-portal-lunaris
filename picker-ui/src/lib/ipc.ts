/// Bridge between the picker UI and the daemon's IPC server.
///
/// The actual Unix socket I/O happens in Rust (the picker-ui Tauri
/// backend); this file invokes those Tauri commands. The frontend
/// listens for `picker:request` events and dispatches responses
/// through `picker:respond`.

import { invoke } from "@tauri-apps/api/core";
import { listen, type Event } from "@tauri-apps/api/event";

import type { PickerRequest, PickerResponse } from "$lib/types/protocol";
import { setBusy, setRequest } from "$lib/stores/pickState.svelte";

let unlistenIncoming: (() => void) | null = null;

/// Subscribe to incoming `picker:request` events from the daemon
/// (proxied through the Tauri Rust backend).
///
/// Idempotent: calling it twice does not stack listeners; the second
/// call replaces the first.
export async function initPickerBridge() {
  if (unlistenIncoming) {
    unlistenIncoming();
  }
  unlistenIncoming = await listen<PickerRequest>("picker:request", (event: Event<PickerRequest>) => {
    setRequest(event.payload);
  });
}

/// Send a response back to the daemon and clear the local state.
///
/// `setBusy(true)` blocks duplicate clicks; the request is cleared
/// only after the Rust side confirms the response was framed and
/// written to the socket.
export async function respond(response: PickerResponse) {
  setBusy(true);
  try {
    await invoke("picker_respond", { response });
  } finally {
    setRequest(null);
  }
}
