//! Wire types for IPC between the daemon and the picker-ui Tauri app.
//!
//! Both processes serialize and deserialize the same message types. The
//! crate exists so a single source defines them — drift between the two
//! sides would silently corrupt picks.
//!
//! Frame format (used by both directions): 4-byte big-endian length, then
//! UTF-8 JSON body. Same as the notification daemon's broadcast socket.
//! Encode/decode helpers live in [`codec`].
//!
//! All types use `rename_all = "camelCase"` because the picker-ui side
//! crosses a Rust-TypeScript boundary inside Tauri.

pub mod codec;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Daemon -> picker-ui. The picker UI shows a dialog for the request and
/// eventually replies with a [`PickerResponse`] carrying the same `handle`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PickerRequest {
    /// Open one or more existing files for reading. Mirrors
    /// `org.freedesktop.impl.portal.FileChooser.OpenFile`.
    OpenFile {
        /// Unique correlation handle. The matching response carries the
        /// same value.
        handle: String,
        /// Caller-controlled string. Display only; do NOT trust for
        /// authorisation. Sandbox detection happens daemon-side via the
        /// caller's cgroup.
        app_id: String,
        title: String,
        /// File-extension or MIME filters. Empty array means "all files".
        filters: Vec<FileFilter>,
        /// Currently-active filter, if the caller pre-selected one.
        current_filter: Option<FileFilter>,
        /// Whether the user can pick more than one file.
        multiple: bool,
        /// Whether the picker should be modal relative to the parent
        /// window. Wayland has no cross-app modal concept; the flag is
        /// recorded but not enforced.
        modal: bool,
        /// Where the picker opens. Falls back to `$HOME` if absent or
        /// invalid (path traversal, non-existent directory, outside the
        /// caller's allowed roots).
        current_folder: Option<PathBuf>,
        /// Caller's parent window in `wayland:NNNN` or `x11:0xABCD` form.
        /// XWayland callers cannot be matched to a Wayland surface; the
        /// picker falls back to the focused output.
        parent_window: Option<String>,
    },
    /// Save a single file. Mirrors
    /// `org.freedesktop.impl.portal.FileChooser.SaveFile`.
    SaveFile {
        handle: String,
        app_id: String,
        title: String,
        filters: Vec<FileFilter>,
        current_filter: Option<FileFilter>,
        current_name: Option<String>,
        current_folder: Option<PathBuf>,
        current_file: Option<PathBuf>,
        parent_window: Option<String>,
    },
    /// Save multiple files into a single directory. Mirrors
    /// `org.freedesktop.impl.portal.FileChooser.SaveFiles`.
    SaveFiles {
        handle: String,
        app_id: String,
        title: String,
        files: Vec<PathBuf>,
        current_folder: Option<PathBuf>,
        parent_window: Option<String>,
    },
    /// Daemon-initiated cancellation, e.g. caller died (E2) or
    /// wall-clock timeout fired (E13). Picker UI hides immediately and
    /// does not respond.
    Cancel { handle: String },
}

/// Picker-ui -> daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PickerResponse {
    /// User confirmed; `paths` is non-empty.
    Picked {
        handle: String,
        paths: Vec<PathBuf>,
        /// Filter the user had selected at confirm time, if any. Echoed
        /// back to the caller via the `current_filter` result key.
        current_filter: Option<FileFilter>,
    },
    /// User dismissed the picker.
    Cancelled { handle: String },
    /// Picker UI hit a fatal error (filesystem access denied, regex DoS
    /// cap exceeded, etc.). Daemon converts this to an error response on
    /// the D-Bus side.
    Error { handle: String, message: String },
}

/// Filter declaration. Matches `org.freedesktop.impl.portal.FileChooser`
/// `filters` option type `a(sa(us))`: name plus a list of `(type, pattern)`
/// where `type` is 0 (glob) or 1 (mime).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFilter {
    /// Display name shown in the filter dropdown, e.g. "Images".
    pub name: String,
    /// Patterns. Each is either a glob (`*.png`) or a MIME type
    /// (`image/png`).
    pub patterns: Vec<FilterPattern>,
}

/// One filter pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum FilterPattern {
    /// Glob pattern like `*.png`. The picker UI matches against the file
    /// name only, not the full path.
    Glob { pattern: String },
    /// MIME type like `image/png`. The picker UI uses `xdg-mime` rules
    /// to derive the MIME from the file name; reading file content for
    /// magic-byte detection is intentionally avoided to keep listing
    /// fast on slow filesystems (E8).
    Mime { mime_type: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wire types must round-trip through JSON unchanged so the daemon
    /// and the picker-ui never disagree about field shapes.
    #[test]
    fn pick_request_round_trip() {
        let req = PickerRequest::OpenFile {
            handle: "h1".into(),
            app_id: "org.example.app".into(),
            title: "Open file".into(),
            filters: vec![FileFilter {
                name: "Images".into(),
                patterns: vec![
                    FilterPattern::Glob { pattern: "*.png".into() },
                    FilterPattern::Mime { mime_type: "image/png".into() },
                ],
            }],
            current_filter: None,
            multiple: false,
            modal: true,
            current_folder: Some(PathBuf::from("/home/example/Pictures")),
            parent_window: Some("wayland:42".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: PickerRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{req:?}"), format!("{back:?}"));
    }

    /// camelCase is required because the picker-ui frontend reads these
    /// directly from the IPC. Snake-case would silently produce undefined
    /// fields on the JS side.
    #[test]
    fn camel_case_field_names() {
        let req = PickerRequest::SaveFile {
            handle: "h2".into(),
            app_id: "".into(),
            title: "Save".into(),
            filters: vec![],
            current_filter: None,
            current_name: Some("draft.txt".into()),
            current_folder: None,
            current_file: None,
            parent_window: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"appId\""));
        assert!(json.contains("\"currentName\""));
        assert!(json.contains("\"parentWindow\""));
        assert!(!json.contains("app_id"));
        assert!(!json.contains("current_name"));
    }

    /// `Picked` carries paths and the filter the user had active at
    /// confirm time. Empty paths is invalid in practice but the type
    /// permits it for cleaner serde shape.
    #[test]
    fn picked_response_round_trip() {
        let resp = PickerResponse::Picked {
            handle: "h1".into(),
            paths: vec![PathBuf::from("/home/example/Pictures/logo.png")],
            current_filter: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: PickerResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{resp:?}"), format!("{back:?}"));
    }
}
