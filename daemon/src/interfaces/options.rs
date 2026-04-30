//! FileChooser portal option-dictionary parsing.
//!
//! The portal spec maps the `a{sv}` options dictionary to a fixed
//! set of well-known keys. Codex review #4 flagged that the F2.4
//! interface dropped almost all of them; this module reconstructs
//! the typed view so the picker UI receives the caller's intent.
//!
//! Types per the spec at
//! https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.impl.portal.FileChooser.html
//!
//! Out of scope for now: `accept_label` (custom button label,
//! cosmetic) and `choices` (extra checkboxes, rare). These get a
//! follow-up if a real first-party app starts using them.

use std::collections::HashMap;
use std::path::PathBuf;

use xdg_portal_lunaris_protocol::{FileFilter, FilterPattern};
use zbus::zvariant::{OwnedValue, Value};

/// Read a boolean option, defaulting to `default` if missing or of
/// the wrong type.
pub fn read_bool(options: &HashMap<&str, OwnedValue>, key: &str, default: bool) -> bool {
    options
        .get(key)
        .and_then(|v| v.downcast_ref::<bool>().ok())
        .unwrap_or(default)
}

/// Read a string option, returning `None` if missing or of the wrong
/// type. Empty strings are returned as `None` since the spec uses
/// "absent" and "empty" interchangeably for these keys.
pub fn read_string(options: &HashMap<&str, OwnedValue>, key: &str) -> Option<String> {
    options
        .get(key)
        .and_then(|v| v.downcast_ref::<&str>().ok())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Read a NUL-terminated byte-array path option. The portal spec
/// uses `ay` for filesystem paths because UTF-8 strings cannot
/// represent every legal byte sequence in a Unix filename.
pub fn read_path_bytes(options: &HashMap<&str, OwnedValue>, key: &str) -> Option<PathBuf> {
    let raw = options.get(key)?;
    let val: Value = raw.try_clone().ok()?.into();
    let bytes: Vec<u8> = val.try_into().ok()?;
    bytes_to_path(&bytes)
}

/// Read the `files` option (used by `SaveFiles`): an array of byte
/// arrays.
pub fn read_path_bytes_array(
    options: &HashMap<&str, OwnedValue>,
    key: &str,
) -> Vec<PathBuf> {
    let Some(raw) = options.get(key) else {
        return vec![];
    };
    let val: Value = match raw.try_clone() {
        Ok(v) => v.into(),
        Err(_) => return vec![],
    };
    let arrays: Vec<Vec<u8>> = match val.try_into() {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    arrays.iter().filter_map(|b| bytes_to_path(b)).collect()
}

/// Strip the trailing NUL byte and convert to a `PathBuf`. The spec
/// requires the trailing NUL; permitting it to be absent keeps us
/// resilient against a buggy frontend that re-encodes the dict.
fn bytes_to_path(bytes: &[u8]) -> Option<PathBuf> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let trimmed: &[u8] = match bytes.last() {
        Some(0) => &bytes[..bytes.len() - 1],
        _ => bytes,
    };
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(OsStr::from_bytes(trimmed)))
}

/// Convert a `(name, patterns)` D-Bus tuple to our typed
/// [`FileFilter`]. Drops any pattern with an unknown kind code,
/// since the spec only defines 0 (glob) and 1 (MIME).
fn tuple_to_filter(name: String, patterns: Vec<(u32, String)>) -> FileFilter {
    FileFilter {
        name,
        patterns: patterns
            .into_iter()
            .filter_map(|(kind, pattern)| match kind {
                0 => Some(FilterPattern::Glob { pattern }),
                1 => Some(FilterPattern::Mime { mime_type: pattern }),
                other => {
                    tracing::warn!(kind = other, "unknown filter kind, dropping");
                    None
                }
            })
            .collect(),
    }
}

/// Read the `filters` option: a list of named filters, each with a
/// list of `(type, pattern)` entries. Type 0 = glob, 1 = MIME.
pub fn read_filters(options: &HashMap<&str, OwnedValue>) -> Vec<FileFilter> {
    let Some(raw) = options.get("filters") else {
        return vec![];
    };
    let val: Value = match raw.try_clone() {
        Ok(v) => v.into(),
        Err(e) => {
            tracing::warn!("filters option clone failed: {e}");
            return vec![];
        }
    };
    // The wire shape is `a(sa(us))` which deserializes to
    // `Vec<(String, Vec<(u32, String)>)>`.
    let filters: Vec<(String, Vec<(u32, String)>)> = match val.try_into() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("filters option malformed: {e}");
            return vec![];
        }
    };
    filters
        .into_iter()
        .map(|(name, patterns)| tuple_to_filter(name, patterns))
        .collect()
}

/// Read the `current_filter` option: a single named filter that the
/// caller wants pre-selected.
pub fn read_current_filter(options: &HashMap<&str, OwnedValue>) -> Option<FileFilter> {
    let raw = options.get("current_filter")?;
    let val: Value = raw.try_clone().ok()?.into();
    let (name, patterns): (String, Vec<(u32, String)>) = match val.try_into() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("current_filter option malformed: {e}");
            return None;
        }
    };
    Some(tuple_to_filter(name, patterns))
}

/// Convert a `FileFilter` to its D-Bus value shape `(sa(us))` so it
/// can ride back to the caller in the result dictionary.
pub fn filter_to_value(filter: &FileFilter) -> Result<Value<'static>, zbus::zvariant::Error> {
    let patterns: Vec<(u32, String)> = filter
        .patterns
        .iter()
        .map(|p| match p {
            FilterPattern::Glob { pattern } => (0, pattern.clone()),
            FilterPattern::Mime { mime_type } => (1, mime_type.clone()),
        })
        .collect();
    let tuple = (filter.name.clone(), patterns);
    Ok(Value::from(tuple))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn opts(items: Vec<(&'static str, Value<'static>)>) -> HashMap<&'static str, OwnedValue> {
        items
            .into_iter()
            .map(|(k, v)| (k, v.try_to_owned().unwrap()))
            .collect()
    }

    /// Bool options round-trip; missing options take the default.
    #[test]
    fn bool_options() {
        let m = opts(vec![("multiple", Value::from(true))]);
        assert!(read_bool(&m, "multiple", false));
        assert!(!read_bool(&m, "directory", false));
        assert!(read_bool(&m, "modal", true));
    }

    /// String options strip empty strings to None so the picker UI
    /// does not show an empty filename or accept-label.
    #[test]
    fn string_options() {
        let m = opts(vec![
            ("current_name", Value::from("draft.txt".to_string())),
            ("accept_label", Value::from(String::new())),
        ]);
        assert_eq!(read_string(&m, "current_name"), Some("draft.txt".into()));
        assert_eq!(read_string(&m, "accept_label"), None);
        assert_eq!(read_string(&m, "missing"), None);
    }

    /// Path bytes strip the trailing NUL; bare paths without NUL are
    /// also accepted defensively.
    #[test]
    fn path_bytes_options() {
        let m = opts(vec![
            (
                "current_folder",
                Value::from(b"/home/user/Documents\0".to_vec()),
            ),
            ("no_nul", Value::from(b"/tmp/x".to_vec())),
        ]);
        assert_eq!(
            read_path_bytes(&m, "current_folder"),
            Some(PathBuf::from("/home/user/Documents"))
        );
        assert_eq!(read_path_bytes(&m, "no_nul"), Some(PathBuf::from("/tmp/x")));
        assert_eq!(read_path_bytes(&m, "missing"), None);
    }

    /// Filters round-trip through the parsing helper.
    #[test]
    fn filters_round_trip() {
        let m = opts(vec![(
            "filters",
            Value::from(vec![
                (
                    "Images".to_string(),
                    vec![
                        (0_u32, "*.png".to_string()),
                        (1_u32, "image/jpeg".to_string()),
                    ],
                ),
                ("All files".to_string(), vec![(0_u32, "*".to_string())]),
            ]),
        )]);
        let parsed = read_filters(&m);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "Images");
        assert_eq!(parsed[0].patterns.len(), 2);
        assert_eq!(
            parsed[0].patterns[0],
            FilterPattern::Glob {
                pattern: "*.png".into()
            }
        );
        assert_eq!(
            parsed[0].patterns[1],
            FilterPattern::Mime {
                mime_type: "image/jpeg".into()
            }
        );
    }

    /// Unknown filter kinds (anything except 0 or 1) are dropped
    /// rather than crashing or pretending they are something we
    /// understand.
    #[test]
    fn filters_drops_unknown_kinds() {
        let m = opts(vec![(
            "filters",
            Value::from(vec![(
                "Mixed".to_string(),
                vec![
                    (0_u32, "*.txt".to_string()),
                    (99_u32, "weird".to_string()),
                ],
            )]),
        )]);
        let parsed = read_filters(&m);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].patterns.len(), 1);
    }

    /// `current_filter` parses a single filter, matching the singular
    /// shape `(sa(us))` versus `filters` which is the plural array.
    #[test]
    fn current_filter_round_trip() {
        let m = opts(vec![(
            "current_filter",
            Value::from((
                "Documents".to_string(),
                vec![(0_u32, "*.pdf".to_string())],
            )),
        )]);
        let parsed = read_current_filter(&m).unwrap();
        assert_eq!(parsed.name, "Documents");
        assert_eq!(
            parsed.patterns,
            vec![FilterPattern::Glob {
                pattern: "*.pdf".into()
            }]
        );
    }

    /// `filter_to_value` is the output side: a FileFilter becomes
    /// a `(sa(us))` tuple ready for the result dict.
    #[test]
    fn filter_round_trip_through_value() {
        let original = FileFilter {
            name: "Images".into(),
            patterns: vec![
                FilterPattern::Glob {
                    pattern: "*.png".into(),
                },
                FilterPattern::Mime {
                    mime_type: "image/png".into(),
                },
            ],
        };
        let value = filter_to_value(&original).unwrap();
        // Convert through a HashMap key cycle to verify the wire
        // shape matches our parser.
        let m = opts(vec![("current_filter", value)]);
        let parsed = read_current_filter(&m).unwrap();
        assert_eq!(parsed, original);
    }
}
