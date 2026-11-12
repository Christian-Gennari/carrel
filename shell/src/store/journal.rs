//! The append-only journal.
//!
//! One record per edit: `(offset, delete length, inserted text, timestamp)`.
//! Records are framed with a checksum so a write torn by a crash is ignored on
//! replay instead of corrupting the document.
//!
//! Checkpoints are journal records too. Before an atomic save replaces the
//! Markdown file, the store appends a checkpoint holding a hash and length of
//! the snapshot. If a crash happens after the rename but before the journal is
//! truncated, replay sees that the file already matches the checkpoint and
//! skips the operations it contains. Without this, the save window would
//! double-apply the journal.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::hash::{fnv1a64, fnv1a64_update};

const MAGIC: &[u8; 4] = b"STLJ";
const VERSION: u8 = 1;

/// 4 magic + 1 version + 1 kind + 2 reserved + 6 u64 fields.
const HEADER_LEN: usize = 56;

/// A single record is never this big; this guards against a corrupt length
/// field eating all memory.
const MAX_TEXT_LEN: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Edit = 1,
    Checkpoint = 2,
}

#[derive(Debug, Clone)]
pub struct Record {
    pub kind: Kind,
    pub seq: u64,
    pub timestamp_ms: u64,
    /// Edit: character offset. Checkpoint: snapshot length in bytes.
    pub offset: u64,
    /// Edit: characters deleted. Checkpoint: snapshot FNV-1a hash.
    pub delete_len: u64,
    pub text: String,
}

impl Record {
    pub fn edit(seq: u64, timestamp_ms: u64, offset: usize, delete_len: usize, text: &str) -> Self {
        Self {
            kind: Kind::Edit,
            seq,
            timestamp_ms,
            offset: offset as u64,
            delete_len: delete_len as u64,
            text: text.to_owned(),
        }
    }

    pub fn checkpoint(seq: u64, timestamp_ms: u64, snapshot: &str) -> Self {
        Self {
            kind: Kind::Checkpoint,
            seq,
            timestamp_ms,
            offset: snapshot.len() as u64,
            delete_len: fnv1a64(snapshot.as_bytes()),
            text: String::new(),
        }
    }

    /// Does this checkpoint describe exactly `snapshot`?
    pub fn checkpoint_matches(&self, snapshot: &str) -> bool {
        self.kind == Kind::Checkpoint
            && self.offset == snapshot.len() as u64
            && self.delete_len == fnv1a64(snapshot.as_bytes())
    }
}

fn record_checksum(header: &[u8], text: &[u8]) -> u64 {
    // The checksum covers the header without the checksum field, then the text.
    fnv1a64_update(fnv1a64(&header[..48]), text)
}

fn encode(record: &Record) -> io::Result<Vec<u8>> {
    let text = record.text.as_bytes();
    if text.len() as u64 > MAX_TEXT_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "journal record too large",
        ));
    }

    let mut out = Vec::with_capacity(HEADER_LEN + text.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(match record.kind {
        Kind::Edit => Kind::Edit as u8,
        Kind::Checkpoint => Kind::Checkpoint as u8,
    });
    out.extend_from_slice(&[0, 0]); // reserved
    out.extend_from_slice(&record.seq.to_le_bytes());
    out.extend_from_slice(&record.timestamp_ms.to_le_bytes());
    out.extend_from_slice(&record.offset.to_le_bytes());
    out.extend_from_slice(&record.delete_len.to_le_bytes());
    out.extend_from_slice(&(text.len() as u64).to_le_bytes());
    debug_assert_eq!(out.len(), 48);
    out.extend_from_slice(&0u64.to_le_bytes()); // checksum placeholder
    debug_assert_eq!(out.len(), HEADER_LEN);

    let checksum = record_checksum(&out, text);
    out[48..56].copy_from_slice(&checksum.to_le_bytes());
    out.extend_from_slice(text);
    Ok(out)
}

#[derive(Debug, Default)]
pub struct Decoded {
    pub records: Vec<Record>,
    /// Byte offset just past each record, aligned with `records`.
    pub ends: Vec<usize>,
    /// End of the last complete, valid record.
    pub valid_len: usize,
}

/// Decode as many complete records as possible. The first bad or truncated
/// record ends the valid prefix; everything after it is ignored.
pub fn decode(bytes: &[u8]) -> Decoded {
    let mut decoded = Decoded::default();
    let mut pos = 0usize;

    while pos + HEADER_LEN <= bytes.len() {
        let header = &bytes[pos..pos + HEADER_LEN];
        if &header[0..4] != MAGIC || header[4] != VERSION {
            break;
        }
        let kind = match header[5] {
            k if k == Kind::Edit as u8 => Kind::Edit,
            k if k == Kind::Checkpoint as u8 => Kind::Checkpoint,
            _ => break,
        };
        let seq = u64::from_le_bytes(header[8..16].try_into().expect("fixed size"));
        let timestamp_ms = u64::from_le_bytes(header[16..24].try_into().expect("fixed size"));
        let offset = u64::from_le_bytes(header[24..32].try_into().expect("fixed size"));
        let delete_len = u64::from_le_bytes(header[32..40].try_into().expect("fixed size"));
        let text_len = u64::from_le_bytes(header[40..48].try_into().expect("fixed size"));
        let checksum = u64::from_le_bytes(header[48..56].try_into().expect("fixed size"));

        if text_len > MAX_TEXT_LEN {
            break;
        }
        let text_len = text_len as usize;
        let end = match pos
            .checked_add(HEADER_LEN)
            .and_then(|p| p.checked_add(text_len))
        {
            Some(end) if end <= bytes.len() => end,
            _ => break,
        };
        let text_bytes = &bytes[pos + HEADER_LEN..end];
        if record_checksum(header, text_bytes) != checksum {
            break;
        }
        let text = match std::str::from_utf8(text_bytes) {
            Ok(text) => text.to_owned(),
            Err(_) => break,
        };

        decoded.records.push(Record {
            kind,
            seq,
            timestamp_ms,
            offset,
            delete_len,
            text,
        });
        decoded.ends.push(end);
        decoded.valid_len = end;
        pos = end;
    }

    decoded
}

pub struct Journal {
    path: PathBuf,
    file: File,
}

impl Journal {
    /// Open (creating if needed) the journal and return the records that were
    /// already durable. A torn tail is cut off so new records can be appended
    /// after the valid prefix.
    pub fn open(path: &Path) -> io::Result<(Self, Decoded)> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let decoded = decode(&bytes);
        if decoded.valid_len < bytes.len() {
            file.set_len(decoded.valid_len as u64)?;
            file.sync_data()?;
        }

        Ok((
            Self {
                path: path.to_path_buf(),
                file,
            },
            decoded,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append a record. This is a plain `write`: `sync_data` is called by the
    /// store's timer, at most one second later.
    pub fn append(&mut self, record: &Record) -> io::Result<()> {
        let bytes = encode(record)?;
        self.file.write_all(&bytes)
    }

    /// fdatasync the journal.
    pub fn sync_data(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }

    /// Cut the journal back to a byte length (used for a corrupt tail).
    pub fn truncate_to(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len)?;
        self.file.seek(SeekFrom::Start(len))?;
        Ok(())
    }

    /// Clear the journal after a save has made the Markdown file current.
    pub fn clear(&mut self) -> io::Result<()> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.sync_data()
    }

    pub fn len(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len().map(|len| len == 0).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn roundtrip(records: &[Record]) -> Vec<Record> {
        let mut bytes = Vec::new();
        for record in records {
            bytes.extend_from_slice(&encode(record).unwrap());
        }
        decode(&bytes).records
    }

    #[test]
    fn roundtrips_edits_and_checkpoints() {
        let records = vec![
            Record::edit(1, 100, 0, 0, "hello"),
            Record::edit(2, 101, 5, 0, " world"),
            Record::checkpoint(3, 102, "hello world"),
        ];
        let decoded = roundtrip(&records);
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].text, "hello");
        assert_eq!(decoded[1].offset, 5);
        assert_eq!(decoded[2].kind, Kind::Checkpoint);
        assert!(decoded[2].checkpoint_matches("hello world"));
    }

    #[test]
    fn roundtrips_unicode_text() {
        let records = vec![Record::edit(1, 0, 0, 0, "héllo — 世界")];
        let decoded = roundtrip(&records);
        assert_eq!(decoded[0].text, "héllo — 世界");
    }

    #[test]
    fn ignores_a_torn_tail() {
        let mut bytes = encode(&Record::edit(1, 0, 0, 0, "one")).unwrap();
        bytes.extend_from_slice(&encode(&Record::edit(2, 0, 3, 0, "two")).unwrap());
        bytes.truncate(bytes.len() - 2);
        let decoded = decode(&bytes);
        assert_eq!(decoded.records.len(), 1);
        assert_eq!(decoded.records[0].text, "one");
    }

    #[test]
    fn ignores_a_corrupt_record() {
        let mut bytes = encode(&Record::edit(1, 0, 0, 0, "one")).unwrap();
        let good_len = bytes.len();
        let mut second = encode(&Record::edit(2, 0, 3, 0, "two")).unwrap();
        let last = second.len() - 1;
        second[last] ^= 0xff;
        bytes.extend_from_slice(&second);
        let decoded = decode(&bytes);
        assert_eq!(decoded.records.len(), 1);
        assert_eq!(decoded.valid_len, good_len);
    }

    #[test]
    fn open_cuts_a_garbage_tail_so_appends_are_readable() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("doc.log");
        {
            let mut journal = Journal::open(&path).unwrap().0;
            journal.append(&Record::edit(1, 0, 0, 0, "hello")).unwrap();
            journal.sync_data().unwrap();
        }
        {
            use std::io::Write as _;
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(b"garbage").unwrap();
        }
        {
            let (mut journal, decoded) = Journal::open(&path).unwrap();
            assert_eq!(decoded.records.len(), 1);
            journal.append(&Record::edit(2, 0, 5, 0, " world")).unwrap();
            journal.sync_data().unwrap();
        }
        let (_, decoded) = Journal::open(&path).unwrap();
        assert_eq!(decoded.records.len(), 2);
        assert_eq!(decoded.records[1].offset, 5);
        assert_eq!(decoded.records[1].text, " world");
    }

    #[test]
    fn clear_empties_the_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("doc.log");
        let (mut journal, _) = Journal::open(&path).unwrap();
        journal.append(&Record::edit(1, 0, 0, 0, "hello")).unwrap();
        assert!(journal.len().unwrap() > 0);
        journal.clear().unwrap();
        assert_eq!(journal.len().unwrap(), 0);
        let (_, decoded) = Journal::open(&path).unwrap();
        assert!(decoded.records.is_empty());
    }
}
