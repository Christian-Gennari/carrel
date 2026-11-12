//! The client sync algorithm from `spec/02-sync-protocol.md`.
//!
//! Order: pull, handle conflicts, push, save the cursor and revisions. Every
//! step is idempotent, so a sync that dies halfway can simply run again. A
//! conflict never merges text: the local file stays where it is and the other
//! version is written next to it as
//! `name (conflict YYYY-MM-DD HHMM from <source>).md`, then both are pushed.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use stylos_protocol::{
    conflict_copy_path, content_hash, endpoints, format_conflict_timestamp, new_file_id, now_unix,
    validate_path, verify_content_hash, ConflictResponse, DeleteResponse, FileContent, FileEntry,
    FileList, PutFileRequest, PutFileResponse,
};
use uuid::Uuid;

use crate::config::SyncConfig;
use crate::http::HttpClient;
use crate::state::{FileState, SyncState};

/// Counters for one sync, used by the CLI, the Sync screen and tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Remote files whose content was downloaded and written locally.
    pub pulled: usize,
    /// Local files created or updated on the server.
    pub pushed: usize,
    /// Files removed locally because the server deleted them.
    pub deleted_local: usize,
    /// Files deleted on the server because they disappeared locally.
    pub deleted_remote: usize,
    /// Conflict copies written.
    pub conflicts: usize,
    /// Local files skipped because they are not valid Stylos Markdown paths
    /// or are not UTF-8.
    pub skipped_local: usize,
    /// Cursor saved at the end of this sync.
    pub cursor: String,
}

impl SyncReport {
    /// Number of content transfers (pull, push, delete).
    pub fn transferred(&self) -> usize {
        self.pulled + self.pushed + self.deleted_local + self.deleted_remote
    }

    /// True when the sync changed nothing at all.
    pub fn is_noop(&self) -> bool {
        self.transferred() == 0 && self.conflicts == 0
    }
}

/// Tuning and test hooks for [`sync_once`].
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// Test hook: stop with [`SyncError::Interrupted`] after this many
    /// content-changing operations. The cursor and per-file revisions are only
    /// saved when each step succeeds, so a rerun resumes safely.
    pub interrupt_after: Option<usize>,
}

/// Errors with meaning to callers (especially tests).
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("sync interrupted after {0} operation(s)")]
    Interrupted(usize),
}

/// Run one sync against the server configured in `<state_dir>/sync.toml`.
///
/// Returns when the sync has completed; any network error ends it immediately
/// (no background retries, per the rules).
pub async fn sync_once(
    writing_dir: &Path,
    state_dir: &Path,
    options: &SyncOptions,
) -> Result<SyncReport> {
    let config = SyncConfig::load(state_dir)?;
    let client =
        HttpClient::new(&config.server_url, config.tls_mode())?.with_token(config.token.clone());
    let state = SyncState::from_dir(state_dir)?;
    fs::create_dir_all(writing_dir)
        .with_context(|| format!("create writing directory {}", writing_dir.display()))?;

    let mut run = Run {
        client,
        state,
        dir: writing_dir.to_path_buf(),
        source: config.source_label(),
        options,
        report: SyncReport::default(),
        ops: 0,
        now: now_unix(),
    };

    run.pull().await?;
    run.push().await?;
    let cursor = run.state.cursor()?;
    run.report.cursor = cursor;
    Ok(run.report)
}

struct Run<'a> {
    client: HttpClient,
    state: SyncState,
    dir: PathBuf,
    source: String,
    options: &'a SyncOptions,
    report: SyncReport,
    ops: usize,
    now: i64,
}

impl Run<'_> {
    async fn pull(&mut self) -> Result<()> {
        let since = self.state.cursor()?;
        let list: FileList = self
            .client
            .get_json(&endpoints::files_since(&since))
            .await?;
        for entry in &list.files {
            validate_path(&entry.path)
                .with_context(|| format!("server sent an invalid path {:?}", entry.path))?;
            if entry.deleted {
                self.handle_remote_deleted(entry).await?;
            } else {
                self.handle_remote_changed(entry).await?;
            }
        }
        // The cursor is saved only after every change in the list was applied.
        self.state.set_cursor(&list.cursor)?;
        self.report.cursor = list.cursor;
        Ok(())
    }

    async fn handle_remote_changed(&mut self, entry: &FileEntry) -> Result<()> {
        let state = self.state.file_by_id(entry.file_id)?;

        if let Some(state) = &state {
            if state.path != entry.path {
                return self.handle_remote_rename(entry, state).await;
            }
            if state.rev == entry.rev {
                // We already have this exact revision (typically our own push
                // seen again because the cursor lags local pushes). Any local
                // edit is uploaded by the push phase.
                return Ok(());
            }
        }

        let local = self.read_local(&entry.path)?;
        match (state, local) {
            (None, None) => self.apply_remote(entry).await,
            (None, Some(local_content)) => {
                if content_hash(local_content.as_bytes()) == entry.hash {
                    self.state.upsert(&state_from_entry(entry))?;
                    Ok(())
                } else {
                    self.conflict_with_remote(entry, &entry.path).await
                }
            }
            (Some(state), None) => {
                if state.deleted {
                    // The server resurrected a file we had deleted.
                    self.apply_remote(entry).await
                } else if state.hash == entry.hash {
                    // Metadata-only change; the local deletion stays pending.
                    self.state.upsert(&state_from_entry(entry))?;
                    Ok(())
                } else {
                    // Edited remotely, deleted locally: keep both versions.
                    let remote = self.fetch_content(entry).await?;
                    self.ensure_conflict_copy(&entry.path, &remote)?;
                    self.state.upsert(&state_from_entry(entry))?;
                    self.report.conflicts += 1;
                    self.tick()?;
                    Ok(())
                }
            }
            (Some(state), Some(local_content)) => {
                let local_hash = content_hash(local_content.as_bytes());
                if state.deleted {
                    if local_hash == entry.hash {
                        self.state.upsert(&state_from_entry(entry))?;
                        Ok(())
                    } else {
                        self.conflict_with_remote(entry, &entry.path).await
                    }
                } else if local_hash == state.hash {
                    if state.hash == entry.hash {
                        // Same content, new revision or timestamp.
                        self.state.upsert(&state_from_entry(entry))?;
                        Ok(())
                    } else {
                        self.apply_remote(entry).await
                    }
                } else if local_hash == entry.hash {
                    // Both sides arrived at the same text.
                    self.state.upsert(&state_from_entry(entry))?;
                    Ok(())
                } else {
                    self.conflict_with_remote(entry, &entry.path).await
                }
            }
        }
    }

    /// The server moved a file to a new path.
    async fn handle_remote_rename(&mut self, entry: &FileEntry, state: &FileState) -> Result<()> {
        let old_path = state.path.clone();
        let Some(local_content) = self.read_local(&old_path)? else {
            if state.hash != entry.hash {
                let remote = self.fetch_content(entry).await?;
                self.ensure_conflict_copy(&entry.path, &remote)?;
                self.report.conflicts += 1;
            }
            // The file is gone locally; the push phase deletes it on the
            // server with the fresh revision.
            self.state.upsert(&state_from_entry(entry))?;
            return Ok(());
        };

        let local_hash = content_hash(local_content.as_bytes());
        if local_hash != state.hash {
            // Local edits plus a remote rename: keep both. The remote copy
            // stays at its new path; the edited local file is pushed as a new
            // file at the old path.
            self.state.upsert(&state_from_entry(entry))?;
            self.report.conflicts += 1;
            return Ok(());
        }

        // Unchanged locally: follow the rename.
        if let Some(existing) = self.read_local(&entry.path)? {
            if content_hash(existing.as_bytes()) != entry.hash {
                // Another local file already uses the new path. Keep it and
                // preserve the remote version in a conflict copy.
                let remote = self.fetch_content(entry).await?;
                self.ensure_conflict_copy(&entry.path, &remote)?;
                self.state.upsert(&state_from_entry(entry))?;
                self.report.conflicts += 1;
                self.tick()?;
                return Ok(());
            }
        }

        if local_hash == entry.hash {
            self.write_local(&entry.path, &local_content)?;
            self.remove_local(&old_path)?;
            self.state.upsert(&state_from_entry(entry))?;
            self.report.pulled += 1;
            self.tick()?;
        } else {
            let remote = self.fetch_content(entry).await?;
            self.write_local(&entry.path, &remote.content)?;
            self.remove_local(&old_path)?;
            self.state.upsert(&state_from_entry(entry))?;
            self.report.pulled += 1;
            self.tick()?;
        }
        Ok(())
    }

    async fn handle_remote_deleted(&mut self, entry: &FileEntry) -> Result<()> {
        let Some(state) = self.state.file_by_id(entry.file_id)? else {
            // We never knew this file. A local file at the same path, if any,
            // is a new file and will be pushed normally.
            return Ok(());
        };
        if state.deleted {
            return Ok(());
        }
        match self.read_local(&state.path)? {
            None => {
                self.state.mark_deleted(entry.file_id, entry.rev)?;
                Ok(())
            }
            Some(content) if content_hash(content.as_bytes()) == state.hash => {
                self.remove_local(&state.path)?;
                self.state.mark_deleted(entry.file_id, entry.rev)?;
                self.report.deleted_local += 1;
                self.tick()?;
                Ok(())
            }
            Some(_) => {
                // Local edits versus remote deletion: keep the local text and
                // let the push phase resurrect it at the fresh revision.
                self.state.upsert(&FileState {
                    file_id: entry.file_id,
                    path: state.path.clone(),
                    rev: entry.rev,
                    // Keep the old hash so the push phase sees a local change.
                    hash: state.hash.clone(),
                    deleted: false,
                    modified_at: entry.modified_at,
                })?;
                self.report.conflicts += 1;
                Ok(())
            }
        }
    }

    async fn apply_remote(&mut self, entry: &FileEntry) -> Result<()> {
        let remote = self.fetch_content(entry).await?;
        self.write_local(&entry.path, &remote.content)?;
        self.state.upsert(&state_from_entry(entry))?;
        self.report.pulled += 1;
        self.tick()?;
        Ok(())
    }

    /// Keep the local file at `base_path` and write the server's version next
    /// to it. The state records the server revision, so the push phase then
    /// uploads the local version over it and both copies survive.
    async fn conflict_with_remote(&mut self, entry: &FileEntry, base_path: &str) -> Result<()> {
        let remote = self.fetch_content(entry).await?;
        self.ensure_conflict_copy(base_path, &remote)?;
        self.state.upsert(&state_from_entry(entry))?;
        self.report.conflicts += 1;
        self.tick()?;
        Ok(())
    }

    async fn fetch_content(&self, entry: &FileEntry) -> Result<FileContent> {
        let content: FileContent = self
            .client
            .get_json(&endpoints::file(entry.file_id))
            .await?;
        if !verify_content_hash(content.content.as_bytes(), &entry.hash) {
            bail!(
                "server sent content that does not match the hash for {}",
                entry.path
            );
        }
        if content.rev != entry.rev {
            bail!(
                "server changed {} while syncing (rev {} != {}); run sync again",
                entry.path,
                content.rev,
                entry.rev
            );
        }
        Ok(content)
    }

    /// Materialize the server's version as a conflict copy, reusing the file
    /// and name from a previous attempt at the same revision.
    fn ensure_conflict_copy(&mut self, base_path: &str, remote: &FileContent) -> Result<String> {
        if let Some(record) = self.state.conflict(remote.file_id, remote.rev)? {
            if let Some(existing) = self.read_local(&record.local_path)? {
                if content_hash(existing.as_bytes()) == record.hash {
                    return Ok(record.local_path);
                }
            }
        }
        let path = self.allocate_conflict_path(base_path, &remote.content)?;
        self.write_local(&path, &remote.content)?;
        self.state
            .record_conflict(remote.file_id, remote.rev, &path, &remote.hash)?;
        Ok(path)
    }

    /// Find a free conflict-copy name in the spec's exact shape. If a copy for
    /// the same minute already exists with different content, try the next
    /// minute (two devices can collide, and the timestamp stays well formed).
    fn allocate_conflict_path(&self, base_path: &str, content: &str) -> Result<String> {
        for minute in 0..240 {
            let timestamp = format_conflict_timestamp(self.now + minute * 60);
            let candidate = conflict_copy_path(base_path, &timestamp, &self.source);
            match self.read_local(&candidate)? {
                None => return Ok(candidate),
                Some(existing) if existing == content => return Ok(candidate),
                Some(_) => continue,
            }
        }
        bail!("could not find a free name for the conflict copy of {base_path}")
    }

    async fn push(&mut self) -> Result<()> {
        let mut passes = 0usize;
        loop {
            passes += 1;
            if passes > 64 {
                bail!("sync did not converge after 64 push passes");
            }
            let (local, skipped) = scan_local(&self.dir)?;
            self.report.skipped_local = skipped;

            let mut did_work = false;
            for file in local.values() {
                let state = self.state.file_by_path(&file.path)?;
                let needs_push = match &state {
                    None => true,
                    Some(state) if state.deleted => true,
                    Some(state) => state.hash != file.hash,
                };
                if needs_push {
                    self.push_file(file, state.as_ref()).await?;
                    did_work = true;
                }
            }

            for state in self.state.files()? {
                if state.deleted || local.contains_key(&state.path) {
                    continue;
                }
                self.push_delete(&state).await?;
                did_work = true;
            }

            if !did_work {
                break;
            }
        }
        Ok(())
    }

    async fn push_file(&mut self, file: &LocalFile, state: Option<&FileState>) -> Result<()> {
        let (file_id, base_rev) = match state {
            Some(state) if !state.deleted => (state.file_id, Some(state.rev)),
            _ => (new_file_id(), None),
        };
        self.put_with_conflicts(file_id, &file.path, &file.content, &file.hash, base_rev)
            .await
    }

    async fn put_with_conflicts(
        &mut self,
        file_id: Uuid,
        path: &str,
        content: &str,
        hash: &str,
        base_rev: Option<u64>,
    ) -> Result<()> {
        let mut base = base_rev;
        for attempt in 0..4 {
            let request = PutFileRequest {
                path: path.to_string(),
                content: content.to_string(),
                base_rev: base,
            };
            let response = self
                .client
                .put_json(&endpoints::file(file_id), &request)
                .await?;

            if response.is_success() {
                let success: PutFileResponse = response.json()?;
                if success.hash != hash {
                    bail!("server stored a different hash for {path}");
                }
                self.state.upsert(&FileState {
                    file_id,
                    path: path.to_string(),
                    rev: success.rev,
                    hash: hash.to_string(),
                    deleted: false,
                    modified_at: self.now,
                })?;
                self.report.pushed += 1;
                self.tick()?;
                return Ok(());
            }

            if response.status != 409 {
                bail!(
                    "writing {path} failed: HTTP {}: {}",
                    response.status,
                    response.error_message()
                );
            }
            let conflict: ConflictResponse = response.json()?;

            // The server does not know this file at all (for example after a
            // server restore). Recreate it as a new file.
            if conflict.conflict.server_rev == 0
                && conflict.conflict.server_file_id.is_none()
                && base.is_some()
            {
                base = None;
                if attempt == 3 {
                    bail!("too many conflicts while writing {path}");
                }
                continue;
            }

            if let Some(other_id) = conflict.conflict.server_file_id {
                if other_id != file_id {
                    // Another file (different file_id) owns this path. Move it
                    // to a conflict-copy name, then retry our write.
                    self.move_server_file_to_conflict(other_id, path).await?;
                    base = base_rev;
                    if attempt == 3 {
                        bail!("too many path conflicts while writing {path}");
                    }
                    continue;
                }
            }

            // Our file changed on the server. Preserve that version as a
            // conflict copy, then retry on top of the server's revision.
            let remote: FileContent = self.client.get_json(&endpoints::file(file_id)).await?;
            self.ensure_conflict_copy(path, &remote)?;
            self.report.conflicts += 1;
            self.tick()?;
            base = Some(conflict.conflict.server_rev);
            if attempt == 3 {
                bail!("too many conflicts while writing {path}");
            }
        }
        bail!("too many conflicts while writing {path}")
    }

    async fn move_server_file_to_conflict(&mut self, other_id: Uuid, path: &str) -> Result<()> {
        let other: FileContent = self.client.get_json(&endpoints::file(other_id)).await?;
        let copy_path = self.ensure_conflict_copy(path, &other)?;
        let request = PutFileRequest {
            path: copy_path.clone(),
            content: other.content.clone(),
            base_rev: Some(other.rev),
        };
        let response = self
            .client
            .put_json(&endpoints::file(other_id), &request)
            .await?;
        if !response.is_success() {
            bail!(
                "moving the conflicting file away failed: HTTP {}: {}",
                response.status,
                response.error_message()
            );
        }
        let success: PutFileResponse = response.json()?;
        self.state.upsert(&FileState {
            file_id: other_id,
            path: copy_path,
            rev: success.rev,
            hash: success.hash,
            deleted: false,
            modified_at: self.now,
        })?;
        self.report.pushed += 1;
        self.tick()?;
        Ok(())
    }

    async fn push_delete(&mut self, state: &FileState) -> Result<()> {
        let mut base = state.rev;
        for attempt in 0..4 {
            let response = self
                .client
                .delete(&endpoints::file_delete(state.file_id, base))
                .await?;
            if response.is_success() {
                let rev = response
                    .json::<DeleteResponse>()
                    .map(|deleted| deleted.rev)
                    .unwrap_or(base + 1);
                self.state.mark_deleted(state.file_id, rev)?;
                self.report.deleted_remote += 1;
                self.tick()?;
                return Ok(());
            }
            if response.status != 409 {
                bail!(
                    "deleting {} failed: HTTP {}: {}",
                    state.path,
                    response.status,
                    response.error_message()
                );
            }
            let conflict: ConflictResponse = response.json()?;
            // The server has a newer version than the one we are deleting.
            // Preserve it as a conflict copy before retrying.
            match self.client.get(&endpoints::file(state.file_id)).await {
                Ok(response) if response.is_success() => {
                    let remote: FileContent = response.json()?;
                    self.ensure_conflict_copy(&state.path, &remote)?;
                    self.report.conflicts += 1;
                    self.tick()?;
                }
                Ok(response) if response.status == 404 => {}
                Ok(response) => bail!(
                    "reading {} before delete failed: HTTP {}: {}",
                    state.path,
                    response.status,
                    response.error_message()
                ),
                Err(error) => return Err(error),
            }
            base = conflict.conflict.server_rev;
            if attempt == 3 {
                bail!("too many conflicts while deleting {}", state.path);
            }
        }
        bail!("too many conflicts while deleting {}", state.path)
    }

    fn tick(&mut self) -> Result<()> {
        self.ops += 1;
        if self.options.interrupt_after == Some(self.ops) {
            return Err(SyncError::Interrupted(self.ops).into());
        }
        Ok(())
    }

    fn read_local(&self, relative: &str) -> Result<Option<String>> {
        let target = self.dir.join(relative);
        match fs::read_to_string(&target) {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("read {}", target.display())),
        }
    }

    fn write_local(&self, relative: &str, content: &str) -> Result<()> {
        write_atomic(&self.dir, relative, content)
    }

    fn remove_local(&self, relative: &str) -> Result<()> {
        remove_file(&self.dir, relative)
    }
}

struct LocalFile {
    path: String,
    content: String,
    hash: String,
}

fn state_from_entry(entry: &FileEntry) -> FileState {
    FileState {
        file_id: entry.file_id,
        path: entry.path.clone(),
        rev: entry.rev,
        hash: entry.hash.clone(),
        deleted: false,
        modified_at: entry.modified_at,
    }
}

fn scan_local(root: &Path) -> Result<(BTreeMap<String, LocalFile>, usize)> {
    let mut files = BTreeMap::new();
    let mut skipped = 0usize;
    walk(root, root, &mut files, &mut skipped)?;
    Ok((files, skipped))
}

fn walk(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, LocalFile>,
    skipped: &mut usize,
) -> Result<()> {
    let entries = fs::read_dir(directory)
        .with_context(|| format!("read directory {}", directory.display()))?;
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            // Never follow links out of the writing directory.
            *skipped += 1;
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            *skipped += 1;
            continue;
        };
        if name.starts_with('.') {
            // `.stylos`, editor temp files and journal directories.
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            walk(root, &path, files, skipped)?;
            continue;
        }
        if !file_type.is_file() || !name.ends_with(".md") {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            *skipped += 1;
            continue;
        };
        let Some(relative) = relative.to_str() else {
            *skipped += 1;
            continue;
        };
        let relative = relative.replace(std::path::MAIN_SEPARATOR, "/");
        if validate_path(&relative).is_err() {
            *skipped += 1;
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(content) => {
                let hash = content_hash(content.as_bytes());
                files.insert(
                    relative.clone(),
                    LocalFile {
                        path: relative,
                        content,
                        hash,
                    },
                );
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                // Not UTF-8 text; never overwrite or upload it.
                *skipped += 1;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("read {}", path.display()));
            }
        }
    }
    Ok(())
}

fn write_atomic(root: &Path, relative: &str, content: &str) -> Result<()> {
    validate_path(relative)?;
    let target = root.join(relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create directory {}", parent.display()))?;
    }
    let temporary = target.with_extension("md.tmp");
    {
        use std::io::Write;
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&temporary, &target).with_context(|| format!("rename to {}", target.display()))?;
    if let Some(parent) = target.parent() {
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(())
}

fn remove_file(root: &Path, relative: &str) -> Result<()> {
    validate_path(relative)?;
    let target = root.join(relative);
    match fs::remove_file(&target) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("remove {}", target.display())),
    }
    let mut current = target.parent().map(Path::to_path_buf);
    while let Some(directory) = current {
        if directory == root {
            break;
        }
        match fs::remove_dir(&directory) {
            Ok(()) => current = directory.parent().map(Path::to_path_buf),
            Err(_) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_finds_markdown_and_skips_junk() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("notes")).unwrap();
        fs::create_dir_all(dir.path().join(".stylos")).unwrap();
        fs::write(dir.path().join("note.md"), "one").unwrap();
        fs::write(dir.path().join("notes/two.md"), "two").unwrap();
        fs::write(dir.path().join("notes/skip.txt"), "no").unwrap();
        fs::write(dir.path().join("notes/temp.md.tmp"), "no").unwrap();
        fs::write(dir.path().join(".stylos/hidden.md"), "no").unwrap();
        let (files, skipped) = scan_local(dir.path()).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.contains_key("note.md"));
        assert!(files.contains_key("notes/two.md"));
        assert_eq!(skipped, 0);
    }

    #[test]
    fn atomic_write_creates_directories() {
        let dir = tempfile::tempdir().unwrap();
        write_atomic(dir.path(), "a/b/c.md", "hello").unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("a/b/c.md")).unwrap(),
            "hello"
        );
        assert!(!dir.path().join("a/b/c.md.tmp").exists());
    }

    #[test]
    fn remove_prunes_empty_directories() {
        let dir = tempfile::tempdir().unwrap();
        write_atomic(dir.path(), "a/b/c.md", "hello").unwrap();
        remove_file(dir.path(), "a/b/c.md").unwrap();
        assert!(!dir.path().join("a").exists());
        assert!(dir.path().exists());
    }
}
