//! The export pipeline: plan the files, then copy, convert or hand them to
//! pandoc.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use chrono::Local;

use crate::citations::{Citation, CitationLookup};
use crate::markdown::{citation_ids, markdown_to_plain};
use crate::pandoc::Pandoc;
use crate::util::{collect_files, create_dir_all, is_writing_file, unique_path};
use crate::{Error, Result};

/// What to export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// One `.md`/`.txt` file, absolute or relative to the writing root.
    File(PathBuf),
    /// One folder below the writing root, including its subfolders.
    Folder(PathBuf),
    /// The whole writing root.
    Everything,
}

/// Output format of an export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Markdown,
    PlainText,
    Docx,
    Odt,
    Pdf,
    Html,
    Epub,
}

impl Format {
    /// Every format the Export screen offers, in screen order.
    pub const ALL: [Format; 7] = [
        Format::Markdown,
        Format::PlainText,
        Format::Docx,
        Format::Odt,
        Format::Pdf,
        Format::Html,
        Format::Epub,
    ];

    /// File extension without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::PlainText => "txt",
            Format::Docx => "docx",
            Format::Odt => "odt",
            Format::Pdf => "pdf",
            Format::Html => "html",
            Format::Epub => "epub",
        }
    }

    /// Label for the Export screen.
    pub fn label(self) -> &'static str {
        match self {
            Format::Markdown => "Markdown",
            Format::PlainText => "Plain text",
            Format::Docx => "Word (.docx)",
            Format::Odt => "OpenDocument (.odt)",
            Format::Pdf => "PDF",
            Format::Html => "HTML",
            Format::Epub => "EPUB",
        }
    }

    /// Whether this format needs the pandoc binary.
    pub fn needs_pandoc(self) -> bool {
        !matches!(self, Format::Markdown | Format::PlainText)
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Called after each file has been written, for the Export screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// 1-based index of the file just written.
    pub index: usize,
    pub total: usize,
    pub source: PathBuf,
    pub destination: PathBuf,
}

/// One file that left the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedFile {
    pub source: PathBuf,
    pub destination: PathBuf,
}

/// Result of an export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    /// Directory that received the files: for [`Selection::Everything`] this is
    /// the dated `Carrel export YYYY-MM-DD/` folder.
    pub destination: PathBuf,
    pub format: Format,
    pub files: Vec<ExportedFile>,
}

impl ExportReport {
    /// Number of files written.
    pub fn count(&self) -> usize {
        self.files.len()
    }
}

/// Export the selection into `dest_dir` (a mounted USB drive, in the daemon).
///
/// Markdown is copied byte for byte; plain text is converted in Rust; every
/// other format goes through pandoc (PDF via `pandoc --pdf-engine=typst`).
/// `nostos:cite/` links become footnotes using `library`.
///
/// Existing files are never replaced: a second export of the same name gets
/// ` (2)`, then ` (3)`. `Selection::Everything` writes a dated
/// `Carrel export YYYY-MM-DD/` folder that keeps the folder structure.
pub fn export(
    selection: Selection,
    format: Format,
    writing_root: &Path,
    dest_dir: &Path,
    library: &dyn CitationLookup,
    mut progress: impl FnMut(Progress),
) -> Result<ExportReport> {
    let writing_root = canonical_directory(writing_root)?;
    let destination = canonical_directory(dest_dir)?;

    let plan = plan(&selection, &writing_root, &destination)?;
    let total = plan.files.len();
    let pandoc = if format.needs_pandoc() {
        Some(Pandoc::new()?)
    } else {
        None
    };

    let mut files = Vec::with_capacity(total);
    for (index, planned) in plan.files.into_iter().enumerate() {
        let output = output_path(&plan.root, &planned.relative, format)?;
        write_one(format, &planned.source, &output, pandoc.as_ref(), library)?;
        progress(Progress {
            index: index + 1,
            total,
            source: planned.source.clone(),
            destination: output.clone(),
        });
        files.push(ExportedFile {
            source: planned.source,
            destination: output,
        });
    }

    Ok(ExportReport {
        destination: plan.root,
        format,
        files,
    })
}

struct Plan {
    /// Directory that receives the files.
    root: PathBuf,
    files: Vec<PlannedFile>,
}

struct PlannedFile {
    source: PathBuf,
    /// Path from the destination root, keeping the source folder structure.
    relative: PathBuf,
}

fn plan(selection: &Selection, writing_root: &Path, destination: &Path) -> Result<Plan> {
    match selection {
        Selection::File(path) => {
            let source = resolve(path, writing_root)?;
            if !source.is_file() {
                return Err(Error::InvalidSelection(path.clone()));
            }
            if !is_writing_file(&source) {
                return Err(Error::UnsupportedSource(source));
            }
            let name = PathBuf::from(
                source
                    .file_name()
                    .ok_or_else(|| Error::InvalidSelection(path.clone()))?,
            );
            Ok(Plan {
                root: destination.to_path_buf(),
                files: vec![PlannedFile {
                    source,
                    relative: name,
                }],
            })
        }

        Selection::Folder(path) => {
            let source = resolve(path, writing_root)?;
            if !source.is_dir() {
                return Err(Error::InvalidSelection(path.clone()));
            }
            let files = writing_files(collect_files(&source)?, &source, Some(destination))?;
            if files.is_empty() {
                return Err(Error::NothingToExport(source));
            }
            let name = source
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Export".to_string());
            let root = unique_path(destination, &name, true);
            create_dir_all(&root)?;
            Ok(Plan { root, files })
        }

        Selection::Everything => {
            let files = writing_files(
                collect_files(writing_root)?,
                writing_root,
                Some(destination),
            )?;
            if files.is_empty() {
                return Err(Error::NothingToExport(writing_root.to_path_buf()));
            }
            let name = format!("Carrel export {}", Local::now().format("%Y-%m-%d"));
            let root = unique_path(destination, &name, true);
            create_dir_all(&root)?;
            Ok(Plan { root, files })
        }
    }
}

/// Turn absolute paths into destination-relative writing files, dropping
/// everything that is not Markdown/text and everything below `exclude` (the
/// destination, if the user exported into the writing root).
fn writing_files(
    files: Vec<PathBuf>,
    base: &Path,
    exclude: Option<&Path>,
) -> Result<Vec<PlannedFile>> {
    let mut planned = Vec::new();
    for source in files {
        if !is_writing_file(&source) {
            continue;
        }
        if let Some(exclude) = exclude {
            if source.starts_with(exclude) {
                continue;
            }
        }
        let relative = source
            .strip_prefix(base)
            .map_err(|_| Error::OutsideWritingRoot(source.clone()))?
            .to_path_buf();
        planned.push(PlannedFile { source, relative });
    }
    Ok(planned)
}

fn write_one(
    format: Format,
    source: &Path,
    destination: &Path,
    pandoc: Option<&Pandoc>,
    library: &dyn CitationLookup,
) -> Result<()> {
    match format {
        Format::Markdown => {
            fs::copy(source, destination).map_err(|error| Error::io(destination, error))?;
        }
        Format::PlainText => {
            let source_is_plain_text = source
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("txt"));
            if source_is_plain_text {
                // Already plain text: copy it byte for byte.
                fs::copy(source, destination).map_err(|error| Error::io(destination, error))?;
            } else {
                let bytes = fs::read(source).map_err(|error| Error::io(source, error))?;
                let text = String::from_utf8_lossy(&bytes);
                fs::write(destination, markdown_to_plain(&text))
                    .map_err(|error| Error::io(destination, error))?;
            }
        }
        _ => {
            let bytes = fs::read(source).map_err(|error| Error::io(source, error))?;
            let text = String::from_utf8_lossy(&bytes);
            let mut citations: BTreeMap<String, Citation> = BTreeMap::new();
            for id in citation_ids(&text) {
                if let Some(citation) = library.lookup(&id) {
                    citations.insert(id, citation);
                }
            }
            let pandoc = pandoc.ok_or(Error::MissingTool {
                tool: "pandoc",
                format,
            })?;
            let title = source
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            pandoc.convert(format, source, destination, &title, &citations)?;
        }
    }
    Ok(())
}

fn output_path(root: &Path, relative: &Path, format: Format) -> Result<PathBuf> {
    let stem = relative
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .ok_or_else(|| Error::InvalidSelection(relative.to_path_buf()))?;
    let directory = match relative.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => root.join(parent),
        _ => root.to_path_buf(),
    };
    create_dir_all(&directory)?;
    Ok(unique_path(
        &directory,
        &format!("{stem}.{}", format.extension()),
        false,
    ))
}

fn canonical_directory(path: &Path) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(resolved) if resolved.is_dir() => Ok(resolved),
        Ok(_) => Err(Error::InvalidDestination(path.to_path_buf())),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            Err(Error::NotFound(path.to_path_buf()))
        }
        Err(error) => Err(Error::io(path, error)),
    }
}

fn resolve(path: &Path, writing_root: &Path) -> Result<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        writing_root.join(path)
    };
    let resolved = match candidate.canonicalize() {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(Error::NotFound(path.to_path_buf()));
        }
        Err(error) => return Err(Error::io(&candidate, error)),
    };
    if !resolved.starts_with(writing_root) {
        return Err(Error::OutsideWritingRoot(path.to_path_buf()));
    }
    Ok(resolved)
}
