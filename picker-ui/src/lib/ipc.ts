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

async function backendLog(msg: string, level: "info" | "warn" | "error" = "info") {
  try {
    await invoke("frontend_log", { level, msg });
  } catch {
    // ignore
  }
}

interface Theme {
  bgApp: string;
  bgCard: string;
  fgApp: string;
  fgMuted: string;
  border: string;
  accent: string;
  danger: string;
}

/// Load the user's Lunaris theme and inject the colors as CSS
/// custom properties on `:root`. Tailwind v4's `@theme inline`
/// resolves at build-time so runtime overrides need to override
/// the same custom-property names from JS.
export async function applyTheme() {
  try {
    const theme = await invoke<Theme>("get_theme");
    const root = document.documentElement;
    root.style.setProperty("--color-bg-app", theme.bgApp);
    root.style.setProperty("--color-bg-card", theme.bgCard);
    root.style.setProperty("--color-fg-app", theme.fgApp);
    root.style.setProperty("--color-fg-muted", theme.fgMuted);
    root.style.setProperty("--color-border", theme.border);
    root.style.setProperty("--color-accent", theme.accent);
    root.style.setProperty("--color-danger", theme.danger);
  } catch (e) {
    backendLog(`applyTheme failed: ${e}`, "warn");
  }
}

/// Subscribe to incoming `picker:request` events from the daemon
/// (proxied through the Tauri Rust backend) AND fetch any request
/// that was staged before the listener was registered. Tauri events
/// fired before the listener is in place are silently dropped, so
/// the event path alone races the webview-load. The pending fetch
/// closes that gap.
///
/// Idempotent: calling it twice does not stack listeners; the second
/// call replaces the first.
export async function initPickerBridge() {
  await backendLog("initPickerBridge entered");

  if (unlistenIncoming) {
    unlistenIncoming();
  }
  try {
    unlistenIncoming = await listen<PickerRequest>("picker:request", (event: Event<PickerRequest>) => {
      backendLog(`picker:request event received in listener, handle=${event.payload.handle}`);
      setRequest(event.payload);
    });
    await backendLog("picker:request listener registered");
  } catch (e) {
    await backendLog(`listen() failed: ${e}`, "error");
  }

  // Drain any request that arrived during webview load.
  try {
    const pending = await invoke<PickerRequest | null>("picker_take_pending");
    await backendLog(
      `picker_take_pending returned: ${pending === null ? "null" : "request handle=" + pending.handle}`,
    );
    if (pending) {
      setRequest(pending);
    }
  } catch (e) {
    await backendLog(`picker_take_pending invoke failed: ${e}`, "error");
  }
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
