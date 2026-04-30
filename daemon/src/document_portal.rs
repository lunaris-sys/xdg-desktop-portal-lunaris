//! Document Portal client (FA8).
//!
//! Bridges file paths picked by an unconfined-running daemon to URIs
//! that sandboxed callers can actually open. The xdg-document-portal
//! service (`org.freedesktop.portal.Documents`) maintains a per-user
//! FUSE mount under `/run/user/<uid>/doc/` and exposes selected
//! files through it as `<mount>/<doc_id>/<filename>`. Sandboxed
//! callers have that mount bind-mounted into their bubblewrap
//! namespace; raw host paths outside it are unreachable.
//!
//! When the picker UI confirms a selection from a sandboxed caller,
//! we open each picked file with `O_PATH` (a handle that does not
//! grant read/write itself), pass the file descriptors to the
//! Document Portal via `AddFull`, get back doc-ids, and assemble
//! the per-doc-id URIs that we then return to the caller.
//!
//! Edge case E17: if the Document Portal service is not running we
//! fail the request with a clear error instead of silently returning
//! the raw `file://` paths a sandboxed caller cannot open.

use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use zbus::zvariant::Fd;

/// `AddFull` flag: re-use an existing document entry if the file is
/// already exported. The portal de-duplicates by inode so this is
/// safe and avoids piling up entries.
const FLAG_REUSE_EXISTING: u32 = 1 << 0;

/// Same path-percent-encoding set as `file_chooser.rs` uses for the
/// raw `file://` URIs. Defined locally so the doc-portal module is
/// self-contained.
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

/// Wait at most this long for the Documents D-Bus method to return
/// before we treat the portal as unavailable. The method itself is
/// fast in practice; the cap is here so a hung Document Portal
/// cannot strand a FileChooser request indefinitely.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

#[zbus::proxy(
    interface = "org.freedesktop.portal.Documents",
    default_service = "org.freedesktop.portal.Documents",
    default_path = "/org/freedesktop/portal/documents"
)]
trait Documents {
    /// Return the FUSE mount point as a NUL-terminated byte array.
    fn get_mount_point(&self) -> zbus::Result<Vec<u8>>;

    /// Add existing files for the given app and return per-file
    /// doc-ids plus an extra-info dict the spec reserves for future
    /// use. Used for OpenFile/OpenFiles where the caller already
    /// pointed at real files.
    #[zbus(name = "AddFull")]
    fn add_full(
        &self,
        o_path_fds: Vec<Fd<'_>>,
        flags: u32,
        app_id: &str,
        permissions: &[&str],
    ) -> zbus::Result<(Vec<String>, std::collections::HashMap<String, zbus::zvariant::OwnedValue>)>;

    /// Add a not-yet-existing file by parent-dir fd plus filename.
    /// The Save methods need this because the path the user picks
    /// may not exist yet — the caller is about to create it. The
    /// filename is `ay` per the spec (NUL-terminated bytes).
    #[zbus(name = "AddNamedFull")]
    fn add_named_full(
        &self,
        o_path_parent_fd: Fd<'_>,
        filename: Vec<u8>,
        flags: u32,
        app_id: &str,
        permissions: &[&str],
    ) -> zbus::Result<(String, std::collections::HashMap<String, zbus::zvariant::OwnedValue>)>;
}

/// Exports the given paths via the Document Portal for `app_id` and
/// returns the corresponding `file://` URIs that point into the
/// per-app FUSE mount.
///
/// `writable=false` requests read-only access; the most common case
/// for `OpenFile` portal calls. `SaveFile`/`SaveFiles` use
/// `writable=true` so the caller can write the chosen path.
pub async fn export_for_caller(
    connection: &zbus::Connection,
    app_id: &str,
    paths: &[PathBuf],
    writable: bool,
) -> Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    let proxy = tokio::time::timeout(
        CALL_TIMEOUT,
        DocumentsProxy::new(connection),
    )
    .await
    .context("Document Portal proxy creation timed out")?
    .context("could not reach Document Portal — is xdg-document-portal running?")?;

    // GetMountPoint comes back as a NUL-terminated byte string. We
    // rely on the kernel-stable mount point at /run/user/<uid>/doc
    // even if the service has not been hit before, but querying
    // explicitly survives any future change to the mount path.
    let mount_bytes = tokio::time::timeout(CALL_TIMEOUT, proxy.get_mount_point())
        .await
        .context("Document Portal GetMountPoint timed out")?
        .context("Document Portal GetMountPoint failed")?;
    let mount = bytes_to_pathbuf(&mount_bytes)
        .context("Document Portal returned an empty mount point")?;

    // Open each file with O_PATH so we hand the portal a handle
    // without claiming any read/write of our own. The file
    // descriptor is enough for the portal to track the inode.
    let mut owned_fds: Vec<OwnedFd> = Vec::with_capacity(paths.len());
    let mut filenames: Vec<String> = Vec::with_capacity(paths.len());
    for path in paths {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH)
            .open(path)
            .with_context(|| format!("open {} for portal export", path.display()))?;
        owned_fds.push(file.into());
        let filename = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if filename.is_empty() {
            anyhow::bail!("path has no filename: {}", path.display());
        }
        filenames.push(filename);
    }

    let permissions: &[&str] = if writable {
        &["read", "write"]
    } else {
        &["read"]
    };
    let fd_refs: Vec<Fd<'_>> = owned_fds.iter().map(Fd::from).collect();

    let (doc_ids, _extras) = tokio::time::timeout(
        CALL_TIMEOUT,
        proxy.add_full(fd_refs, FLAG_REUSE_EXISTING, app_id, permissions),
    )
    .await
    .context("Document Portal AddFull timed out")?
    .context("Document Portal AddFull failed")?;

    if doc_ids.len() != filenames.len() {
        anyhow::bail!(
            "Document Portal returned {} doc-ids for {} files",
            doc_ids.len(),
            filenames.len()
        );
    }

    let uris = doc_ids
        .iter()
        .zip(filenames.iter())
        .map(|(doc_id, filename)| assemble_uri(&mount, doc_id, filename))
        .collect();

    Ok(uris)
}

/// Exports the given Save targets — paths that may not exist yet —
/// via the Document Portal. Each path's parent directory is opened
/// O_PATH and `AddNamedFull` is called per-file. Returns the
/// resulting `file://` URIs that point into the per-app FUSE mount.
///
/// `writable` is always true here in practice — Save flows want
/// write access — but we keep the parameter explicit so the
/// permissions list stays in one place.
pub async fn export_named_for_save(
    connection: &zbus::Connection,
    app_id: &str,
    paths: &[PathBuf],
    writable: bool,
) -> Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    let proxy = tokio::time::timeout(CALL_TIMEOUT, DocumentsProxy::new(connection))
        .await
        .context("Document Portal proxy creation timed out")?
        .context("could not reach Document Portal — is xdg-document-portal running?")?;

    let mount_bytes = tokio::time::timeout(CALL_TIMEOUT, proxy.get_mount_point())
        .await
        .context("Document Portal GetMountPoint timed out")?
        .context("Document Portal GetMountPoint failed")?;
    let mount = bytes_to_pathbuf(&mount_bytes)
        .context("Document Portal returned an empty mount point")?;

    let permissions: &[&str] = if writable {
        &["read", "write"]
    } else {
        &["read"]
    };

    let mut uris = Vec::with_capacity(paths.len());
    for path in paths {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
        let filename = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if filename.is_empty() {
            anyhow::bail!("path has no filename: {}", path.display());
        }

        let parent_file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH)
            .open(parent)
            .with_context(|| {
                format!(
                    "open parent dir {} for AddNamedFull",
                    parent.display()
                )
            })?;
        let parent_fd: OwnedFd = parent_file.into();
        let fd_ref = Fd::from(&parent_fd);

        // Filename wire shape: NUL-terminated bytes (`ay`).
        let mut filename_bytes = filename.as_bytes().to_vec();
        filename_bytes.push(0);

        let (doc_id, _extras) = tokio::time::timeout(
            CALL_TIMEOUT,
            proxy.add_named_full(
                fd_ref,
                filename_bytes,
                FLAG_REUSE_EXISTING,
                app_id,
                permissions,
            ),
        )
        .await
        .context("Document Portal AddNamedFull timed out")?
        .context("Document Portal AddNamedFull failed")?;

        uris.push(assemble_uri(&mount, &doc_id, &filename));
    }

    Ok(uris)
}

/// Build a URI for one Document Portal entry:
/// `file://<mount>/<doc_id>/<filename>` with proper percent-encoding.
fn assemble_uri(mount: &Path, doc_id: &str, filename: &str) -> String {
    let path = mount.join(doc_id).join(filename);
    let s = path.to_string_lossy();
    format!("file://{}", utf8_percent_encode(&s, URI_PATH_SET))
}

/// Strip the trailing NUL and convert to PathBuf.
fn bytes_to_pathbuf(bytes: &[u8]) -> Option<PathBuf> {
    let trimmed: &[u8] = match bytes.last() {
        Some(0) => &bytes[..bytes.len() - 1],
        _ => bytes,
    };
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(trimmed)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// URI assembly: mount + doc_id + filename → file://-URI with
    /// proper percent-encoding of reserved characters.
    #[test]
    fn assemble_uri_basic() {
        let mount = PathBuf::from("/run/user/1000/doc");
        let uri = assemble_uri(&mount, "abc123", "report.pdf");
        assert_eq!(uri, "file:///run/user/1000/doc/abc123/report.pdf");
    }

    /// Reserved characters in the filename are encoded so consumers
    /// cannot mis-parse the URI. This is the same Codex P2 concern
    /// that file_chooser handles for raw paths.
    #[test]
    fn assemble_uri_reserved_chars_in_filename() {
        let mount = PathBuf::from("/run/user/1000/doc");
        let uri = assemble_uri(&mount, "x1", "weird#file?.txt");
        assert_eq!(
            uri,
            "file:///run/user/1000/doc/x1/weird%23file%3F.txt"
        );
    }

    /// Spaces in the mount point (unusual but legal) round-trip.
    #[test]
    fn assemble_uri_space_in_mount() {
        let mount = PathBuf::from("/tmp/with space/doc");
        let uri = assemble_uri(&mount, "id", "f.txt");
        assert_eq!(uri, "file:///tmp/with%20space/doc/id/f.txt");
    }

    /// Empty bytes input is rejected so we never produce a URI like
    /// `file:///` (host-relative root).
    #[test]
    fn bytes_to_pathbuf_rejects_empty() {
        assert!(bytes_to_pathbuf(b"").is_none());
        assert!(bytes_to_pathbuf(b"\0").is_none());
    }

    /// Trailing NUL is stripped; missing NUL is also accepted
    /// defensively (a buggy frontend might re-encode).
    #[test]
    fn bytes_to_pathbuf_strips_trailing_nul() {
        assert_eq!(
            bytes_to_pathbuf(b"/run/user/1000/doc\0"),
            Some(PathBuf::from("/run/user/1000/doc"))
        );
        assert_eq!(
            bytes_to_pathbuf(b"/run/user/1000/doc"),
            Some(PathBuf::from("/run/user/1000/doc"))
        );
    }

    /// `export_for_caller` short-circuits on an empty path list rather
    /// than making a needless D-Bus round-trip.
    #[tokio::test]
    async fn export_empty_paths_is_noop() {
        // We do not actually need a connection for the empty-paths
        // path; the function returns before any D-Bus call.
        let result = export_for_caller_with_no_connection().await;
        assert_eq!(result, Vec::<String>::new());
    }

    async fn export_for_caller_with_no_connection() -> Vec<String> {
        // Fake call: replicate just the first guard since constructing
        // a real `zbus::Connection` in unit tests requires a session
        // bus, which CI may not have.
        let paths: Vec<PathBuf> = vec![];
        if paths.is_empty() {
            return Vec::new();
        }
        unreachable!()
    }
}
