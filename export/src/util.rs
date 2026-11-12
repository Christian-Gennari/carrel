//! Filesystem helpers shared by export and import.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Whether a file belongs to the writing root: `.md` or `.txt`, either case.
pub(crate) fn is_writing_file(path: &Path) -> bool {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) => {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("txt")
        }
        None => false,
    }
}

/// All regular files below `root`, sorted, without following symlinks and
/// without hidden entries (`.stylos/`, `.git/`, …).
pub(crate) fn collect_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    walk(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(directory).map_err(|error| Error::io(directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| Error::io(directory, error))?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| Error::io(&path, error))?;
        if file_type.is_dir() {
            walk(&path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
        // Symlinks and special files are never writing files: skipping them
        // also keeps a link from pulling content from outside the writing root
        // into an export.
    }
    Ok(())
}

pub(crate) fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|error| Error::io(path, error))
}

/// Add ` (2)`, ` (3)`, … to `name` until nothing exists at that path. Used for
/// both files and folders; a name that already exists is never overwritten.
pub(crate) fn unique_path(directory: &Path, name: &str, is_dir: bool) -> PathBuf {
    let first = directory.join(name);
    if !first.exists() {
        return first;
    }

    let (stem, extension) = if is_dir {
        (name.to_string(), None)
    } else {
        let path = Path::new(name);
        match (path.file_stem(), path.extension()) {
            (Some(stem), Some(extension)) => (
                stem.to_string_lossy().into_owned(),
                Some(extension.to_string_lossy().into_owned()),
            ),
            _ => (name.to_string(), None),
        }
    };

    let mut number = 2u32;
    loop {
        let candidate = match &extension {
            Some(extension) => directory.join(format!("{stem} ({number}).{extension}")),
            None => directory.join(format!("{stem} ({number})")),
        };
        if !candidate.exists() {
            return candidate;
        }
        number = number.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writing_files_are_markdown_or_text() {
        assert!(is_writing_file(Path::new("a.md")));
        assert!(is_writing_file(Path::new("a.MD")));
        assert!(is_writing_file(Path::new("a.txt")));
        assert!(!is_writing_file(Path::new("a.pdf")));
        assert!(!is_writing_file(Path::new("README")));
    }

    #[test]
    fn unique_path_keeps_existing_files() {
        let temp = tempfile::tempdir().unwrap();
        let first = unique_path(temp.path(), "chapter.md", false);
        assert_eq!(first, temp.path().join("chapter.md"));
        fs::write(&first, "one").unwrap();

        let second = unique_path(temp.path(), "chapter.md", false);
        assert_eq!(second, temp.path().join("chapter (2).md"));
        fs::write(&second, "two").unwrap();

        let third = unique_path(temp.path(), "chapter.md", false);
        assert_eq!(third, temp.path().join("chapter (3).md"));
        assert_eq!(fs::read_to_string(&first).unwrap(), "one");
    }

    #[test]
    fn unique_path_handles_folders_and_dots() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("Stylos export 2026-11-12")).unwrap();
        let next = unique_path(temp.path(), "Stylos export 2026-11-12", true);
        assert_eq!(next, temp.path().join("Stylos export 2026-11-12 (2)"));

        fs::write(temp.path().join("a.b.md"), "").unwrap();
        let next = unique_path(temp.path(), "a.b.md", false);
        assert_eq!(next, temp.path().join("a.b (2).md"));
    }

    #[test]
    fn collect_skips_hidden_and_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".stylos")).unwrap();
        fs::write(temp.path().join(".stylos/cache.md"), "x").unwrap();
        fs::create_dir_all(temp.path().join("Notes")).unwrap();
        fs::write(temp.path().join("Notes/a.md"), "x").unwrap();
        fs::write(temp.path().join("top.txt"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(temp.path().join("top.txt"), temp.path().join("link.md"))
            .unwrap();

        let files = collect_files(temp.path()).unwrap();
        assert_eq!(
            files,
            vec![temp.path().join("Notes/a.md"), temp.path().join("top.txt")]
        );
    }
}
