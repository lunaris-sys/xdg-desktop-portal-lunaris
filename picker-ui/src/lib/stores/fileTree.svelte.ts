/// File-tree state for the picker UI.
///
/// Module-level `$state` so all picker components see the same
/// directory/selection without having to drill props or pass
/// callbacks through layout boundaries.
///
/// `selected` is a Set<string> because Svelte 5 proxies Set
/// mutations as reactive operations; toggling membership is the
/// natural shape and avoids the Array-search round-trip on every
/// click.

import { invoke } from "@tauri-apps/api/core";

import type { DirEntry, FileFilter } from "$lib/types/protocol";

interface FileTreeState {
  currentDir: string;
  entries: DirEntry[];
  selected: Set<string>;
  showHidden: boolean;
  activeFilter: FileFilter | null;
  loading: boolean;
  saveFilename: string;
  loadError: string | null;
  /// Transient user-visible notice (e.g. "selection limit reached").
  /// Auto-clears after 3 s. `null` when nothing to show.
  notice: string | null;
}

const state = $state<FileTreeState>({
  currentDir: "",
  entries: [],
  selected: new Set(),
  showHidden: false,
  activeFilter: null,
  loading: false,
  saveFilename: "",
  loadError: null,
  notice: null,
});

let noticeTimer: ReturnType<typeof setTimeout> | null = null;

/// Show a transient notice for 3 s. Replaces an existing notice
/// (instead of stacking) — picker UI is small and stacking
/// notices would overflow the window. Used for the multi-select
/// cap announcement (E22) and reserved for similar future
/// short-lived signals.
export function showNotice(message: string) {
  if (noticeTimer !== null) {
    clearTimeout(noticeTimer);
  }
  state.notice = message;
  noticeTimer = setTimeout(() => {
    state.notice = null;
    noticeTimer = null;
  }, 3000);
}

/// Monotonically-increasing navigation id. Each `navigateTo` bumps
/// it before the async `list_directory` call; the response is only
/// applied if the id still matches when it returns. Without this,
/// two quick navigations on a slow filesystem can land in the wrong
/// order — Codex flagged this as a navigation race that would let
/// the UI display directory B but submit directory A.
let navId = 0;

export function getTreeState(): FileTreeState {
  return state;
}

/// Resolve and load a starting directory. The Rust side picks the
/// caller's `current_folder` if it exists, otherwise `$HOME`.
export async function startSession(provided: string | null) {
  const resolved = await invoke<string>("resolve_start_dir", { provided });
  state.activeFilter = null;
  await navigateTo(resolved);
}

/// Navigate to a directory. Cancels selection because the previous
/// selection is no longer in scope. Stale results from earlier
/// navigations are discarded so the displayed listing always matches
/// the most recently requested directory.
export async function navigateTo(path: string) {
  const myNav = ++navId;
  state.loading = true;
  state.loadError = null;
  state.selected = new Set();
  try {
    const entries: DirEntry[] = await invoke("list_directory", { path });
    if (myNav !== navId) return;
    state.currentDir = path;
    state.entries = entries;
  } catch (e) {
    if (myNav !== navId) return;
    state.loadError = String(e);
  } finally {
    if (myNav === navId) state.loading = false;
  }
}

export async function navigateUp() {
  const parent: string | null = await invoke("parent_dir", {
    path: state.currentDir,
  });
  if (parent) {
    await navigateTo(parent);
  }
}

export function toggleHidden() {
  state.showHidden = !state.showHidden;
}

/// Toggle membership of an entry in the selection set.
///
/// `multiple=false` clears the prior selection so the user always
/// has at most one path; `multiple=true` flips membership in place.
export function toggleSelected(path: string, multiple: boolean) {
  if (!multiple) {
    state.selected = new Set([path]);
    return;
  }
  if (state.selected.has(path)) {
    state.selected.delete(path);
    state.selected = new Set(state.selected);
  } else {
    state.selected.add(path);
    state.selected = new Set(state.selected);
  }
}

export function setActiveFilter(filter: FileFilter | null) {
  state.activeFilter = filter;
}

export function setSaveFilename(name: string) {
  state.saveFilename = name;
}

/// Multi-select size cap (E22). The wire-side D-Bus message-size
/// limit is ~16 KB; with long paths a thousand-file selection can
/// overflow. We cap at 256 in the UI and surface a toast if the
/// user reaches it. Keeping this constant exported makes the cap
/// testable in component tests.
export const MULTI_SELECT_CAP = 256;

/// Validate a Save filename input. The picker UI builds save paths
/// as `<currentDir>/<filename>`; if the filename contains path
/// separators or `..` it can escape the displayed directory and
/// hand the caller (especially a sandboxed one) a writable export
/// for an unintended location. Codex flagged the construction
/// without validation as a trust-boundary bug.
///
/// Returns `null` when the name is acceptable, or an error string
/// suitable for inline display. Daemon-side `validate_save_path`
/// is the second line of defence.
export function validateFilename(name: string): string | null {
  if (!name || name.length === 0) return "Filename is required.";
  if (name === "." || name === "..") return "Reserved name.";
  if (name.includes("/")) return "Slashes are not allowed in the filename.";
  if (name.includes("\0")) return "Filename cannot contain a NUL byte.";
  for (const c of name) {
    if (c.charCodeAt(0) < 0x20) return "Filename cannot contain control characters.";
  }
  return null;
}

/// Glob-match an entry name against a single pattern. The picker
/// uses simple ends-with for `*.ext` because that covers the
/// realistic pattern set; full glob would need a dependency for
/// no real benefit.
export function matchesGlob(name: string, pattern: string): boolean {
  if (pattern === "*") return true;
  if (pattern.startsWith("*.")) {
    return name.toLowerCase().endsWith(pattern.slice(1).toLowerCase());
  }
  return name.toLowerCase() === pattern.toLowerCase();
}

/// Whether `entry` should be visible given the current filter and
/// hidden-files toggle. Directories always pass so the user can
/// navigate even when filtering files.
export function entryVisible(entry: DirEntry): boolean {
  if (entry.isHidden && !state.showHidden) return false;
  if (entry.isDirectory) return true;
  const filter = state.activeFilter;
  if (!filter) return true;
  for (const pat of filter.patterns) {
    if (pat.kind === "glob" && matchesGlob(entry.name, pat.pattern)) return true;
    if (pat.kind === "mime") {
      // Best-effort MIME match: derive from extension. Real MIME
      // detection would need xdg-mime; the common image/audio cases
      // map cleanly from extension already.
      if (matchesMime(entry.name, pat.mimeType)) return true;
    }
  }
  return false;
}

function matchesMime(name: string, mimeType: string): boolean {
  const ext = name.toLowerCase().split(".").pop() ?? "";
  // Compact mapping covering the common types portal callers use.
  // Add entries here as real callers surface unmatched ones.
  const map: Record<string, string[]> = {
    "image/png": ["png"],
    "image/jpeg": ["jpg", "jpeg"],
    "image/gif": ["gif"],
    "image/webp": ["webp"],
    "image/svg+xml": ["svg"],
    "image/heic": ["heic", "heif"],
    "application/pdf": ["pdf"],
    "text/plain": ["txt", "log", "md"],
    "text/markdown": ["md"],
    "audio/mpeg": ["mp3"],
    "audio/ogg": ["ogg"],
    "video/mp4": ["mp4"],
    "video/webm": ["webm"],
  };
  if (mimeType.endsWith("/*")) {
    const prefix = mimeType.slice(0, -2);
    return Object.entries(map).some(
      ([type, exts]) => type.startsWith(`${prefix}/`) && exts.includes(ext),
    );
  }
  return (map[mimeType] ?? []).includes(ext);
}
