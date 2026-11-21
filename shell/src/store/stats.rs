//! Daily word count for the bottom line.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use serde::{Deserialize, Serialize};

use super::fsutil::atomic_write;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyStats {
    /// Local date, `YYYY-MM-DD`.
    pub date: String,
    pub words: u64,
}

/// Write daily word-count stats atomically.
pub fn write_stats(path: &Path, stats: &DailyStats) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(stats).map_err(io::Error::other)?;
    atomic_write(path, &json)
}

/// Background coalescing writer for `stats.json` so periodic word-count saves
/// never block the GTK main thread on `fsync`.
pub struct StatsWriter {
    path: PathBuf,
    shared: Arc<StatsShared>,
    write_lock: Arc<Mutex<()>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

struct StatsShared {
    pending: Mutex<StatsPending>,
    cv: Condvar,
}

struct StatsPending {
    stats: Option<DailyStats>,
    closed: bool,
}

impl StatsWriter {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            shared: Arc::new(StatsShared {
                pending: Mutex::new(StatsPending {
                    stats: None,
                    closed: false,
                }),
                cv: Condvar::new(),
            }),
            write_lock: Arc::new(Mutex::new(())),
            thread: Mutex::new(None),
        }
    }

    /// Queue `stats` to be written on the background thread, overwriting any
    /// older un-written snapshot. Never blocks on disk I/O.
    pub fn enqueue(&self, stats: DailyStats) {
        {
            let mut pending = self.shared.pending.lock().expect("stats pending mutex");
            if pending.closed {
                return;
            }
            pending.stats = Some(stats);
            self.shared.cv.notify_all();
        }
        let mut handle = self.thread.lock().expect("stats thread mutex");
        if handle.is_none() {
            let path = self.path.clone();
            let shared = Arc::clone(&self.shared);
            let write_lock = Arc::clone(&self.write_lock);
            match thread::Builder::new()
                .name("carrel-stats".to_owned())
                .spawn(move || stats_writer_loop(path, shared, write_lock))
            {
                Ok(join_handle) => *handle = Some(join_handle),
                Err(error) => eprintln!("carrel: could not start stats writer thread: {error}"),
            }
        }
    }

    /// Synchronously flush `stats` to disk, discarding any older queued snapshot.
    pub fn flush(&self, stats: &DailyStats) -> io::Result<()> {
        let _write_guard = self.write_lock.lock().expect("stats write mutex");
        {
            let mut pending = self.shared.pending.lock().expect("stats pending mutex");
            pending.stats = None;
        }
        write_stats(&self.path, stats)
    }
}

impl Drop for StatsWriter {
    fn drop(&mut self) {
        {
            let mut pending = self.shared.pending.lock().expect("stats pending mutex");
            pending.closed = true;
            self.shared.cv.notify_all();
        }
        if let Some(handle) = self.thread.lock().expect("stats thread mutex").take() {
            let _ = handle.join();
        }
    }
}

fn stats_writer_loop(path: PathBuf, shared: Arc<StatsShared>, write_lock: Arc<Mutex<()>>) {
    loop {
        {
            let mut pending = shared.pending.lock().expect("stats pending mutex");
            while pending.stats.is_none() && !pending.closed {
                pending = shared.cv.wait(pending).expect("stats pending mutex");
            }
            if pending.stats.is_none() && pending.closed {
                return;
            }
        }

        let _write_guard = write_lock.lock().expect("stats write mutex");
        let snapshot = {
            let mut pending = shared.pending.lock().expect("stats pending mutex");
            pending.stats.take()
        };
        if let Some(stats) = snapshot {
            if let Err(error) = write_stats(&path, &stats) {
                eprintln!("carrel: could not save word count: {error}");
            }
        }
    }
}

/// Count words in prose. Standalone Markdown markers do not count.
pub fn count_words(text: &str) -> u64 {
    text.split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count() as u64
}

/// Add a signed delta without going below zero.
pub fn add_delta(words: u64, delta: i64) -> u64 {
    if delta >= 0 {
        words.saturating_add(delta as u64)
    } else {
        words.saturating_sub(delta.unsigned_abs())
    }
}

/// Group digits with a comma: `1234` → `1,234`.
pub fn format_number(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn counts_prose_not_markers() {
        assert_eq!(count_words(""), 0);
        assert_eq!(count_words("# Chapter one\n\nA **quiet** morning."), 5);
        assert_eq!(count_words("---\n> quote"), 1);
        assert_eq!(count_words("héllo — 世界"), 2);
    }

    #[test]
    fn delta_does_not_underflow() {
        assert_eq!(add_delta(10, 5), 15);
        assert_eq!(add_delta(3, -5), 0);
    }

    #[test]
    fn formats_thousands() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1234567), "1,234,567");
    }

    #[test]
    fn stats_writer_coalesces_and_flushes_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.json");
        {
            let writer = StatsWriter::new(path.clone());
            for words in 1..=50u64 {
                writer.enqueue(DailyStats {
                    date: "2026-11-21".to_owned(),
                    words,
                });
            }
        }
        let saved: DailyStats = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.words, 50);
    }

    #[test]
    fn stats_writer_flush_supersedes_older_queued_stats() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.json");
        let writer = StatsWriter::new(path.clone());
        writer.enqueue(DailyStats {
            date: "2026-11-21".to_owned(),
            words: 10,
        });
        writer
            .flush(&DailyStats {
                date: "2026-11-21".to_owned(),
                words: 99,
            })
            .unwrap();
        thread::sleep(Duration::from_millis(50));
        let saved: DailyStats = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.words, 99);
    }
}
