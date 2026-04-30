<script lang="ts">
  /// F2.2 placeholder picker UI.
  ///
  /// Wires the IPC bridge end-to-end: receives a request from the
  /// daemon, lets the user click Cancel or "Pick /tmp/test.txt", and
  /// sends the corresponding response. F2.3 replaces the placeholder
  /// body with the real file-tree, filter dropdown, and breadcrumb
  /// bar. The bridge code itself stays as-is.
  import { onMount } from "svelte";

  import { initPickerBridge, respond } from "$lib/ipc";
  import { getPickState } from "$lib/stores/pickState.svelte";
  import type { PickerRequest } from "$lib/types/protocol";

  const state = getPickState();

  onMount(() => {
    initPickerBridge();
  });

  function requestHandle(req: PickerRequest): string {
    return req.handle;
  }

  function requestTitle(req: PickerRequest): string {
    if ("title" in req) return req.title;
    return req.type;
  }

  async function pickPlaceholder() {
    const req = state.request;
    if (!req) return;
    await respond({
      type: "picked",
      handle: requestHandle(req),
      paths: ["/tmp/picker-ui-f2-2-placeholder.txt"],
      currentFilter: null,
    });
  }

  async function cancel() {
    const req = state.request;
    if (!req) return;
    await respond({ type: "cancelled", handle: requestHandle(req) });
  }
</script>

<main class="picker-shell">
  {#if state.request}
    <header>
      <h1>{requestTitle(state.request)}</h1>
      <p class="meta">handle: {requestHandle(state.request)} · type: {state.request.type}</p>
    </header>
    <section class="body">
      <p class="placeholder">
        F2.2 placeholder. The real picker UI (file tree, filter dropdown,
        breadcrumbs) lands in F2.3.
      </p>
    </section>
    <footer>
      <button class="btn-ghost" onclick={cancel} disabled={state.busy}>
        Cancel
      </button>
      <button class="btn-primary" onclick={pickPlaceholder} disabled={state.busy}>
        Use placeholder path
      </button>
    </footer>
  {:else}
    <div class="idle">
      <p>Picker is idle. Waiting for a request from the daemon.</p>
    </div>
  {/if}
</main>

<style>
  .picker-shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background: var(--color-bg-app);
    color: var(--color-fg-app);
  }

  header {
    padding: 16px 20px 12px;
    border-bottom: 1px solid var(--color-border);
  }

  header h1 {
    margin: 0;
    font-size: 1.0625rem;
    font-weight: 600;
  }

  header .meta {
    margin: 4px 0 0;
    font-size: 0.75rem;
    color: var(--color-fg-muted);
    font-family: ui-monospace, SFMono-Regular, monospace;
  }

  .body {
    flex: 1;
    padding: 20px;
    overflow-y: auto;
  }

  .placeholder {
    color: var(--color-fg-muted);
    font-style: italic;
  }

  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--color-border);
    background: var(--color-bg-card);
  }

  .btn-ghost,
  .btn-primary {
    padding: 6px 14px;
    border-radius: var(--radius-md);
    font-size: 0.875rem;
    border: 1px solid transparent;
    transition:
      background 80ms ease,
      border-color 80ms ease,
      opacity 80ms ease;
  }

  .btn-ghost {
    background: transparent;
    color: var(--color-fg-app);
    border-color: var(--color-border);
  }

  .btn-ghost:hover:not(:disabled) {
    background: color-mix(in srgb, var(--color-fg-app) 8%, transparent);
  }

  .btn-primary {
    background: var(--color-accent);
    color: white;
  }

  .btn-primary:hover:not(:disabled) {
    background: color-mix(in srgb, var(--color-accent) 85%, white);
  }

  .btn-ghost:disabled,
  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .idle {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--color-fg-muted);
    padding: 24px;
    text-align: center;
  }
</style>
