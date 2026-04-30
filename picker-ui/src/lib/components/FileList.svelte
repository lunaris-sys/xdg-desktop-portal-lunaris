<script lang="ts">
  import { Folder, FileText, FileImage, FileAudio, FileVideo, FileArchive, File as FileIcon } from "lucide-svelte";

  import type { DirEntry } from "$lib/types/protocol";
  import {
    entryVisible,
    getTreeState,
    navigateTo,
    toggleSelected,
    MULTI_SELECT_CAP,
  } from "$lib/stores/fileTree.svelte";

  interface Props {
    multiple: boolean;
    /// If true, show only directories. Used for `directory` mode
    /// where the user is selecting a folder.
    directoriesOnly: boolean;
    onSubmit: () => void;
  }

  let { multiple, directoriesOnly, onSubmit }: Props = $props();

  const state = getTreeState();

  /// Choose an icon based on extension. Falls back to a generic
  /// file icon. Lucide does not ship every MIME type; we cover the
  /// ones that show up in practice.
  function iconFor(entry: DirEntry) {
    if (entry.isDirectory) return Folder;
    const ext = entry.name.toLowerCase().split(".").pop() ?? "";
    if (["png", "jpg", "jpeg", "gif", "webp", "svg", "heic"].includes(ext)) return FileImage;
    if (["mp3", "wav", "ogg", "flac", "m4a"].includes(ext)) return FileAudio;
    if (["mp4", "webm", "mkv", "mov", "avi"].includes(ext)) return FileVideo;
    if (["zip", "tar", "gz", "xz", "7z", "rar"].includes(ext)) return FileArchive;
    if (["txt", "md", "log", "rs", "ts", "js", "py", "json", "toml"].includes(ext))
      return FileText;
    return FileIcon;
  }

  let visible = $derived(
    state.entries.filter((e) => {
      if (!entryVisible(e)) return false;
      if (directoriesOnly && !e.isDirectory) return false;
      return true;
    }),
  );

  function rowClick(entry: DirEntry, event: MouseEvent) {
    if (entry.isDirectory) {
      if (directoriesOnly && multiple) {
        // Multi-folder pick: every directory click toggles its
        // membership in the selection set instead of navigating.
        // Codex flagged that previously a multi=true,directory=true
        // request only ever returned `currentDir`; here every
        // selected folder rides home in the response. Double-click
        // navigates (handled in `rowDblClick`).
        if (state.selected.size >= MULTI_SELECT_CAP && !state.selected.has(entry.path)) {
          return;
        }
        toggleSelected(entry.path, true);
        return;
      }
      if (directoriesOnly) {
        // Single directory pick: clicks navigate, the currently
        // displayed dir is the implicit answer.
        if (event.ctrlKey || event.shiftKey) return;
        navigateTo(entry.path);
        return;
      }
      // File picker that allows directory rows: ctrl/shift adds to
      // selection (when multiple), plain click navigates.
      if (event.ctrlKey || event.shiftKey) {
        if (multiple) toggleSelected(entry.path, true);
      } else {
        navigateTo(entry.path);
      }
      return;
    }
    if (multiple) {
      if (state.selected.size >= MULTI_SELECT_CAP && !state.selected.has(entry.path)) {
        // The cap is generous (256); silently dropping further
        // additions would confuse the user, so we no-op and assume
        // a future toast (added in F2.5) surfaces it.
        return;
      }
      toggleSelected(entry.path, true);
    } else {
      toggleSelected(entry.path, false);
    }
  }

  function rowDblClick(entry: DirEntry) {
    if (entry.isDirectory) {
      navigateTo(entry.path);
      return;
    }
    if (directoriesOnly) return;
    // Double-click on a file confirms with that single file.
    toggleSelected(entry.path, false);
    onSubmit();
  }
</script>

<div class="file-list" class:loading={state.loading}>
  {#if state.loadError}
    <div class="error">
      <p>Could not list directory: {state.loadError}</p>
    </div>
  {:else if state.loading}
    <div class="placeholder">Loading…</div>
  {:else if visible.length === 0}
    <div class="placeholder">Empty directory</div>
  {:else}
    <ul role="listbox" aria-multiselectable={multiple}>
      {#each visible as entry (entry.path)}
        {@const Icon = iconFor(entry)}
        <li>
          <button
            type="button"
            class="row"
            class:selected={state.selected.has(entry.path)}
            class:hidden-file={entry.isHidden}
            role="option"
            aria-selected={state.selected.has(entry.path)}
            onclick={(e) => rowClick(entry, e)}
            ondblclick={() => rowDblClick(entry)}
          >
            <Icon size={16} strokeWidth={1.75} class="row-icon" />
            <span class="row-name">{entry.name}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .file-list {
    flex: 1;
    overflow-y: auto;
    background: var(--color-bg-app);
  }

  .file-list.loading {
    pointer-events: none;
    opacity: 0.6;
  }

  ul {
    margin: 0;
    padding: 4px;
    list-style: none;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 6px 10px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    color: var(--color-fg-app);
    text-align: left;
    cursor: default;
    transition: background 60ms ease;
  }

  .row:hover {
    background: color-mix(in srgb, var(--color-fg-app) 6%, transparent);
  }

  .row.selected {
    background: color-mix(in srgb, var(--color-accent) 25%, transparent);
    border-color: color-mix(in srgb, var(--color-accent) 50%, transparent);
  }

  .row.hidden-file .row-name {
    color: var(--color-fg-muted);
  }

  .row-name {
    font-size: 0.875rem;
    flex: 1;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  :global(.row .row-icon) {
    color: var(--color-fg-muted);
    flex-shrink: 0;
  }

  .row.selected :global(.row-icon) {
    color: var(--color-accent);
  }

  .placeholder,
  .error {
    padding: 32px;
    text-align: center;
    color: var(--color-fg-muted);
  }

  .error {
    color: var(--color-danger);
  }
</style>
