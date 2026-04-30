<script lang="ts">
  /// Picker UI main view.
  ///
  /// On every incoming PickerRequest the file-tree resets to the
  /// caller's `currentFolder` (or `$HOME`), and the user navigates
  /// + selects + confirms. Confirm and cancel both go through
  /// `respond()` which sends a frame to the daemon and hides the
  /// window.
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { ArrowLeft, EyeOff, Eye } from "lucide-svelte";

  import { applyTheme, initPickerBridge, respond } from "$lib/ipc";
  import { getPickState } from "$lib/stores/pickState.svelte";
  import {
    getTreeState,
    navigateTo,
    navigateUp,
    setActiveFilter,
    startSession,
    toggleHidden,
    validateFilename,
  } from "$lib/stores/fileTree.svelte";
  import type { PickerRequest } from "$lib/types/protocol";
  import Breadcrumb from "$lib/components/Breadcrumb.svelte";
  import FileList from "$lib/components/FileList.svelte";
  import FilterDropdown from "$lib/components/FilterDropdown.svelte";
  import SaveBar from "$lib/components/SaveBar.svelte";

  const pickState = getPickState();
  const treeState = getTreeState();

  /// Track which request we have already initialised the file tree
  /// for. Without this guard, every reactive update on the request
  /// would re-navigate to the start dir and discard user navigation.
  let lastInitedHandle = $state<string | null>(null);

  onMount(() => {
    applyTheme();
    initPickerBridge();
  });

  $effect(() => {
    const req = pickState.request;
    if (req && req.handle !== lastInitedHandle) {
      lastInitedHandle = req.handle;

      // SaveFile filename precedence (Codex P2): caller-supplied
      // `currentName` wins; otherwise fall back to the basename of
      // `currentFile` so the user does not have to retype it. Drops
      // to empty string for OpenFile / SaveFiles where neither
      // applies.
      if (req.type === "saveFile") {
        if (req.currentName) {
          treeState.saveFilename = req.currentName;
        } else if (req.currentFile) {
          treeState.saveFilename = basename(req.currentFile);
        } else {
          treeState.saveFilename = "";
        }
      } else {
        treeState.saveFilename = "";
      }

      // Filter init (Codex P2): honour the caller's `currentFilter`
      // verbatim. If they did not preselect one, we leave
      // `activeFilter` null so the user sees "All files" rather
      // than a filter-of-our-choosing that hides their files.
      if ("currentFilter" in req && req.currentFilter) {
        setActiveFilter(req.currentFilter);
      } else {
        setActiveFilter(null);
      }

      const start =
        ("currentFolder" in req && req.currentFolder) ??
        ("currentFile" in req && req.currentFile
          ? parentDirString(req.currentFile)
          : null);
      startSession(start);
    }
  });

  function parentDirString(path: string): string {
    const idx = path.lastIndexOf("/");
    return idx > 0 ? path.slice(0, idx) : "/";
  }

  function basename(path: string): string {
    const idx = path.lastIndexOf("/");
    return idx >= 0 ? path.slice(idx + 1) : path;
  }

  function isOpenFile(req: PickerRequest): req is Extract<PickerRequest, { type: "openFile" }> {
    return req.type === "openFile";
  }

  function isSaveFile(req: PickerRequest): req is Extract<PickerRequest, { type: "saveFile" }> {
    return req.type === "saveFile";
  }

  function isSaveFiles(
    req: PickerRequest,
  ): req is Extract<PickerRequest, { type: "saveFiles" }> {
    return req.type === "saveFiles";
  }

  let multiple = $derived.by(() => {
    const r = pickState.request;
    return r && isOpenFile(r) ? r.multiple : false;
  });

  let directoriesOnly = $derived.by(() => {
    const r = pickState.request;
    return r && isOpenFile(r) ? r.directory : false;
  });

  let filters = $derived.by(() => {
    const r = pickState.request;
    if (!r) return [];
    if (isOpenFile(r) || isSaveFile(r)) return r.filters;
    return [];
  });

  let confirmDisabled = $derived.by(() => {
    const r = pickState.request;
    if (!r || pickState.busy) return true;
    if (isOpenFile(r)) {
      // directoriesOnly without multiple: currentDir is always
      // valid as the answer. directoriesOnly && multiple: at least
      // one folder must be ticked.
      if (directoriesOnly) {
        if (multiple) return treeState.selected.size === 0;
        return false;
      }
      return treeState.selected.size === 0;
    }
    if (isSaveFile(r)) {
      return validateFilename(treeState.saveFilename) !== null;
    }
    if (isSaveFiles(r)) return r.files.length === 0;
    return true;
  });

  let confirmLabel = $derived.by(() => {
    const r = pickState.request;
    if (!r) return "Open";
    if (isSaveFile(r) || isSaveFiles(r)) return "Save";
    if (directoriesOnly) return "Choose folder";
    return "Open";
  });

  async function confirm() {
    const r = pickState.request;
    if (!r) return;
    if (isOpenFile(r)) {
      if (directoriesOnly) {
        // multi-directory: hand back every ticked folder so a
        // caller asking for `directory && multiple` actually
        // receives the user's selection (Codex P1). Single
        // directory mode falls back to the currently-displayed
        // folder.
        const paths =
          multiple && treeState.selected.size > 0
            ? Array.from(treeState.selected)
            : [treeState.currentDir];
        await respond({
          type: "picked",
          handle: r.handle,
          paths,
          currentFilter: treeState.activeFilter,
        });
        return;
      }
      const paths = Array.from(treeState.selected);
      if (paths.length === 0) return;
      await respond({
        type: "picked",
        handle: r.handle,
        paths,
        currentFilter: treeState.activeFilter,
      });
      return;
    }
    if (isSaveFile(r)) {
      const name = treeState.saveFilename.trim();
      // Frontend defence layer (Codex H2). The daemon revalidates
      // the resulting path; this guard catches the bad input
      // earlier so the user gets clear feedback in the field.
      if (validateFilename(name) !== null) return;
      const path = `${treeState.currentDir.replace(/\/$/, "")}/${name}`;
      const exists = await invoke<boolean>("file_exists", { path });
      if (exists) {
        const ok = window.confirm(`Replace ${name}?`);
        if (!ok) return;
      }
      await respond({
        type: "picked",
        handle: r.handle,
        paths: [path],
        currentFilter: treeState.activeFilter,
      });
      return;
    }
    if (isSaveFiles(r)) {
      const dir = treeState.currentDir.replace(/\/$/, "");
      const paths = r.files.map((p) => {
        const idx = p.lastIndexOf("/");
        const filename = idx >= 0 ? p.slice(idx + 1) : p;
        return `${dir}/${filename}`;
      });
      await respond({
        type: "picked",
        handle: r.handle,
        paths,
        currentFilter: null,
      });
    }
  }

  async function cancel() {
    const r = pickState.request;
    if (!r) return;
    await respond({ type: "cancelled", handle: r.handle });
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      cancel();
    } else if (e.key === "h" && e.ctrlKey) {
      e.preventDefault();
      toggleHidden();
    } else if (e.key === "Enter" && !confirmDisabled) {
      confirm();
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

<main class="picker-shell">
  {#if pickState.request}
    <header data-tauri-drag-region>
      <div class="title-row" data-tauri-drag-region>
        <button
          type="button"
          class="back"
          aria-label="Up one directory"
          title="Up one directory"
          onclick={() => navigateUp()}
        >
          <ArrowLeft size={14} strokeWidth={2} />
        </button>
        <h1 data-tauri-drag-region>{pickState.request.title || (directoriesOnly ? "Choose folder" : "Open file")}</h1>
        <button
          type="button"
          class="hidden-toggle"
          aria-label={treeState.showHidden ? "Hide hidden files" : "Show hidden files"}
          title="Toggle hidden files (Ctrl+H)"
          onclick={() => toggleHidden()}
        >
          {#if treeState.showHidden}
            <Eye size={14} strokeWidth={2} />
          {:else}
            <EyeOff size={14} strokeWidth={2} />
          {/if}
        </button>
      </div>
      <Breadcrumb path={treeState.currentDir} onNavigate={navigateTo} />
    </header>
    <FileList {multiple} {directoriesOnly} onSubmit={confirm} />
    {#if isSaveFile(pickState.request)}
      <SaveBar />
    {/if}
    <footer>
      <FilterDropdown {filters} />
      <div class="actions">
        <button class="btn-ghost" onclick={cancel} disabled={pickState.busy}>
          Cancel
        </button>
        <button class="btn-primary" onclick={confirm} disabled={confirmDisabled}>
          {confirmLabel}
        </button>
      </div>
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
    padding: 10px 16px;
    border-bottom: 1px solid var(--color-border);
  }

  .title-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
  }

  .title-row h1 {
    flex: 1;
    margin: 0;
    font-size: 0.9375rem;
    font-weight: 600;
  }

  .back,
  .hidden-toggle {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--color-fg-muted);
    border-radius: var(--radius-sm);
    transition: background 80ms ease, color 80ms ease;
  }

  .back:hover,
  .hidden-toggle:hover {
    background: color-mix(in srgb, var(--color-fg-app) 8%, transparent);
    color: var(--color-fg-app);
  }

  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 10px 16px;
    border-top: 1px solid var(--color-border);
    background: var(--color-bg-card);
  }

  .actions {
    display: flex;
    gap: 8px;
  }

  .btn-ghost,
  .btn-primary {
    padding: 6px 14px;
    border-radius: var(--radius-md);
    font-size: 0.875rem;
    border: 1px solid transparent;
    transition: background 80ms ease, opacity 80ms ease;
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
