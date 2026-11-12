//! Google Drive: OAuth 2.0 device authorization and a one-way mirror.
//!
//! The device is the source of truth. Each sync uploads changed Markdown files
//! into a `Stylos` folder, reuses Drive file IDs from
//! `sync-state.sqlite`, and moves files that disappeared locally into
//! `Stylos/.deleted/`. Google Drive edits do not come back.
//!
//! Modules:
//! - [`config`]: `oauth.toml` client credentials and the `[gdrive]` section of
//!   `sync.toml`.
//! - [`oauth`]: the RFC 8628 device flow, token refresh and revoke.
//! - [`api`]: the few Drive v3 calls the mirror needs.
//! - [`state`]: `path → file id` mapping in `sync-state.sqlite`.
//! - [`mirror`]: the mirror algorithm and the [`DocxConverter`] hook.
//! - [`runner`]: the daemon-facing trait and the real backend.

pub mod api;
pub mod config;
pub mod mirror;
pub mod oauth;
pub mod runner;
pub mod state;

pub use api::{DriveClient, DriveEndpoints, DriveError, DriveFile};
pub use config::{GdriveConfig, OAuthApp};
pub use mirror::{mirror_once, DocxConverter, GdriveReport, MirrorOptions};
pub use oauth::{DeviceCode, OAuthClient, OAuthEndpoints};
pub use runner::{DriveSyncRunner, GdriveRunner};
pub use state::{DriveEntry, EntryKind, GdriveState};
