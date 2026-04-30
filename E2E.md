# E2E test checklist

Manual scenarios for `xdg-desktop-portal-lunaris`. None of these are
automated — they need a Wayland session, a Lunaris compositor, and
real apps that exercise the portal interfaces. Walk this list after
shipping any Sprint F changes.

## Prerequisites

- `./distro/dev-portal-setup.sh` succeeded (see repo root README).
- `./distro/start-dev.sh --with-portal` is running.
- `busctl --user list | grep org.freedesktop.impl.portal.desktop.lunaris`
  prints the bus name. If not, `tail -f logs/portal-daemon.log`.

For the Flatpak scenarios you need at least one Flatpak app with a
file-picker dialog. Any of these works (whichever you happen to
have): `org.gnome.gedit`, `org.kde.kdenlive`, `org.gnome.TextEditor`,
`org.mozilla.firefox`. List your installed Flatpaks with
`flatpak list --user --app` and pick one whose UI exposes File →
Open or File → Save As.

Quick install if none are present (assumes the flathub remote is
configured for the user; see
`flatpak remote-add --user --if-not-exists flathub
https://flathub.org/repo/flathub.flatpakrepo`):

```bash
flatpak install --user flathub org.kde.kdenlive
```

## Scenarios

### S1 — Settings DirectoryPicker (unconfined caller, file://)

1. Open the **Settings → Knowledge** panel.
2. Click "Browse" next to "Project Watch Path".
3. **Expect:** Lunaris-themed picker window appears.
4. Pick `~/Documents`, click "Choose folder".
5. **Expect:** path appears in the row; `graph.toml` updates
   (verify with `cat ~/.config/lunaris/graph.toml`).
6. Repeat, click "Cancel" instead.
7. **Expect:** no toml change, picker closes silently.

### S2 — Flatpak File Open (sandboxed caller, Document Portal)

1. Run any installed Flatpak app, e.g. `flatpak run --user org.kde.kdenlive`.
2. Trigger its file-open dialog (File → Open Project, or similar).
3. **Expect:** Lunaris picker (not the toolkit's native picker or
   GTK fallback). The window title comes from the caller.
4. Pick a file, click "Open".
5. **Expect:** the file loads in the caller.
6. Verify Document Portal received an entry:
   `flatpak permission-show desktop-used-apps` should list the
   caller's app id against the picked file.

### S3 — Flatpak File Save (sandboxed caller, AddNamedFull)

1. In the same Flatpak app, trigger File → Save As (or its
   equivalent — Kdenlive: File → Save Project As; gedit: Save As).
2. Name the file something new (does NOT exist yet).
3. **Expect:** picker accepts the new filename, save succeeds.
4. Verify the file exists at the picked location.
5. **Codex Sprint F regression check:** the old `AddFull`
   path failed with ENOENT for non-existent targets; this
   test is the proof that `AddNamedFull` is wired.

### S4 — OpenURI passthrough (http(s))

```bash
busctl --user call org.freedesktop.portal.Desktop \
    /org/freedesktop/portal/desktop \
    org.freedesktop.portal.OpenURI \
    OpenURI ssa{sv} \
    "" "https://example.com" 0
```

**Expect:** browser opens example.com. Daemon log shows
"OpenURI passthrough" with redacted URI (`https://example.com/...`).

### S5 — OpenURI scheme rejection

```bash
busctl --user call org.freedesktop.portal.Desktop \
    /org/freedesktop/portal/desktop \
    org.freedesktop.portal.OpenURI \
    OpenURI ssa{sv} \
    "" "javascript:alert(1)" 0
```

**Expect:** D-Bus call returns response code 2 (OTHER) with
an error message. Daemon log shows "OpenURI rejected scheme".

### S6 — Sandboxed file:// path-traversal rejection (Codex Sprint F regression)

Real Flatpak sandboxes don't ship `busctl` so this scenario is
covered by unit tests rather than an end-to-end run:

- `file_uri_traversal_rejected_for_sandboxed`
- `file_uri_percent_encoded_traversal_rejected`

Both in `daemon/src/interfaces/open_uri.rs`. Run with
`cargo test -p xdg-desktop-portal-lunaris` from the daemon
directory.

For end-to-end confirmation that sandboxed callers DO get
proper Document Portal access on legitimate paths, S2 above
(real File Open + `flatpak permission-show desktop-used-apps`)
is the load-bearing test.

### S7 — User cancels mid-pick (E11)

1. Trigger any picker (Settings DirectoryPicker is easiest).
2. Wait until the picker is showing.
3. Press `Escape`.
4. **Expect:** picker closes, caller sees a Cancelled
   response (no error toast, no log warning).

### S8 — Picker crash mid-pick (E11 disambiguation)

1. Trigger any picker.
2. While the picker is open, find its PID:
   `pgrep -f xdg-desktop-portal-lunaris-picker`.
3. `kill -9 <pid>`.
4. **Expect:** caller sees an Error response (NOT Cancelled).
   Daemon log shows
   "picker-ui disconnected without responding".

## Cleanup

`./distro/dev-portal-teardown.sh` removes the per-user D-Bus
service shim. `target/` build artefacts stay so the next dev
session is fast.
