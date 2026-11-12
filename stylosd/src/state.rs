//! Local sync state: last known server revision per file, the cursor and the
//! paths of conflict copies.
//!
//! Stored in `<state>/sync-state.sqlite`. The cursor and every file's revision
//! are written as soon as a step succeeds, so an interrupted sync simply
//! repeats the step; every protocol operation is idempotent.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::config::state_database;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS files (
    file_id TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    rev INTEGER NOT NULL,
    hash TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    modified_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS files_path ON files(path);

CREATE TABLE IF NOT EXISTS conflict_copies (
    remote_file_id TEXT NOT NULL,
    remote_rev INTEGER NOT NULL,
    local_path TEXT NOT NULL,
    hash TEXT NOT NULL,
    PRIMARY KEY (remote_file_id, remote_rev)
);

CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// The last-known state of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileState {
    pub file_id: Uuid,
    pub path: String,
    pub rev: u64,
    pub hash: String,
    pub deleted: bool,
    pub modified_at: i64,
}

/// A conflict copy already written for a remote revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictRecord {
    pub local_path: String,
    pub hash: String,
}

/// SQLite backed sync state.
pub struct SyncState {
    conn: Connection,
}

impl SyncState {
    /// Open `<state_dir>/sync-state.sqlite`.
    pub fn from_dir(state_dir: &Path) -> Result<Self> {
        Self::open(&state_database(state_dir))
    }

    /// Open an explicit database path.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open sync state {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Last stored cursor, `"0"` before the first sync.
    pub fn cursor(&self) -> Result<String> {
        let cursor = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'cursor'", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        Ok(cursor.unwrap_or_else(|| "0".to_string()))
    }

    /// Store the cursor after a fully applied pull.
    pub fn set_cursor(&mut self, cursor: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('cursor', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [cursor],
        )?;
        Ok(())
    }

    /// All tracked files, deleted tombstones included.
    pub fn files(&self) -> Result<Vec<FileState>> {
        let mut statement = self.conn.prepare(
            "SELECT file_id, path, rev, hash, deleted, modified_at
             FROM files ORDER BY path ASC",
        )?;
        let rows = statement.query_map([], row_to_state)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Look up a file by id.
    pub fn file_by_id(&self, file_id: Uuid) -> Result<Option<FileState>> {
        let state = self
            .conn
            .query_row(
                "SELECT file_id, path, rev, hash, deleted, modified_at
                 FROM files WHERE file_id = ?1",
                [file_id.to_string()],
                row_to_state,
            )
            .optional()?;
        Ok(state)
    }

    /// Look up a file by local path. When several rows share the path (which
    /// can only happen after an unusual rename race), prefer a live row.
    pub fn file_by_path(&self, path: &str) -> Result<Option<FileState>> {
        let state = self
            .conn
            .query_row(
                "SELECT file_id, path, rev, hash, deleted, modified_at
                 FROM files WHERE path = ?1
                 ORDER BY deleted ASC, rev DESC LIMIT 1",
                [path],
                row_to_state,
            )
            .optional()?;
        Ok(state)
    }

    /// Insert or update a file, keeping one local path per file.
    pub fn upsert(&mut self, state: &FileState) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM files WHERE path = ?1 AND file_id <> ?2",
            params![state.path, state.file_id.to_string()],
        )?;
        tx.execute(
            "INSERT INTO files (file_id, path, rev, hash, deleted, modified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(file_id) DO UPDATE SET
                 path = excluded.path,
                 rev = excluded.rev,
                 hash = excluded.hash,
                 deleted = excluded.deleted,
                 modified_at = excluded.modified_at",
            params![
                state.file_id.to_string(),
                state.path,
                state.rev as i64,
                state.hash,
                state.deleted as i64,
                state.modified_at
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Mark a tracked file as deleted at `rev`.
    pub fn mark_deleted(&mut self, file_id: Uuid, rev: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE files SET deleted = 1, rev = ?1 WHERE file_id = ?2",
            params![rev as i64, file_id.to_string()],
        )?;
        Ok(())
    }

    /// Path previously used for the conflict copy of `remote_rev`.
    pub fn conflict(
        &self,
        remote_file_id: Uuid,
        remote_rev: u64,
    ) -> Result<Option<ConflictRecord>> {
        let record = self
            .conn
            .query_row(
                "SELECT local_path, hash FROM conflict_copies
                 WHERE remote_file_id = ?1 AND remote_rev = ?2",
                params![remote_file_id.to_string(), remote_rev as i64],
                |row| {
                    Ok(ConflictRecord {
                        local_path: row.get(0)?,
                        hash: row.get(1)?,
                    })
                },
            )
            .optional()?;
        Ok(record)
    }

    /// Remember where the conflict copy for a remote revision was written.
    pub fn record_conflict(
        &mut self,
        remote_file_id: Uuid,
        remote_rev: u64,
        local_path: &str,
        hash: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO conflict_copies (remote_file_id, remote_rev, local_path, hash)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(remote_file_id, remote_rev) DO UPDATE SET
                 local_path = excluded.local_path,
                 hash = excluded.hash",
            params![
                remote_file_id.to_string(),
                remote_rev as i64,
                local_path,
                hash
            ],
        )?;
        Ok(())
    }
}

fn row_to_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileState> {
    let file_id: String = row.get(0)?;
    Ok(FileState {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_updates_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = SyncState::from_dir(dir.path()).unwrap();
        assert_eq!(state.cursor().unwrap(), "0");
        state.set_cursor("12").unwrap();
        assert_eq!(state.cursor().unwrap(), "12");

        let id = Uuid::now_v7();
        let file = FileState {
            file_id: id,
            path: "a.md".into(),
            rev: 1,
            hash: "h1".into(),
            deleted: false,
            modified_at: 10,
        };
        state.upsert(&file).unwrap();
        assert_eq!(state.file_by_id(id).unwrap(), Some(file.clone()));
        assert_eq!(state.file_by_path("a.md").unwrap(), Some(file.clone()));

        let mut renamed = file.clone();
        renamed.path = "b.md".into();
        renamed.rev = 2;
        state.upsert(&renamed).unwrap();
        assert_eq!(state.file_by_path("a.md").unwrap(), None);
        assert_eq!(state.file_by_path("b.md").unwrap(), Some(renamed));
        assert_eq!(state.files().unwrap().len(), 1);

        state.mark_deleted(id, 3).unwrap();
        assert!(state.file_by_id(id).unwrap().unwrap().deleted);
    }

    #[test]
    fn remembers_conflict_copies() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = SyncState::from_dir(dir.path()).unwrap();
        let id = Uuid::now_v7();
        assert!(state.conflict(id, 4).unwrap().is_none());
        state
            .record_conflict(id, 4, "a (conflict x).md", "hash")
            .unwrap();
        let record = state.conflict(id, 4).unwrap().unwrap();
        assert_eq!(record.local_path, "a (conflict x).md");
        assert_eq!(record.hash, "hash");
    }

    #[test]
    fn path_prefers_live_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = SyncState::from_dir(dir.path()).unwrap();
        let deleted = FileState {
            file_id: Uuid::now_v7(),
            path: "a.md".into(),
            rev: 2,
            hash: "old".into(),
            deleted: true,
            modified_at: 1,
        };
        let live = FileState {
            file_id: Uuid::now_v7(),
            path: "a.md".into(),
            rev: 1,
            hash: "new".into(),
            deleted: false,
            modified_at: 2,
        };
        state.upsert(&deleted).unwrap();
        state.upsert(&live).unwrap();
        assert_eq!(state.file_by_path("a.md").unwrap().unwrap(), live);
    }
}
