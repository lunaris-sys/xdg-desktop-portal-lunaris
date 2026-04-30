//! `org.freedesktop.impl.portal.OpenURI` implementation.
//!
//! Two methods: `OpenURI` (a string URI) and `OpenFile` (a file
//! descriptor). Caller-controlled URIs go through a scheme allow-list
//! per Sprint-E A1 pre-read: `http(s)://` passes through to
//! `xdg-open`, `file://` is sandbox-validated for confined callers
//! before forwarding, `mailto:` / `tel:` / `sms:` pass through, and
//! everything else is rejected.
//!
//! Spec:
//! https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.impl.portal.OpenURI.html
//!
//! `OpenFile` (fd variant) is not yet wired — it returns OTHER with
//! a clear "not implemented" error so callers fall through to
//! whatever fallback they have (typically a file:// URI). Real apps
//! use `OpenURI` overwhelmingly; the fd path is rare and can land
//! as a follow-up.

use std::collections::HashMap;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use zbus::interface;
use zbus::zvariant::{Fd, ObjectPath, OwnedValue, Value};

use crate::request::{response, RequestHandle};
use crate::sandbox::CallerIdentity;
use crate::state::DaemonState;

/// Schemes we forward to `xdg-open` without question. `mailto:` and
/// friends are authorityless but the kernel's xdg-open dispatcher
/// understands them. `https?` is the bulk of real-world traffic.
const PASSTHROUGH_SCHEMES: &[&str] = &[
    "http://",
    "https://",
    "mailto:",
    "tel:",
    "sms:",
    "xmpp:",
    "ftps://",
];

/// Schemes we explicitly reject. Listed for readability and for the
/// rejection log lines; `classify_scheme` returns `Rejected` for
/// anything not in `PASSTHROUGH_SCHEMES` or starting with `file://`.
const REJECTED_SCHEMES: &[&str] = &["javascript:", "data:", "vbscript:", "lunaris:"];

/// Document Portal mount root as a filesystem path.
/// `/run/user/<uid>/doc/`. Sandboxed callers can only OpenURI
/// `file://` URIs that resolve inside this mount.
fn document_portal_mount_path() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let mut p = PathBuf::from(runtime);
    p.push("doc");
    Some(p)
}

/// Same path-percent-encoding set as the FileChooser URI helper,
/// duplicated here for self-containment when the OpenFile (fd)
/// path constructs a `file://` URI from a /proc/self/fd readlink.
const URI_PATH_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// Parse a `file://` URI into a filesystem path. Percent-decodes,
/// rejects path-traversal segments, and refuses anything that
/// does not resolve to an absolute path — Codex flagged that the
/// previous string-prefix authorization let
/// `file:///run/user/1000/doc/../../etc/passwd` pass because
/// `starts_with` is not a containment check.
fn parse_file_uri(uri: &str) -> Result<PathBuf, &'static str> {
    let suffix = uri.strip_prefix("file://").ok_or("not a file:// URI")?;
    // Strip optional host (always empty for local file://).
    let path_part = suffix.split('/').enumerate().fold(
        String::new(),
        |mut acc, (idx, seg)| {
            if idx == 0 && seg.is_empty() {
                acc.push('/');
            } else if idx == 0 && !seg.is_empty() {
                // file://host/path — the host segment we drop.
                // Path starts at the next slash.
            } else {
                if !acc.ends_with('/') {
                    acc.push('/');
                }
                acc.push_str(seg);
            }
            acc
        },
    );
    if path_part.is_empty() {
        return Err("empty path");
    }
    let decoded = percent_decode_str(&path_part)
        .decode_utf8()
        .map_err(|_| "invalid UTF-8 percent-encoding")?;
    let path = PathBuf::from(decoded.as_ref());
    if !path.is_absolute() {
        return Err("not absolute");
    }
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err("NUL byte");
    }
    for comp in path.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            return Err("contains ..");
        }
    }
    Ok(path)
}

#[derive(Debug, PartialEq, Eq)]
enum SchemeClass {
    /// http(s), mailto, tel, sms, xmpp — pass straight through.
    Passthrough,
    /// `file://` — needs caller-identity-aware validation.
    File,
    /// Explicitly rejected scheme (javascript:, data:, lunaris:, ...).
    Rejected,
}

fn classify_scheme(uri: &str) -> SchemeClass {
    if uri.starts_with("file://") {
        return SchemeClass::File;
    }
    if PASSTHROUGH_SCHEMES.iter().any(|s| uri.starts_with(s)) {
        return SchemeClass::Passthrough;
    }
    SchemeClass::Rejected
}

/// Pure version of `file_uri_authorized` that takes the Document
/// Portal mount path as an argument. Cargo tests run in parallel
/// and share the process environment; tests that mutated
/// `XDG_RUNTIME_DIR` would race against each other, so the public
/// helper threads the resolved path through this function and
/// tests pass a literal.
///
/// Authorization rules:
/// - `Unknown` (identity-resolution failed) → deny. Codex flagged
///   that fail-open here let a transient D-Bus glitch waive the
///   sandbox check.
/// - `Unconfined` → allow. The caller could open the file from a
///   shell anyway.
/// - `Flatpak`/`Snap` → URI must parse cleanly (no traversal,
///   no NUL, percent-decode UTF-8) AND the resulting path must
///   start with the Document Portal mount path. The path-based
///   check replaces the previous string-prefix check that was
///   bypassable via `file:///mount/../escape` (Codex critical).
fn file_uri_authorized_with_prefix(
    uri: &str,
    identity: &CallerIdentity,
    document_mount: Option<&Path>,
) -> bool {
    if matches!(identity, CallerIdentity::Unknown) {
        return false;
    }
    if matches!(identity, CallerIdentity::Unconfined) {
        return true;
    }
    let Some(mount) = document_mount else {
        return false;
    };
    let Ok(path) = parse_file_uri(uri) else {
        return false;
    };
    path.starts_with(mount)
}

/// Sandbox-authorisation gate for `file://` URIs. See the
/// `_with_prefix` variant for rules.
fn file_uri_authorized(uri: &str, identity: &CallerIdentity) -> bool {
    let mount = document_portal_mount_path();
    file_uri_authorized_with_prefix(uri, identity, mount.as_deref())
}

/// Same path-percent-encoding semantics as the FileChooser URI
/// helper, applied here only for the redacted log line. The
/// production response shape just forwards the URI as-is to
/// xdg-open — we do not rewrite caller URIs.
fn redact_uri(uri: &str) -> String {
    if let Some(scheme_end) = uri.find("://") {
        let after_scheme = &uri[scheme_end + 3..];
        let host_end = after_scheme
            .find(['/', '?', '#'])
            .unwrap_or(after_scheme.len());
        let host = &after_scheme[..host_end];
        return format!("{}://{}/...", &uri[..scheme_end], host);
    }
    if let Some(colon) = uri.find(':') {
        return format!("{}:...", &uri[..colon]);
    }
    "<unparseable>".to_string()
}

#[derive(Clone)]
pub struct OpenUri {
    state: DaemonState,
}

impl OpenUri {
    pub fn new(state: DaemonState) -> Self {
        Self { state }
    }

    /// Determine caller identity from the frontend-supplied
    /// `app_id` argument. See `file_chooser::caller_identity` for
    /// the rationale on not using cgroup detection.
    fn caller_identity(method_app_id: &str) -> CallerIdentity {
        if !method_app_id.is_empty() {
            return CallerIdentity::Flatpak {
                app_id: method_app_id.to_string(),
            };
        }
        CallerIdentity::Unconfined
    }
}

fn error_results(message: &str) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    if let Ok(owned) = Value::new(message.to_string()).try_to_owned() {
        map.insert("lunaris-error".to_string(), owned);
    }
    map
}

#[interface(name = "org.freedesktop.impl.portal.OpenURI")]
#[allow(clippy::too_many_arguments)] // spec-mandated method signatures
impl OpenUri {
    /// Open a URI in the user's preferred handler.
    #[zbus(name = "OpenURI")]
    async fn open_uri(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        uri: &str,
        _options: HashMap<&str, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let _guard = self.state.track_request();
        let req = RequestHandle::from_object_path(handle.into());
        let identity = Self::caller_identity(app_id);
        let redacted = redact_uri(uri);

        match classify_scheme(uri) {
            SchemeClass::Passthrough => {
                tracing::info!(
                    request = %req.path,
                    app_id,
                    parent_window,
                    uri = %redacted,
                    identity = ?identity,
                    "OpenURI passthrough"
                );
                spawn_xdg_open(uri).await
            }
            SchemeClass::File => {
                if !file_uri_authorized(uri, &identity) {
                    tracing::warn!(
                        request = %req.path,
                        app_id,
                        uri = %redacted,
                        identity = ?identity,
                        "OpenURI file:// rejected — not in Document Portal mount"
                    );
                    return (
                        response::OTHER,
                        error_results(
                            "file:// URIs from sandboxed callers must point inside the Document Portal mount",
                        ),
                    );
                }
                tracing::info!(
                    request = %req.path,
                    app_id,
                    uri = %redacted,
                    identity = ?identity,
                    "OpenURI file:// authorised"
                );
                spawn_xdg_open(uri).await
            }
            SchemeClass::Rejected => {
                tracing::warn!(
                    request = %req.path,
                    app_id,
                    uri = %redacted,
                    "OpenURI rejected scheme"
                );
                let listed = REJECTED_SCHEMES
                    .iter()
                    .find(|s| uri.starts_with(*s))
                    .map(|s| s.trim_end_matches(':'))
                    .unwrap_or("unsupported");
                (
                    response::OTHER,
                    error_results(&format!("scheme not allowed: {listed}")),
                )
            }
        }
    }

    /// Open a file descriptor in the user's preferred handler.
    /// The fd is dup'd into the daemon process so we can resolve
    /// its filesystem path via `/proc/self/fd/<n>`, then the path
    /// is authorized against the caller's sandbox identity exactly
    /// like a file:// URI before xdg-open sees it.
    async fn open_file(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        fd: Fd<'_>,
        _options: HashMap<&str, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let _guard = self.state.track_request();
        let req = RequestHandle::from_object_path(handle.into());
        let identity = Self::caller_identity(app_id);

        let path = match resolve_fd_to_path(&fd) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    request = %req.path,
                    app_id,
                    parent_window,
                    "OpenFile fd resolution failed: {e}"
                );
                return (
                    response::OTHER,
                    error_results(&format!("resolve fd: {e}")),
                );
            }
        };

        let uri = format!(
            "file://{}",
            utf8_percent_encode(&path.to_string_lossy(), URI_PATH_SET)
        );

        if !file_uri_authorized(&uri, &identity) {
            tracing::warn!(
                request = %req.path,
                app_id,
                identity = ?identity,
                path = %path.display(),
                "OpenFile fd rejected — not in Document Portal mount or identity unknown"
            );
            return (
                response::OTHER,
                error_results(
                    "fd target is not authorised for this caller — file must be inside the Document Portal mount",
                ),
            );
        }

        tracing::info!(
            request = %req.path,
            app_id,
            parent_window,
            path = %path.display(),
            "OpenFile fd authorised"
        );
        spawn_xdg_open(&uri).await
    }
}

/// Resolve a borrowed D-Bus file descriptor to the absolute path
/// it currently points at. Works by dup-ing the fd into our
/// process (so the kernel keeps the inode pinned for our lookup)
/// and reading the magic `/proc/self/fd/<n>` symlink that the
/// kernel maintains for every open fd.
fn resolve_fd_to_path(fd: &Fd<'_>) -> Result<PathBuf, std::io::Error> {
    let owned = fd.as_fd().try_clone_to_owned()?;
    let raw = std::os::fd::AsRawFd::as_raw_fd(&owned);
    let proc_path = format!("/proc/self/fd/{raw}");
    std::fs::read_link(proc_path)
}

/// Spawn `xdg-open` for the given URI, fire-and-forget. xdg-open
/// is the standard freedesktop dispatcher; it forwards to the
/// user's configured browser, mail client, or default file
/// handler depending on the URI.
async fn spawn_xdg_open(uri: &str) -> (u32, HashMap<String, OwnedValue>) {
    let result = tokio::process::Command::new("xdg-open")
        .arg(uri)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn();
    match result {
        Ok(_child) => (response::SUCCESS, HashMap::new()),
        Err(e) => (
            response::OTHER,
            error_results(&format!("xdg-open spawn failed: {e}")),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// http(s), mailto, tel, sms classify as Passthrough.
    #[test]
    fn classify_passthrough_schemes() {
        assert_eq!(
            classify_scheme("https://example.com"),
            SchemeClass::Passthrough
        );
        assert_eq!(
            classify_scheme("http://example.com/path?x=1"),
            SchemeClass::Passthrough
        );
        assert_eq!(
            classify_scheme("mailto:alice@example.com"),
            SchemeClass::Passthrough
        );
        assert_eq!(classify_scheme("tel:+15555550100"), SchemeClass::Passthrough);
        assert_eq!(classify_scheme("sms:+15555550100"), SchemeClass::Passthrough);
    }

    /// `file://` is its own class (sandbox-validated).
    #[test]
    fn classify_file_scheme() {
        assert_eq!(
            classify_scheme("file:///home/user/x.txt"),
            SchemeClass::File
        );
        assert_eq!(
            classify_scheme("file:///run/user/1000/doc/abc/x"),
            SchemeClass::File
        );
    }

    /// Anything outside the allow-list classifies as Rejected.
    /// Notable: javascript: (XSS via opener), data: (data exfil),
    /// lunaris: (no-op), and bare strings (no scheme).
    #[test]
    fn classify_rejected_schemes() {
        assert_eq!(
            classify_scheme("javascript:alert(1)"),
            SchemeClass::Rejected
        );
        assert_eq!(classify_scheme("data:text/html,..."), SchemeClass::Rejected);
        assert_eq!(classify_scheme("lunaris:foo"), SchemeClass::Rejected);
        assert_eq!(classify_scheme("ftp://example.com"), SchemeClass::Rejected);
        assert_eq!(classify_scheme("not-a-uri"), SchemeClass::Rejected);
        assert_eq!(classify_scheme(""), SchemeClass::Rejected);
    }

    fn doc_mount() -> Option<&'static Path> {
        Some(Path::new("/run/user/1000/doc"))
    }

    /// Unconfined callers can open any `file://` URI — they could
    /// already do that from a shell anyway. Independent of
    /// `XDG_RUNTIME_DIR`, so we test through the pure helper to
    /// avoid env-var races with parallel tests.
    #[test]
    fn file_uri_unconfined_always_authorised() {
        let id = CallerIdentity::Unconfined;
        assert!(file_uri_authorized_with_prefix(
            "file:///etc/passwd",
            &id,
            doc_mount()
        ));
        assert!(file_uri_authorized_with_prefix(
            "file:///home/user/file.txt",
            &id,
            doc_mount()
        ));
        // Even with `None` mount — unconfined is unconditional.
        assert!(file_uri_authorized_with_prefix(
            "file:///etc/passwd",
            &id,
            None
        ));
    }

    /// Sandboxed callers (Flatpak, Snap) only get `file://` URIs
    /// that resolve inside the Document Portal mount. Tested
    /// through the pure helper with an explicit mount path.
    #[test]
    fn file_uri_sandboxed_only_doc_portal() {
        let id = CallerIdentity::Flatpak {
            app_id: "org.gnome.Calculator".into(),
        };
        assert!(file_uri_authorized_with_prefix(
            "file:///run/user/1000/doc/abc/report.pdf",
            &id,
            doc_mount()
        ));
        assert!(!file_uri_authorized_with_prefix(
            "file:///home/user/Documents/report.pdf",
            &id,
            doc_mount()
        ));
        assert!(!file_uri_authorized_with_prefix(
            "file:///etc/passwd",
            &id,
            doc_mount()
        ));
    }

    /// Without a mount (no `XDG_RUNTIME_DIR`), sandboxed callers
    /// cannot reach any file:// URI — better to refuse than to
    /// guess at a mount path.
    #[test]
    fn file_uri_sandboxed_without_prefix() {
        let id = CallerIdentity::Flatpak {
            app_id: "x".into(),
        };
        assert!(!file_uri_authorized_with_prefix(
            "file:///run/user/1000/doc/abc/x",
            &id,
            None
        ));
    }

    /// Codex CRITICAL: path traversal in the URI must not bypass
    /// the mount-membership check. `file:///mount/../etc/passwd`
    /// previously satisfied `starts_with(mount-prefix)`; now it
    /// is rejected at parse time before the prefix check runs.
    #[test]
    fn file_uri_traversal_rejected_for_sandboxed() {
        let id = CallerIdentity::Flatpak {
            app_id: "x".into(),
        };
        assert!(!file_uri_authorized_with_prefix(
            "file:///run/user/1000/doc/../../../etc/passwd",
            &id,
            doc_mount()
        ));
        assert!(!file_uri_authorized_with_prefix(
            "file:///run/user/1000/doc/abc/../etc/passwd",
            &id,
            doc_mount()
        ));
    }

    /// Percent-encoded `..` is also caught. The classic
    /// `..` → `%2E%2E` smuggling is rejected because we
    /// percent-decode before walking the components.
    #[test]
    fn file_uri_percent_encoded_traversal_rejected() {
        let id = CallerIdentity::Flatpak {
            app_id: "x".into(),
        };
        // %2E%2E is `..`
        assert!(!file_uri_authorized_with_prefix(
            "file:///run/user/1000/doc/%2E%2E/%2E%2E/etc/passwd",
            &id,
            doc_mount()
        ));
    }

    /// Codex HIGH: identity-resolution failure must fail-closed
    /// for file:// URIs even if the URI looks safe.
    #[test]
    fn file_uri_unknown_identity_denies() {
        let id = CallerIdentity::Unknown;
        assert!(!file_uri_authorized_with_prefix(
            "file:///run/user/1000/doc/abc/x",
            &id,
            doc_mount()
        ));
        // Including paths an Unconfined caller would be allowed.
        assert!(!file_uri_authorized_with_prefix(
            "file:///home/user/notes.md",
            &id,
            doc_mount()
        ));
    }

    /// `parse_file_uri` rejects malformed and unsafe inputs.
    /// Path-traversal segments are caught before the prefix check
    /// runs.
    #[test]
    fn parse_rejects_traversal_and_non_file_schemes() {
        assert!(parse_file_uri("file:///foo/../bar").is_err());
        assert!(parse_file_uri("file:foo").is_err());
        assert!(parse_file_uri("https://example.com").is_err());
    }

    /// RFC 8089 `file://host/path` form drops the host and parses
    /// to `/path`. We accept this since the path is still absolute
    /// and clean, but the test pins the behaviour so future
    /// refactors don't accidentally start accepting traversal in
    /// the host segment.
    #[test]
    fn parse_drops_host_segment() {
        let p = parse_file_uri("file://localhost/etc/hostname").unwrap();
        assert_eq!(p, PathBuf::from("/etc/hostname"));
    }

    #[test]
    fn parse_decodes_percent_encoded_path() {
        let p = parse_file_uri("file:///home/user/My%20Documents/x.txt").unwrap();
        assert_eq!(p, PathBuf::from("/home/user/My Documents/x.txt"));
    }

    /// Codex review-style coverage for the redactor: secret-bearing
    /// query strings strip cleanly.
    #[test]
    fn redact_strips_query_and_fragment() {
        assert_eq!(
            redact_uri("https://example.com/secret?token=abc#frag"),
            "https://example.com/..."
        );
        assert_eq!(
            redact_uri("mailto:secret@example.com"),
            "mailto:..."
        );
        assert_eq!(redact_uri(""), "<unparseable>");
    }
}
