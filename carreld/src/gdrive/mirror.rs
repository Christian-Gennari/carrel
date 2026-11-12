//! The one-way mirror from `/data/writing` into the `Carrel` folder in Google
//! Drive.
//!
//! The device is the source of truth: local folders become Drive folders,
//! Markdown files become Drive files, only changed files are uploaded, and a
//! file that disappeared locally is moved to `Carrel/.deleted/` (never
//! hard-deleted). Google Drive edits never come back.
//!
//! Every step is idempotent and the state row for a file is written only after
//! its upload succeeded. A sync that dies halfway can simply run again.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use carrel_protocol::{content_hash, validate_path};

use crate::gdrive::api::{DriveClient, DriveFile, DOCX_MIME, MARKDOWN_MIME};
use crate::gdrive::state::{DriveEntry, EntryKind, GdriveState};

/// Name of the folder Carrel creates in Drive.
pub const ROOT_FOLDER_NAME: &str = "Carrel";
/// Where deleted files go; never hard-delete in the cloud.
pub const DELETED_FOLDER_NAME: &str = ".deleted";

/// Marker in `appProperties` on the root folder, so a user rename does not
/// break the mirror.
const ROOT_PROPERTY: (&str, &str) = ("carrel", "root");
/// Marker on the `.deleted` folder.
const DELETED_PROPERTY: (&str, &str) = ("carrel", "deleted");
/// Marker on mirrored subfolders.
const FOLDER_PROPERTY: (&str, &str) = ("carrel", "folder");

/// Counters for one mirror run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GdriveReport {
    /// Folders created in Drive.
    pub folders: usize,
    /// Files created or updated (Markdown and optional `.docx` copies).
    pub uploaded: usize,
    /// Local Markdown files whose hash was already in Drive.
    pub unchanged: usize,
    /// Files moved to `Carrel/.deleted/`.
    pub deleted: usize,
    /// Local files skipped because they are not UTF-8 Markdown or not a valid
    /// Carrel path.
    pub skipped: usize,
}

impl GdriveReport {
    pub fn is_noop(&self) -> bool {
        self.folders == 0 && self.uploaded == 0 && self.deleted == 0
    }
}

/// Hooks the optional `.docx` copies into the export pipeline. The real
/// implementation arrives with the export crate; when it is absent, Carrel
/// mirrors Markdown only.
pub trait DocxConverter: Send + Sync {
    /// Convert one Markdown document to `.docx` bytes.
    fn convert(&self, path: &str, markdown: &str) -> Result<Vec<u8>>;
}

/// Settings of the mirror.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MirrorOptions {
    pub docx_copy: bool,
}

/// Mirror `writing_dir` into Drive. `state` is updated as each file succeeds.
pub async fn mirror_once(
    api: &DriveClient,
    writing_dir: &Path,
    state: &mut GdriveState,
    options: &MirrorOptions,
    converter: Option<&dyn DocxConverter>,
) -> Result<GdriveReport> {
    let tree = scan_local(writing_dir)?;
    let root = ensure_folder(api, ROOT_FOLDER_NAME, None, ROOT_PROPERTY).await?;
    let deleted = ensure_folder(api, DELETED_FOLDER_NAME, Some(&root.id), DELETED_PROPERTY).await?;

    let mut index = RemoteIndex::build(api.list_all().await?, &root.id, &deleted.id);
    let mut report = GdriveReport {
        skipped: tree.skipped,
        ..GdriveReport::default()
    };

    // Files created by an earlier failed attempt can appear twice. Keep one
    // copy per path and move the others out of the way.
    for duplicate in index.take_duplicates() {
        api.move_file(&duplicate.id, &deleted.id, duplicate.parent())
            .await?;
        report.deleted += 1;
    }

    let mut folders = FolderMap::from_index(&index, &root.id);
    for directory in &tree.dirs {
        if folders.contains(directory) {
            continue;
        }
        let parent = parent_path(directory);
        let parent_id = folders
            .get(parent)
            .with_context(|| format!("parent folder {parent:?} of {directory:?} is missing"))?;
        let created = api
            .create_folder(file_name(directory), Some(parent_id), FOLDER_PROPERTY)
            .await
            .with_context(|| format!("create the Drive folder for {directory:?}"))?;
        folders.insert(directory.clone(), created.id);
        report.folders += 1;
    }

    for file in tree.files.values() {
        let parent = parent_path(&file.path);
        let parent_id = folders
            .get(parent)
            .with_context(|| format!("parent folder {parent:?} of {:?} is missing", file.path))?;
        sync_markdown(api, state, &index, &file.path, parent_id, file, &mut report).await?;
        if options.docx_copy {
            if let Some(converter) = converter {
                sync_docx(
                    api,
                    state,
                    &index,
                    &file.path,
                    parent_id,
                    file,
                    converter,
                    &mut report,
                )
                .await?;
            }
        }
    }

    // Anything tracked that is no longer on disk goes to .deleted, including
    // the optional .docx copy of the same file.
    for entry in state.entries()? {
        // Skip rows for files that still exist locally, even ones the scan
        // refused (a symlink, invalid UTF-8): moving them to .deleted would be
        // a surprise and is not a rename.
        if tree.files.contains_key(&entry.path) || exists_locally(writing_dir, &entry.path) {
            continue;
        }
        if let Some(node) = index.by_id.get(&entry.file_id) {
            // Only move files that are still below the root. One that is
            // already in .deleted (a retry after a failed state write) just
            // loses its row.
            if path_of(node, &index.by_id, &root.id, &deleted.id).is_some() {
                api.move_file(&node.id, &deleted.id, node.parent())
                    .await
                    .with_context(|| format!("move {:?} to {DELETED_FOLDER_NAME}", entry.path))?;
                report.deleted += 1;
            }
        }
        state.remove(&entry.path, entry.kind)?;
    }

    Ok(report)
}

async fn sync_markdown(
    api: &DriveClient,
    state: &mut GdriveState,
    index: &RemoteIndex,
    path: &str,
    parent_id: &str,
    file: &LocalFile,
    report: &mut GdriveReport,
) -> Result<()> {
    let row = state.entry(path, EntryKind::Markdown)?;
    let target = index.target(row.as_ref().map(|row| row.file_id.as_str()), path);
    let name = file_name(path);

    if let Some(target) = &target {
        fix_metadata(api, target, name, parent_id)
            .await
            .with_context(|| format!("restore the Drive name of {path:?}"))?;
    }

    let needs_upload = match (&row, &target) {
        (Some(row), Some(target)) => row.hash != file.hash || row.file_id != target.id,
        _ => true,
    };
    if !needs_upload {
        report.unchanged += 1;
        return Ok(());
    }

    let file_id = match target {
        Some(target) => {
            api.update_content(&target.id, MARKDOWN_MIME, &file.bytes)
                .await?;
            target.id
        }
        None => {
            let created = api
                .create_file(name, parent_id, MARKDOWN_MIME)
                .await
                .with_context(|| format!("create the Drive file for {path:?}"))?;
            api.update_content(&created.id, MARKDOWN_MIME, &file.bytes)
                .await?;
            created.id
        }
    };
    state.upsert(&DriveEntry {
        path: path.to_string(),
        kind: EntryKind::Markdown,
        file_id,
        hash: file.hash.clone(),
    })?;
    report.uploaded += 1;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn sync_docx(
    api: &DriveClient,
    state: &mut GdriveState,
    index: &RemoteIndex,
    path: &str,
    parent_id: &str,
    file: &LocalFile,
    converter: &dyn DocxConverter,
    report: &mut GdriveReport,
) -> Result<()> {
    let docx_path = docx_path(path);
    let row = state.entry(path, EntryKind::Docx)?;
    let target = index.target(row.as_ref().map(|row| row.file_id.as_str()), &docx_path);
    let name = file_name(&docx_path);

    if let Some(target) = &target {
        fix_metadata(api, target, name, parent_id)
            .await
            .with_context(|| format!("restore the Drive name of {docx_path:?}"))?;
    }

    let needs_upload = match (&row, &target) {
        (Some(row), Some(target)) => row.hash != file.hash || row.file_id != target.id,
        _ => true,
    };
    if !needs_upload {
        return Ok(());
    }

    let bytes = converter
        .convert(path, &file.text)
        .with_context(|| format!("make the .docx copy of {path:?}"))?;
    if bytes.is_empty() {
        bail!("the .docx converter returned an empty document for {path:?}");
    }
    let file_id = match target {
        Some(target) => {
            api.update_content(&target.id, DOCX_MIME, &bytes).await?;
            target.id
        }
        None => {
            let created = api
                .create_file(name, parent_id, DOCX_MIME)
                .await
                .with_context(|| format!("create the Drive .docx for {path:?}"))?;
            api.update_content(&created.id, DOCX_MIME, &bytes).await?;
            created.id
        }
    };
    state.upsert(&DriveEntry {
        path: path.to_string(),
        kind: EntryKind::Docx,
        file_id,
        hash: file.hash.clone(),
    })?;
    report.uploaded += 1;
    Ok(())
}

async fn fix_metadata(
    api: &DriveClient,
    node: &DriveFile,
    name: &str,
    parent_id: &str,
) -> Result<()> {
    if node.name != name {
        api.rename_file(&node.id, name).await?;
    }
    if node.parent() != Some(parent_id) {
        api.move_file(&node.id, parent_id, node.parent()).await?;
    }
    Ok(())
}

/// Find or create the folder with an `appProperties` marker.
async fn ensure_folder(
    api: &DriveClient,
    name: &str,
    parent: Option<&str>,
    property: (&str, &str),
) -> Result<DriveFile> {
    if let Some(folder) = api.find_by_app_property(property.0, property.1).await? {
        if !folder.is_folder() {
            bail!("the Drive marker {property:?} is on a file, not on a folder");
        }
        return Ok(folder);
    }
    api.create_folder(name, parent, property)
        .await
        .with_context(|| format!("create the {name:?} folder in Google Drive"))
}

/// The paths under the root that already exist in Drive, and their file IDs.
struct RemoteIndex {
    /// Every non-trashed node the app can see, by ID.
    by_id: BTreeMap<String, DriveFile>,
    /// The first node at each path under the Carrel root (`.deleted` excluded).
    by_path: BTreeMap<String, DriveFile>,
    /// Extra files with a path that is already taken, from an interrupted run.
    duplicates: Vec<DriveFile>,
}

impl RemoteIndex {
    fn build(files: Vec<DriveFile>, root_id: &str, deleted_id: &str) -> Self {
        let mut by_id: BTreeMap<String, DriveFile> = files
            .into_iter()
            .filter(|file| !file.trashed)
            .map(|file| (file.id.clone(), file))
            .collect();
        let mut by_path = BTreeMap::new();
        let mut duplicates = Vec::new();
        for node in by_id.values() {
            if node.id == root_id || node.id == deleted_id {
                continue;
            }
            let Some(path) = path_of(node, &by_id, root_id, deleted_id) else {
                continue;
            };
            match by_path.entry(path) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(node.clone());
                }
                std::collections::btree_map::Entry::Occupied(_) if !node.is_folder() => {
                    duplicates.push(node.clone());
                }
                std::collections::btree_map::Entry::Occupied(_) => {}
            }
        }
        for duplicate in &duplicates {
            // A duplicate must not be a valid target for this path.
            by_id.remove(&duplicate.id);
        }
        Self {
            by_id,
            by_path,
            duplicates,
        }
    }

    /// Prefer the file the state database remembers (it may have been renamed
    /// by the user in Drive), otherwise reuse whatever is already at the path.
    fn target(&self, known_id: Option<&str>, path: &str) -> Option<DriveFile> {
        if let Some(id) = known_id {
            if let Some(node) = self.by_id.get(id) {
                return Some(node.clone());
            }
        }
        self.by_path
            .get(path)
            .filter(|node| !node.is_folder())
            .cloned()
    }

    fn take_duplicates(&mut self) -> Vec<DriveFile> {
        std::mem::take(&mut self.duplicates)
    }
}

/// Resolve a node's path relative to the Carrel root, or `None` when it is not
/// below the root or belongs to `.deleted`.
fn path_of(
    node: &DriveFile,
    by_id: &BTreeMap<String, DriveFile>,
    root_id: &str,
    deleted_id: &str,
) -> Option<String> {
    let mut segments = vec![node.name.clone()];
    let mut current = node;
    for _ in 0..64 {
        let parent = current.parent()?;
        if parent == root_id {
            segments.reverse();
            return Some(segments.join("/"));
        }
        if parent == deleted_id {
            return None;
        }
        let parent_node = by_id.get(parent)?;
        if !parent_node.is_folder() {
            return None;
        }
        segments.push(parent_node.name.clone());
        current = parent_node;
    }
    None
}

/// Folder IDs by path, including `""` for the root.
struct FolderMap {
    folders: BTreeMap<String, String>,
}

impl FolderMap {
    fn from_index(index: &RemoteIndex, root_id: &str) -> Self {
        let mut folders = BTreeMap::new();
        folders.insert(String::new(), root_id.to_string());
        for (path, node) in &index.by_path {
            if node.is_folder() {
                folders.insert(path.clone(), node.id.clone());
            }
        }
        Self { folders }
    }

    fn contains(&self, path: &str) -> bool {
        self.folders.contains_key(path)
    }

    fn get(&self, path: &str) -> Option<&String> {
        self.folders.get(path)
    }

    fn insert(&mut self, path: String, id: String) {
        self.folders.insert(path, id);
    }
}

struct LocalFile {
    path: String,
    bytes: Vec<u8>,
    /// UTF-8 text, for the optional .docx converter.
    text: String,
    hash: String,
}

#[derive(Default)]
struct LocalTree {
    files: BTreeMap<String, LocalFile>,
    dirs: BTreeSet<String>,
    skipped: usize,
}

fn scan_local(root: &Path) -> Result<LocalTree> {
    let mut tree = LocalTree::default();
    walk(root, root, &mut tree)?;
    Ok(tree)
}

fn walk(root: &Path, directory: &Path, tree: &mut LocalTree) -> Result<()> {
    let entries = fs::read_dir(directory)
        .with_context(|| format!("read directory {}", directory.display()))?;
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            tree.skipped += 1;
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            tree.skipped += 1;
            continue;
        };
        if name.starts_with('.') {
            // `.carrel`, editor temp files and journal directories.
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            let Some(relative) = relative_path(root, &path) else {
                tree.skipped += 1;
                continue;
            };
            if !safe_directory(&relative) {
                tree.skipped += 1;
                continue;
            }
            tree.dirs.insert(relative);
            walk(root, &path, tree)?;
            continue;
        }
        if !file_type.is_file() || !name.ends_with(".md") {
            continue;
        }
        let Some(relative) = relative_path(root, &path) else {
            tree.skipped += 1;
            continue;
        };
        if validate_path(&relative).is_err() {
            tree.skipped += 1;
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Err(error).with_context(|| format!("read {}", path.display()));
            }
        };
        let Ok(text) = String::from_utf8(bytes.clone()) else {
            tree.skipped += 1;
            continue;
        };
        let hash = content_hash(&bytes);
        tree.files.insert(
            relative.clone(),
            LocalFile {
                path: relative,
                bytes,
                text,
                hash,
            },
        );
    }
    Ok(())
}

/// True when `path` exists locally, without following a symlink. State rows
/// were written from validated paths; the check is only for deletions.
fn exists_locally(root: &Path, path: &str) -> bool {
    match validate_path(path) {
        Ok(()) => fs::symlink_metadata(root.join(path)).is_ok(),
        Err(_) => false,
    }
}

fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?.to_str()?;
    Some(relative.replace(std::path::MAIN_SEPARATOR, "/"))
}

fn safe_directory(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.starts_with('.'))
}

/// Parent folder path of a file or folder path (`""` for the top level).
fn parent_path(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((parent, _)) => parent,
        None => "",
    }
}

/// Last component of a path.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `notes/a.md` becomes `notes/a.docx`, for the optional Google Docs copy.
fn docx_path(path: &str) -> String {
    match path.strip_suffix(".md") {
        Some(stem) => format!("{stem}.docx"),
        None => format!("{path}.docx"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_parents_names_and_docx_paths() {
        assert_eq!(parent_path("a.md"), "");
        assert_eq!(parent_path("notes/a.md"), "notes");
        assert_eq!(parent_path("notes/2026/a.md"), "notes/2026");
        assert_eq!(file_name("notes/a.md"), "a.md");
        assert_eq!(docx_path("notes/a.md"), "notes/a.docx");
        assert_eq!(docx_path("odd"), "odd.docx");
    }

    #[test]
    fn scans_folders_and_markdown_only() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("notes/deep")).unwrap();
        fs::create_dir_all(dir.path().join(".carrel")).unwrap();
        fs::write(dir.path().join("note.md"), "one").unwrap();
        fs::write(dir.path().join("notes/two.md"), "two").unwrap();
        fs::write(dir.path().join("notes/deep/three.md"), "three").unwrap();
        fs::write(dir.path().join("notes/skip.txt"), "no").unwrap();
        fs::write(dir.path().join("notes/temp.md.tmp"), "no").unwrap();
        fs::write(dir.path().join(".carrel/hidden.md"), "no").unwrap();
        fs::write(dir.path().join("notes/not-utf8.md"), [0xff, 0xfe, 0x00]).unwrap();

        let tree = scan_local(dir.path()).unwrap();
        assert_eq!(tree.files.len(), 3);
        assert!(tree.files.contains_key("note.md"));
        assert!(tree.files.contains_key("notes/deep/three.md"));
        assert_eq!(
            tree.dirs.iter().cloned().collect::<Vec<_>>(),
            ["notes", "notes/deep"]
        );
        assert_eq!(tree.skipped, 1);
        assert_eq!(tree.files["notes/two.md"].text, "two");
    }

    #[test]
    fn remote_paths_follow_the_root_and_skip_deleted() {
        let folder = |id: &str, name: &str, parent: Option<&str>| DriveFile {
            id: id.into(),
            name: name.into(),
            mime_type: crate::gdrive::api::FOLDER_MIME.into(),
            parents: parent.map(|p| vec![p.to_string()]).unwrap_or_default(),
            app_properties: BTreeMap::new(),
            trashed: false,
            sha256_checksum: None,
        };
        let files = vec![
            folder("root", "Carrel", None),
            folder("deleted", ".deleted", Some("root")),
            folder("notes", "notes", Some("root")),
            DriveFile {
                id: "file".into(),
                name: "a.md".into(),
                mime_type: MARKDOWN_MIME.into(),
                parents: vec!["notes".into()],
                app_properties: BTreeMap::new(),
                trashed: false,
                sha256_checksum: None,
            },
            DriveFile {
                id: "gone".into(),
                name: "old.md".into(),
                mime_type: MARKDOWN_MIME.into(),
                parents: vec!["deleted".into()],
                app_properties: BTreeMap::new(),
                trashed: false,
                sha256_checksum: None,
            },
        ];
        let index = RemoteIndex::build(files, "root", "deleted");
        assert_eq!(
            index.by_path.keys().cloned().collect::<Vec<_>>(),
            ["notes", "notes/a.md"]
        );
        assert_eq!(index.target(None, "notes/a.md").unwrap().id, "file");
        assert!(index.target(None, "notes/old.md").is_none());
        assert_eq!(index.target(Some("file"), "other.md").unwrap().id, "file");
    }

    #[test]
    fn duplicate_files_are_recorded_and_not_targets() {
        let file = |id: &str| DriveFile {
            id: id.into(),
            name: "a.md".into(),
            mime_type: MARKDOWN_MIME.into(),
            parents: vec!["root".into()],
            app_properties: BTreeMap::new(),
            trashed: false,
            sha256_checksum: None,
        };
        let root = DriveFile {
            id: "root".into(),
            name: "Carrel".into(),
            mime_type: crate::gdrive::api::FOLDER_MIME.into(),
            parents: Vec::new(),
            app_properties: BTreeMap::new(),
            trashed: false,
            sha256_checksum: None,
        };
        let deleted = DriveFile {
            id: "deleted".into(),
            name: ".deleted".into(),
            parents: vec!["root".into()],
            ..root.clone()
        };
        let mut index =
            RemoteIndex::build(vec![root, deleted, file("a"), file("b")], "root", "deleted");
        assert_eq!(index.by_path.len(), 1);
        assert_eq!(index.take_duplicates().len(), 1);
        // The duplicate ID is gone; the canonical file at the path wins.
        assert_eq!(index.target(Some("b"), "a.md").unwrap().id, "a");
    }
}
