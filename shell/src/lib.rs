//! Stylos shell library.
//!
//! `store` is pure Rust and has no GTK dependency; everything else is the
//! GTK4 interface. The crash-writer test helper uses the store through this
//! library crate.

pub mod app;
pub mod battery;
pub mod daemon;
pub mod editor;
pub mod export;
pub mod files;
pub mod first_run;
pub mod history;
pub mod locale;
pub mod markdown;
pub mod nav;
pub mod settings;
pub mod settings_page;
pub mod store;
pub mod sync;
pub mod theme;
pub mod ui;
pub mod wifi;
