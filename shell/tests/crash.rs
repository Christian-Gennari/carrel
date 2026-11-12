//! The crash gate: write through the real store, SIGKILL the writer at random
//! points, and check that every byte acknowledged more than one second before
//! the kill is on disk when a new process replays the journal.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use carrel_shell::store::Store;

const ALPHABET: &[char] = &[
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's',
    't', 'u', 'v', 'w', 'x', 'y', 'z', ' ', ' ', ' ', ' ', '\n', 'é', '—', '世',
];

/// Must match `src/bin/crash-writer.rs`.
fn stream_char(index: u64) -> char {
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

fn next_random(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value >> 12;
    value ^= value << 25;
    value ^= value >> 27;
    *state = value;
    value.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

fn read_acks(path: &Path) -> Vec<(u64, u64)> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let (length, time) = line.split_once('\t')?;
            Some((length.parse().ok()?, time.parse().ok()?))
        })
        .collect()
}

#[test]
fn every_byte_acknowledged_one_second_before_sigkill_is_recovered() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("writing");
    let store = Store::open(&root).unwrap();
    store.create_file(Path::new("crash.md")).unwrap();
    let writer = env!("CARGO_BIN_EXE_carrel-crash-writer");

    let mut random = 0x5e17_00d1_5eed_1234u64;
    let mut required_total = 0u64;
    let mut long_iterations = 0u64;
    let mut long_required = 0u64;

    for iteration in 0..200u64 {
        let ack_path = dir.path().join(format!("ack-{iteration}.txt"));
        let mut child = Command::new(writer)
            .arg(&root)
            .arg(&ack_path)
            .arg(iteration.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn crash writer");

        let draw = next_random(&mut random);
        let long = iteration % 8 == 0;
        let delay_ms = if long {
            1_100 + draw % 300
        } else {
            20 + draw % 260
        };
        std::thread::sleep(Duration::from_millis(delay_ms));

        let kill_time = now_ms();
        child.kill().expect("SIGKILL the writer");
        let status = child.wait().expect("wait for the writer");
        assert!(
            !status.success(),
            "iteration {iteration}: writer exited normally instead of being killed"
        );

        let acks = read_acks(&ack_path);
        let required = acks
            .iter()
            .filter(|(_, ack_time)| ack_time.saturating_add(1_000) <= kill_time)
            .map(|(length, _)| *length)
            .max()
            .unwrap_or(0);
        required_total = required_total.max(required);
        if long {
            long_iterations += 1;
            long_required = long_required.max(required);
        }

        // This is a fresh process opening the file: everything it sees comes
        // from the Markdown file plus the journal on disk.
        let doc = store
            .open_document(Path::new("crash.md"))
            .expect("replay crash.md");
        let text = doc.text();
        let recovered = text.chars().count() as u64;
        assert!(
            recovered >= required,
            "iteration {iteration}: acknowledged {required} characters before the \
             cut-off but only {recovered} were recovered"
        );

        for (index, ch) in text.chars().enumerate() {
            assert_eq!(
                ch,
                stream_char(index as u64),
                "iteration {iteration}: recovered text diverges at character {index}"
            );
        }

        // Save between iterations so the next child starts from a clean
        // Markdown file and the test does not grow a journal forever.
        doc.flush().expect("flush recovered text");
    }

    assert!(long_iterations > 0, "the test needs some longer runs");
    assert!(
        long_required > 0,
        "no byte was acknowledged more than a second before a kill; \
         the timing test would be vacuous"
    );
    assert!(required_total > 0);

    let doc = store.open_document(Path::new("crash.md")).unwrap();
    let text = doc.text();
    doc.flush().unwrap();
    assert_eq!(
        fs::read_to_string(root.join("crash.md")).unwrap(),
        text,
        "after the final flush the Markdown file must hold the recovered text"
    );
}
