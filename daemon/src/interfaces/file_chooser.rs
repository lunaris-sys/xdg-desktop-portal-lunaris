//! `org.freedesktop.impl.portal.FileChooser` implementation.
//!
//! Three methods: `OpenFile`, `SaveFile`, `SaveFiles`. F1 stubs them
//! all to return `(OTHER, {error: "..."})` so the wire round-trip is
//! testable without UI integration AND the failure mode is honest —
//! callers and logs see a backend-defect, not a user-cancelled action.
//! F2 connects them to the picker-ui subprocess and the Document Portal.
//!
//! Spec:
//! https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.impl.portal.FileChooser.html

use std::collections::HashMap;

use zbus::interface;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::request::{response, RequestHandle};
use crate::state::DaemonState;

/// Sentinel error key the F1 stubs put in the results-dict so anything
/// downstream (logs, debug toasts, integration tests) can tell a real
/// user-cancel apart from a backend-not-yet-implemented case.
const STUB_ERROR_KEY: &str = "lunaris-stub-error";

/// Build a results-dict that flags the F1-stub state. Returns
/// `HashMap<String, OwnedValue>` because that is the spec-required
/// shape for FileChooser results.
fn stub_results(method: &str) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    let val = format!("F1 stub for {method}; backend not yet implemented");
    if let Ok(owned) = Value::new(val).try_to_owned() {
        map.insert(STUB_ERROR_KEY.to_string(), owned);
    }
    map
}

/// FileChooser1 interface. Cheap to clone — `state` is an `Arc` inside.
#[derive(Clone)]
pub struct FileChooser {
    state: DaemonState,
}

impl FileChooser {
    pub fn new(state: DaemonState) -> Self {
        Self { state }
    }
}

#[interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooser {
    /// Open one or more existing files for reading.
    ///
    /// F1 stub: returns `OTHER` (response code 2, i.e. backend failure)
    /// with a sentinel `lunaris-stub-error` key in the results-dict so
    /// the failure mode is honest. `CANCELLED` would lie to the caller
    /// by claiming the user dismissed the dialog.
    async fn open_file(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        title: &str,
        _options: HashMap<&str, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let _guard = self.state.track_request();
        let req = RequestHandle::from_object_path(handle.into());
        tracing::warn!(
            request = %req.path,
            app_id,
            parent_window,
            title,
            "OpenFile (F1 stub: backend not implemented, returning OTHER)"
        );
        (response::OTHER, stub_results("OpenFile"))
    }

    /// Save a single file. F1 stub: see `open_file` for stub semantics.
    async fn save_file(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        title: &str,
        _options: HashMap<&str, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let _guard = self.state.track_request();
        let req = RequestHandle::from_object_path(handle.into());
        tracing::warn!(
            request = %req.path,
            app_id,
            parent_window,
            title,
            "SaveFile (F1 stub: backend not implemented, returning OTHER)"
        );
        (response::OTHER, stub_results("SaveFile"))
    }

    /// Save multiple files into a single directory. F1 stub.
    async fn save_files(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        title: &str,
        _options: HashMap<&str, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let _guard = self.state.track_request();
        let req = RequestHandle::from_object_path(handle.into());
        tracing::warn!(
            request = %req.path,
            app_id,
            parent_window,
            title,
            "SaveFiles (F1 stub: backend not implemented, returning OTHER)"
        );
        (response::OTHER, stub_results("SaveFiles"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sentinel key + payload land in the results dict so downstream
    /// observers can tell stubs from real cancels.
    #[test]
    fn stub_results_carries_sentinel() {
        let r = stub_results("OpenFile");
        assert!(r.contains_key(STUB_ERROR_KEY));
    }
}
