//! `org.freedesktop.impl.portal.OpenURI` implementation.
//!
//! Two methods: `OpenURI` (a string URI) and `OpenFile` (a file
//! descriptor). F1 stubs both to `OTHER` (response code 2, backend
//! failure) with a sentinel error in the results-dict, mirroring the
//! FileChooser pattern. `CANCELLED` is reserved for actual user
//! dismissal — a stub must not impersonate that. F3 wires the scheme
//! allow-list split (http(s) passthrough vs file:// sandbox-validate)
//! per Sprint-E pre-read A1.
//!
//! Spec:
//! https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.impl.portal.OpenURI.html

use std::collections::HashMap;

use zbus::interface;
use zbus::zvariant::{Fd, ObjectPath, OwnedValue, Value};

use crate::request::{response, RequestHandle};
use crate::state::DaemonState;

const STUB_ERROR_KEY: &str = "lunaris-stub-error";

fn stub_results(method: &str) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    let val = format!("F1 stub for {method}; backend not yet implemented");
    if let Ok(owned) = Value::new(val).try_to_owned() {
        map.insert(STUB_ERROR_KEY.to_string(), owned);
    }
    map
}

/// Coarse summary of a URI for log lines: `scheme://host` only, with
/// path/query/fragment stripped. URIs frequently carry secrets in the
/// query (OAuth callbacks, signed download links) or in the path
/// (document identifiers, share tokens) and we are not in the business
/// of persisting those to the journal. The full URI is only available
/// at trace level for explicit debugging sessions.
fn redact_uri(uri: &str) -> String {
    if let Some(scheme_end) = uri.find("://") {
        let after_scheme = &uri[scheme_end + 3..];
        let host_end = after_scheme
            .find(['/', '?', '#'])
            .unwrap_or(after_scheme.len());
        let host = &after_scheme[..host_end];
        return format!("{}://{}/...", &uri[..scheme_end], host);
    }
    // Schemes without an authority (mailto:, tel:, javascript:): keep
    // only the part before the first separator.
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
}

#[interface(name = "org.freedesktop.impl.portal.OpenURI")]
impl OpenUri {
    /// Open a URI in the user's preferred handler. F1 stub.
    ///
    /// `#[zbus(name = "OpenURI")]` overrides the auto-PascalCase that
    /// would turn `open_uri` into `OpenUri`. The freedesktop spec
    /// declares the method as `OpenURI` and the frontend daemon
    /// dispatches by exact method name.
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
        tracing::warn!(
            request = %req.path,
            app_id,
            parent_window,
            uri = %redact_uri(uri),
            "OpenURI (F1 stub: backend not implemented, returning OTHER)"
        );
        // Full URI only at trace level for explicit debug sessions.
        tracing::trace!(uri, "OpenURI full URI (trace only)");
        (response::OTHER, stub_results("OpenURI"))
    }

    /// Open a file descriptor in the user's preferred handler. F1 stub.
    async fn open_file(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        _fd: Fd<'_>,
        _options: HashMap<&str, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let _guard = self.state.track_request();
        let req = RequestHandle::from_object_path(handle.into());
        tracing::warn!(
            request = %req.path,
            app_id,
            parent_window,
            "OpenFile (F1 stub: backend not implemented, returning OTHER)"
        );
        (response::OTHER, stub_results("OpenFile"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_results_carries_sentinel() {
        let r = stub_results("OpenURI");
        assert!(r.contains_key(STUB_ERROR_KEY));
    }

    /// http(s) URIs keep scheme + host but drop everything after the
    /// host. Real-world URIs that motivated this: signed download
    /// links and OAuth callbacks that carry secrets in the query.
    #[test]
    fn redact_strips_path_and_query() {
        assert_eq!(
            redact_uri("https://example.com/secret?token=abc#frag"),
            "https://example.com/..."
        );
        assert_eq!(
            redact_uri("http://api.example.com:8443/path"),
            "http://api.example.com:8443/..."
        );
    }

    /// Schemes without an authority (mailto:, tel:, javascript:)
    /// collapse to `scheme:...`. The body of a `mailto:` URL is
    /// already a privacy concern (recipient list).
    #[test]
    fn redact_collapses_authorityless_schemes() {
        assert_eq!(redact_uri("mailto:secret@example.com"), "mailto:...");
        assert_eq!(redact_uri("tel:+1-555-0100"), "tel:...");
    }

    #[test]
    fn redact_handles_empty_and_garbage() {
        assert_eq!(redact_uri(""), "<unparseable>");
        assert_eq!(redact_uri("not-a-uri"), "<unparseable>");
    }
}
