//! Filesystem-side Tauri commands the picker UI invokes from Svelte.
//!
//! All paths exchanged with the frontend are absolute. Relative-path
//! handling happens in Rust so the JS side never has to think about
//! the current working directory.
//!
//! Errors are stringified; the picker UI surfaces them as toasts
//! rather than dropping the user into an undefined state. Hidden
//! files are tagged but not filtered — that decision lives in the
//! frontend so the user can flip the toggle without re-reading the
//! directory.

use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_directory: bool,
    pub is_hidden: bool,
}

/// List the contents of `path`. Returns entries sorted directories-
/// first, then alphabetically (case-insensitive) — the conventional
/// file-manager order.
#[tauri::command]
pub async fn list_directory(path: PathBuf) -> Result<Vec<DirEntry>, String> {
    let canonical = match tokio::fs::canonicalize(&path).await {
        Ok(p) => p,
        Err(e) => return Err(format!("canonicalize {}: {e}", path.display())),
    };
    let mut reader = tokio::fs::read_dir(&canonical)
        .await
        .map_err(|e| format!("read_dir {}: {e}", canonical.display()))?;
    let mut entries: Vec<DirEntry> = Vec::new();
    loop {
        let entry = match reader.next_entry().await {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!("read_dir entry error: {e}");
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        // Skip the dot-self entries; they are noise in a picker.
        if name == "." || name == ".." {
            continue;
        }
        let is_hidden = name.starts_with('.');
        let file_type = entry.file_type().await;
        let is_directory = file_type.map(|t| t.is_dir()).unwrap_or(false);
        let path = entry.path();
        entries.push(DirEntry {
            name,
            path,
            is_directory,
            is_hidden,
        });
    }
    // Folder-first, then alphabetic case-insensitive — matches what
    // GNOME Files / Dolphin / macOS Finder do by default.
    entries.sort_by(|a, b| match (a.is_directory, b.is_directory) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(entries)
}

/// Resolve a starting directory: caller-provided if it exists and is
/// a directory, else `$HOME`, else `/`.
#[tauri::command]
pub async fn resolve_start_dir(provided: Option<PathBuf>) -> PathBuf {
    if let Some(p) = provided {
        if let Ok(meta) = tokio::fs::metadata(&p).await {
            if meta.is_dir() {
                return p;
            }
        }
    }
    home_or_root()
}

fn home_or_root() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home);
        if p.is_dir() {
            return p;
        }
    }
    PathBuf::from("/")
}

/// Return the parent directory of `path`, or `None` for the root.
#[tauri::command]
pub fn parent_dir(path: PathBuf) -> Option<PathBuf> {
    path.parent().map(Path::to_path_buf)
}

/// True if `path` exists and is a regular file. Used for the
/// SaveFile overwrite-confirm dialog.
#[tauri::command]
pub async fn file_exists(path: PathBuf) -> bool {
    match tokio::fs::metadata(&path).await {
        Ok(meta) => meta.is_file(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Listing a tempdir returns entries with hidden flags set
    /// correctly and folders first.
    #[tokio::test]
    async fn lists_with_hidden_and_sort() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir(dir.join("zfolder")).unwrap();
        std::fs::create_dir(dir.join("afolder")).unwrap();
        std::fs::write(dir.join("zfile.txt"), b"x").unwrap();
        std::fs::write(dir.join("afile.txt"), b"x").unwrap();
        std::fs::write(dir.join(".hidden"), b"x").unwrap();

        let entries = list_directory(dir.to_path_buf()).await.unwrap();
        // Folders first (afolder, zfolder), then files (.hidden, afile, zfile)
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["afolder", "zfolder", ".hidden", "afile.txt", "zfile.txt"]
        );
        let hidden = entries
            .iter()
            .find(|e| e.name == ".hidden")
            .expect("hidden");
        assert!(hidden.is_hidden);
        let visible = entries
            .iter()
            .find(|e| e.name == "afile.txt")
            .expect("visible");
        assert!(!visible.is_hidden);
    }

    /// Caller-provided dir wins when it exists; falls back to $HOME.
    #[tokio::test]
    async fn resolve_start_dir_picks_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let resolved = resolve_start_dir(Some(tmp.path().to_path_buf())).await;
        assert_eq!(
            resolved.canonicalize().unwrap(),
            tmp.path().canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn resolve_start_dir_rejects_nonexistent() {
        let resolved = resolve_start_dir(Some(PathBuf::from("/no/such/dir"))).await;
        // Should fall back to HOME or /, not the bogus path.
        assert_ne!(resolved, PathBuf::from("/no/such/dir"));
    }

    /// Parent of `/foo/bar` is `/foo`; parent of `/` is None.
    #[test]
    fn parent_of() {
        assert_eq!(
            parent_dir(PathBuf::from("/home/user/Documents")),
            Some(PathBuf::from("/home/user"))
        );
        assert_eq!(parent_dir(PathBuf::from("/")), None);
    }

    #[tokio::test]
    async fn file_exists_yes_no() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x.txt");
        assert!(!file_exists(p.clone()).await);
        std::fs::write(&p, b"x").unwrap();
        assert!(file_exists(p).await);
    }
}
