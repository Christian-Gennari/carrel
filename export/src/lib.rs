#![forbid(unsafe_code)]
//! Export and import of Carrel writing.
//!
//! This crate is GTK-free and network-free: it only touches the writing root,
//! a destination directory (a mounted USB drive in the daemon) and the local
//! `pandoc`/`typst` binaries. [`export`] turns Markdown files into Markdown,
//! plain text, Word, OpenDocument, PDF, HTML or EPUB; [`import`] copies
//! `.md`/`.txt` files back into `<writing_root>/Imported/`. [`Drives`] lists,
//! mounts and powers off removable drives.
//!
//! Citations use the library link format from `spec/02-sync-protocol.md` and
//! `spec/03-ui.md`: a link whose target starts with `nostos:cite/`. Pandoc
//! output turns those links into footnotes using the cached library text from
//! [`CitationLookup`], with the link label as the fallback.

mod citations;
mod drives;
mod error;
mod export;
mod import;
mod markdown;
mod pandoc;
mod util;

pub use citations::{Citation, CitationLookup, MapCitations};
pub use drives::{DriveCall, DriveInfo, Drives, FakeDrives, UDisksDrives, MOUNT_OPTIONS};
pub use error::{Error, Result};
pub use export::{export, ExportReport, ExportedFile, Format, Progress, Selection};
pub use import::{import, ImportReport, ImportedFile};
