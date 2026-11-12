//! `stylos-protocol`: shared types and helpers for the Stylos sync protocol v1.
//!
//! This crate is deliberately free of I/O, TLS, GTK and database code. It is
//! licensed MIT so that other implementations (such as Nostos Cloud) can use it
//! without adopting the GPL daemon.
//!
//! The protocol is specified in `spec/02-sync-protocol.md`.

pub mod hash;
pub mod pairing;
pub mod paths;
pub mod types;

pub use hash::{
    content_hash, decode_sha256_hex, hex_decode, hex_encode, sha256_hex, verify_content_hash,
};
pub use pairing::{
    format_pairing_code, normalize_pairing_code, pairing_code_from_bytes, pairing_code_matches,
    PAIRING_CODE_ALPHABET, PAIRING_CODE_LEN, PAIRING_TTL_SECONDS,
};
pub use paths::{conflict_copy_path, format_conflict_timestamp, validate_path, PathError};
pub use types::*;

/// Version advertised by this implementation, in mDNS TXT (`v=1`) and docs.
pub const PROTOCOL_VERSION: u32 = 1;

/// mDNS service type announced by self-hosted servers.
pub const MDNS_SERVICE_TYPE: &str = "_stylos._tcp.local.";

/// Largest file body accepted by a server, in bytes.
///
/// Writing files are plain Markdown; 16 MiB is far above any realistic draft
/// and keeps a malicious request from exhausting the server's memory.
pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;

/// Longest allowed relative file path, in bytes.
pub const MAX_PATH_BYTES: usize = 1024;

/// TXT records announced over mDNS, per the discovery spec.
pub fn mdns_txt_records(label: &str) -> Vec<String> {
    vec!["v=1".to_string(), format!("name={label}")]
}

/// Current wall-clock time in Unix seconds.
///
/// The protocol uses Unix seconds for `expires_at` and `modified_at`.
pub fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => 0,
    }
}

/// Build a fresh file id as required by the spec: UUIDv7, time ordered.
pub fn new_file_id() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}

/// Build a fresh pairing id (UUIDv7).
pub fn new_pair_id() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}
