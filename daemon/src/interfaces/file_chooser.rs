//! `org.freedesktop.impl.portal.FileChooser` implementation.
//!
//! Three methods: `OpenFile`, `SaveFile`, `SaveFiles`. Each method
//! ensures the picker subprocess is running, dispatches a
//! `PickerRequest` over the IPC socket, awaits the response, and
//! translates it into the spec's `(response_code, results)` tuple.
//!
//! Document Portal integration for sandboxed callers (FA8) is not
//! yet wired here — that lands as a follow-up when the picker UI
//! returns real paths instead of placeholder paths. For unconfined
//! callers and Lunaris-native apps the raw `file://` URIs in the
//! response are correct as-is.
//!
//! Spec:
//! https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.impl.portal.FileChooser.html

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use zbus::interface;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use xdg_portal_lunaris_protocol::{FileFilter, PickerRequest, PickerResponse};

use crate::document_portal;
use crate::interfaces::options;
use crate::request::{response, RequestHandle};
use crate::sandbox::{self, CallerIdentity};
use crate::state::DaemonState;

/// Result-key the spec mandates for the URI list returned by
/// FileChooser methods. Picker UI returns absolute paths; we
/// `file://`-encode them here.
const RESULT_URIS: &str = "uris";

/// Result-key for the filter the user had selected at confirm
/// time. Echoed only when the picker actually carried one.
const RESULT_CURRENT_FILTER: &str = "current_filter";

#[derive(Clone)]
pub struct FileChooser {
    state: DaemonState,
}

impl FileChooser {
    pub fn new(state: DaemonState) -> Self {
        Self { state }
    }

    /// Common dispatch: ensure picker running, submit IPC request,
    /// translate `PickerResponse` to D-Bus tuple. Sandboxed callers
    /// have their picked paths re-exported through the Document
    /// Portal (FA8) so the URIs they receive are reachable inside
    /// their bubblewrap mount namespace.
    async fn dispatch(
        &self,
        method: &str,
        request_path: ObjectPath<'_>,
        request: PickerRequest,
        identity: CallerIdentity,
        connection: &zbus::Connection,
        writable: bool,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let _guard = self.state.track_request();
        let req = RequestHandle::from_object_path(request_path.into());

        if let Err(e) = self.state.picker_lifecycle.ensure_running().await {
            tracing::warn!(request = %req.path, method, error = %e, "picker-ui spawn failed");
            return (response::OTHER, error_results(&format!("picker spawn: {e}")));
        }

        let rx = match self.state.picker_ipc.submit(request).await {
            Ok(rx) => rx,
            Err(e) => {
                tracing::warn!(request = %req.path, method, error = %e, "picker IPC submit failed");
                return (response::OTHER, error_results(&format!("picker submit: {e}")));
            }
        };

        let response = match rx.await {
            Ok(r) => r,
            Err(_) => {
                tracing::warn!(request = %req.path, method, "picker IPC oneshot dropped");
                return (
                    response::OTHER,
                    error_results("picker IPC channel closed"),
                );
            }
        };

        match response {
            PickerResponse::Picked {
                paths,
                current_filter,
                ..
            } => {
                let uris = match build_uris_for_caller(
                    &paths,
                    &identity,
                    connection,
                    writable,
                )
                .await
                {
                    Ok(uris) => uris,
                    Err(e) => {
                        tracing::warn!(
                            request = %req.path,
                            method,
                            "Document Portal export failed: {e}"
                        );
                        return (
                            response::OTHER,
                            error_results(&format!("portal export: {e}")),
                        );
                    }
                };
                (
                    response::SUCCESS,
                    success_results(&uris, current_filter.as_ref()),
                )
            }
            PickerResponse::Cancelled { .. } => (response::CANCELLED, HashMap::new()),
            PickerResponse::Error { message, .. } => {
                tracing::warn!(request = %req.path, method, "picker reported error: {message}");
                (response::OTHER, error_results(&message))
            }
        }
    }
}

/// Construct the URI list returned to the caller. Sandboxed callers
/// route through the Document Portal (FA8); unconfined callers get
/// raw `file://` URIs.
async fn build_uris_for_caller(
    paths: &[PathBuf],
    identity: &CallerIdentity,
    connection: &zbus::Connection,
    writable: bool,
) -> anyhow::Result<Vec<String>> {
    match identity.app_id() {
        Some(app_id) => {
            // Sandboxed: hand paths to Document Portal so the caller
            // sees URIs that resolve inside its bubblewrap mount.
            document_portal::export_for_caller(connection, app_id, paths, writable).await
        }
        None => {
            // Unconfined: raw `file://` URIs are reachable directly.
            Ok(paths.iter().map(|p| path_to_file_uri(p)).collect())
        }
    }
}

/// Resolve the calling D-Bus connection's PID and turn it into a
/// sandbox identity. Unconfined-on-error: a missing PID or unreachable
/// `org.freedesktop.DBus` is treated as unconfined rather than
/// rejecting the call, since portal callers always have a valid
/// connection in practice and the cost of a wrong unconfined
/// classification is "we hand back a raw `file://` URI", which only
/// breaks the call for actually sandboxed callers — and the picker
/// would have been pointless to show in the first place if we
/// reject unconditionally.
async fn caller_identity(
    header: &zbus::message::Header<'_>,
    connection: &zbus::Connection,
) -> CallerIdentity {
    let Some(sender) = header.sender() else {
        return CallerIdentity::Unconfined;
    };
    let dbus = match zbus::fdo::DBusProxy::new(connection).await {
        Ok(p) => p,
        Err(_) => return CallerIdentity::Unconfined,
    };
    let pid = match dbus
        .get_connection_unix_process_id(sender.clone().into())
        .await
    {
        Ok(p) => p,
        Err(_) => return CallerIdentity::Unconfined,
    };
    sandbox::detect(pid)
}

fn success_results(
    uris: &[String],
    current_filter: Option<&FileFilter>,
) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    if let Ok(owned) = Value::new(uris.to_vec()).try_to_owned() {
        map.insert(RESULT_URIS.to_string(), owned);
    }
    // Codex P3: echo the user's selected filter back so callers with
    // multiple filters can disambiguate which one confirmed.
    if let Some(filter) = current_filter {
        match options::filter_to_value(filter).and_then(|v| v.try_to_owned()) {
            Ok(owned) => {
                map.insert(RESULT_CURRENT_FILTER.to_string(), owned);
            }
            Err(e) => {
                tracing::warn!("encode current_filter for results: {e}");
            }
        }
    }
    map
}

fn error_results(message: &str) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    if let Ok(owned) = Value::new(message.to_string()).try_to_owned() {
        map.insert("lunaris-error".to_string(), owned);
    }
    map
}

/// Bytes that need percent-encoding in the path component of a
/// `file://` URI per RFC 3986. The `pchar` production allows
/// `unreserved / pct-encoded / sub-delims / ":" / "@"`, plus `/`
/// for path separators. Anything outside that set must be encoded.
///
/// We start from `CONTROLS` (all 0x00-0x1F and 0x7F) and add the
/// ASCII characters that are reserved or otherwise forbidden in the
/// pchar set: space, the URI delimiters (`#`, `?`), the percent
/// itself (so `%20` round-trips through encoding), and the
/// gen-delims that are not pchars (`<`, `>`, `[`, `]`, etc.).
/// Non-ASCII bytes get encoded automatically because `utf8_percent_encode`
/// emits each multi-byte UTF-8 sequence as a series of `%xx`.
const FILE_URI_PATH_SET: &AsciiSet = &CONTROLS
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

/// Encode an absolute path as a `file://` URI per RFC 8089 + RFC 3986.
/// Slashes are preserved as path separators. Reserved characters
/// (`#`, `?`, `%`) are percent-encoded so consumers cannot mis-parse
/// the URI as having a fragment, query, or partially-encoded byte.
fn path_to_file_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    let encoded = utf8_percent_encode(&s, FILE_URI_PATH_SET);
    format!("file://{encoded}")
}

/// Normalize the parent_window argument to `None` when empty so the
/// picker UI can distinguish "no parent" from "empty string". Empty
/// is what callers pass when they have no toplevel window available.
fn parse_parent_window(parent_window: &str) -> Option<String> {
    if parent_window.is_empty() {
        None
    } else {
        Some(parent_window.to_string())
    }
}

#[interface(name = "org.freedesktop.impl.portal.FileChooser")]
#[allow(clippy::too_many_arguments)] // spec-mandated method signatures
impl FileChooser {
    async fn open_file(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        title: &str,
        opts: HashMap<&str, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let identity = caller_identity(&header, connection).await;
        let request = PickerRequest::OpenFile {
            handle: handle.to_string(),
            app_id: app_id.to_string(),
            title: title.to_string(),
            filters: options::read_filters(&opts),
            current_filter: options::read_current_filter(&opts),
            multiple: options::read_bool(&opts, "multiple", false),
            modal: options::read_bool(&opts, "modal", true),
            current_folder: options::read_path_bytes(&opts, "current_folder"),
            parent_window: parse_parent_window(parent_window),
        };
        self.dispatch("OpenFile", handle, request, identity, connection, false)
            .await
    }

    async fn save_file(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        title: &str,
        opts: HashMap<&str, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let identity = caller_identity(&header, connection).await;
        let request = PickerRequest::SaveFile {
            handle: handle.to_string(),
            app_id: app_id.to_string(),
            title: title.to_string(),
            filters: options::read_filters(&opts),
            current_filter: options::read_current_filter(&opts),
            current_name: options::read_string(&opts, "current_name"),
            current_folder: options::read_path_bytes(&opts, "current_folder"),
            current_file: options::read_path_bytes(&opts, "current_file"),
            parent_window: parse_parent_window(parent_window),
        };
        self.dispatch("SaveFile", handle, request, identity, connection, true)
            .await
    }

    async fn save_files(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        title: &str,
        opts: HashMap<&str, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let identity = caller_identity(&header, connection).await;
        let request = PickerRequest::SaveFiles {
            handle: handle.to_string(),
            app_id: app_id.to_string(),
            title: title.to_string(),
            files: options::read_path_bytes_array(&opts, "files"),
            current_folder: options::read_path_bytes(&opts, "current_folder"),
            parent_window: parse_parent_window(parent_window),
        };
        self.dispatch("SaveFiles", handle, request, identity, connection, true)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Ordinary ASCII paths pass through unchanged after the file://
    /// prefix.
    #[test]
    fn ascii_path_to_uri() {
        let p = PathBuf::from("/home/user/Documents/report.pdf");
        assert_eq!(path_to_file_uri(&p), "file:///home/user/Documents/report.pdf");
    }

    /// Spaces are percent-encoded so the URI parses cleanly across
    /// the D-Bus boundary.
    #[test]
    fn space_in_path() {
        let p = PathBuf::from("/home/user/My Documents/x.txt");
        assert_eq!(
            path_to_file_uri(&p),
            "file:///home/user/My%20Documents/x.txt"
        );
    }

    /// Control characters become %xx. NUL is the most important one
    /// because it cannot appear literally in a D-Bus string.
    #[test]
    fn control_chars_in_path() {
        let p = PathBuf::from("/tmp/a\tb");
        assert_eq!(path_to_file_uri(&p), "file:///tmp/a%09b");
    }

    /// Codex P2: `#`, `?`, `%` must be percent-encoded so they do not
    /// turn into URI fragment / query / partial-encoding markers in
    /// consumers.
    #[test]
    fn reserved_uri_chars_in_path() {
        let p = PathBuf::from("/tmp/a#b.txt");
        assert_eq!(path_to_file_uri(&p), "file:///tmp/a%23b.txt");
        let p = PathBuf::from("/tmp/q?x.txt");
        assert_eq!(path_to_file_uri(&p), "file:///tmp/q%3Fx.txt");
        let p = PathBuf::from("/tmp/p%c.txt");
        assert_eq!(path_to_file_uri(&p), "file:///tmp/p%25c.txt");
    }

    /// Codex P2 follow-on: non-ASCII bytes percent-encode each UTF-8
    /// byte. `ä` is U+00E4 → 0xC3 0xA4 → %C3%A4.
    #[test]
    fn non_ascii_in_path() {
        let p = PathBuf::from("/home/user/Über.txt");
        assert_eq!(path_to_file_uri(&p), "file:///home/user/%C3%9Cber.txt");
    }

    /// Slashes are preserved as path separators. Forward slash is in
    /// the pchar set for paths and must not be encoded.
    #[test]
    fn slashes_preserved() {
        let p = PathBuf::from("/a/b/c");
        assert_eq!(path_to_file_uri(&p), "file:///a/b/c");
    }
}
