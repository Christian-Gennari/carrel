//! Server-side storage: plain Markdown files on disk plus a SQLite index.
//!
//! The data directory is the user's writing folder. A file with protocol path
//! `notes/one.md` lives at `<data-dir>/notes/one.md`, so any computer can read
//! it without Stylos. The index (`<data-dir>/.stylos/index.sqlite`) only holds
//! sync metadata: file ids, revisions, hashes, tombstones and the change
//! cursor. Everything else lives in the Markdown files.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use stylos_protocol::{
    content_hash, new_pair_id, normalize_pairing_code, pairing_code_from_bytes, validate_path,
    ConflictInfo, FileContent, FileEntry, PairStatus, MAX_FILE_BYTES, PAIRING_TTL_SECONDS,
};
use uuid::Uuid;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO meta (key, value) VALUES ('next_cursor', '1');
INSERT OR IGNORE INTO meta (key, value) VALUES ('schema', '1');

CREATE TABLE IF NOT EXISTS files (
    file_id TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    rev INTEGER NOT NULL,
    hash TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    modified_at INTEGER NOT NULL,
    cursor INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS files_live_path ON files(path) WHERE deleted = 0;
CREATE INDEX IF NOT EXISTS files_cursor ON files(cursor);

CREATE TABLE IF NOT EXISTS pairings (
    pair_id TEXT PRIMARY KEY,
    device_name TEXT NOT NULL,
    device_pubkey TEXT NOT NULL,
    code TEXT NOT NULL,
    status TEXT NOT NULL,
    token TEXT UNIQUE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS pairings_code ON pairings(code);
CREATE INDEX IF NOT EXISTS pairings_status ON pairings(status);
"#;

/// A file row as stored in the index.
#[derive(Debug, Clone)]
pub struct FileRecord {
    pub file_id: Uuid,
    pub path: String,
    pub rev: u64,
    pub hash: String,
    pub deleted: bool,
    pub modified_at: i64,
}

impl FileRecord {
    fn entry(&self) -> FileEntry {
        FileEntry {
            file_id: self.file_id,
            path: self.path.clone(),
            hash: self.hash.clone(),
            rev: self.rev,
            deleted: self.deleted,
            modified_at: self.modified_at,
        }
    }

    pub fn conflict_info(&self) -> ConflictInfo {
        ConflictInfo {
            server_rev: self.rev,
            server_hash: self.hash.clone(),
            server_file_id: Some(self.file_id),
        }
    }
}

/// Result of a `PUT`.
#[derive(Debug)]
pub enum PutOutcome {
    Success { rev: u64, hash: String },
    Conflict(ConflictInfo),
}

/// Result of a `DELETE`.
#[derive(Debug)]
pub enum DeleteOutcome {
    Success { rev: u64 },
    Conflict(ConflictInfo),
}

/// A pairing row.
#[derive(Debug, Clone)]
pub struct PairingRecord {
    pub pair_id: Uuid,
    pub device_name: String,
    pub device_pubkey: String,
    pub code: String,
    pub status: PairStatus,
    pub token: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Outcome of an approval attempt.
#[derive(Debug)]
pub enum ApproveOutcome {
    Approved { token: String, device_name: String },
    AlreadyApproved,
    Expired,
    NotFound,
}

/// SQLite-backed index and file store rooted at a writing directory.
pub struct Store {
    conn: Connection,
    root: PathBuf,
}

impl Store {
    /// Open (or create) the index below `root`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let meta_dir = root.join(".stylos");
        fs::create_dir_all(&meta_dir).with_context(|| format!("create {}", meta_dir.display()))?;
        let conn = Connection::open(meta_dir.join("index.sqlite"))
            .with_context(|| format!("open index in {}", meta_dir.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn, root })
    }

    /// Data directory containing the Markdown files.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return all files changed after `since`, in cursor order, plus the
    /// cursor a client should store.
    pub fn changes_since(&mut self, since: u64) -> Result<(Vec<FileEntry>, String)> {
        let tx = self.conn.transaction()?;
        let max_cursor: i64 =
            tx.query_row("SELECT COALESCE(MAX(cursor), 0) FROM files", [], |row| {
                row.get(0)
            })?;
        let files = {
            let mut stmt = tx.prepare(
                "SELECT file_id, path, rev, hash, deleted, modified_at
                 FROM files WHERE cursor > ?1 ORDER BY cursor ASC",
            )?;
            let rows = stmt.query_map([since as i64], row_to_record)?;
            rows.collect::<rusqlite::Result<Vec<FileRecord>>>()?
        };
        tx.commit()?;
        Ok((
            files.iter().map(FileRecord::entry).collect(),
            max_cursor.to_string(),
        ))
    }

    /// Look up a live record by id. Deleted files are reported too so callers
    /// can answer conflicts with the tombstone's revision.
    pub fn record(&self, file_id: Uuid) -> Result<Option<FileRecord>> {
        let record = self
            .conn
            .query_row(
                "SELECT file_id, path, rev, hash, deleted, modified_at FROM files WHERE file_id = ?1",
                [file_id.to_string()],
                row_to_record,
            )
            .optional()?;
        Ok(record)
    }

    /// Read a live file and its content. Returns `None` for missing or
    /// deleted files.
    pub fn file_content(&self, file_id: Uuid) -> Result<Option<FileContent>> {
        let Some(record) = self.record(file_id)? else {
            return Ok(None);
        };
        if record.deleted {
            return Ok(None);
        }
        let content = match fs::read_to_string(self.root.join(&record.path)) {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("read file content"),
        };
        Ok(Some(FileContent {
            file_id: record.file_id,
            path: record.path,
            rev: record.rev,
            hash: record.hash,
            content,
        }))
    }

    /// Create or update a file with optimistic concurrency on `base_rev`.
    pub fn put(
        &mut self,
        file_id: Uuid,
        path: &str,
        content: &str,
        base_rev: Option<u64>,
        now: i64,
    ) -> Result<PutOutcome> {
        validate_path(path).map_err(|error| anyhow!(error))?;
        if content.len() > MAX_FILE_BYTES {
            bail!("file is larger than {MAX_FILE_BYTES} bytes");
        }
        let hash = content_hash(content.as_bytes());

        let tx = self.conn.transaction()?;
        let existing = record_in(&tx, file_id)?;
        let path_owner = live_record_by_path(&tx, path)?;
        if let Some(owner) = &path_owner {
            if owner.file_id != file_id {
                return Ok(PutOutcome::Conflict(owner.conflict_info()));
            }
        }

        let next_rev = match &existing {
            None => {
                if base_rev.is_some() {
                    return Ok(PutOutcome::Conflict(ConflictInfo {
                        server_rev: 0,
                        server_hash: String::new(),
                        server_file_id: None,
                    }));
                }
                1
            }
            Some(record) => {
                if base_rev != Some(record.rev) {
                    return Ok(PutOutcome::Conflict(record.conflict_info()));
                }
                record.rev + 1
            }
        };

        // Write the Markdown file first. If the database update then fails the
        // client sees an error and retries with the same base_rev; every step
        // is idempotent, so re-writing the same bytes is harmless.
        write_content(&self.root, path, content)?;
        let cursor = next_cursor(&tx)?;
        match &existing {
            Some(record) => {
                tx.execute(
                    "UPDATE files SET path = ?1, rev = ?2, hash = ?3, deleted = 0,
                        modified_at = ?4, cursor = ?5 WHERE file_id = ?6",
                    params![
                        path,
                        next_rev as i64,
                        hash,
                        now,
                        cursor,
                        file_id.to_string()
                    ],
                )?;
                if record.path != path && !record.deleted {
                    remove_content(&self.root, &record.path)?;
                }
            }
            None => {
                tx.execute(
                    "INSERT INTO files (file_id, path, rev, hash, deleted, modified_at, cursor)
                     VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
                    params![
                        file_id.to_string(),
                        path,
                        next_rev as i64,
                        hash,
                        now,
                        cursor
                    ],
                )?;
            }
        }
        tx.commit()?;
        Ok(PutOutcome::Success {
            rev: next_rev,
            hash,
        })
    }

    /// Tombstone a file. Deleting a file that never existed is a no-op so a
    /// retried delete stays idempotent.
    pub fn delete(&mut self, file_id: Uuid, base_rev: u64, now: i64) -> Result<DeleteOutcome> {
        let tx = self.conn.transaction()?;
        let Some(record) = record_in(&tx, file_id)? else {
            return Ok(DeleteOutcome::Success { rev: 0 });
        };
        if record.rev != base_rev {
            return Ok(DeleteOutcome::Conflict(record.conflict_info()));
        }
        if record.deleted {
            return Ok(DeleteOutcome::Success { rev: record.rev });
        }
        let next_rev = record.rev + 1;
        let cursor = next_cursor(&tx)?;
        tx.execute(
            "UPDATE files SET rev = ?1, deleted = 1, modified_at = ?2, cursor = ?3
             WHERE file_id = ?4",
            params![next_rev as i64, now, cursor, file_id.to_string()],
        )?;
        tx.commit()?;
        remove_content(&self.root, &record.path)?;
        Ok(DeleteOutcome::Success { rev: next_rev })
    }

    /// Add a pending pairing, generating a code that is not already pending.
    pub fn create_pairing(
        &mut self,
        device_name: &str,
        device_pubkey: &str,
        random_bytes: &[u8],
        now: i64,
    ) -> Result<Option<(Uuid, String, i64)>> {
        let Some(code) = pairing_code_from_bytes(random_bytes) else {
            bail!("not enough random bytes for a pairing code");
        };
        let taken: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pairings
                WHERE code = ?1 AND status = 'pending' AND expires_at > ?2)",
            params![code, now],
            |row| row.get(0),
        )?;
        if taken {
            return Ok(None);
        }
        let pair_id = new_pair_id();
        let expires_at = now + PAIRING_TTL_SECONDS;
        self.conn.execute(
            "INSERT INTO pairings
                (pair_id, device_name, device_pubkey, code, status, token, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', NULL, ?5, ?6)",
            params![
                pair_id.to_string(),
                device_name,
                device_pubkey,
                code,
                now,
                expires_at
            ],
        )?;
        Ok(Some((pair_id, code, expires_at)))
    }

    /// Look up a pairing, marking it expired first when its time is up.
    pub fn pairing(&mut self, pair_id: Uuid, now: i64) -> Result<Option<PairingRecord>> {
        self.expire_stale(now)?;
        let record = self
            .conn
            .query_row(
                "SELECT pair_id, device_name, device_pubkey, code, status, token,
                        created_at, expires_at
                 FROM pairings WHERE pair_id = ?1",
                [pair_id.to_string()],
                row_to_pairing,
            )
            .optional()?;
        Ok(record)
    }

    /// Pending pairings, newest first, for the approval page.
    pub fn pending_pairings(&mut self, now: i64) -> Result<Vec<PairingRecord>> {
        self.expire_stale(now)?;
        let mut stmt = self.conn.prepare(
            "SELECT pair_id, device_name, device_pubkey, code, status, token,
                    created_at, expires_at
             FROM pairings WHERE status = 'pending' ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_pairing)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Approve a pending pairing either by id (button) or by code (typed).
    pub fn approve(
        &mut self,
        pair_id: Option<Uuid>,
        code: Option<&str>,
        token: Option<String>,
        now: i64,
    ) -> Result<ApproveOutcome> {
        self.expire_stale(now)?;
        let record = match pair_id {
            Some(pair_id) => self
                .conn
                .query_row(
                    "SELECT pair_id, device_name, device_pubkey, code, status, token,
                        created_at, expires_at
                 FROM pairings WHERE pair_id = ?1",
                    [pair_id.to_string()],
                    row_to_pairing,
                )
                .optional()?,
            None => {
                let Some(code) = code.and_then(normalize_pairing_code) else {
                    return Ok(ApproveOutcome::NotFound);
                };
                self.conn
                    .query_row(
                        "SELECT pair_id, device_name, device_pubkey, code, status, token,
                                created_at, expires_at
                         FROM pairings WHERE code = ?1
                         ORDER BY created_at DESC LIMIT 1",
                        [code],
                        row_to_pairing,
                    )
                    .optional()?
            }
        };
        let Some(record) = record else {
            return Ok(ApproveOutcome::NotFound);
        };
        match record.status {
            PairStatus::Approved => Ok(ApproveOutcome::AlreadyApproved),
            PairStatus::Expired => Ok(ApproveOutcome::Expired),
            PairStatus::Pending => {
                if record.expires_at <= now {
                    self.expire_stale(now)?;
                    return Ok(ApproveOutcome::Expired);
                }
                let token = token.unwrap_or_else(random_token);
                self.conn.execute(
                    "UPDATE pairings SET status = 'approved', token = ?1 WHERE pair_id = ?2",
                    params![token, record.pair_id.to_string()],
                )?;
                Ok(ApproveOutcome::Approved {
                    token,
                    device_name: record.device_name,
                })
            }
        }
    }

    /// True when `token` belongs to an approved pairing.
    pub fn token_valid(&self, token: &str) -> Result<bool> {
        let valid: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pairings WHERE token = ?1 AND status = 'approved')",
            [token],
            |row| row.get(0),
        )?;
        Ok(valid)
    }

    /// Remove all pairings (used by tests and by a future unpair action).
    pub fn clear_pairings(&mut self) -> Result<()> {
        self.conn.execute("DELETE FROM pairings", [])?;
        Ok(())
    }

    fn expire_stale(&mut self, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE pairings SET status = 'expired'
             WHERE status = 'pending' AND expires_at <= ?1",
            [now],
        )?;
        Ok(())
    }
}

fn record_in(tx: &Transaction<'_>, file_id: Uuid) -> Result<Option<FileRecord>> {
    let record = tx
        .query_row(
            "SELECT file_id, path, rev, hash, deleted, modified_at FROM files WHERE file_id = ?1",
            [file_id.to_string()],
            row_to_record,
        )
        .optional()?;
    Ok(record)
}

fn live_record_by_path(tx: &Transaction<'_>, path: &str) -> Result<Option<FileRecord>> {
    let record = tx
        .query_row(
            "SELECT file_id, path, rev, hash, deleted, modified_at
             FROM files WHERE path = ?1 AND deleted = 0",
            [path],
            row_to_record,
        )
        .optional()?;
    Ok(record)
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileRecord> {
    let file_id: String = row.get(0)?;
    Ok(FileRecord {
        file_id: Uuid::parse_str(&file_id).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        path: row.get(1)?,
        rev: row.get::<_, i64>(2)? as u64,
        hash: row.get(3)?,
        deleted: row.get::<_, i64>(4)? != 0,
        modified_at: row.get(5)?,
    })
}

fn row_to_pairing(row: &rusqlite::Row<'_>) -> rusqlite::Result<PairingRecord> {
    let pair_id: String = row.get(0)?;
    let status: String = row.get(4)?;
    Ok(PairingRecord {
        pair_id: Uuid::parse_str(&pair_id).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        device_name: row.get(1)?,
        device_pubkey: row.get(2)?,
        code: row.get(3)?,
        status: PairStatus::parse(&status).unwrap_or(PairStatus::Expired),
        token: row.get(5)?,
        created_at: row.get(6)?,
        expires_at: row.get(7)?,
    })
}

fn next_cursor(tx: &Transaction<'_>) -> Result<i64> {
    let current: String = tx.query_row(
        "SELECT value FROM meta WHERE key = 'next_cursor'",
        [],
        |row| row.get(0),
    )?;
    let current: i64 = current.parse().context("parse next_cursor")?;
    tx.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'next_cursor'",
        [(current + 1).to_string()],
    )?;
    Ok(current)
}

fn write_content(root: &Path, path: &str, content: &str) -> Result<()> {
    let target = root.join(path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create directory {}", parent.display()))?;
    }
    let tmp = target.with_extension("md.tmp");
    {
        use std::io::Write;
        let mut handle =
            fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        handle.write_all(content.as_bytes())?;
        handle.sync_all()?;
    }
    fs::rename(&tmp, &target)?;
    if let Some(parent) = target.parent() {
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

fn remove_content(root: &Path, path: &str) -> Result<()> {
    let target = root.join(path);
    match fs::remove_file(&target) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("remove file"),
    }
    // Remove directories that became empty, but never the root itself and
    // never `.stylos`.
    let mut current = target.parent().map(Path::to_path_buf);
    while let Some(dir) = current {
        if dir == root || dir == root.join(".stylos") {
            break;
        }
        match fs::remove_dir(&dir) {
            Ok(()) => current = dir.parent().map(Path::to_path_buf),
            Err(_) => break,
        }
    }
    Ok(())
}

/// Random 48-character hex token for a paired device.
pub fn random_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    stylos_protocol::hex_encode(&bytes)
}

/// Random 32-character hex CSRF token for the approval page.
pub fn random_csrf_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    stylos_protocol::hex_encode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stylos_protocol::new_file_id;

    fn temp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn put_get_and_list() {
        let (dir, mut store) = temp_store();
        let id = new_file_id();
        let outcome = store.put(id, "notes/one.md", "hello\n", None, 100).unwrap();
        let PutOutcome::Success { rev, hash } = outcome else {
            panic!("expected success");
        };
        assert_eq!(rev, 1);
        assert_eq!(hash, content_hash(b"hello\n"));
        assert_eq!(
            fs::read_to_string(dir.path().join("notes/one.md")).unwrap(),
            "hello\n"
        );

        let content = store.file_content(id).unwrap().unwrap();
        assert_eq!(content.content, "hello\n");
        assert_eq!(content.rev, 1);

        let (files, cursor) = store.changes_since(0).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "notes/one.md");
        assert_eq!(cursor, "1");

        let (files, cursor) = store.changes_since(1).unwrap();
        assert!(files.is_empty());
        assert_eq!(cursor, "1");
    }

    #[test]
    fn stale_put_conflicts_and_changes_nothing() {
        let (dir, mut store) = temp_store();
        let id = new_file_id();
        store.put(id, "a.md", "one", None, 100).unwrap();
        let outcome = store.put(id, "a.md", "two", Some(0), 101).unwrap();
        let PutOutcome::Conflict(conflict) = outcome else {
            panic!("expected conflict");
        };
        assert_eq!(conflict.server_rev, 1);
        assert_eq!(conflict.server_hash, content_hash(b"one"));
        assert_eq!(fs::read_to_string(dir.path().join("a.md")).unwrap(), "one");
        assert_eq!(store.file_content(id).unwrap().unwrap().rev, 1);
    }

    #[test]
    fn new_file_requires_null_base_rev() {
        let (_dir, mut store) = temp_store();
        let id = new_file_id();
        let outcome = store.put(id, "a.md", "one", Some(3), 100).unwrap();
        assert!(matches!(outcome, PutOutcome::Conflict(_)));
        assert!(store.record(id).unwrap().is_none());
    }

    #[test]
    fn path_collision_is_a_conflict() {
        let (_dir, mut store) = temp_store();
        let first = new_file_id();
        let second = new_file_id();
        store.put(first, "same.md", "first", None, 100).unwrap();
        let outcome = store.put(second, "same.md", "second", None, 101).unwrap();
        let PutOutcome::Conflict(conflict) = outcome else {
            panic!("expected conflict");
        };
        assert_eq!(conflict.server_file_id, Some(first));
        assert_eq!(conflict.server_hash, content_hash(b"first"));
    }

    #[test]
    fn delete_requires_matching_base_rev() {
        let (dir, mut store) = temp_store();
        let id = new_file_id();
        store.put(id, "a.md", "one", None, 100).unwrap();

        let outcome = store.delete(id, 0, 101).unwrap();
        let DeleteOutcome::Conflict(conflict) = outcome else {
            panic!("expected conflict");
        };
        assert_eq!(conflict.server_rev, 1);
        assert!(dir.path().join("a.md").exists());

        let outcome = store.delete(id, 1, 102).unwrap();
        let DeleteOutcome::Success { rev } = outcome else {
            panic!("expected success");
        };
        assert_eq!(rev, 2);
        assert!(!dir.path().join("a.md").exists());

        let (files, _) = store.changes_since(1).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].deleted);
        assert_eq!(files[0].rev, 2);

        // Retrying the same delete is idempotent.
        let outcome = store.delete(id, 2, 103).unwrap();
        assert!(matches!(outcome, DeleteOutcome::Success { rev: 2 }));
    }

    #[test]
    fn delete_of_unknown_file_is_a_noop() {
        let (_dir, mut store) = temp_store();
        let outcome = store.delete(new_file_id(), 1, 100).unwrap();
        assert!(matches!(outcome, DeleteOutcome::Success { rev: 0 }));
    }

    #[test]
    fn path_can_be_reused_after_delete() {
        let (_dir, mut store) = temp_store();
        let first = new_file_id();
        store.put(first, "a.md", "one", None, 100).unwrap();
        store.delete(first, 1, 101).unwrap();
        let second = new_file_id();
        let outcome = store.put(second, "a.md", "two", None, 102).unwrap();
        assert!(matches!(outcome, PutOutcome::Success { rev: 1, .. }));
        assert_eq!(store.file_content(second).unwrap().unwrap().content, "two");
        assert!(store.file_content(first).unwrap().is_none());
    }

    #[test]
    fn resurrecting_a_tombstone_works() {
        let (_dir, mut store) = temp_store();
        let id = new_file_id();
        store.put(id, "a.md", "one", None, 100).unwrap();
        store.delete(id, 1, 101).unwrap();
        let outcome = store.put(id, "a.md", "back", Some(2), 102).unwrap();
        assert!(matches!(outcome, PutOutcome::Success { rev: 3, .. }));
        assert_eq!(store.file_content(id).unwrap().unwrap().content, "back");
    }

    #[test]
    fn rename_moves_the_file() {
        let (dir, mut store) = temp_store();
        let id = new_file_id();
        store.put(id, "old.md", "text", None, 100).unwrap();
        let outcome = store.put(id, "new.md", "text", Some(1), 101).unwrap();
        assert!(matches!(outcome, PutOutcome::Success { rev: 2, .. }));
        assert!(!dir.path().join("old.md").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("new.md")).unwrap(),
            "text"
        );
    }

    #[test]
    fn unsafe_paths_are_rejected() {
        let (dir, mut store) = temp_store();
        for path in ["../escape.md", "/abs.md", ".hidden/a.md", "a.txt"] {
            assert!(
                store.put(new_file_id(), path, "x", None, 100).is_err(),
                "{path}"
            );
        }
        assert!(!dir.path().parent().unwrap().join("escape.md").exists());
    }

    #[test]
    fn pairing_lifecycle() {
        let (_dir, mut store) = temp_store();
        let (pair_id, code, expires_at) = store
            .create_pairing("X250", "abcd", &[1, 2, 3, 4, 5, 6], 100)
            .unwrap()
            .unwrap();
        assert_eq!(code.len(), 7);
        assert_eq!(expires_at, 100 + PAIRING_TTL_SECONDS);
        assert_eq!(store.pending_pairings(100).unwrap().len(), 1);

        let pending = store.pairing(pair_id, 101).unwrap().unwrap();
        assert_eq!(pending.status, PairStatus::Pending);
        assert!(pending.token.is_none());
        assert!(!store.token_valid("nope").unwrap());

        let approved = store
            .approve(Some(pair_id), None, Some("tok".into()), 102)
            .unwrap();
        assert!(matches!(approved, ApproveOutcome::Approved { .. }));
        let record = store.pairing(pair_id, 103).unwrap().unwrap();
        assert_eq!(record.status, PairStatus::Approved);
        assert_eq!(record.token.as_deref(), Some("tok"));
        assert!(store.token_valid("tok").unwrap());
        assert!(store.pending_pairings(104).unwrap().is_empty());

        let again = store.approve(None, Some("K7Q-4MZ"), None, 105).unwrap();
        assert!(matches!(again, ApproveOutcome::NotFound));
    }

    #[test]
    fn pairing_by_code_and_expiry() {
        let (_dir, mut store) = temp_store();
        let (_pair_id, code, _) = store
            .create_pairing("X250", "abcd", &[9, 8, 7, 6, 5, 4], 100)
            .unwrap()
            .unwrap();
        let outcome = store
            .approve(None, Some(&code.to_lowercase()), Some("tok2".into()), 101)
            .unwrap();
        assert!(matches!(outcome, ApproveOutcome::Approved { .. }));
        assert!(store.token_valid("tok2").unwrap());

        let (_id, _code, expires_at) = store
            .create_pairing("T430", "abcd", &[1, 9, 2, 8, 3, 7], 200)
            .unwrap()
            .unwrap();
        assert_eq!(
            store.pairing(_id, expires_at + 1).unwrap().unwrap().status,
            PairStatus::Expired
        );
        let outcome = store
            .approve(Some(_id), None, None, expires_at + 2)
            .unwrap();
        assert!(matches!(outcome, ApproveOutcome::Expired));
    }

    #[test]
    fn cursor_advances_only_on_changes() {
        let (_dir, mut store) = temp_store();
        let (files, cursor) = store.changes_since(0).unwrap();
        assert!(files.is_empty());
        assert_eq!(cursor, "0");
        let id = new_file_id();
        store.put(id, "a.md", "one", None, 100).unwrap();
        store.put(id, "a.md", "two", Some(1), 101).unwrap();
        let (files, cursor) = store.changes_since(0).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].rev, 2);
        assert_eq!(cursor, "2");
    }
}
