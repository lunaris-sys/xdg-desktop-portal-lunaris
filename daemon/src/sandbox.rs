// FA7 originally derived caller identity from `/proc/<pid>/cgroup`.
// In practice the impl-portal interface receives a frontend-verified
// `app_id` argument and Flatpak callers reach the bus via
// `xdg-dbus-proxy` (whose own cgroup is the user-session scope, not
// the bubblewrap scope), so cgroup detection silently misclassifies
// real Flatpak callers as Unconfined. The cgroup helpers below are
// kept for diagnostic use and a future fallback path that does not
// require D-Bus PID lookup, but no production code calls them today.
#![allow(dead_code)]

//! Caller sandbox / app-id detection (FA7).
//!
//! Portal callers can pass `app_id` in their method arguments, but the
//! value is caller-controlled and therefore untrusted. We derive the
//! real identity from the caller's cgroup, which the kernel maintains
//! and the caller cannot spoof.
//!
//! Supported formats:
//!
//! - Flatpak: `/user.slice/.../app-flatpak-<app_id>-<n>.scope`
//! - Snap (recognised, not exercised): `snap.<name>.<launcher>.scope`
//! - Anything else → `Unconfined`. The portal method handler then has
//!   to decide whether to grant the request based on user consent
//!   alone, since there is no sandbox boundary to honour.
//!
//! The cgroup is read from `/proc/<pid>/cgroup`, which exists on every
//! Linux ≥ 2.6.24 with cgroups enabled (always true on Lunaris).

use std::fs;
use std::path::Path;

/// Outcome of sandbox detection for a given caller PID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerIdentity {
    /// Flatpak-confined caller. `app_id` is the Flatpak application
    /// id (`org.gnome.Calculator`, `com.spotify.Client`, ...).
    Flatpak { app_id: String },
    /// Snap-confined caller. `name` is the Snap package name.
    Snap { name: String },
    /// Anything else: native binary, systemd service, container we
    /// have not explicitly detected. The caller can do whatever the
    /// invoking user can do regardless of what app_id they pass.
    Unconfined,
    /// Identity could not be determined: D-Bus message had no
    /// sender header, `org.freedesktop.DBus` was unreachable, or
    /// PID-to-cgroup lookup failed. Authorization decisions that
    /// touch a security boundary (file:// access through the host)
    /// must fail-closed for this state — Codex review found that
    /// silently coalescing this into `Unconfined` would let a
    /// transient D-Bus glitch waive the sandbox check.
    Unknown,
}

impl CallerIdentity {
    /// Best-effort app-id string suitable for logs and Document
    /// Portal calls. `None` for unconfined callers and for the
    /// Unknown failure state.
    pub fn app_id(&self) -> Option<&str> {
        match self {
            CallerIdentity::Flatpak { app_id } => Some(app_id),
            CallerIdentity::Snap { name } => Some(name),
            CallerIdentity::Unconfined | CallerIdentity::Unknown => None,
        }
    }

    /// True when sandbox detection produced a definite answer
    /// (Flatpak / Snap / Unconfined). False only for Unknown.
    /// Callers that need to fail-closed on identity-resolution
    /// failures gate on this.
    pub fn is_known(&self) -> bool {
        !matches!(self, CallerIdentity::Unknown)
    }
}

/// Detect the identity of the process at `pid` by reading its cgroup.
///
/// Returns `Unconfined` for any cgroup that does not match a known
/// sandbox pattern; this is intentional — we prefer to under-report
/// confinement than to mis-attribute confinement to an attacker who
/// has crafted a misleading cgroup name.
pub fn detect(pid: u32) -> CallerIdentity {
    let path = format!("/proc/{pid}/cgroup");
    match fs::read_to_string(Path::new(&path)) {
        Ok(content) => parse_cgroup(&content),
        Err(_) => CallerIdentity::Unconfined,
    }
}

/// Parse a `/proc/<pid>/cgroup` payload. Public for test coverage.
pub fn parse_cgroup(content: &str) -> CallerIdentity {
    for line in content.lines() {
        if let Some(id) = match_flatpak(line) {
            return CallerIdentity::Flatpak { app_id: id };
        }
        if let Some(name) = match_snap(line) {
            return CallerIdentity::Snap { name };
        }
    }
    CallerIdentity::Unconfined
}

/// Pull the Flatpak app-id out of a cgroup line. Flatpak's cgroup
/// format is `app-flatpak-<app_id>-<pid_or_random>.scope`.
fn match_flatpak(line: &str) -> Option<String> {
    let scope = line.rsplit('/').next()?;
    let stripped = scope.strip_suffix(".scope")?;
    let inner = stripped.strip_prefix("app-flatpak-")?;
    // The numeric suffix can be PID or any digits Flatpak uses for
    // disambiguation. We rsplit once and trim the leading dash.
    let (app_id, _suffix) = inner.rsplit_once('-')?;
    if app_id.is_empty() {
        return None;
    }
    Some(app_id.to_string())
}

/// Pull the Snap package name out of a cgroup line. Snap format is
/// `snap.<name>.<launcher>.scope`.
fn match_snap(line: &str) -> Option<String> {
    let scope = line.rsplit('/').next()?;
    let stripped = scope.strip_suffix(".scope")?;
    let inner = stripped.strip_prefix("snap.")?;
    // We want the part before the first `.`.
    let (name, _) = inner.split_once('.')?;
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-world Flatpak cgroup line from `flatpak run org.gnome.Calculator`.
    /// The cgroup file contains v1 and v2 entries on hybrid systems;
    /// only the unified entry has the `app-flatpak-` scope.
    #[test]
    fn detects_flatpak() {
        let content = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-org.gnome.Calculator-12345.scope\n";
        assert_eq!(
            parse_cgroup(content),
            CallerIdentity::Flatpak {
                app_id: "org.gnome.Calculator".into()
            }
        );
    }

    /// Snap cgroup pattern.
    #[test]
    fn detects_snap() {
        let content = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/snap.firefox.firefox.scope\n";
        assert_eq!(
            parse_cgroup(content),
            CallerIdentity::Snap {
                name: "firefox".into()
            }
        );
    }

    /// Plain user-session process — neither Flatpak nor Snap.
    #[test]
    fn unconfined_for_plain_user_session() {
        let content = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.gnome.Terminal-12345.scope\n";
        assert_eq!(parse_cgroup(content), CallerIdentity::Unconfined);
    }

    /// Empty file (cannot read /proc, or the process exited before we
    /// got there) collapses to Unconfined rather than panicking.
    #[test]
    fn empty_file_is_unconfined() {
        assert_eq!(parse_cgroup(""), CallerIdentity::Unconfined);
    }

    /// Malformed Flatpak scope (missing the trailing -<n>) does not
    /// produce a partial app-id — better unconfined than wrong.
    #[test]
    fn malformed_flatpak_is_unconfined() {
        let content = "0::/user.slice/.../app-flatpak.scope\n";
        assert_eq!(parse_cgroup(content), CallerIdentity::Unconfined);
    }

    /// `app_id()` returns the app id for confined callers, None
    /// for both unconfined and unknown — Unknown deliberately
    /// shares the no-app-id shape because it must not be treated
    /// as "has app id with unknown app".
    #[test]
    fn app_id_accessor() {
        assert_eq!(
            CallerIdentity::Flatpak {
                app_id: "x".into()
            }
            .app_id(),
            Some("x")
        );
        assert_eq!(CallerIdentity::Unconfined.app_id(), None);
        assert_eq!(CallerIdentity::Unknown.app_id(), None);
    }

    /// `is_known()` distinguishes the "we definitely couldn't
    /// determine" state from any successful classification.
    #[test]
    fn is_known_accessor() {
        assert!(CallerIdentity::Flatpak {
            app_id: "x".into()
        }
        .is_known());
        assert!(CallerIdentity::Snap {
            name: "y".into()
        }
        .is_known());
        assert!(CallerIdentity::Unconfined.is_known());
        assert!(!CallerIdentity::Unknown.is_known());
    }
}
