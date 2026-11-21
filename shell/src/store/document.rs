//! Durable document handles.
//!
//! A `Document` owns the in-memory text, its per-file journal and a small
//! background thread that makes the two promises from the spec:
//!
//! * `fdatasync` the journal at most half a second after an edit (the spec
//!   allows one second, half leaves room for process scheduling), and
//! * after five seconds without edits, write the Markdown file atomically:
//!   `name.md.tmp` → fsync → rename → fsync directory, then clear the journal.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::fsutil::atomic_write;
use super::hash::fnv1a64;
use super::journal::{Journal, Kind, Record};
use super::text::{apply_edit, EditOutOfRange};
use super::{StoreError, StoreResult};

pub const DEFAULT_SYNC_INTERVAL: Duration = Duration::from_millis(500);
pub const DEFAULT_IDLE_SAVE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
pub struct SavePolicy {
    /// How often a dirty journal is fdatasync'ed while typing.
    pub sync_interval: Duration,
    /// How long the document must be idle before the Markdown file is saved.
    pub idle_save: Duration,
}

impl Default for SavePolicy {
    fn default() -> Self {
        Self {
            sync_interval: DEFAULT_SYNC_INTERVAL,
            idle_save: DEFAULT_IDLE_SAVE,
        }
    }
}

pub struct Document {
    inner: Arc<DocInner>,
    thread: Option<JoinHandle<()>>,
}

struct DocInner {
    rel: PathBuf,
    path: PathBuf,
    cfg: SavePolicy,
    /// Duplicate fd for `fdatasync` outside the state mutex.
    journal_sync: File,
    /// Serializes `save_snapshot` across the writer thread, `flush`, and
    /// `save_now`. Always acquired before `state`, never while holding `state`.
    save_lock: Mutex<()>,
    state: Mutex<DocState>,
    cv: Condvar,
    #[cfg(test)]
    before_atomic_write: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

struct DocState {
    text: String,
    char_len: usize,
    seq: u64,
    journal: Journal,
    /// There are journal operations that the Markdown file does not have yet.
    dirty: bool,
    /// When the oldest un-synced journal write happened.
    dirty_since: Option<Instant>,
    /// When the last edit happened, used for the five second idle save.
    last_edit: Option<Instant>,
    closed: bool,
}

/// Open a document and replay its journal. Called through `Store`.
pub fn open(
    root: &Path,
    rel: &Path,
    journal_path: PathBuf,
    cfg: SavePolicy,
) -> StoreResult<Document> {
    let path = root.join(rel);
    let base = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(StoreError::Io(error)),
    };

    let (journal, decoded) = Journal::open(&journal_path)?;

    // Find the newest checkpoint that the file on disk already contains.
    // If the crash happened between the checkpoint and the rename, the hash
    // does not match and every operation is replayed, which is exactly right.
    let checkpoint_seq = decoded
        .records
        .iter()
        .filter(|record| record.checkpoint_matches(&base))
        .map(|record| record.seq)
        .max();

    let mut text = base;
    let mut char_len = text.chars().count();
    let mut applied_edits = false;
    let mut journal = journal;

    for (index, record) in decoded.records.iter().enumerate() {
        if record.kind != Kind::Edit {
            continue;
        }
        if checkpoint_seq.is_some_and(|seq| record.seq <= seq) {
            continue;
        }
        match apply_edit(
            &mut text,
            &mut char_len,
            record.offset as usize,
            record.delete_len as usize,
            &record.text,
        ) {
            Ok(()) => applied_edits = true,
            Err(EditOutOfRange { .. }) => {
                // A journal operation that does not fit is corrupt. Keep the
                // valid prefix and cut off this record and everything after.
                let keep = if index == 0 {
                    0
                } else {
                    decoded.ends[index - 1] as u64
                };
                journal.truncate_to(keep)?;
                break;
            }
        }
    }

    let seq = decoded
        .records
        .iter()
        .map(|record| record.seq)
        .max()
        .map_or(1, |seq| seq.saturating_add(1));

    let journal_sync = journal.try_clone_file()?;
    let now = Instant::now();
    let state = DocState {
        text,
        char_len,
        seq,
        journal,
        dirty: applied_edits,
        // Recovered operations are already on disk, but syncing once more is
        // cheap and protects against a power cut right after startup.
        dirty_since: applied_edits.then_some(now),
        last_edit: applied_edits.then_some(now),
        closed: false,
    };

    let inner = Arc::new(DocInner {
        rel: rel.to_path_buf(),
        path,
        cfg,
        journal_sync,
        save_lock: Mutex::new(()),
        state: Mutex::new(state),
        cv: Condvar::new(),
        #[cfg(test)]
        before_atomic_write: Mutex::new(None),
    });
    let thread_inner = Arc::clone(&inner);
    let thread = thread::Builder::new()
        .name("carrel-store".to_owned())
        .spawn(move || writer_loop(thread_inner))
        .map_err(StoreError::Io)?;

    Ok(Document {
        inner,
        thread: Some(thread),
    })
}

impl Document {
    pub fn rel_path(&self) -> &Path {
        &self.inner.rel
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn journal_path(&self) -> PathBuf {
        self.inner
            .state
            .lock()
            .expect("journal mutex")
            .journal
            .path()
            .to_path_buf()
    }

    /// The full text. Used once when a file is opened; the editor keeps the
    /// live copy after that.
    pub fn text(&self) -> String {
        self.inner.state.lock().expect("journal mutex").text.clone()
    }

    pub fn char_len(&self) -> usize {
        self.inner.state.lock().expect("journal mutex").char_len
    }

    pub fn is_dirty(&self) -> bool {
        self.inner.state.lock().expect("journal mutex").dirty
    }

    /// Record one edit and return its journal sequence number. The edit is
    /// acknowledged only after its record has been written to the journal.
    ///
    /// Never blocks on `fdatasync` or `atomic_write`: the background writer
    /// drops `state` before flushing to disk.
    pub fn apply(&self, offset: usize, delete_len: usize, insert: &str) -> StoreResult<u64> {
        let mut state = self.inner.state.lock().expect("journal mutex");
        if state.closed {
            return Err(StoreError::Closed);
        }
        if offset.saturating_add(delete_len) > state.char_len {
            return Err(StoreError::InvalidEdit(EditOutOfRange {
                offset,
                delete_len,
                text_len: state.char_len,
            }));
        }

        let seq = state.seq;
        let record = Record::edit(seq, now_ms(), offset, delete_len, insert);
        state.journal.append(&record)?;
        state.seq = seq.saturating_add(1);

        {
            let DocState { text, char_len, .. } = &mut *state;
            apply_edit(text, char_len, offset, delete_len, insert)
                .map_err(StoreError::InvalidEdit)?;
        }
        state.dirty = true;
        let now = Instant::now();
        state.last_edit = Some(now);
        if state.dirty_since.is_none() {
            state.dirty_since = Some(now);
        }
        self.inner.cv.notify_all();
        Ok(seq)
    }

    /// fdatasync the journal and, if needed, atomically save the Markdown
    /// file. Called before suspend, file switch and exit.
    pub fn flush(&self) -> StoreResult<()> {
        let _save_guard = self.inner.save_lock.lock().expect("save mutex");
        save_snapshot(&self.inner, false, true)
    }

    /// Save now even if the idle timer has not fired.
    pub fn save_now(&self) -> StoreResult<()> {
        let _save_guard = self.inner.save_lock.lock().expect("save mutex");
        save_snapshot(&self.inner, false, false)
    }

    fn shutdown(&mut self) {
        {
            let mut state = self.inner.state.lock().expect("journal mutex");
            state.closed = true;
            self.inner.cv.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = self.flush();
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Save a snapshot of the document without holding the state mutex across
/// `fdatasync` or `atomic_write`. Caller must hold `inner.save_lock`.
fn save_snapshot(inner: &DocInner, require_idle: bool, always_sync: bool) -> StoreResult<()> {
    let snapshot = {
        let mut state = inner.state.lock().expect("journal mutex");
        if !state.dirty {
            drop(state);
            if always_sync {
                inner.journal_sync.sync_data()?;
            }
            return Ok(());
        }
        if require_idle {
            let now = Instant::now();
            let is_idle = state
                .last_edit
                .is_some_and(|last| now >= last + inner.cfg.idle_save);
            if !is_idle {
                return Ok(());
            }
        }
        // The checkpoint must be durable before the rename. Otherwise a power
        // cut after the rename could leave a file that already contains the
        // edits and a journal without the checkpoint that says so.
        let checkpoint_seq = state.seq;
        let snapshot_text = state.text.clone();
        let checkpoint = Record::checkpoint(checkpoint_seq, now_ms(), &snapshot_text);
        state.journal.append(&checkpoint)?;
        state.seq = checkpoint_seq.saturating_add(1);
        (checkpoint_seq, snapshot_text)
    };

    let (checkpoint_seq, snapshot_text) = snapshot;
    inner.journal_sync.sync_data()?;

    #[cfg(test)]
    {
        let hook = inner
            .before_atomic_write
            .lock()
            .expect("hook mutex")
            .clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    atomic_write(&inner.path, snapshot_text.as_bytes())?;

    let mut truncated = false;
    {
        let mut state = inner.state.lock().expect("journal mutex");
        // Only clear the journal if no new edits arrived while we were syncing
        // and writing. If edits arrived in flight, keep the journal (with the
        // durable checkpoint) so crash recovery skips <= checkpoint_seq and
        // replays only the newer edits.
        if state.seq == checkpoint_seq.saturating_add(1) {
            state.journal.truncate_to(0)?;
            state.dirty = false;
            state.dirty_since = None;
            state.last_edit = None;
            truncated = true;
        }
    }
    if truncated {
        inner.journal_sync.sync_data()?;
    }
    Ok(())
}

fn writer_loop(inner: Arc<DocInner>) {
    loop {
        let (sync_due, sync_seq, sync_started, save_due, closed) = {
            let mut state = inner.state.lock().expect("journal mutex");

            if state.closed {
                (false, 0, Instant::now(), state.dirty, true)
            } else {
                let now = Instant::now();
                let sync_deadline = state
                    .dirty_since
                    .map(|since| since + inner.cfg.sync_interval);
                let save_deadline = if state.dirty {
                    state.last_edit.map(|last| last + inner.cfg.idle_save)
                } else {
                    None
                };
                let next = match (sync_deadline, save_deadline) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };

                match next {
                    Some(deadline) if deadline > now => {
                        let (guard, _) = inner
                            .cv
                            .wait_timeout(state, deadline.saturating_duration_since(now))
                            .expect("journal mutex");
                        state = guard;
                    }
                    Some(_) => {}
                    None => {
                        state = inner.cv.wait(state).expect("journal mutex");
                    }
                }

                if state.closed {
                    (false, 0, Instant::now(), state.dirty, true)
                } else {
                    let now = Instant::now();
                    let sync_due = state
                        .dirty_since
                        .is_some_and(|since| now >= since + inner.cfg.sync_interval);
                    let sync_seq = state.seq;
                    let save_due = state.dirty
                        && state
                            .last_edit
                            .is_some_and(|last| now >= last + inner.cfg.idle_save);
                    (sync_due, sync_seq, now, save_due, false)
                }
            }
        };

        if closed {
            let _save_guard = inner.save_lock.lock().expect("save mutex");
            let _ = save_snapshot(&inner, false, true);
            return;
        }

        if sync_due {
            match inner.journal_sync.sync_data() {
                Ok(()) => {
                    let mut state = inner.state.lock().expect("journal mutex");
                    if state.seq == sync_seq {
                        state.dirty_since = None;
                    } else if state.dirty {
                        // Edits arrived while fdatasync was running; schedule the
                        // next sync relative to when this sync started.
                        state.dirty_since = Some(sync_started);
                    }
                }
                Err(error) => {
                    eprintln!("carrel: journal sync failed: {error}");
                    // Don't spin; try again one interval later.
                    let mut state = inner.state.lock().expect("journal mutex");
                    if state.dirty {
                        state.dirty_since = Some(Instant::now());
                    }
                }
            }
        }

        if save_due {
            let _save_guard = inner.save_lock.lock().expect("save mutex");
            if let Err(error) = save_snapshot(&inner, true, false) {
                eprintln!("carrel: save failed: {error}");
                // Retry after another idle period rather than hammering a full
                // disk.
                let mut state = inner.state.lock().expect("journal mutex");
                if state.dirty {
                    state.last_edit = Some(Instant::now());
                }
            }
        }
    }
}

/// Stable id for a file's journal. The readable prefix helps when someone
/// looks inside `.carrel/journal`; the hash makes it unique per path.
pub fn journal_id(rel: &Path) -> String {
    let stem: String = rel
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(40)
        .collect();
    let hash = fnv1a64(rel.to_string_lossy().as_bytes());
    if stem.is_empty() {
        format!("{hash:016x}.log")
    } else {
        format!("{stem}-{hash:016x}.log")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::journal::{Journal, Record};
    use crate::store::Store;

    fn fast_policy() -> SavePolicy {
        SavePolicy {
            sync_interval: Duration::from_millis(20),
            idle_save: Duration::from_millis(120),
        }
    }

    fn setup() -> (tempfile::TempDir, Store, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_with_policy(dir.path(), fast_policy()).unwrap();
        store.create_file(Path::new("note.md")).unwrap();
        let journal = store.journal_path(Path::new("note.md"));
        (dir, store, journal)
    }

    fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn replays_a_journal_into_the_file() {
        let (dir, store, journal_path) = setup();
        std::fs::write(dir.path().join("note.md"), "").unwrap();
        {
            let (mut journal, _) = Journal::open(&journal_path).unwrap();
            journal.append(&Record::edit(1, 10, 0, 0, "hello")).unwrap();
            journal
                .append(&Record::edit(2, 11, 5, 0, " world"))
                .unwrap();
            journal.sync_data().unwrap();
        }
        let doc = store.open_document(Path::new("note.md")).unwrap();
        assert_eq!(doc.text(), "hello world");
    }

    #[test]
    fn apply_writes_the_journal_before_acknowledging() {
        let (_dir, store, journal_path) = setup();
        let doc = store.open_document(Path::new("note.md")).unwrap();
        doc.apply(0, 0, "typed").unwrap();
        let bytes = std::fs::read(&journal_path).unwrap();
        let decoded = crate::store::journal::decode(&bytes);
        assert_eq!(decoded.records.len(), 1);
        assert_eq!(decoded.records[0].text, "typed");
    }

    #[test]
    fn idle_save_writes_the_file_and_clears_the_journal() {
        let (dir, store, journal) = setup();
        let doc = store.open_document(Path::new("note.md")).unwrap();
        doc.apply(0, 0, "# Morning\n\nA quiet start.").unwrap();
        assert!(wait_until(|| {
            std::fs::read_to_string(dir.path().join("note.md"))
                .map(|text| text == "# Morning\n\nA quiet start.")
                .unwrap_or(false)
        }));
        assert!(wait_until(|| {
            std::fs::metadata(&journal)
                .map(|meta| meta.len() == 0)
                .unwrap_or(false)
        }));
        assert!(!doc.is_dirty());
    }

    #[test]
    fn flush_saves_even_without_waiting() {
        let (_dir, store, journal) = setup();
        let doc = store.open_document(Path::new("note.md")).unwrap();
        doc.apply(0, 0, "saved now").unwrap();
        assert!(doc.is_dirty());
        doc.flush().unwrap();
        assert!(!doc.is_dirty());
        assert_eq!(std::fs::metadata(&journal).unwrap().len(), 0);
        assert_eq!(
            std::fs::read_to_string(store.root().join("note.md")).unwrap(),
            "saved now"
        );
    }

    #[test]
    fn a_checkpoint_after_the_rename_is_not_applied_twice() {
        let (dir, store, journal_path) = setup();
        let text = "the saved text";
        std::fs::write(dir.path().join("note.md"), text).unwrap();

        {
            let (mut journal, _) = Journal::open(&journal_path).unwrap();
            journal
                .append(&Record::edit(1, 10, 0, 0, "the saved text"))
                .unwrap();
            journal
                .append(&Record::checkpoint(2, 11, "the saved text"))
                .unwrap();
            journal.sync_data().unwrap();
        }

        let doc = store.open_document(Path::new("note.md")).unwrap();
        assert_eq!(doc.text(), "the saved text");
        assert!(!doc.is_dirty());
    }

    #[test]
    fn a_checkpoint_before_the_rename_still_replays_the_edit() {
        let (dir, store, journal_path) = setup();
        std::fs::write(dir.path().join("note.md"), "the saved").unwrap();

        {
            let (mut journal, _) = Journal::open(&journal_path).unwrap();
            journal.append(&Record::edit(1, 11, 9, 0, " text")).unwrap();
            // A checkpoint for a snapshot that never made it to disk.
            journal
                .append(&Record::checkpoint(2, 12, "the saved text"))
                .unwrap();
            journal.sync_data().unwrap();
        }

        let doc = store.open_document(Path::new("note.md")).unwrap();
        assert_eq!(doc.text(), "the saved text");
        assert!(doc.is_dirty());
    }

    #[test]
    fn a_torn_record_at_the_tail_is_ignored_and_cut_off() {
        let (dir, store, journal_path) = setup();
        std::fs::write(dir.path().join("note.md"), "").unwrap();
        let complete = {
            let (mut journal, _) = Journal::open(&journal_path).unwrap();
            journal
                .append(&Record::edit(1, 10, 0, 0, "complete"))
                .unwrap();
            journal.sync_data().unwrap();
            std::fs::read(&journal_path).unwrap()
        };
        let mut bytes = complete;
        bytes.extend_from_slice(b"STLJ\x01\x01\x00\x00partial");
        std::fs::write(&journal_path, &bytes).unwrap();

        let doc = store.open_document(Path::new("note.md")).unwrap();
        assert_eq!(doc.text(), "complete");
        // The next append must be after the valid prefix, not behind garbage.
        doc.apply(8, 0, " text").unwrap();
        doc.flush().unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("note.md")).unwrap(),
            "complete text"
        );
    }

    #[test]
    fn rejects_edits_past_the_end() {
        let (_dir, store, _journal) = setup();
        let doc = store.open_document(Path::new("note.md")).unwrap();
        assert!(doc.apply(0, 1, "x").is_err());
        assert_eq!(doc.text(), "");
    }

    #[test]
    fn appending_unicode_tracks_character_offsets() {
        let (_dir, store, _journal) = setup();
        let doc = store.open_document(Path::new("note.md")).unwrap();
        doc.apply(0, 0, "héllo").unwrap();
        doc.apply(5, 0, " — 世界").unwrap();
        assert_eq!(doc.char_len(), 10);
        doc.apply(8, 2, "world").unwrap();
        assert_eq!(doc.text(), "héllo — world");
        assert_eq!(doc.char_len(), 13);
    }

    #[test]
    fn apply_does_not_block_while_save_is_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        // Long idle_save so only our explicit save_now triggers during the hook.
        let policy = SavePolicy {
            sync_interval: Duration::from_secs(60),
            idle_save: Duration::from_secs(60),
        };
        let store = Store::open_with_policy(dir.path(), policy).unwrap();
        store.create_file(Path::new("note.md")).unwrap();
        let journal_path = store.journal_path(Path::new("note.md"));
        let doc = Arc::new(store.open_document(Path::new("note.md")).unwrap());
        doc.apply(0, 0, "first").unwrap();

        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let release_hook = Arc::clone(&release);
        *doc.inner.before_atomic_write.lock().unwrap() = Some(Arc::new(move || {
            let _ = entered_tx.send(());
            let (lock, cv) = &*release_hook;
            let mut go = lock.lock().unwrap();
            while !*go {
                go = cv.wait(go).unwrap();
            }
        }));

        let saver_doc = Arc::clone(&doc);
        let saver = thread::spawn(move || saver_doc.save_now().unwrap());
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        // While save_now() is paused mid-save (right before atomic_write),
        // Document::apply() must succeed without blocking.
        let started = Instant::now();
        doc.apply(5, 0, " second").unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "apply blocked on in-flight save: {:?}",
            started.elapsed()
        );

        // Clear the hook and let the in-flight save finish.
        *doc.inner.before_atomic_write.lock().unwrap() = None;
        {
            let (lock, cv) = &*release;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
        saver.join().unwrap();

        // Because a concurrent edit arrived mid-save, the journal must stay
        // intact (dirty) so crash recovery replays only the post-checkpoint
        // edit without double-applying "first".
        assert!(doc.is_dirty());
        assert!(std::fs::metadata(&journal_path).unwrap().len() > 0);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("note.md")).unwrap(),
            "first"
        );

        // Simulate reopening from disk + journal right after the save: the
        // checkpoint skips "first" and replays only " second".
        std::mem::forget(doc);
        let reopened = store.open_document(Path::new("note.md")).unwrap();
        assert_eq!(reopened.text(), "first second");
        assert!(reopened.is_dirty());
        reopened.flush().unwrap();
        assert!(!reopened.is_dirty());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("note.md")).unwrap(),
            "first second"
        );
    }

    #[test]
    fn concurrent_applies_and_saves_preserve_all_edits() {
        let (_dir, store, _journal) = setup();
        let doc = Arc::new(store.open_document(Path::new("note.md")).unwrap());

        let writer_doc = Arc::clone(&doc);
        let writer = thread::spawn(move || {
            for i in 0..100 {
                let len = writer_doc.char_len();
                writer_doc.apply(len, 0, &format!("{i},")).unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        });

        let saver_doc = Arc::clone(&doc);
        let saver = thread::spawn(move || {
            for _ in 0..30 {
                saver_doc.save_now().unwrap();
                thread::sleep(Duration::from_millis(3));
            }
        });

        writer.join().unwrap();
        saver.join().unwrap();
        doc.flush().unwrap();

        let expected: String = (0..100).map(|i| format!("{i},")).collect();
        assert_eq!(doc.text(), expected);
        assert!(!doc.is_dirty());
        assert_eq!(
            std::fs::read_to_string(store.root().join("note.md")).unwrap(),
            expected
        );
    }
}
