//! Pure-Rust storage and durability. No GTK in this module, ever.

pub mod document;
pub mod fsutil;
pub mod hash;
pub mod journal;
pub mod naming;
pub mod stats;
pub mod text;

use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub use document::{Document, SavePolicy};
pub use fsutil::atomic_write;
pub use stats::DailyStats;
pub use text::EditOutOfRange;

pub const DEFAULT_ROOT: &str = "/data/writing";

/// The writing root: `CARREL_WRITING_DIR`, else `/data/writing`.
pub fn root_from_env() -> PathBuf {
    std::env::var_os("CARREL_WRITING_DIR")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ROOT))
}

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    InvalidPath(PathBuf),
    InvalidEdit(EditOutOfRange),
    Closed,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::InvalidPath(path) => write!(f, "invalid path: {}", path.display()),
            Self::InvalidEdit(error) => write!(f, "{error}"),
            Self::Closed => f.write_str("document is closed"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidEdit(error) => Some(error),
            Self::InvalidPath(_) | Self::Closed => None,
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Clone, Debug)]
pub struct Entry {
    pub rel: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub modified: SystemTime,
}

#[derive(Clone)]
pub struct Store {
    root: Arc<PathBuf>,
    policy: SavePolicy,
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        Self::open_with_policy(root, SavePolicy::default())
    }

    pub fn open_with_policy(root: impl Into<PathBuf>, policy: SavePolicy) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        fs::create_dir_all(root.join(".carrel").join("journal"))?;
        fs::create_dir_all(root.join(".carrel").join("trash"))?;
        Ok(Self {
            root: Arc::new(root),
            policy,
        })
    }

    pub fn open_default() -> io::Result<Self> {
        Self::open(root_from_env())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn policy(&self) -> SavePolicy {
        self.policy
    }

    pub fn carrel_dir(&self) -> PathBuf {
        self.root.join(".carrel")
    }

    pub fn journal_path(&self, rel: &Path) -> PathBuf {
        self.carrel_dir()
            .join("journal")
            .join(document::journal_id(rel))
    }

    /// Reject absolute paths, `..`, and hidden components so the shell can
    /// never wander out of the writing root or into its own internals.
    pub fn resolve(&self, rel: &Path) -> StoreResult<PathBuf> {
        if rel.is_absolute() {
            return Err(StoreError::InvalidPath(rel.to_path_buf()));
        }
        for component in rel.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(name) => {
                    if name.to_string_lossy().starts_with('.') {
                        return Err(StoreError::InvalidPath(rel.to_path_buf()));
                    }
                }
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(StoreError::InvalidPath(rel.to_path_buf()));
                }
            }
        }
        Ok(self.root.join(rel))
    }

    pub fn list(&self, rel_dir: &Path) -> StoreResult<Vec<Entry>> {
        let dir = self.resolve(rel_dir)?;
        let mut entries = Vec::new();
        let read_dir = match fs::read_dir(&dir) {
            Ok(read_dir) => read_dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(entries),
            Err(error) => return Err(error.into()),
        };
        for item in read_dir {
            let item = item?;
            let file_name = item.file_name();
            let name = file_name.to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let file_type = item.file_type()?;
            let is_dir = file_type.is_dir();
            if !is_dir && !is_document_name(&name) {
                continue;
            }
            let relative = if rel_dir.as_os_str().is_empty() {
                PathBuf::from(&name)
            } else {
                rel_dir.join(&name)
            };
            let modified = item
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            entries.push(Entry {
                rel: relative,
                name,
                is_dir,
                modified,
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    pub fn create_file(&self, rel: &Path) -> StoreResult<()> {
        let path = self.resolve(rel)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        if let Some(parent) = path.parent() {
            fsutil::sync_dir(parent)?;
        }
        Ok(())
    }

    pub fn create_dir(&self, rel: &Path) -> StoreResult<()> {
        let path = self.resolve(rel)?;
        fs::create_dir_all(path)?;
        Ok(())
    }

    pub fn rename(&self, from: &Path, to: &Path) -> StoreResult<()> {
        let from_path = self.resolve(from)?;
        let to_path = self.resolve(to)?;
        if to_path.exists() {
            return Err(StoreError::Io(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists", to_path.display()),
            )));
        }
        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Journals are keyed by path. Collect paths before the move so a
        // renamed folder takes its files' pending journals with it.
        let mut files = Vec::new();
        if from_path.is_dir() {
            collect_files(&from_path, from, &mut files)?;
        } else {
            files.push(from.to_path_buf());
        }

        fs::rename(&from_path, &to_path)?;
        if let Some(parent) = to_path.parent() {
            fsutil::sync_dir(parent)?;
        }

        for old_rel in files {
            let new_rel = moved_rel(&old_rel, from, to);
            let old_journal = self.journal_path(&old_rel);
            if old_journal.exists() {
                let new_journal = self.journal_path(&new_rel);
                if let Some(parent) = new_journal.parent() {
                    fs::create_dir_all(parent)?;
                }
                let _ = fs::rename(old_journal, new_journal);
            }
        }
        Ok(())
    }

    /// Delete by moving into `.carrel/trash/`. Nothing is ever destroyed by
    /// the shell, so a mistaken delete can still be recovered.
    pub fn delete(&self, rel: &Path) -> StoreResult<()> {
        let source = self.resolve(rel)?;
        if !source.exists() {
            return Ok(());
        }
        let is_dir = source.is_dir();
        let mut files = Vec::new();
        if is_dir {
            collect_files(&source, rel, &mut files)?;
        } else {
            files.push(rel.to_path_buf());
        }

        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_millis().to_string())
            .unwrap_or_else(|_| "0".to_owned());
        let trash_root = self.carrel_dir().join("trash");
        let trash_dir = fsutil::unique_path(&trash_root, &stamp, "");
        fs::create_dir_all(&trash_dir)?;

        let target = trash_dir.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&source, &target)?;
        fsutil::sync_dir(&trash_dir)?;

        for (index, file_rel) in files.iter().enumerate() {
            let journal = self.journal_path(file_rel);
            if journal.exists() {
                let name = format!("{index:02}-{}", document::journal_id(file_rel));
                let _ = fs::rename(&journal, trash_dir.join(name));
            }
        }
        Ok(())
    }

    pub fn open_document(&self, rel: &Path) -> StoreResult<Document> {
        self.resolve(rel)?;
        let journal_path = self.journal_path(rel);
        document::open(&self.root, rel, journal_path, self.policy)
    }

    /// A free name inside `dir_rel`, returned relative to the root.
    pub fn unique_rel_path(
        &self,
        dir_rel: &Path,
        stem: &str,
        extension: &str,
    ) -> StoreResult<PathBuf> {
        let dir = self.resolve(dir_rel)?;
        let path = fsutil::unique_path(&dir, stem, extension);
        let name = path
            .file_name()
            .map(PathBuf::from)
            .ok_or_else(|| StoreError::InvalidPath(path.clone()))?;
        Ok(if dir_rel.as_os_str().is_empty() {
            name
        } else {
            dir_rel.join(name)
        })
    }

    /// Most recently modified Markdown file at the top level, used when the
    /// remembered file has gone away.
    /// Every document under the writing root, relative and sorted by name.
    /// The command palette lists these so a few letters open any file.
    pub fn document_paths(&self) -> Vec<PathBuf> {
        let mut all = Vec::new();
        if collect_files(self.root(), Path::new(""), &mut all).is_err() {
            return Vec::new();
        }
        all.retain(|path| {
            path.file_name()
                .map(|name| is_document_name(&name.to_string_lossy()))
                .unwrap_or(false)
        });
        all.sort_by(|a, b| {
            a.to_string_lossy()
                .to_lowercase()
                .cmp(&b.to_string_lossy().to_lowercase())
        });
        all
    }

    pub fn most_recent_file(&self) -> StoreResult<Option<PathBuf>> {
        let mut best: Option<(SystemTime, PathBuf)> = None;
        for entry in self.list(Path::new(""))? {
            if entry.is_dir {
                continue;
            }
            if best.as_ref().is_none_or(|(time, _)| entry.modified > *time) {
                best = Some((entry.modified, entry.rel));
            }
        }
        Ok(best.map(|(_, rel)| rel))
    }

    pub fn last_file(&self) -> Option<PathBuf> {
        let state = self.load_shell_state();
        state
            .last_file
            .map(PathBuf::from)
            .filter(|rel| self.resolve(rel).is_ok() && self.root.join(rel).is_file())
    }

    pub fn set_last_file(&self, rel: Option<&Path>) -> io::Result<()> {
        let state = ShellState {
            last_file: rel.map(|rel| rel.to_string_lossy().to_string()),
        };
        let json = serde_json::to_vec_pretty(&state).map_err(io::Error::other)?;
        atomic_write(&self.carrel_dir().join("state.json"), &json)
    }

    fn load_shell_state(&self) -> ShellState {
        fs::read(self.carrel_dir().join("state.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn load_stats(&self, today: &str) -> DailyStats {
        let stats = fs::read(self.carrel_dir().join("stats.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DailyStats>(&bytes).ok())
            .unwrap_or_default();
        if stats.date == today {
            stats
        } else {
            DailyStats {
                date: today.to_owned(),
                words: 0,
            }
        }
    }

    pub fn save_stats(&self, stats: &DailyStats) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(stats).map_err(io::Error::other)?;
        atomic_write(&self.carrel_dir().join("stats.json"), &json)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ShellState {
    last_file: Option<String>,
}

pub fn is_document_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".txt")
}

/// The name a screen shows for a document. The document extension is not part
/// of the title: "The quiet morning", not "The quiet morning.md". Folders and
/// other names are unchanged.
pub fn display_name(name: &str) -> String {
    if !is_document_name(name) {
        return name.to_owned();
    }
    let lower = name.to_ascii_lowercase();
    for extension in [".markdown", ".md", ".txt"] {
        if lower.ends_with(extension) {
            return name[..name.len() - extension.len()].to_owned();
        }
    }
    name.to_owned()
}

/// A relative path with its final document name shown without extension.
pub fn display_path(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|name| display_name(&name.to_string_lossy()))
        .unwrap_or_default();
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            format!("{}/{}", parent.display(), name)
        }
        _ => name,
    }
}

/// Recursively list document paths under `dir`, with paths relative to the
/// writing root. Hidden entries (including `.carrel`) are skipped.
fn collect_files(dir: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let read_dir = match fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for item in read_dir {
        let item = item?;
        let name = item.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let child_rel = if rel.as_os_str().is_empty() {
            PathBuf::from(&name)
        } else {
            rel.join(&name)
        };
        if item.file_type()?.is_dir() {
            collect_files(&item.path(), &child_rel, out)?;
        } else {
            out.push(child_rel);
        }
    }
    Ok(())
}

fn moved_rel(path: &Path, from: &Path, to: &Path) -> PathBuf {
    if let Ok(suffix) = path.strip_prefix(from) {
        if suffix.as_os_str().is_empty() {
            to.to_path_buf()
        } else {
            to.join(suffix)
        }
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn creates_expected_layout() {
        let (dir, store) = store();
        assert!(dir.path().join(".carrel/journal").is_dir());
        assert_eq!(store.root(), dir.path());
    }

    #[test]
    fn rejects_escaping_paths() {
        let (_dir, store) = store();
        assert!(store.resolve(Path::new("../x")).is_err());
        assert!(store.resolve(Path::new("/etc/passwd")).is_err());
        assert!(store.resolve(Path::new(".carrel/journal/x")).is_err());
        assert!(store.resolve(Path::new("chapter/../x")).is_err());
        assert!(store.resolve(Path::new("chapter/one.md")).is_ok());
    }

    #[test]
    fn lists_folders_then_documents_and_hides_internals() {
        let (_dir, store) = store();
        store.create_dir(Path::new("Novel")).unwrap();
        store.create_file(Path::new("A.md")).unwrap();
        store.create_file(Path::new("b.md")).unwrap();
        store.create_file(Path::new("note.txt")).unwrap();
        let entries = store.list(Path::new("")).unwrap();
        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["Novel", "A.md", "b.md", "note.txt"]);
        assert!(entries[0].is_dir);
    }

    #[test]
    fn rename_moves_the_journal_too() {
        let (dir, store) = store();
        store.create_file(Path::new("old.md")).unwrap();
        let doc = store.open_document(Path::new("old.md")).unwrap();
        doc.apply(0, 0, "hello").unwrap();
        doc.flush().unwrap();
        drop(doc);
        let old_journal = store.journal_path(Path::new("old.md"));
        assert!(old_journal.exists());

        store
            .rename(Path::new("old.md"), Path::new("new.md"))
            .unwrap();
        assert!(!old_journal.exists());
        assert!(store.journal_path(Path::new("new.md")).exists());
        assert!(dir.path().join("new.md").is_file());
    }

    #[test]
    fn delete_moves_file_and_journal_to_trash() {
        let (_dir, store) = store();
        store.create_file(Path::new("gone.md")).unwrap();
        let doc = store.open_document(Path::new("gone.md")).unwrap();
        doc.apply(0, 0, "still here").unwrap();
        doc.flush().unwrap();
        drop(doc);

        store.delete(Path::new("gone.md")).unwrap();
        assert!(!store.root().join("gone.md").exists());
        assert!(!store.journal_path(Path::new("gone.md")).exists());

        let mut trash_entries = fs::read_dir(store.carrel_dir().join("trash"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(trash_entries.len(), 1);
        let trash = trash_entries.pop().unwrap();
        assert!(trash.join("gone.md").is_file());
        assert_eq!(
            fs::read_to_string(trash.join("gone.md")).unwrap(),
            "still here"
        );
    }

    #[test]
    fn rename_of_a_folder_moves_nested_journals() {
        let (_dir, store) = store();
        store.create_dir(Path::new("Novel")).unwrap();
        store.create_file(Path::new("Novel/one.md")).unwrap();
        let journal = store.journal_path(Path::new("Novel/one.md"));
        std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
        std::fs::write(&journal, b"pending").unwrap();

        store.rename(Path::new("Novel"), Path::new("Book")).unwrap();
        assert!(!journal.exists());
        assert!(store.journal_path(Path::new("Book/one.md")).exists());
    }

    #[test]
    fn delete_of_a_folder_moves_nested_journals_to_trash() {
        let (_dir, store) = store();
        store.create_dir(Path::new("Novel")).unwrap();
        store.create_file(Path::new("Novel/one.md")).unwrap();
        let journal = store.journal_path(Path::new("Novel/one.md"));
        std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
        std::fs::write(&journal, b"pending").unwrap();

        store.delete(Path::new("Novel")).unwrap();
        assert!(!store.root().join("Novel").exists());
        assert!(!journal.exists());
        let trash = std::fs::read_dir(store.carrel_dir().join("trash"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(trash.join("Novel/one.md").is_file());
    }

    #[test]
    fn document_paths_lists_every_document_and_nothing_else() {
        let (dir, store) = store();
        fs::create_dir_all(dir.path().join("Novel")).unwrap();
        fs::write(dir.path().join("Notes.md"), "x").unwrap();
        fs::write(dir.path().join("Novel/Chapter one.md"), "x").unwrap();
        fs::write(dir.path().join("Novel/travel notes.txt"), "x").unwrap();
        fs::write(dir.path().join("Novel/cover.png"), "x").unwrap();
        fs::write(dir.path().join(".hidden.md"), "x").unwrap();
        assert_eq!(
            store.document_paths(),
            vec![
                PathBuf::from("Notes.md"),
                PathBuf::from("Novel/Chapter one.md"),
                PathBuf::from("Novel/travel notes.txt"),
            ]
        );
    }

    #[test]
    fn last_file_roundtrip() {
        let (_dir, store) = store();
        store.create_file(Path::new("one.md")).unwrap();
        assert_eq!(store.last_file(), None);
        store.set_last_file(Some(Path::new("one.md"))).unwrap();
        assert_eq!(store.last_file(), Some(PathBuf::from("one.md")));
        store.delete(Path::new("one.md")).unwrap();
        assert_eq!(store.last_file(), None);
    }

    #[test]
    fn unique_names_skip_taken_ones() {
        let (_dir, store) = store();
        store.create_file(Path::new("Untitled.md")).unwrap();
        assert_eq!(
            store
                .unique_rel_path(Path::new(""), "Untitled", "md")
                .unwrap(),
            PathBuf::from("Untitled 2.md")
        );
    }

    #[test]
    fn display_names_hide_the_document_extension() {
        assert_eq!(display_name("The quiet morning.md"), "The quiet morning");
        assert_eq!(display_name("Notes.markdown"), "Notes");
        assert_eq!(display_name("Imported.txt"), "Imported");
        assert_eq!(display_name("Novel"), "Novel");
        assert_eq!(
            display_path(Path::new("Novel/Chapter one.md")),
            "Novel/Chapter one"
        );
        assert_eq!(display_path(Path::new("Notes.md")), "Notes");
    }
}
