//! Durable document handles.
//!
//! A `Document` owns the in-memory text, its per-file journal and two small
//! background threads. [`Document::apply`] only changes memory and queues a
//! record, because a `write` to a slow USB stick can block for seconds and it
//! runs on the GTK main loop. One thread writes the queued records to the
//! journal at once; the other makes the two promises from the spec:
//!
//! * `fdatasync` the journal at most half a second after an edit (the spec
//!   allows one second, half leaves room for process scheduling), and
//! * after five seconds without edits, write the Markdown file atomically:
//!   `name.md.tmp` → fsync → rename → fsync directory, then clear the journal.
//!
//! A sync can write the file while it is open. A save never loses that text:
//! it is first moved to a conflict copy. A document with nothing to save takes
//! it over instead ([`Document::look_at_disk`]).

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use carrel_protocol::{conflict_copy_path, format_conflict_timestamp, now_unix};

use super::fsutil::{atomic_write, slow_disk, sync_dir};
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
    threads: Vec<JoinHandle<()>>,
}

/// Where a document lives.
#[derive(Debug, Clone)]
struct Location {
    rel: PathBuf,
    path: PathBuf,
}

/// Lock order: `save_lock`, `journal`, then `state`, `location` or
/// `journal_path`. The last three are never held across a disk access, so the
/// main loop never waits on one.
struct DocInner {
    location: Mutex<Location>,
    journal_path: Mutex<PathBuf>,
    cfg: SavePolicy,
    journal: Mutex<Journal>,
    /// Duplicate fd for `fdatasync` without any lock.
    journal_sync: File,
    /// Serializes saving and moving across the writer thread, `flush` and
    /// `save_now`.
    save_lock: Mutex<()>,
    state: Mutex<DocState>,
    cv: Condvar,
    append_cv: Condvar,
    #[cfg(test)]
    before_atomic_write: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

struct DocState {
    text: String,
    char_len: usize,
    seq: u64,
    /// Records not written to the journal yet, in order.
    pending: Vec<Record>,
    /// A rename asked for and not done yet.
    move_to: Option<(Location, PathBuf)>,
    /// A save, journal write or rename that failed.
    failed: bool,
    rename_failed: bool,
    /// A rename was asked for and has not finished.
    renaming: bool,
    /// There are journal operations that the Markdown file does not have yet.
    dirty: bool,
    /// When the oldest un-synced journal write happened.
    dirty_since: Option<Instant>,
    /// When the last edit happened, used for the five second idle save.
    last_edit: Option<Instant>,
    closed: bool,
    /// Hash of the file as this document last read or wrote it.
    disk: u64,
    /// Someone asked for the file to be read again.
    look: bool,
    /// The file's new text and the `disk` hash it differs from.
    external: Option<(String, u64)>,
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

    let disk = fnv1a64(base.as_bytes());
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
        pending: Vec::new(),
        move_to: None,
        failed: false,
        rename_failed: false,
        renaming: false,
        dirty: applied_edits,
        // Recovered operations are already on disk, but syncing once more is
        // cheap and protects against a power cut right after startup.
        dirty_since: applied_edits.then_some(now),
        last_edit: applied_edits.then_some(now),
        closed: false,
        disk,
        look: false,
        external: None,
    };

    let inner = Arc::new(DocInner {
        location: Mutex::new(Location {
            rel: rel.to_path_buf(),
            path,
        }),
        journal_path: Mutex::new(journal_path),
        cfg,
        journal: Mutex::new(journal),
        journal_sync,
        save_lock: Mutex::new(()),
        state: Mutex::new(state),
        cv: Condvar::new(),
        append_cv: Condvar::new(),
        #[cfg(test)]
        before_atomic_write: Mutex::new(None),
    });
    let mut document = Document {
        inner,
        threads: Vec::new(),
    };
    for (name, run) in [
        ("carrel-journal", append_loop as fn(Arc<DocInner>)),
        ("carrel-store", writer_loop),
    ] {
        let inner = Arc::clone(&document.inner);
        let thread = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || run(inner))
            .map_err(StoreError::Io)?;
        document.threads.push(thread);
    }

    Ok(document)
}

impl Document {
    pub fn rel_path(&self) -> PathBuf {
        self.inner.location.lock().expect("location").rel.clone()
    }

    pub fn path(&self) -> PathBuf {
        self.inner.location.lock().expect("location").path.clone()
    }

    pub fn journal_path(&self) -> PathBuf {
        self.inner
            .journal_path
            .lock()
            .expect("journal path")
            .clone()
    }

    /// The full text. Used once when a file is opened; the editor keeps the
    /// live copy after that.
    pub fn text(&self) -> String {
        self.inner
            .state
            .lock()
            .expect("document state")
            .text
            .clone()
    }

    pub fn char_len(&self) -> usize {
        self.inner.state.lock().expect("document state").char_len
    }

    pub fn is_dirty(&self) -> bool {
        self.inner.state.lock().expect("document state").dirty
    }

    /// True once a write, save or rename failed, until a save succeeds.
    pub fn failed(&self) -> bool {
        self.inner.state.lock().expect("document state").failed
    }

    /// Record one edit and return its journal sequence number.
    ///
    /// Never touches the disk and never waits for a thread that does: the
    /// record is queued, and the journal thread writes it right away.
    pub fn apply(&self, offset: usize, delete_len: usize, insert: &str) -> StoreResult<u64> {
        let mut state = self.inner.state.lock().expect("document state");
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
        {
            let DocState { text, char_len, .. } = &mut *state;
            apply_edit(text, char_len, offset, delete_len, insert)
                .map_err(StoreError::InvalidEdit)?;
        }
        let record = Record::edit(seq, now_ms(), offset, delete_len, insert);
        state.pending.push(record);
        state.seq = seq.saturating_add(1);
        state.dirty = true;
        let now = Instant::now();
        state.last_edit = Some(now);
        if state.dirty_since.is_none() {
            state.dirty_since = Some(now);
        }
        drop(state);
        self.inner.append_cv.notify_all();
        self.inner.cv.notify_all();
        Ok(seq)
    }

    /// Give the document a new name without waiting for the disk: the writer
    /// thread saves and renames the file and its journal. `flush` waits for it.
    /// The caller has checked that `rel` is free.
    pub fn rename_later(&self, rel: PathBuf, path: PathBuf, journal_path: PathBuf) {
        let mut state = self.inner.state.lock().expect("document state");
        state.move_to = Some((Location { rel, path }, journal_path));
        state.renaming = true;
        drop(state);
        self.inner.cv.notify_all();
    }

    /// Wait for a rename asked for with [`Self::rename_later`], for a screen
    /// that is about to list or move files.
    pub fn finish_rename(&self) -> StoreResult<()> {
        if !self.inner.state.lock().expect("document state").renaming {
            return Ok(());
        }
        let _save_guard = self.inner.save_lock.lock().expect("save mutex");
        move_document(&self.inner)
    }

    /// True once after a [`Self::rename_later`] that could not be done; the
    /// document keeps its old name.
    pub fn take_rename_error(&self) -> bool {
        std::mem::take(
            &mut self
                .inner
                .state
                .lock()
                .expect("document state")
                .rename_failed,
        )
    }

    /// Read the file again on the writer thread, after a sync may have
    /// written it. [`Self::take_external`] hands over a changed text.
    pub fn look_at_disk(&self) {
        self.inner.state.lock().expect("document state").look = true;
        self.inner.cv.notify_all();
    }

    /// The text someone else wrote to the file, now also this document's. The
    /// caller must show it before the next [`Self::apply`]. `None` while there
    /// are unsaved edits: their save keeps the other text as a conflict copy.
    pub fn take_external(&self) -> Option<String> {
        let mut state = self.inner.state.lock().expect("document state");
        let (text, replaces) = state.external.take()?;
        if state.dirty || state.disk != replaces {
            return None;
        }
        state.disk = fnv1a64(text.as_bytes());
        state.char_len = text.chars().count();
        state.text = text.clone();
        Some(text)
    }

    /// Do a pending rename, fdatasync the journal and, if needed, atomically
    /// save the Markdown file. Called before suspend, file switch and exit.
    pub fn flush(&self) -> StoreResult<()> {
        let _save_guard = self.inner.save_lock.lock().expect("save mutex");
        move_document(&self.inner)?;
        save_snapshot(&self.inner, false, true)
    }

    /// Save now even if the idle timer has not fired.
    pub fn save_now(&self) -> StoreResult<()> {
        let _save_guard = self.inner.save_lock.lock().expect("save mutex");
        save_snapshot(&self.inner, false, false)
    }

    fn shutdown(&mut self) {
        {
            let mut state = self.inner.state.lock().expect("document state");
            state.closed = true;
            self.inner.cv.notify_all();
            self.inner.append_cv.notify_all();
        }
        for thread in self.threads.drain(..) {
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

/// Write every queued record to the journal. The journal lock keeps the file
/// in queue order; the state lock is held only to take the queue.
fn drain(inner: &DocInner, journal: &mut Journal) -> io::Result<()> {
    let batch = std::mem::take(&mut inner.state.lock().expect("document state").pending);
    if batch.is_empty() {
        return Ok(());
    }
    if let Err(error) = journal.append_all(&batch) {
        let mut state = inner.state.lock().expect("document state");
        state.failed = true;
        let newer = std::mem::replace(&mut state.pending, batch);
        state.pending.extend(newer);
        return Err(error);
    }
    Ok(())
}

fn append_loop(inner: Arc<DocInner>) {
    loop {
        {
            let mut state = inner.state.lock().expect("document state");
            while state.pending.is_empty() && !state.closed {
                state = inner.append_cv.wait(state).expect("document state");
            }
            // `shutdown` writes what is left.
            if state.closed {
                return;
            }
        }
        let result = drain(&inner, &mut inner.journal.lock().expect("journal mutex"));
        if let Err(error) = result {
            eprintln!("carrel: journal write failed: {error}");
            thread::sleep(inner.cfg.sync_interval);
        }
    }
}

/// Carry out a pending rename. Caller must hold `inner.save_lock`.
fn move_document(inner: &DocInner) -> StoreResult<()> {
    let pending = inner.state.lock().expect("document state").move_to.take();
    let Some((to, journal_to)) = pending else {
        return Ok(());
    };
    let _done = RenameDone(inner);
    // Save first, so the rename leaves as little as possible in the journal.
    save_snapshot(inner, false, true)?;
    let from = inner.location.lock().expect("location").clone();
    let renamed =
        rename_files(inner, &from, &to, journal_to).and_then(|()| match to.path.parent() {
            Some(parent) => sync_dir(parent),
            None => Ok(()),
        });
    if renamed.is_err() {
        let mut state = inner.state.lock().expect("document state");
        state.failed = true;
        state.rename_failed = true;
    }
    renamed.map_err(StoreError::Io)
}

/// Clears `renaming` however the rename ends.
struct RenameDone<'a>(&'a DocInner);

impl Drop for RenameDone<'_> {
    fn drop(&mut self) {
        self.0.state.lock().expect("document state").renaming = false;
    }
}

fn rename_files(
    inner: &DocInner,
    from: &Location,
    to: &Location,
    journal_to: PathBuf,
) -> io::Result<()> {
    let mut journal = inner.journal.lock().expect("journal mutex");
    slow_disk(&from.path);
    if to.path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a file with that name exists",
        ));
    }
    let journal_from = inner.journal_path.lock().expect("journal path").clone();
    fs::rename(&journal_from, &journal_to)?;
    if let Err(error) = fs::rename(&from.path, &to.path) {
        let _ = fs::rename(&journal_to, &journal_from);
        return Err(error);
    }
    journal.moved_to(journal_to.clone());
    *inner.journal_path.lock().expect("journal path") = journal_to;
    *inner.location.lock().expect("location") = to.clone();
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Save a snapshot of the document without holding the state mutex across
/// any disk access. Caller must hold `inner.save_lock`.
fn save_snapshot(inner: &DocInner, require_idle: bool, always_sync: bool) -> StoreResult<()> {
    let result = write_snapshot(inner, require_idle, always_sync);
    if result.is_err() {
        inner.state.lock().expect("document state").failed = true;
    }
    result
}

fn write_snapshot(inner: &DocInner, require_idle: bool, always_sync: bool) -> StoreResult<()> {
    let snapshot = {
        let mut state = inner.state.lock().expect("document state");
        if state.dirty {
            if require_idle {
                let now = Instant::now();
                let is_idle = state
                    .last_edit
                    .is_some_and(|last| now >= last + inner.cfg.idle_save);
                if !is_idle {
                    return Ok(());
                }
            }
            // The checkpoint must be durable before the rename. Otherwise a
            // power cut after the rename could leave a file that already
            // contains the edits and a journal without the checkpoint that
            // says so.
            let checkpoint_seq = state.seq;
            let snapshot_text = state.text.clone();
            let checkpoint = Record::checkpoint(checkpoint_seq, now_ms(), &snapshot_text);
            state.pending.push(checkpoint);
            state.seq = checkpoint_seq.saturating_add(1);
            Some((checkpoint_seq, snapshot_text))
        } else {
            None
        }
    };
    let Some((checkpoint_seq, snapshot_text)) = snapshot else {
        if always_sync {
            drain(inner, &mut inner.journal.lock().expect("journal mutex"))?;
            sync_journal(inner)?;
        }
        return Ok(());
    };

    drain(inner, &mut inner.journal.lock().expect("journal mutex"))?;
    sync_journal(inner)?;

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

    let path = inner.location.lock().expect("location").path.clone();
    keep_other_version(inner, &path, &snapshot_text)?;
    atomic_write(&path, snapshot_text.as_bytes())?;
    inner.state.lock().expect("document state").disk = fnv1a64(snapshot_text.as_bytes());

    // Only clear the journal if no new edits arrived while we were syncing and
    // writing. If edits arrived in flight, keep the journal (with the durable
    // checkpoint) so crash recovery skips <= checkpoint_seq and replays only
    // the newer edits. Edits arriving after this check stay queued until the
    // journal lock is free, so they land after the cut.
    let clean = {
        let mut journal = inner.journal.lock().expect("journal mutex");
        let clean = {
            let mut state = inner.state.lock().expect("document state");
            state.failed = false;
            let clean = state.seq == checkpoint_seq.saturating_add(1);
            if clean {
                state.dirty = false;
                state.dirty_since = None;
                state.last_edit = None;
            }
            clean
        };
        if clean {
            journal.truncate_to(0)?;
        }
        clean
    };
    if clean {
        sync_journal(inner)?;
    }
    Ok(())
}

/// Before a save replaces the file: if someone else wrote it since this
/// document last did, keep their text next to it, named as `carreld` names a
/// conflict copy (spec/02-sync-protocol.md).
fn keep_other_version(inner: &DocInner, path: &Path, saving: &str) -> io::Result<()> {
    let other = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let known = inner.state.lock().expect("document state").disk;
    if fnv1a64(&other) == known || other == saving.as_bytes() {
        return Ok(());
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(());
    };
    let name = name.to_string_lossy();
    for minute in 0.. {
        let stamp = format_conflict_timestamp(now_unix() + minute * 60);
        let copy = parent.join(conflict_copy_path(&name, &stamp, "sync"));
        match fs::read(&copy) {
            Ok(existing) if existing == other => break,
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                atomic_write(&copy, &other)?;
                break;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Read the file for [`Document::look_at_disk`].
fn read_external(inner: &DocInner) {
    let _save_guard = inner.save_lock.lock().expect("save mutex");
    let path = inner.location.lock().expect("location").path.clone();
    slow_disk(&path);
    // A file that is gone or no longer text leaves the document as it is.
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let mut state = inner.state.lock().expect("document state");
    if fnv1a64(text.as_bytes()) != state.disk {
        state.external = Some((text, state.disk));
    }
}

fn sync_journal(inner: &DocInner) -> io::Result<()> {
    let path = inner.journal_path.lock().expect("journal path").clone();
    slow_disk(&path);
    inner.journal_sync.sync_data()
}

fn writer_loop(inner: Arc<DocInner>) {
    loop {
        let (sync_due, sync_seq, sync_started, save_due, move_due, look) = {
            let mut state = inner.state.lock().expect("document state");
            if state.closed {
                return;
            }
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

            if state.move_to.is_none() && !state.look {
                match next {
                    Some(deadline) if deadline > now => {
                        let (guard, _) = inner
                            .cv
                            .wait_timeout(state, deadline.saturating_duration_since(now))
                            .expect("document state");
                        state = guard;
                    }
                    Some(_) => {}
                    None => {
                        state = inner.cv.wait(state).expect("document state");
                    }
                }
            }

            // `shutdown` flushes after joining this thread.
            if state.closed {
                return;
            }
            let now = Instant::now();
            let sync_due = state
                .dirty_since
                .is_some_and(|since| now >= since + inner.cfg.sync_interval);
            let save_due = state.dirty
                && state
                    .last_edit
                    .is_some_and(|last| now >= last + inner.cfg.idle_save);
            let look = std::mem::take(&mut state.look);
            (
                sync_due,
                state.seq,
                now,
                save_due,
                state.move_to.is_some(),
                look,
            )
        };

        if look {
            read_external(&inner);
        }

        if move_due {
            let _save_guard = inner.save_lock.lock().expect("save mutex");
            if let Err(error) = move_document(&inner) {
                eprintln!("carrel: could not rename: {error}");
            }
        }

        if sync_due {
            let written = drain(&inner, &mut inner.journal.lock().expect("journal mutex"));
            match written.and_then(|()| sync_journal(&inner)) {
                Ok(()) => {
                    let mut state = inner.state.lock().expect("document state");
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
                    let mut state = inner.state.lock().expect("document state");
                    state.failed = true;
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
                let mut state = inner.state.lock().expect("document state");
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
    fn an_edit_reaches_the_journal_right_away() {
        let dir = tempfile::tempdir().unwrap();
        let policy = SavePolicy {
            sync_interval: Duration::from_secs(60),
            idle_save: Duration::from_secs(60),
        };
        let store = Store::open_with_policy(dir.path(), policy).unwrap();
        store.create_file(Path::new("note.md")).unwrap();
        let journal_path = store.journal_path(Path::new("note.md"));
        let doc = store.open_document(Path::new("note.md")).unwrap();
        doc.apply(0, 0, "typed").unwrap();
        assert!(wait_until(|| {
            let bytes = std::fs::read(&journal_path).unwrap();
            let decoded = crate::store::journal::decode(&bytes);
            decoded.records.len() == 1 && decoded.records[0].text == "typed"
        }));
    }

    /// Bytes the calling thread has passed to `write`.
    fn bytes_written_by_this_thread() -> u64 {
        std::fs::read_to_string("/proc/thread-self/io")
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("wchar: "))
            .unwrap()
            .parse()
            .unwrap()
    }

    #[test]
    fn typing_writes_nothing_on_the_typing_thread() {
        let (_dir, store, _journal) = setup();
        let doc = store.open_document(Path::new("note.md")).unwrap();
        let before = bytes_written_by_this_thread();
        for offset in 0..200 {
            doc.apply(offset, 0, "x").unwrap();
        }
        assert_eq!(bytes_written_by_this_thread(), before);
    }

    #[test]
    fn typing_never_waits_for_a_slow_disk() {
        let (dir, store, _journal) = setup();
        let doc = store.open_document(Path::new("note.md")).unwrap();
        let slow = crate::store::fsutil::slow_down(dir.path(), Duration::from_millis(300));
        // Several journal writes, fdatasyncs and idle saves happen meanwhile.
        let mut worst = Duration::ZERO;
        let started = Instant::now();
        let mut offset = 0;
        while started.elapsed() < Duration::from_secs(3) {
            let before = Instant::now();
            doc.apply(offset, 0, "x").unwrap();
            worst = worst.max(before.elapsed());
            offset += 1;
            // A pause long enough for the idle save every so often.
            thread::sleep(Duration::from_millis(if offset % 40 == 0 {
                400
            } else {
                5
            }));
        }
        drop(slow);
        assert!(worst < Duration::from_millis(100), "apply took {worst:?}");
        doc.flush().unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("note.md")).unwrap(),
            "x".repeat(offset)
        );
    }

    #[test]
    fn a_rename_later_keeps_the_text_and_the_edits_that_follow() {
        let (dir, store, journal) = setup();
        let doc = store.open_document(Path::new("note.md")).unwrap();
        doc.apply(0, 0, "first").unwrap();
        let target = Path::new("named.md");
        doc.rename_later(
            target.to_path_buf(),
            dir.path().join(target),
            store.journal_path(target),
        );
        doc.apply(5, 0, " second").unwrap();
        doc.flush().unwrap();
        assert_eq!(doc.rel_path(), target);
        assert!(!dir.path().join("note.md").exists());
        assert!(!journal.exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(target)).unwrap(),
            "first second"
        );
        doc.apply(12, 0, " third").unwrap();
        let moved = store.journal_path(target);
        assert!(wait_until(|| std::fs::metadata(&moved).unwrap().len() > 0));
        // As after a crash: the journal under the new name holds the edit.
        std::mem::forget(doc);
        let reopened = store.open_document(target).unwrap();
        assert_eq!(reopened.text(), "first second third");
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

    /// The conflict copies next to `note.md`, with their text.
    fn copies(dir: &Path) -> Vec<(String, String)> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with("note (conflict "))
            .map(|name| {
                let text = std::fs::read_to_string(dir.join(&name)).unwrap();
                (name, text)
            })
            .collect()
    }

    #[test]
    fn a_save_keeps_what_someone_else_wrote_as_a_conflict_copy() {
        let (dir, store, _journal) = setup();
        let doc = store.open_document(Path::new("note.md")).unwrap();
        doc.apply(0, 0, "mine").unwrap();
        doc.flush().unwrap();
        assert!(copies(dir.path()).is_empty());

        std::fs::write(dir.path().join("note.md"), "from the server").unwrap();
        doc.apply(4, 0, " and more").unwrap();
        doc.flush().unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("note.md")).unwrap(),
            "mine and more"
        );
        let copies = copies(dir.path());
        assert_eq!(copies.len(), 1, "{copies:?}");
        assert!(copies[0].0.ends_with(" from sync).md"), "{copies:?}");
        assert_eq!(copies[0].1, "from the server");
    }

    #[test]
    fn a_clean_document_takes_over_what_someone_else_wrote() {
        let (dir, store, _journal) = setup();
        let doc = store.open_document(Path::new("note.md")).unwrap();
        std::fs::write(dir.path().join("note.md"), "pulled").unwrap();
        doc.look_at_disk();
        let mut taken = None;
        assert!(wait_until(|| {
            taken = doc.take_external();
            taken.is_some()
        }));
        assert_eq!(taken.as_deref(), Some("pulled"));
        doc.apply(6, 0, " and typed").unwrap();
        doc.flush().unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("note.md")).unwrap(),
            "pulled and typed"
        );
        assert!(copies(dir.path()).is_empty());
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
