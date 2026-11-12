//! USB export and import for the daemon, and the WP6b `.docx` hook.
//!
//! The Export screen lists removable drives, then runs one export or import
//! against one drive. The daemon mounts the drive only for that operation and
//! unmounts and powers it off afterwards, so the writer can pull the stick as
//! soon as the screen says "Safe to remove" (`spec/01-architecture.md`).
//!
//! Everything here is GTK-free and synchronous: [`crate::daemon`] calls it
//! from `spawn_blocking` so a slow pandoc run never stalls the D-Bus object.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use carrel_export::{DriveInfo, Drives, FakeDrives, Format, MapCitations, Selection, UDisksDrives};
use serde::{Deserialize, Serialize};

use crate::gdrive::DocxConverter;

/// Test hook: replace udisks2 with one [`FakeDrives`] drive whose mount points
/// live below this directory. The drive mounts at `<dir>/drive-0`.
pub const FAKE_DRIVES_ENV: &str = "CARREL_FAKE_DRIVES_DIR";

/// Id of the fake drive used by tests and the UI's fake-drive flow.
pub const FAKE_DRIVE_ID: &str = "fake-drive";
/// Name the Export screen shows for the fake drive.
pub const FAKE_DRIVE_NAME: &str = "Fake USB drive";

/// A removable drive as the Export screen sees it.
///
/// This is the D-Bus-friendly projection of [`carrel_export::DriveInfo`]:
/// no `PathBuf`, only strings, and a plain `mounted` flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct UsbDrive {
    /// Opaque id to pass back to `Export` and `Import`.
    pub id: String,
    /// Label, else vendor/model, else the device node.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Whether the drive is mounted right now.
    pub mounted: bool,
    /// First mount point, or `""` when the drive is not mounted.
    pub mount_point: String,
}

impl From<DriveInfo> for UsbDrive {
    fn from(drive: DriveInfo) -> Self {
        let mounted = drive.is_mounted();
        let mount_point = drive
            .mount_point()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        Self {
            id: drive.id,
            name: drive.name,
            size: drive.size,
            mounted,
            mount_point,
        }
    }
}

/// List the drives attached to this device. Listing never mounts anything.
pub fn list_drives(drives: &dyn Drives) -> Result<Vec<UsbDrive>> {
    let drives = drives.list().context("list removable drives")?;
    Ok(drives.into_iter().map(UsbDrive::from).collect())
}

/// Parse the format string of the `Export` D-Bus call.
///
/// Accepts the short names the shell uses (`md`, `txt`, `docx`, …) and the
/// labels of the Export screen, so a caller is never surprised by capital
/// letters or a label.
pub fn parse_format(name: &str) -> Result<Format> {
    let key = name.trim().to_ascii_lowercase();
    let format = match key.as_str() {
        "md" | "markdown" => Format::Markdown,
        "txt" | "text" | "plain" | "plaintext" | "plain text" | "text/plain" => Format::PlainText,
        "docx" | "word" | "word (.docx)" | "word .docx" => Format::Docx,
        "odt" | "opendocument" | "opendocument (.odt)" | "opendocument .odt" => Format::Odt,
        "pdf" => Format::Pdf,
        "html" | "htm" => Format::Html,
        "epub" => Format::Epub,
        other => {
            bail!("unknown export format {other:?}; use one of md, txt, docx, odt, pdf, html, epub")
        }
    };
    Ok(format)
}

/// Map the `Export` D-Bus selection to a [`carrel_export::Selection`].
///
/// - no paths: everything in the writing root;
/// - one path: that file or folder, absolute or relative to the writing root.
///
/// The export crate resolves and checks the path against the writing root, so
/// the shell can never export a file from elsewhere on the system.
pub fn selection_from_request(request: &[String], writing_dir: &Path) -> Result<Selection> {
    match request {
        [] => Ok(Selection::Everything),
        [path] => {
            let path = path.trim();
            if path.is_empty() {
                bail!("the export selection is empty");
            }
            // The shell may spell out "everything" instead of sending an empty
            // list; both mean the whole writing root.
            if path.eq_ignore_ascii_case("everything") || path.eq_ignore_ascii_case("all") {
                return Ok(Selection::Everything);
            }
            let path = PathBuf::from(path);
            let candidate = if path.is_absolute() {
                path.clone()
            } else {
                writing_dir.join(&path)
            };
            let metadata = fs::metadata(&candidate)
                .with_context(|| format!("read the export selection {}", candidate.display()))?;
            if metadata.is_dir() {
                Ok(Selection::Folder(path))
            } else if metadata.is_file() {
                Ok(Selection::File(path))
            } else {
                bail!("{} is neither a file nor a folder", candidate.display())
            }
        }
        _ => bail!("export takes one file, one folder, or no path for everything"),
    }
}

/// Resolve the `Import` D-Bus paths against the mounted drive.
///
/// No paths means "everything on the drive". Every path must stay below the
/// mount point: the daemon never copies a file from elsewhere on the system,
/// even if the shell's selection is wrong.
pub fn import_sources(mount_point: &Path, paths: &[String]) -> Result<Vec<PathBuf>> {
    let root = mount_point
        .canonicalize()
        .with_context(|| format!("read the mount point {}", mount_point.display()))?;
    if paths.is_empty() {
        return Ok(vec![root]);
    }
    let mut sources = Vec::with_capacity(paths.len());
    for path in paths {
        let path = path.trim();
        if path.is_empty() {
            bail!("the import path is empty");
        }
        let path = Path::new(path);
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        let resolved = candidate
            .canonicalize()
            .with_context(|| format!("read {}", candidate.display()))?;
        if !resolved.starts_with(&root) {
            bail!("{} is outside the drive", resolved.display());
        }
        sources.push(resolved);
    }
    Ok(sources)
}

/// A drive mounted for exactly one export or import.
///
/// Dropping it unmounts and powers the drive off, also on an error or panic, so
/// a failed export never leaves a stick mounted.
pub struct MountedDrive {
    drives: Arc<dyn Drives>,
    id: String,
    path: PathBuf,
}

impl MountedDrive {
    pub fn mount(drives: Arc<dyn Drives>, id: &str) -> Result<Self> {
        let path = drives
            .mount(id)
            .with_context(|| format!("mount drive {id:?}"))?;
        Ok(Self {
            drives,
            id: id.to_string(),
            path,
        })
    }

    /// Where the drive is mounted.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for MountedDrive {
    fn drop(&mut self) {
        if let Err(error) = self.drives.unmount(&self.id) {
            tracing::warn!(%error, drive = %self.id, "could not unmount the drive");
        }
        if let Err(error) = self.drives.power_off(&self.id) {
            tracing::warn!(%error, drive = %self.id, "could not power off the drive");
        }
    }
}

/// The `Drives` implementation the daemon starts with.
///
/// Tests set [`FAKE_DRIVES_ENV`] to get a fake drive. Otherwise this uses
/// udisks2; when the system bus is unreachable (a container, or a broken
/// image) the daemon still runs and the Export screen just shows no drive.
pub fn system_drives() -> Arc<dyn Drives> {
    if let Some(directory) = std::env::var_os(FAKE_DRIVES_ENV) {
        let drive = DriveInfo {
            id: FAKE_DRIVE_ID.to_string(),
            name: FAKE_DRIVE_NAME.to_string(),
            size: 64 * 1024 * 1024,
            mount_points: Vec::new(),
        };
        return Arc::new(FakeDrives::new(PathBuf::from(directory), vec![drive]));
    }
    match UDisksDrives::new() {
        Ok(drives) => Arc::new(drives),
        Err(error) => {
            tracing::warn!(
                %error,
                "udisks2 is unavailable; the Export screen will show no drives"
            );
            Arc::new(NoDrives)
        }
    }
}

/// Used when udisks2 is unreachable. Listing is empty; mounting is an error
/// with a message the Export screen can show.
#[derive(Debug, Default)]
pub struct NoDrives;

impl Drives for NoDrives {
    fn list(&self) -> carrel_export::Result<Vec<DriveInfo>> {
        Ok(Vec::new())
    }

    fn mount(&self, _id: &str) -> carrel_export::Result<PathBuf> {
        Err(carrel_export::Error::Drive(
            "no removable-drive service is available".into(),
        ))
    }

    fn unmount(&self, _id: &str) -> carrel_export::Result<()> {
        Ok(())
    }

    fn power_off(&self, _id: &str) -> carrel_export::Result<()> {
        Ok(())
    }
}

/// The real [`DocxConverter`] behind the Google Drive `.docx` copy (WP6b).
///
/// It runs the same pandoc pipeline as a `.docx` USB export, so the copy that
/// opens in Google Docs looks like every other Carrel `.docx`.
#[derive(Debug, Default)]
pub struct PandocDocxConverter;

impl DocxConverter for PandocDocxConverter {
    fn convert(&self, path: &str, markdown: &str) -> Result<Vec<u8>> {
        let directory = tempfile::Builder::new()
            .prefix("carrel-docx-")
            .tempdir()
            .context("create the .docx build folder")?;
        let writing = directory.path().join("writing");
        let output = directory.path().join("out");
        fs::create_dir_all(&writing).with_context(|| format!("create {}", writing.display()))?;
        fs::create_dir_all(&output).with_context(|| format!("create {}", output.display()))?;

        // The export pipeline takes a path below a writing root. Put the
        // Markdown under its own name so the document title matches the file.
        let name = docx_source_name(path);
        let source = writing.join(&name);
        fs::write(&source, markdown).with_context(|| format!("write {}", source.display()))?;

        let report = carrel_export::export(
            Selection::File(PathBuf::from(&name)),
            Format::Docx,
            &writing,
            &output,
            &MapCitations::new(),
            |_progress| {},
        )
        .with_context(|| format!("convert {path:?} to .docx"))?;

        let converted = report
            .files
            .first()
            .context("the .docx conversion produced no file")?;
        let bytes = fs::read(&converted.destination)
            .with_context(|| format!("read {}", converted.destination.display()))?;
        if bytes.is_empty() {
            bail!("the .docx conversion of {path:?} is empty");
        }
        Ok(bytes)
    }
}

/// File name to use for the temporary Markdown source of a `.docx` copy.
fn docx_source_name(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document.md");
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".txt") {
        name.to_string()
    } else {
        format!("{name}.md")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_accept_short_names_and_screen_labels() {
        assert_eq!(parse_format("markdown").unwrap(), Format::Markdown);
        assert_eq!(parse_format(" MD ").unwrap(), Format::Markdown);
        assert_eq!(parse_format("Plain text").unwrap(), Format::PlainText);
        assert_eq!(parse_format("word (.docx)").unwrap(), Format::Docx);
        assert_eq!(parse_format("Word .docx").unwrap(), Format::Docx);
        assert_eq!(parse_format("OpenDocument (.odt)").unwrap(), Format::Odt);
        assert_eq!(parse_format("OpenDocument .odt").unwrap(), Format::Odt);
        assert_eq!(parse_format("PDF").unwrap(), Format::Pdf);
        assert_eq!(parse_format("html").unwrap(), Format::Html);
        assert_eq!(parse_format("epub").unwrap(), Format::Epub);
        assert!(parse_format("rtf").is_err());
    }

    #[test]
    fn selection_is_everything_a_file_or_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Notes")).unwrap();
        fs::write(dir.path().join("draft.md"), "text").unwrap();

        assert_eq!(
            selection_from_request(&[], dir.path()).unwrap(),
            Selection::Everything
        );
        assert_eq!(
            selection_from_request(&["everything".into()], dir.path()).unwrap(),
            Selection::Everything
        );
        assert_eq!(
            selection_from_request(&["draft.md".into()], dir.path()).unwrap(),
            Selection::File(PathBuf::from("draft.md"))
        );
        assert_eq!(
            selection_from_request(&["Notes".into()], dir.path()).unwrap(),
            Selection::Folder(PathBuf::from("Notes"))
        );
        assert!(selection_from_request(&["missing.md".into()], dir.path()).is_err());
        assert!(selection_from_request(&["a.md".into(), "b.md".into()], dir.path()).is_err());
        assert!(selection_from_request(&[" ".into()], dir.path()).is_err());
    }

    #[test]
    fn import_paths_may_not_escape_the_drive() {
        let dir = tempfile::tempdir().unwrap();
        let mount = dir.path().join("drive");
        fs::create_dir_all(mount.join("Inbox")).unwrap();
        fs::write(mount.join("Inbox/letter.txt"), "hello").unwrap();
        fs::write(dir.path().join("outside.md"), "no").unwrap();

        assert_eq!(
            import_sources(&mount, &["Inbox/letter.txt".into()]).unwrap(),
            vec![mount.join("Inbox/letter.txt").canonicalize().unwrap()]
        );
        assert_eq!(
            import_sources(&mount, &[]).unwrap(),
            vec![mount.canonicalize().unwrap()]
        );
        let absolute = mount.join("Inbox/letter.txt").display().to_string();
        assert_eq!(import_sources(&mount, &[absolute]).unwrap().len(), 1);

        let escape = dir.path().join("outside.md").display().to_string();
        assert!(import_sources(&mount, &[escape]).is_err());
        assert!(import_sources(&mount, &["../outside.md".into()]).is_err());
        assert!(import_sources(&mount, &["".into()]).is_err());
    }

    #[test]
    fn docx_source_names_keep_the_markdown_name() {
        assert_eq!(docx_source_name("notes/a.md"), "a.md");
        assert_eq!(docx_source_name("notes/b.txt"), "b.txt");
        assert_eq!(docx_source_name("notes/c"), "c.md");
        assert_eq!(docx_source_name(""), "document.md");
    }

    #[test]
    fn the_docx_converter_produces_a_real_word_document() {
        let bytes = PandocDocxConverter
            .convert("notes/a.md", "# Hello\n\nA sentence with *emphasis*.\n")
            .expect("pandoc is in the dev image");
        assert!(bytes.starts_with(b"PK\x03\x04"), "not a zip: {bytes:?}");
        assert!(
            bytes.len() > 1000,
            "suspiciously small: {} bytes",
            bytes.len()
        );
    }
}
