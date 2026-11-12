//! `stylosd`: the Stylos system daemon.
//!
//! The crate contains the sync client and pairing (WP3), local Git history,
//! radio control, Wi-Fi, battery handling and the `org.stylos.Daemon1` D-Bus
//! service (WP4). Policy lives in [`daemon`], D-Bus glue in [`dbus`]; everything
//! outside the platform adapters is pure Rust and testable with fakes.

pub mod config;
pub mod daemon;
pub mod dbus;
pub mod history;
pub mod http;
pub mod network;
pub mod pairing;
pub mod power;
pub mod radio;
pub mod state;
pub mod sync;
pub mod tls;

pub use config::SyncConfig;
pub use daemon::{DaemonCore, DaemonDeps, DaemonEvent, PairingInfo, SyncInfo};
pub use history::HistoryEntry;
pub use pairing::{generate_device_pubkey, PairCredential, PairingSession};
pub use sync::{sync_once, SyncError, SyncOptions, SyncReport};
