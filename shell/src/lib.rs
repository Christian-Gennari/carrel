//! Stylos shell library.
//!
//! `store` is pure Rust and has no GTK dependency; everything else is the
//! GTK4 interface. The crash-writer test helper uses the store through this
//! library crate.

pub mod app;
pub mod battery;
pub mod editor;
pub mod files;
pub mod markdown;
pub mod settings;
pub mod store;
pub mod theme;
