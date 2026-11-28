//! Filesystem helpers with the durability rules from the spec.
//!
//! A USB stick can take seconds for one `write` or `fsync`, so nothing here
//! that touches the disk may run on the GTK main loop while the writer types.
//! Small state files (settings, the recent list) go through [`write_later`].

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;

#[derive(Default)]
struct Later {
    /// Newest bytes per file that are not on disk yet.
    pending: Mutex<BTreeMap<PathBuf, Vec<u8>>>,
    wake: Condvar,
    /// Held while writing, so two writers never share a temporary file.
    writing: Mutex<()>,
}

fn later() -> &'static Later {
    static LATER: OnceLock<&'static Later> = OnceLock::new();
    LATER.get_or_init(|| {
        let later: &'static Later = Box::leak(Box::default());
        let spawned = thread::Builder::new()
            .name("carrel-writer".to_owned())
            .spawn(move || loop {
                {
                    let mut pending = later.pending.lock().expect("pending writes");
                    while pending.is_empty() {
                        pending = later.wake.wait(pending).expect("pending writes");
                    }
                }
                if let Err(error) = settle() {
                    eprintln!("carrel: could not save: {error}");
                }
            });
        if let Err(error) = spawned {
            eprintln!("carrel: could not start the writer thread: {error}");
        }
        later
    })
}

/// Write `bytes` to `path` atomically on a background thread; a newer call for
/// the same file replaces this one. [`settle`] waits for it.
pub fn write_later(path: &Path, bytes: Vec<u8>) {
    let later = later();
    later
        .pending
        .lock()
        .expect("pending writes")
        .insert(path.to_path_buf(), bytes);
    later.wake.notify_all();
}

/// Write `path` now, dropping anything older still queued for it.
pub fn write_now(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let later = later();
    let _writing = later.writing.lock().expect("writer");
    later.pending.lock().expect("pending writes").remove(path);
    atomic_write(path, bytes)
}

/// The bytes of `path`, counting a queued write that is not on disk yet.
pub fn read_latest(path: &Path) -> io::Result<Vec<u8>> {
    let queued = later()
        .pending
        .lock()
        .expect("pending writes")
        .get(path)
        .cloned();
    match queued {
        Some(bytes) => Ok(bytes),
        None => fs::read(path),
    }
}

/// Write everything queued and wait for it. Called before exit, suspend and
/// power off. A write that fails is dropped and reported.
pub fn settle() -> io::Result<()> {
    let later = later();
    let _writing = later.writing.lock().expect("writer");
    let mut result = Ok(());
    loop {
        let next = later
            .pending
            .lock()
            .expect("pending writes")
            .first_key_value()
            .map(|(path, bytes)| (path.clone(), bytes.clone()));
        let Some((path, bytes)) = next else {
            return result;
        };
        if let Err(error) = atomic_write(&path, &bytes) {
            result = Err(error);
        }
        let mut pending = later.pending.lock().expect("pending writes");
        if pending.get(&path) == Some(&bytes) {
            pending.remove(&path);
        }
    }
}

/// Test hook: act like a slow USB stick for files under a directory.
#[cfg(test)]
static SLOW_DISKS: Mutex<Vec<(PathBuf, std::time::Duration)>> = Mutex::new(Vec::new());

/// Every disk write below `under` takes `delay` longer until the guard drops.
#[cfg(test)]
pub(crate) fn slow_down(under: &Path, delay: std::time::Duration) -> SlowDisk {
    let under = under.to_path_buf();
    SLOW_DISKS
        .lock()
        .expect("slow disk")
        .push((under.clone(), delay));
    SlowDisk(under)
}

#[cfg(test)]
pub(crate) struct SlowDisk(PathBuf);

#[cfg(test)]
impl Drop for SlowDisk {
    fn drop(&mut self) {
        SLOW_DISKS
            .lock()
            .expect("slow disk")
            .retain(|(under, _)| under != &self.0);
    }
}

/// Called before every disk write of the store; does nothing outside tests.
pub(crate) fn slow_disk(_path: &Path) {
    #[cfg(test)]
    {
        let delay = SLOW_DISKS
            .lock()
            .expect("slow disk")
            .iter()
            .find_map(|(under, delay)| _path.starts_with(under).then_some(*delay));
        if let Some(delay) = delay {
            thread::sleep(delay);
        }
    }
}

/// Write `bytes` to `path` atomically:
/// `name.ext.tmp` → fsync → rename → fsync the directory.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "file has no parent directory")
    })?;
    fs::create_dir_all(parent)?;

    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file has no name"))?;
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);

    slow_disk(path);
    let existing_permissions = fs::metadata(path).ok().map(|meta| meta.permissions());
    let result = (|| -> io::Result<()> {
        let mut file = File::create(&tmp)?;
        if let Some(permissions) = existing_permissions {
            let _ = file.set_permissions(permissions);
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        sync_dir(parent)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

/// Unique path inside `dir` for `stem.ext`: `stem.ext`, `stem 2.ext`, ...
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut candidate = join_name(dir, stem, ext);
    let mut counter = 2u32;
    while candidate.exists() {
        candidate = join_name(dir, &format!("{stem} {counter}"), ext);
        counter += 1;
    }
    candidate
}

fn join_name(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    if ext.is_empty() {
        dir.join(stem)
    } else {
        dir.join(format!("{stem}.{ext}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn atomic_write_replaces_and_leaves_no_tmp() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("note.md");
        atomic_write(&path, b"first").unwrap();
        atomic_write(&path, b"second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert!(!dir.path().join("note.md.tmp").exists());
    }

    #[test]
    fn later_writes_are_readable_at_once_and_on_disk_after_settle() {
        // The queue is shared with every test, so only this file's content is
        // checked, not what `settle` reports about the others.
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        write_later(&path, b"one".to_vec());
        write_later(&path, b"two".to_vec());
        assert_eq!(read_latest(&path).unwrap(), b"two");
        let _ = settle();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        write_later(&path, b"three".to_vec());
        write_now(&path, b"four").unwrap();
        let _ = settle();
        assert_eq!(fs::read(&path).unwrap(), b"four");
    }

    #[test]
    fn unique_path_counts_up() {
        let dir = tempdir().unwrap();
        assert_eq!(
            unique_path(dir.path(), "Untitled", "md"),
            dir.path().join("Untitled.md")
        );
        fs::write(dir.path().join("Untitled.md"), "").unwrap();
        assert_eq!(
            unique_path(dir.path(), "Untitled", "md"),
            dir.path().join("Untitled 2.md")
        );
    }
}
