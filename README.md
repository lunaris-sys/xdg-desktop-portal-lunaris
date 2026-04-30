# xdg-desktop-portal-lunaris

Lunaris backend for the freedesktop XDG Desktop Portal. Implements
`FileChooser` (file/directory pickers, save dialogs) and `OpenURI`
(open links and files in the user's preferred handler).

The full architecture spec lives at
`../docs/architecture/xdg-desktop-portal-lunaris.md`. The
end-to-end test checklist lives at [`E2E.md`](E2E.md).

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         CALLER (any app)                         │
│   GTK / Qt / Tauri / Flatpak — calls the standard portal API     │
└─────────────────────────┬───────────────────────────────────────┘
                          │  org.freedesktop.portal.FileChooser
                          │  org.freedesktop.portal.OpenURI
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│            xdg-desktop-portal (frontend, freedesktop)            │
│  Routes by XDG_CURRENT_DESKTOP + .portal config UseIn= field    │
└─────────────────────────┬───────────────────────────────────────┘
                          │  org.freedesktop.impl.portal.*
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│                xdg-desktop-portal-lunaris (this repo)            │
│  daemon/  ── zbus interface impls + sandbox detection            │
│  protocol/ ── shared IPC wire types                              │
│  picker-ui/ ── Tauri picker window (spawned per session)         │
└──────────────────────────┬──────────────────────────────────────┘
                           │  Unix socket
                           ▼   (length-prefixed JSON, FA4)
                ┌────────────────────────┐
                │   picker-ui process    │
                │   (Tauri + SvelteKit)  │
                └────────────────────────┘
```

For sandboxed callers (Flatpak, Snap), picked file paths are
re-exported through `xdg-document-portal` so the URIs the caller
receives resolve inside its bubblewrap mount namespace.

## Build

```bash
# Daemon
cargo build --release

# Picker UI
(cd picker-ui && npm install && npm run build)
(cd picker-ui/src-tauri && cargo build --release)
```

Tests:

```bash
cargo test                              # daemon + protocol crates
(cd picker-ui/src-tauri && cargo test)  # picker fs commands
```

Total: **62 unit tests** across daemon (54), protocol (8), and
picker-ui src-tauri (5).

## Install

Production (system-wide, requires sudo):

```bash
../distro/install-portal.sh
```

Development (per-user symlinks, no sudo):

```bash
../distro/dev-portal-setup.sh
./distro/start-dev.sh --with-portal
```

Teardown:

```bash
../distro/dev-portal-teardown.sh
```

## Testing

The portal is exercised end-to-end by [`E2E.md`](E2E.md). Manual
because every scenario depends on a real Wayland session and
real callers.

## Design notes

The non-obvious bits are documented in
`../docs/architecture/xdg-desktop-portal-lunaris.md`:

- Why a standalone backend instead of forking
  `xdg-desktop-portal-gtk` (FA1)
- Why the picker UI runs as a separate Tauri process (FA3)
- Why caller identity comes from cgroup rather than the `app_id`
  argument (FA7)
- How sandboxed callers receive Document Portal URIs (FA8)
- The 25 edge cases from E1 ("concurrent requests") through E25
  ("WebKitGTK leak after N picks")
