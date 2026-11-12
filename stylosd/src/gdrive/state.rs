//! The Google Drive mirror state in `<state>/sync-state.sqlite`.
//!
//! One row per uploaded file (Markdown and optionally `.docx`): the local
//! path, Drive file ID and the SHA-256 of the local bytes that were uploaded.
//! The hash is what makes a second sync upload nothing. Rows are written only
//! after the upload succeeded, so an interrupted sync simply repeats the last
//! step.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::config::state_database;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS gdrive_files (
    path TEXT NOT NULL,
    kind TEXT NOT NULL,
    file_id TEXT NOT NULL,
    hash TEXT NOT NULL,
    PRIMARY KEY (path, kind)
);
"#;

/// Which flavour of file a row describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Markdown,
    Docx,
}

impl EntryKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Docx => "docx",
        }
    }

    fn from_str(text: &str) -> Result<Self> {
        match text {
            "md" => Ok(Self::Markdown),
            "docx" => Ok(Self::Docx),
            other => anyhow::bail!("unknown Google Drive entry kind {other:?}"),
        }
    }
}

/// One uploaded file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveEntry {
    /// Local Markdown path, `notes/a.md`. The `.docx` row uses the same path.
    pub path: String,
    pub kind: EntryKind,
    pub file_id: String,
    /// SHA-256 of the Markdown content this upload came from.
    pub hash: String,
}

/// SQLite-backed Google Drive mirror state.
pub struct GdriveState {
    conn: Connection,
}

impl GdriveState {
    /// Open `<state_dir>/sync-state.sqlite`, sharing it with the server sync
    /// state and history paths.
    pub fn from_dir(state_dir: &Path) -> Result<Self> {
        Self::open(&state_database(state_dir))
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open Google Drive state {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Every row, ordered by path and kind.
    pub fn entries(&self) -> Result<Vec<DriveEntry>> {
        let mut statement = self.conn.prepare(
            "SELECT path, kind, file_id, hash FROM gdrive_files
             ORDER BY path ASC, kind ASC",
        )?;
        let rows = statement.query_map([], |row| {
            let kind: String = row.get(1)?;
            Ok((
                row.get::<_, String>(0)?,
                kind,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let (path, kind, file_id, hash) = row?;
            entries.push(DriveEntry {
                path,
                kind: EntryKind::from_str(&kind)?,
                file_id,
                hash,
            });
        }
        Ok(entries)
    }

    pub fn entry(&self, path: &str, kind: EntryKind) -> Result<Option<DriveEntry>> {
        let row = self
            .conn
            .query_row(
                "SELECT file_id, hash FROM gdrive_files WHERE path = ?1 AND kind = ?2",
                params![path, kind.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        Ok(row.map(|(file_id, hash)| DriveEntry {
            path: path.to_string(),
            kind,
            file_id,
            hash,
        }))
    }

    pub fn upsert(&mut self, entry: &DriveEntry) -> Result<()> {
        self.conn.execute(
            "INSERT INTO gdrive_files (path, kind, file_id, hash)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path, kind) DO UPDATE SET
                 file_id = excluded.file_id,
                 hash = excluded.hash",
            params![entry.path, entry.kind.as_str(), entry.file_id, entry.hash],
        )?;
        Ok(())
    }

    pub fn remove(&mut self, path: &str, kind: EntryKind) -> Result<()> {
        self.conn.execute(
            "DELETE FROM gdrive_files WHERE path = ?1 AND kind = ?2",
            params![path, kind.as_str()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_updates_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = GdriveState::from_dir(dir.path()).unwrap();
        assert!(state.entries().unwrap().is_empty());

        let entry = DriveEntry {
            path: "notes/a.md".into(),
            kind: EntryKind::Markdown,
            file_id: "f1".into(),
            hash: "h1".into(),
        };
        state.upsert(&entry).unwrap();
        assert_eq!(
            state.entry("notes/a.md", EntryKind::Markdown).unwrap(),
            Some(entry.clone())
        );

        let docx = DriveEntry {
            kind: EntryKind::Docx,
            ..entry.clone()
        };
        state.upsert(&docx).unwrap();
        assert_eq!(state.entries().unwrap().len(), 2);

        let updated = DriveEntry {
            file_id: "f2".into(),
            hash: "h2".into(),
            ..entry.clone()
        };
        state.upsert(&updated).unwrap();
        assert_eq!(
            state.entry("notes/a.md", EntryKind::Markdown).unwrap(),
            Some(updated)
        );

        state.remove("notes/a.md", EntryKind::Markdown).unwrap();
        assert!(state
            .entry("notes/a.md", EntryKind::Markdown)
            .unwrap()
            .is_none());
        assert!(state
            .entry("notes/a.md", EntryKind::Docx)
            .unwrap()
            .is_some());
    }

    #[test]
    fn shares_the_database_with_the_server_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = crate::state::SyncState::from_dir(dir.path()).unwrap();
        server.set_cursor("7").unwrap();
        let mut drive = GdriveState::from_dir(dir.path()).unwrap();
        drive
            .upsert(&DriveEntry {
                path: "a.md".into(),
                kind: EntryKind::Markdown,
                file_id: "f1".into(),
                hash: "h1".into(),
            })
            .unwrap();
        assert_eq!(server.cursor().unwrap(), "7");
        assert_eq!(drive.entries().unwrap().len(), 1);
    }
}
