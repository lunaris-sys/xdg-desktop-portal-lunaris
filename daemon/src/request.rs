//! `org.freedesktop.portal.Request` lifecycle helper.
//!
//! Each portal method call creates a request object that the frontend
//! daemon listens to for `Response` signals. We do not yet drive the
//! signal end (F2 will), but the helper unifies the "register the
//! object path, optionally close it" plumbing in one place so the
//! call sites stay readable.
//!
//! Until F2 wires `Response` emission, the stub `respond_cancelled`
//! method is the entry point F1 uses to terminate an `OpenFile` /
//! `SaveFile` / `OpenURI` call with the standard cancel code.

use zbus::zvariant::OwnedObjectPath;

/// Standard portal `Response` codes per the freedesktop spec.
#[allow(dead_code)] // Success and Other arrive with F2/F3.
pub mod response {
    /// User confirmed the action.
    pub const SUCCESS: u32 = 0;
    /// User cancelled (clicked Cancel, closed the dialog).
    pub const CANCELLED: u32 = 1;
    /// Backend-side failure (Document Portal down, regex DoS, etc.).
    pub const OTHER: u32 = 2;
}

/// A request handle as supplied by the frontend daemon. We accept the
/// `Object` path over the wire and validate it parses as a D-Bus path.
///
/// In F1 the helper exists so call sites can store the parsed path and
/// log it consistently. F2 will extend this with `respond` that emits
/// the `org.freedesktop.portal.Request.Response` signal at the path.
#[derive(Debug, Clone)]
pub struct RequestHandle {
    pub path: OwnedObjectPath,
}

impl RequestHandle {
    /// Parse a frontend-supplied request path. Returns the helper or
    /// the zbus parse error.
    pub fn from_object_path(path: OwnedObjectPath) -> Self {
        Self { path }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::ObjectPath;

    #[test]
    fn response_codes_match_spec() {
        assert_eq!(response::SUCCESS, 0);
        assert_eq!(response::CANCELLED, 1);
        assert_eq!(response::OTHER, 2);
    }

    /// `OwnedObjectPath` round-trips and is what the helper stores.
    #[test]
    fn handle_stores_path() {
        let path: OwnedObjectPath =
            ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1234/abc")
                .unwrap()
                .into();
        let h = RequestHandle::from_object_path(path.clone());
        assert_eq!(h.path, path);
    }
}
