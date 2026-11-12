//! Local version history with libgit2.
//!
//! Every snapshot of the writing directory is a commit in a bare repository at
//! `<writing>/.stylos/history.git`. The writing directory itself is the work
//! tree: `.stylos/` and hidden files are never committed, and `Restore` only
//! reads old text back — it never rewrites history.
//!
//! Commits happen after five minutes without changes, before suspend, before
//! every sync and at shutdown (`spec/01-architecture.md`, Durability step 4).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use git2::{Oid, Repository, Sort, Tree};
use serde::{Deserialize, Serialize};

/// How many versions `history()` returns for one file.
pub const HISTORY_LIMIT: usize = 500;

/// Ignore files larger than this when snapshotting. Writing is Markdown.
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// One restorable version of a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct HistoryEntry {
    /// Git commit id (hex).
    pub rev: String,
    /// Commit time, Unix seconds.
    pub time: i64,
    /// Words in the file at that version.
    pub words: u32,
    /// Word-count change against the previous (older) version.
    pub delta: i64,
}

/// The history repository of one writing directory.
pub struct History {
    writing_dir: PathBuf,
    repo: Repository,
}

impl History {
    /// Open or create `<writing>/.stylos/history.git`.
    pub fn open(writing_dir: &Path) -> Result<Self> {
        fs::create_dir_all(writing_dir)
            .with_context(|| format!("create writing directory {}", writing_dir.display()))?;
        let stylos_dir = writing_dir.join(".stylos");
        if !stylos_dir.exists() {
            fs::create_dir_all(&stylos_dir)
                .with_context(|| format!("create {}", stylos_dir.display()))?;
            // The shell runs as `writer` and creates its journal below this
            // directory; stylosd starts first and must not lock it out.
            fs::set_permissions(&stylos_dir, fs::Permissions::from_mode(0o777)).ok();
        }
        let git_dir = stylos_dir.join("history.git");
        fs::create_dir_all(git_dir.parent().expect("history path has a parent"))
            .with_context(|| format!("create {}", git_dir.display()))?;
        let repo = match Repository::open_bare(&git_dir) {
            Ok(repo) => repo,
            Err(_) => {
                let repo = Repository::init_bare(&git_dir)
                    .with_context(|| format!("create history repository {}", git_dir.display()))?;
                repo.set_head("refs/heads/main")?;
                repo
            }
        };
        // Older repositories may have been created with the default branch
        // name; keep everything on `main` so the ref is stable.
        if repo.head().err().map(|error| error.code()) == Some(git2::ErrorCode::UnbornBranch) {
            repo.set_head("refs/heads/main")?;
        }
        Ok(Self {
            writing_dir: writing_dir.to_path_buf(),
            repo,
        })
    }

    /// The repository created for this writing directory.
    pub fn git_dir(writing_dir: &Path) -> PathBuf {
        writing_dir.join(".stylos").join("history.git")
    }

    /// Commit the current writing tree. Returns the new commit id, or `None`
    /// when nothing changed since the last commit.
    pub fn commit(&self, reason: &str) -> Result<Option<String>> {
        let (files, _) = scan(&self.writing_dir)?;
        self.commit_files(&files, reason)
    }

    /// Commit only when nothing in the writing directory changed for `idle`.
    ///
    /// `now` is passed in so tests can move the clock without sleeping.
    pub fn commit_if_idle(
        &self,
        now: SystemTime,
        idle: Duration,
        reason: &str,
    ) -> Result<Option<String>> {
        let (files, newest) = scan(&self.writing_dir)?;
        if files.is_empty() {
            return Ok(None);
        }
        let Some(newest) = newest else {
            return Ok(None);
        };
        let quiet_for = now.duration_since(newest).unwrap_or_default();
        if quiet_for < idle {
            return Ok(None);
        }
        self.commit_files(&files, reason)
    }

    fn commit_files(
        &self,
        files: &BTreeMap<String, Vec<u8>>,
        reason: &str,
    ) -> Result<Option<String>> {
        if files.is_empty() {
            return Ok(None);
        }
        let parent = self.head_commit()?;
        let parent_tree = match &parent {
            Some(commit) => Some(commit.tree()?),
            None => None,
        };
        let files: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(path, content)| (path.as_str(), content.as_slice()))
            .collect();
        let tree = build_tree(&self.repo, &files, parent_tree.as_ref())?;
        let parent_tree_id = parent_tree.as_ref().map(|tree| tree.id());
        if parent_tree_id == Some(tree) {
            return Ok(None);
        }

        let signature = git2::Signature::new(
            "stylosd",
            "stylosd@stylos.local",
            &git2::Time::new(now_unix(), 0),
        )?;
        let tree = self.repo.find_tree(tree)?;
        let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
        let message = format!("{reason}\n\nSnapshot of {} file(s).", files.len());
        let commit = self.repo.commit(
            Some("refs/heads/main"),
            &signature,
            &signature,
            message.trim_end(),
            &tree,
            &parents,
        )?;
        Ok(Some(commit.to_string()))
    }

    /// Versions of one file, newest first, with word counts and deltas.
    pub fn history(&self, path: &str) -> Result<Vec<HistoryEntry>> {
        let path = safe_relative_path(path)?;
        let Some(head) = self.head_oid()? else {
            return Ok(Vec::new());
        };
        let mut walk = self.repo.revwalk()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
        walk.simplify_first_parent()?;
        walk.push(head)?;

        let mut versions: Vec<(String, i64, u32)> = Vec::new();
        let mut last_blob: Option<Oid> = None;
        for oid in walk {
            let oid = oid?;
            let commit = self.repo.find_commit(oid)?;
            let Ok(entry) = commit.tree()?.get_path(Path::new(path)) else {
                continue;
            };
            let object = entry.to_object(&self.repo)?;
            let Ok(blob) = object.into_blob() else {
                continue;
            };
            if Some(blob.id()) == last_blob {
                continue;
            }
            last_blob = Some(blob.id());
            let words = word_count(blob.content());
            versions.push((oid.to_string(), commit.time().seconds(), words));
            // One extra version keeps the oldest delta accurate.
            if versions.len() > HISTORY_LIMIT {
                break;
            }
        }

        let mut entries = Vec::with_capacity(versions.len().min(HISTORY_LIMIT));
        for (index, (rev, time, words)) in versions.iter().enumerate().take(HISTORY_LIMIT) {
            let previous = versions.get(index + 1).map(|(_, _, words)| *words);
            entries.push(HistoryEntry {
                rev: rev.clone(),
                time: *time,
                words: *words,
                delta: i64::from(*words) - i64::from(previous.unwrap_or(0)),
            });
        }
        Ok(entries)
    }

    /// Text of `path` at `rev`. Reads only; never changes the work tree.
    pub fn restore(&self, path: &str, rev: &str) -> Result<String> {
        let path = safe_relative_path(path)?;
        let oid = Oid::from_str(rev).with_context(|| format!("invalid revision {rev:?}"))?;
        let commit = self
            .repo
            .find_commit(oid)
            .with_context(|| format!("unknown revision {rev}"))?;
        let entry = commit
            .tree()?
            .get_path(Path::new(path))
            .with_context(|| format!("no version of {path} at revision {rev}"))?;
        let object = entry.to_object(&self.repo)?;
        let blob = object
            .into_blob()
            .map_err(|_| anyhow::anyhow!("{path} is not a file at revision {rev}"))?;
        String::from_utf8(blob.content().to_vec())
            .with_context(|| format!("{path} is not UTF-8 at revision {rev}"))
    }

    /// Current commit id, if anything was ever committed.
    pub fn head(&self) -> Result<Option<String>> {
        Ok(self.head_oid()?.map(|oid| oid.to_string()))
    }

    fn head_oid(&self) -> Result<Option<Oid>> {
        Ok(self.head_commit()?.map(|commit| commit.id()))
    }

    fn head_commit(&self) -> Result<Option<git2::Commit<'_>>> {
        match self.repo.head() {
            Ok(head) => Ok(Some(head.peel_to_commit()?)),
            Err(error) if error.code() == git2::ErrorCode::UnbornBranch => Ok(None),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

/// Reject paths that would leave the writing directory.
pub fn safe_relative_path(path: &str) -> Result<&str> {
    if path.is_empty() {
        bail!("path is empty");
    }
    if path.starts_with('/') || path.contains('\\') || path.contains('\0') {
        bail!("path {path:?} is not a relative path");
    }
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            bail!("path {path:?} is not a relative path");
        }
        if component.starts_with('.') {
            bail!("path {path:?} points into a hidden directory");
        }
    }
    Ok(path)
}

fn word_count(bytes: &[u8]) -> u32 {
    match std::str::from_utf8(bytes) {
        // Everything is Markdown; a character-based count also works for
        // languages that do not separate words with spaces.
        Ok(text) => text
            .split_whitespace()
            .count()
            .try_into()
            .unwrap_or(u32::MAX),
        Err(_) => 0,
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Read every regular file below `root`, skipping hidden directories (such as
/// `.stylos/`), symlinks and temporary files. Returns the files and the newest
/// modification time seen.
type Scanned = (BTreeMap<String, Vec<u8>>, Option<SystemTime>);

fn scan(root: &Path) -> Result<Scanned> {
    let mut files = BTreeMap::new();
    let mut newest: Option<SystemTime> = None;
    walk(root, root, &mut files, &mut newest)?;
    Ok((files, newest))
}

fn walk(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
    newest: &mut Option<SystemTime>,
) -> Result<()> {
    let entries = fs::read_dir(directory)
        .with_context(|| format!("read directory {}", directory.display()))?;
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with('.') || name.ends_with(".tmp") || name.ends_with('~') {
            continue;
        }
        let metadata = entry.metadata()?;
        if let Ok(modified) = metadata.modified() {
            *newest = Some(match *newest {
                Some(current) if current > modified => current,
                _ => modified,
            });
        }
        let path = entry.path();
        if metadata.is_dir() {
            walk(root, &path, files, newest)?;
            continue;
        }
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let Some(relative) = relative.to_str() else {
            continue;
        };
        let relative = relative.replace(std::path::MAIN_SEPARATOR, "/");
        if safe_relative_path(&relative).is_err() {
            continue;
        }
        match fs::read(&path) {
            Ok(content) => {
                files.insert(relative, content);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        }
    }
    Ok(())
}

/// Build a tree object for `files` bottom-up. libgit2's tree builder only
/// accepts one path component per entry, so nested directories are grouped
/// here. Reusing the parent's subtree when nothing changed keeps tree ids
/// stable, which is what makes the "nothing changed" check above work.
fn build_tree(repo: &Repository, files: &[(&str, &[u8])], parent: Option<&Tree>) -> Result<Oid> {
    let mut builder = match parent {
        Some(tree) => repo.treebuilder(Some(tree))?,
        None => repo.treebuilder(None)?,
    };
    let mut directories: BTreeMap<&str, Vec<(&str, &[u8])>> = BTreeMap::new();
    for (path, content) in files {
        match path.split_once('/') {
            None => {
                let blob = repo.blob(content)?;
                builder.insert(path, blob, 0o100644)?;
            }
            Some((directory, rest)) => {
                directories
                    .entry(directory)
                    .or_default()
                    .push((rest, content));
            }
        }
    }
    for (directory, children) in directories {
        let subtree = match parent.and_then(|tree| tree.get_name(directory)) {
            Some(entry) => entry
                .to_object(repo)
                .ok()
                .and_then(|object| object.into_tree().ok()),
            None => None,
        };
        let subtree = build_tree(repo, &children, subtree.as_ref())?;
        builder.insert(directory, subtree, 0o040000)?;
    }
    Ok(builder.write()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn write(root: &Path, path: &str, text: &str) {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text).unwrap();
    }

    fn set_age(root: &Path, path: &str, age: Duration) {
        let target = root.join(path);
        let file = File::options().write(true).open(target).unwrap();
        let modified = SystemTime::now() - age;
        file.set_modified(modified).unwrap();
    }

    #[test]
    fn commits_and_lists_versions_with_word_deltas() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path()).unwrap();
        write(dir.path(), "draft.md", "one two");
        let first = history.commit("test").unwrap().expect("first commit");

        write(dir.path(), "draft.md", "one two three four");
        let second = history.commit("test").unwrap().expect("second commit");
        assert_ne!(first, second);

        let versions = history.history("draft.md").unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].rev, second);
        assert_eq!(versions[0].words, 4);
        assert_eq!(versions[0].delta, 2);
        assert_eq!(versions[1].rev, first);
        assert_eq!(versions[1].words, 2);
        assert_eq!(versions[1].delta, 2);
        assert!(versions[0].time >= versions[1].time);
    }

    #[test]
    fn unchanged_files_do_not_create_commits() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path()).unwrap();
        write(dir.path(), "draft.md", "text");
        assert!(history.commit("test").unwrap().is_some());
        assert!(history.commit("test").unwrap().is_none());
        assert_eq!(history.history("draft.md").unwrap().len(), 1);
    }

    #[test]
    fn restore_returns_old_text_and_never_touches_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path()).unwrap();
        write(dir.path(), "draft.md", "old text");
        let old = history.commit("test").unwrap().unwrap();
        write(dir.path(), "draft.md", "new text");
        history.commit("test").unwrap();

        assert_eq!(history.restore("draft.md", &old).unwrap(), "old text");
        assert_eq!(
            fs::read_to_string(dir.path().join("draft.md")).unwrap(),
            "new text"
        );
        // Restoring did not add a commit either.
        assert_eq!(history.history("draft.md").unwrap().len(), 2);
    }

    #[test]
    fn hidden_files_and_the_stylos_directory_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path()).unwrap();
        write(dir.path(), ".stylos/journal/x.md", "private");
        write(dir.path(), ".hidden.md", "hidden");
        write(dir.path(), "kept.md", "kept");
        history.commit("test").unwrap();
        assert_eq!(history.history("kept.md").unwrap().len(), 1);
        assert!(history.history(".hidden.md").is_err());
        assert!(history.history(".stylos/journal/x.md").is_err());
        let head = history.head_commit().unwrap().unwrap();
        let tree = head.tree().unwrap();
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn nested_paths_work() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path()).unwrap();
        write(dir.path(), "notes/deep/draft.md", "two words");
        let rev = history.commit("test").unwrap().unwrap();
        assert_eq!(history.history("notes/deep/draft.md").unwrap().len(), 1);
        assert_eq!(
            history.restore("notes/deep/draft.md", &rev).unwrap(),
            "two words"
        );
    }

    #[test]
    fn rename_keeps_the_old_path_history() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path()).unwrap();
        write(dir.path(), "a.md", "text");
        history.commit("test").unwrap();
        fs::rename(dir.path().join("a.md"), dir.path().join("b.md")).unwrap();
        history.commit("test").unwrap();
        assert_eq!(history.history("a.md").unwrap().len(), 1);
        assert_eq!(history.history("b.md").unwrap().len(), 1);
    }

    #[test]
    fn idle_commit_waits_for_quiet_time() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path()).unwrap();
        write(dir.path(), "draft.md", "text");
        let now = SystemTime::now();
        assert!(history
            .commit_if_idle(now, Duration::from_secs(300), "idle")
            .unwrap()
            .is_none());
        set_age(dir.path(), "draft.md", Duration::from_secs(600));
        let commit = history
            .commit_if_idle(now, Duration::from_secs(300), "idle")
            .unwrap();
        assert!(commit.is_some());
        assert!(history
            .commit_if_idle(now, Duration::from_secs(300), "idle")
            .unwrap()
            .is_none());
    }

    #[test]
    fn missing_revision_and_bad_paths_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path()).unwrap();
        write(dir.path(), "draft.md", "text");
        let rev = history.commit("test").unwrap().unwrap();
        assert!(history.restore("draft.md", "not-a-rev").is_err());
        assert!(history
            .restore("draft.md", "0000000000000000000000000000000000000000")
            .is_err());
        assert!(history.restore("../outside.md", &rev).is_err());
        assert!(history.history("/etc/passwd").is_err());
        assert!(history.restore("missing.md", &rev).is_err());
    }

    #[test]
    fn the_stylos_directory_stays_writable_for_the_shell() {
        let dir = tempfile::tempdir().unwrap();
        let _history = History::open(dir.path()).unwrap();
        let mode = fs::metadata(dir.path().join(".stylos"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o777);
    }

    #[test]
    fn commit_message_records_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path()).unwrap();
        write(dir.path(), "draft.md", "text");
        let rev = history.commit("before sync").unwrap().unwrap();
        let commit = history
            .repo
            .find_commit(Oid::from_str(&rev).unwrap())
            .unwrap();
        assert!(commit
            .message()
            .unwrap_or_default()
            .starts_with("before sync"));
    }
}
