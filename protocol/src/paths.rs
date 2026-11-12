//! File path rules and conflict-copy naming.
//!
//! A protocol path is relative, `/`-separated, uses no `.`/`..`/hidden
//! components and ends in `.md`. Servers write files below their data
//! directory using exactly this path, so validation is a security boundary.

use crate::MAX_PATH_BYTES;

/// Why a path is not acceptable to a Stylos server or client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    Empty,
    TooLong,
    Absolute,
    ContainsBackslash,
    ContainsNul,
    EmptyComponent,
    CurrentDirectory,
    ParentDirectory,
    HiddenComponent,
    NotMarkdown,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            PathError::Empty => "path is empty",
            PathError::TooLong => "path is too long",
            PathError::Absolute => "path must be relative",
            PathError::ContainsBackslash => "path must use '/' separators",
            PathError::ContainsNul => "path contains a NUL byte",
            PathError::EmptyComponent => "path contains an empty component",
            PathError::CurrentDirectory => "path contains a '.' component",
            PathError::ParentDirectory => "path contains a '..' component",
            PathError::HiddenComponent => "path contains a hidden component",
            PathError::NotMarkdown => "path must end in '.md'",
        };
        f.write_str(message)
    }
}

impl std::error::Error for PathError {}

/// Validate a protocol file path.
pub fn validate_path(path: &str) -> Result<(), PathError> {
    if path.is_empty() {
        return Err(PathError::Empty);
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(PathError::TooLong);
    }
    if path.starts_with('/') {
        return Err(PathError::Absolute);
    }
    if path.contains('\\') {
        return Err(PathError::ContainsBackslash);
    }
    if path.contains('\0') {
        return Err(PathError::ContainsNul);
    }
    if !path.ends_with(".md") {
        return Err(PathError::NotMarkdown);
    }
    for component in path.split('/') {
        match component {
            "" => return Err(PathError::EmptyComponent),
            "." => return Err(PathError::CurrentDirectory),
            ".." => return Err(PathError::ParentDirectory),
            _ if component.starts_with('.') => return Err(PathError::HiddenComponent),
            _ => {}
        }
    }
    Ok(())
}

/// The name given to the other side's version when two devices edited the same
/// file: `name (conflict YYYY-MM-DD HHMM from <source>).md`.
///
/// `timestamp` is formatted in UTC with [`format_conflict_timestamp`]. `source`
/// is the label of the server (or other device) that produced the version; it
/// is sanitized so the result is always a safe file name next to `path`.
pub fn conflict_copy_path(path: &str, timestamp: &str, source: &str) -> String {
    let stem = path.strip_suffix(".md").unwrap_or(path);
    let timestamp = sanitize_timestamp(timestamp);
    let source = sanitize_source(source);
    format!("{stem} (conflict {timestamp} from {source}).md")
}

/// Format Unix seconds as the conflict-copy timestamp `YYYY-MM-DD HHMM` (UTC).
pub fn format_conflict_timestamp(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let seconds_of_day = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}{:02}",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60
    )
}

/// Convert a Unix day number to a Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn sanitize_timestamp(timestamp: &str) -> String {
    let cleaned: String = timestamp
        .chars()
        .map(|c| {
            if c.is_ascii_digit() || c == '-' || c == ' ' || c == ':' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned.to_string()
    }
}

fn sanitize_source(source: &str) -> String {
    let cleaned: String = source
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c: char| c == '.' || c == ' ' || c == '-');
    let truncated: String = trimmed.chars().take(48).collect();
    if truncated.is_empty() {
        "server".to_string()
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_paths() {
        for path in [
            "note.md",
            "notes/2026/november.md",
            "with space/file name.md",
            "Ünïcode/groß.md",
            "a.b/c.md",
        ] {
            assert!(validate_path(path).is_ok(), "{path}");
        }
    }

    #[test]
    fn rejects_unsafe_paths() {
        let cases = [
            ("", PathError::Empty),
            ("/note.md", PathError::Absolute),
            ("../note.md", PathError::ParentDirectory),
            ("dir/../note.md", PathError::ParentDirectory),
            ("dir/./note.md", PathError::CurrentDirectory),
            ("dir//note.md", PathError::EmptyComponent),
            ("dir/.hidden/note.md", PathError::HiddenComponent),
            (".stylos/note.md", PathError::HiddenComponent),
            ("dir\\note.md", PathError::ContainsBackslash),
            ("note.txt", PathError::NotMarkdown),
            ("note.md/", PathError::NotMarkdown),
        ];
        for (path, expected) in cases {
            assert_eq!(validate_path(path), Err(expected), "{path}");
        }
        assert_eq!(
            validate_path(&"a".repeat(MAX_PATH_BYTES + 1)),
            Err(PathError::TooLong)
        );
    }

    #[test]
    fn conflict_timestamp_is_utc() {
        assert_eq!(format_conflict_timestamp(0), "1970-01-01 0000");
        assert_eq!(format_conflict_timestamp(86_399), "1970-01-01 2359");
        assert_eq!(format_conflict_timestamp(1_767_957_000), "2026-01-09 1110");
        assert_eq!(format_conflict_timestamp(1_794_493_800), "2026-11-12 1430");
    }

    #[test]
    fn conflict_copy_name_follows_spec() {
        assert_eq!(
            conflict_copy_path("note.md", "2026-11-12 1430", "X250"),
            "note (conflict 2026-11-12 1430 from X250).md"
        );
        assert_eq!(
            conflict_copy_path("notes/one.md", "2026-11-12 1430", "server"),
            "notes/one (conflict 2026-11-12 1430 from server).md"
        );
        let unsafe_source = conflict_copy_path("a.md", "2026-11-12 1430", "../evil/path");
        assert_eq!(
            unsafe_source,
            "a (conflict 2026-11-12 1430 from _evil_path).md"
        );
        assert!(validate_path(&unsafe_source).is_ok());
        for path in ["a.md", "deep/dir/b.md"] {
            let copy = conflict_copy_path(path, "2026-11-12 1430", "");
            assert!(validate_path(&copy).is_ok(), "{copy}");
            assert!(copy.ends_with(".md"));
        }
    }
}
