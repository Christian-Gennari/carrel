//! Import `.md`/`.txt` files from a drive into `<writing_root>/Imported/`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::util::{collect_files, create_dir_all, is_writing_file, unique_path};
use crate::{Error, Result};

/// One file copied into the writing root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedFile {
    pub source: PathBuf,
    pub destination: PathBuf,
}

/// Result of an import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// `<writing_root>/Imported/`.
    pub destination: PathBuf,
    pub files: Vec<ImportedFile>,
    /// Paths that were not `.md`/`.txt` (or not regular files).
    pub skipped: Vec<PathBuf>,
}

/// Copy the given files (or every writing file below a given folder) into
/// `<writing_root>/Imported/`. `.txt` files become `.md`; nothing is ever
/// overwritten — a second `name.md` becomes `name (2).md`.
///
/// Folder sources keep their internal structure (without the source folder's
/// own name), so a USB folder `Notes/` with `Notes/a.md` and `Notes/Old/b.txt`
/// imports as `Imported/a.md` and `Imported/Old/b.md`.
pub fn import(src_paths: &[PathBuf], writing_root: &Path) -> Result<ImportReport> {
    let writing_root = match writing_root.canonicalize() {
        Ok(resolved) if resolved.is_dir() => resolved,
        Ok(_) => return Err(Error::InvalidDestination(writing_root.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::NotFound(writing_root.to_path_buf()));
        }
        Err(error) => return Err(Error::io(writing_root, error)),
    };
    let imported_root = writing_root.join("Imported");

    let mut report = ImportReport {
        destination: imported_root.clone(),
        files: Vec::new(),
        skipped: Vec::new(),
    };
    let mut created_root = false;

    for src in src_paths {
        let resolved = match src.canonicalize() {
            Ok(resolved) => resolved,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::NotFound(src.clone()));
            }
            Err(error) => return Err(Error::io(src, error)),
        };
        let metadata = fs::metadata(&resolved).map_err(|error| Error::io(&resolved, error))?;
        if metadata.is_dir() {
            for source in collect_files(&resolved)? {
                if !is_writing_file(&source) {
                    report.skipped.push(source);
                    continue;
                }
                let relative = source
                    .strip_prefix(&resolved)
                    .map_err(|_| Error::OutsideWritingRoot(source.clone()))?
                    .to_path_buf();
                import_one(
                    &source,
                    &relative,
                    &imported_root,
                    &mut created_root,
                    &mut report,
                )?;
            }
        } else if metadata.is_file() && is_writing_file(&resolved) {
            let relative = PathBuf::from(
                resolved
                    .file_name()
                    .ok_or_else(|| Error::InvalidSelection(src.clone()))?,
            );
            import_one(
                &resolved,
                &relative,
                &imported_root,
                &mut created_root,
                &mut report,
            )?;
        } else {
            report.skipped.push(src.clone());
        }
    }

    Ok(report)
}

fn import_one(
    source: &Path,
    relative: &Path,
    imported_root: &Path,
    created_root: &mut bool,
    report: &mut ImportReport,
) -> Result<()> {
    if !*created_root {
        create_dir_all(imported_root)?;
        *created_root = true;
    }

    let stem = relative
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| Error::InvalidSelection(relative.to_path_buf()))?;
    let directory = match relative.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => imported_root.join(parent),
        _ => imported_root.to_path_buf(),
    };
    create_dir_all(&directory)?;

    let destination = unique_path(&directory, &format!("{stem}.md"), false);
    fs::copy(source, &destination).map_err(|error| Error::io(&destination, error))?;
    report.files.push(ImportedFile {
        source: source.to_path_buf(),
        destination,
    });
    Ok(())
}
