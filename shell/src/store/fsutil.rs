//! Filesystem helpers with the durability rules from the spec.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Write `bytes` to `path` atomically:
/// `name.ext.tmp` → fsync → rename → fsync the directory.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "file has no parent directory")
    })?;
    fs::create_dir_all(parent)?;

    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file has no name"))?;
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);

    let existing_permissions = fs::metadata(path).ok().map(|meta| meta.permissions());
    let result = (|| -> io::Result<()> {
        let mut file = File::create(&tmp)?;
        if let Some(permissions) = existing_permissions {
            let _ = file.set_permissions(permissions);
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        sync_dir(parent)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

/// Unique path inside `dir` for `stem.ext`: `stem.ext`, `stem 2.ext`, ...
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut candidate = join_name(dir, stem, ext);
    let mut counter = 2u32;
    while candidate.exists() {
        candidate = join_name(dir, &format!("{stem} {counter}"), ext);
        counter += 1;
    }
    candidate
}

fn join_name(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    if ext.is_empty() {
        dir.join(stem)
    } else {
        dir.join(format!("{stem}.{ext}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn atomic_write_replaces_and_leaves_no_tmp() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("note.md");
        atomic_write(&path, b"first").unwrap();
        atomic_write(&path, b"second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert!(!dir.path().join("note.md.tmp").exists());
    }

    #[test]
    fn unique_path_counts_up() {
        let dir = tempdir().unwrap();
        assert_eq!(
            unique_path(dir.path(), "Untitled", "md"),
            dir.path().join("Untitled.md")
        );
        fs::write(dir.path().join("Untitled.md"), "").unwrap();
        assert_eq!(
            unique_path(dir.path(), "Untitled", "md"),
            dir.path().join("Untitled 2.md")
        );
    }
}
