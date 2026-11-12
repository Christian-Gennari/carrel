//! Turning a first line into a file name.

/// A title from the first non-empty line, without Markdown decoration.
///
/// `# The quiet morning` → `The quiet morning`.
/// Returns `None` when the line has no usable letters (for example `---`).
pub fn title_from_first_line(text: &str) -> Option<String> {
    let line = text.lines().find(|line| !line.trim().is_empty())?;
    let mut title = line.trim();

    // Strip heading and quote markers.
    title = title.trim_start_matches('#').trim_start();
    title = title.trim_start_matches('>').trim_start();

    // Strip simple emphasis markers at the edges.
    for marker in ["***", "**", "*", "___", "__", "_"] {
        if title.starts_with(marker) {
            title = &title[marker.len()..];
        }
        if title.ends_with(marker) {
            title = &title[..title.len() - marker.len()];
        }
    }
    let title = title.trim();

    // Citations are links, not names.
    if title.is_empty() || title.starts_with("nostos:cite/") {
        return None;
    }

    let mut clean = String::new();
    for ch in title.chars() {
        if ch.is_control() {
            continue;
        }
        match ch {
            '/' | '\\' | ':' | '?' => clean.push('-'),
            _ => clean.push(ch),
        }
    }
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean = clean.trim_matches(|ch: char| ch == '.' || ch.is_whitespace());
    if clean.is_empty() || !clean.chars().any(char::is_alphanumeric) {
        return None;
    }

    let mut truncated: String = clean.chars().take(60).collect();
    while truncated.ends_with(' ') {
        truncated.pop();
    }
    Some(truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_first_non_empty_line() {
        assert_eq!(
            title_from_first_line("\n\n# The quiet morning\nand then"),
            Some("The quiet morning".to_owned())
        );
    }

    #[test]
    fn strips_quotes_and_emphasis() {
        assert_eq!(
            title_from_first_line("> **A walk**\n"),
            Some("A walk".to_owned())
        );
        assert_eq!(
            title_from_first_line("*chapter one*"),
            Some("chapter one".to_owned())
        );
    }

    #[test]
    fn removes_path_separators() {
        assert_eq!(
            title_from_first_line("Notes/on: writing?"),
            Some("Notes-on- writing-".to_owned())
        );
    }

    #[test]
    fn rejects_marker_only_lines() {
        assert_eq!(title_from_first_line("---\n"), None);
        assert_eq!(title_from_first_line("***\n"), None);
        assert_eq!(title_from_first_line(""), None);
    }

    #[test]
    fn truncates_long_titles_on_a_char_boundary() {
        let line = "é".repeat(120);
        let title = title_from_first_line(&line).unwrap();
        assert_eq!(title.chars().count(), 60);
    }
}
