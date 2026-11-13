//! Sync conflict copies, named exactly as `carreld` writes them:
//! `name (conflict YYYY-MM-DD HHMM from <source>).md`
//! (spec/02-sync-protocol.md, `protocol/src/paths.rs`).
//!
//! The Files screen shows the base name with a Conflict tag and offers the
//! writer a way out: keep mine, keep theirs, or keep both. No GTK here.

use std::path::{Path, PathBuf};

use crate::store::{display_name, is_document_name};

/// The two parts of a conflict-copy name that the resolution needs: the name
/// of the document it conflicts with, and where the other version came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictCopy {
    /// The document name without its extension or the conflict suffix.
    pub base: String,
    /// The server or device label; may be empty when the daemon had none.
    pub source: String,
}

/// Read a file name as a conflict copy. Anything that does not have the
/// daemon's exact shape is an ordinary document.
pub fn parse(name: &str) -> Option<ConflictCopy> {
    if !is_document_name(name) {
        return None;
    }
    let stem = display_name(name);
    // A copy of a copy keeps the older suffix in its base name, so the last
    // marker is the one that belongs to this file.
    let marker = " (conflict ";
    let start = stem.rfind(marker)?;
    let base = stem.get(..start)?.trim_end();
    if base.is_empty() || base.starts_with('.') || base.contains('/') || base.contains('\\') {
        return None;
    }
    let body = stem.get(start + marker.len()..)?.strip_suffix(')')?;
    let (stamp, source) = body.split_once(" from ")?;
    if !is_stamp(stamp) {
        return None;
    }
    Some(ConflictCopy {
        base: base.to_owned(),
        source: source.to_owned(),
    })
}

/// The document a conflict copy belongs beside: same folder, same extension,
/// base name without the suffix.
pub fn original_rel(rel: &Path) -> Option<PathBuf> {
    let name = rel.file_name()?.to_string_lossy();
    let copy = parse(&name)?;
    let extension = rel.extension()?.to_string_lossy();
    Some(rel.with_file_name(format!("{}.{}", copy.base, extension)))
}

/// `YYYY-MM-DD HHMM` in UTC, exactly 15 bytes.
fn is_stamp(stamp: &str) -> bool {
    let bytes = stamp.as_bytes();
    bytes.len() == 15
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && bytes[10] == b' '
        && bytes[11..15].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::{original_rel, parse, ConflictCopy};
    use std::path::{Path, PathBuf};

    #[test]
    fn daemon_copies_are_recognised() {
        assert_eq!(
            parse("note (conflict 2026-11-12 1430 from X250).md"),
            Some(ConflictCopy {
                base: "note".to_owned(),
                source: "X250".to_owned(),
            })
        );
        assert_eq!(
            parse("one (conflict 2026-11-12 1430 from Studio).markdown"),
            Some(ConflictCopy {
                base: "one".to_owned(),
                source: "Studio".to_owned(),
            })
        );
        // The daemon can have no source label.
        assert_eq!(
            parse("a (conflict 2026-11-12 1430 from ).md"),
            Some(ConflictCopy {
                base: "a".to_owned(),
                source: String::new(),
            })
        );
    }

    #[test]
    fn ordinary_documents_are_not_conflicts() {
        for name in [
            "Notes.md",
            "Notes (draft).md",
            "Notes (conflict).md",
            "Notes (conflict 2026-11-12 from Studio).md",
            "Notes (conflict 2026-11-12 1430).md",
            "Notes (conflict 2026-11-12 1430 from).md",
            "Notes (conflict 26-11-12 1430 from Studio).md",
            "Notes (conflict 2026-11-12 1430 from Studio).txt.bak",
            ".hidden (conflict 2026-11-12 1430 from Studio).md",
        ] {
            assert_eq!(parse(name), None, "{name}");
        }
    }

    #[test]
    fn the_original_keeps_the_folder_and_extension() {
        assert_eq!(
            original_rel(Path::new("note (conflict 2026-11-12 1430 from X250).md")),
            Some(PathBuf::from("note.md"))
        );
        assert_eq!(
            original_rel(Path::new("Novel/one (conflict 2026-11-12 1430 from s).txt")),
            Some(PathBuf::from("Novel/one.txt"))
        );
        assert_eq!(original_rel(Path::new("Novel/one.md")), None);
    }

    #[test]
    fn a_copy_of_a_copy_keeps_the_older_suffix_as_its_base() {
        assert_eq!(
            parse("a (conflict 2026-11-12 1430 from X) (conflict 2026-11-13 0912 from Y).md"),
            Some(ConflictCopy {
                base: "a (conflict 2026-11-12 1430 from X)".to_owned(),
                source: "Y".to_owned(),
            })
        );
    }
}
