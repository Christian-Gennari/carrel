//! Test helper: type at the end of a document through the real store until it
//! is killed. `tests/crash.rs` spawns this process and sends SIGKILL.
//!
//! Usage: stylos-crash-writer <writing-root> <ack-file> <seed>
//!
//! Each acknowledgement line is `<character length>\t<unix millis>`. The
//! character stream is deterministic from the absolute index, so the parent
//! can verify both length and content.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use stylos_shell::store::Store;

const ALPHABET: &[char] = &[
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's',
    't', 'u', 'v', 'w', 'x', 'y', 'z', ' ', ' ', ' ', ' ', '\n', 'é', '—', '世',
];

/// The same splitmix64-based stream the test checks against.
pub fn stream_char(index: u64) -> char {
    let mut value = index.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    ALPHABET[(value as usize) % ALPHABET.len()]
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: stylos-crash-writer <writing-root> <ack-file> <seed>");
        std::process::exit(2);
    }
    let root = PathBuf::from(&args[1]);
    let ack_path = PathBuf::from(&args[2]);
    let mut seed: u64 = args[3].parse().unwrap_or(1);
    if seed == 0 {
        seed = 1;
    }

    let store = match Store::open(&root) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("crash-writer: cannot open store: {error}");
            std::process::exit(1);
        }
    };
    if !root.join("crash.md").exists() {
        if let Err(error) = store.create_file(Path::new("crash.md")) {
            eprintln!("crash-writer: cannot create file: {error}");
            std::process::exit(1);
        }
    }
    let doc = match store.open_document(Path::new("crash.md")) {
        Ok(doc) => doc,
        Err(error) => {
            eprintln!("crash-writer: cannot open document: {error}");
            std::process::exit(1);
        }
    };
    let mut ack = match OpenOptions::new().create(true).append(true).open(&ack_path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("crash-writer: cannot open ack file: {error}");
            std::process::exit(1);
        }
    };

    let mut length = doc.char_len();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(4) {
        let ch = stream_char(length as u64);
        let mut buffer = [0u8; 4];
        let text = ch.encode_utf8(&mut buffer);
        match doc.apply(length, 0, text) {
            Ok(_) => {
                length += 1;
                // Keep the acknowledgement unbuffered: File writes go straight
                // to the kernel, and the parent reads them after SIGKILL.
                let _ = writeln!(ack, "{length}\t{}", now_ms());
            }
            Err(error) => {
                eprintln!("crash-writer: apply failed: {error}");
                std::process::exit(1);
            }
        }
        // A few milliseconds between keys, like a fast writer.
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let pause = 1 + (seed >> 60) % 3;
        std::thread::sleep(Duration::from_millis(pause));
    }
    let _ = doc.flush();
}
