//! `stylosd`: the Stylos system daemon.
//!
//! This crate currently contains the sync client, pairing client and their
//! state. The D-Bus service, radio control, Git history and library cache are
//! later work packages. Everything here is pure Rust so it is testable without
//! GTK or a session bus.

pub mod config;
pub mod http;
pub mod pairing;
pub mod state;
pub mod sync;
pub mod tls;

pub use config::SyncConfig;
pub use pairing::{generate_device_pubkey, PairCredential, PairingSession};
pub use sync::{sync_once, SyncError, SyncOptions, SyncReport};
