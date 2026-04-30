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
            uri,
            "OpenURI (F1 stub: backend not implemented, returning OTHER)"
        );
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
}
