//! A tiny, deliberately incomplete Markdown scanner.
//!
//! The editor does not render a preview and it never rewrites text: it only
//! asks this module which character ranges should look bold, italic, larger or
//! dimmed. That keeps the Markdown in the buffer exactly the Markdown on disk.
//!
//! Supported, because the spec asks for them:
//! * `**bold**` and `__bold__`
//! * `*italic*` and `_italic_` (not inside words, so `snake_case` is safe)
//! * `# headings` (levels 1–6)
//! * `> quotes` (indented, marker dimmed)
//! * `nostos:cite/…` links (small, raised, dimmed)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bold,
    Italic,
    Heading(u8),
    Quote,
    Cite,
    /// Markdown syntax itself; drawn dimmed.
    Marker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// Character ranges for `text`. Offsets count characters, not bytes, because
/// that is what GTK text buffers use.
pub fn spans(text: &str) -> Vec<Span> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut line_start = 0usize;

    while line_start <= chars.len() {
        let line_end = chars[line_start..]
            .iter()
            .position(|&ch| ch == '\n')
            .map_or(chars.len(), |offset| line_start + offset);
        scan_line(&chars, line_start, line_end, &mut spans);
        if line_end == chars.len() {
            break;
        }
        line_start = line_end + 1;
    }

    spans.retain(|span| span.start < span.end);
    spans
}

fn scan_line(chars: &[char], start: usize, end: usize, spans: &mut Vec<Span>) {
    if start >= end {
        return;
    }

    // Heading.
    let mut hashes = 0usize;
    while start + hashes < end && chars[start + hashes] == '#' && hashes < 6 {
        hashes += 1;
    }
    if hashes > 0 && (start + hashes == end || chars[start + hashes] == ' ') {
        spans.push(Span {
            start,
            end,
            kind: Kind::Heading(hashes as u8),
        });
        spans.push(Span {
            start,
            end: start + hashes,
            kind: Kind::Marker,
        });
    }

    // Quote. Allow leading spaces, as writers often indent quotes.
    let mut quote_at = start;
    while quote_at < end && chars[quote_at] == ' ' {
        quote_at += 1;
    }
    if quote_at < end && chars[quote_at] == '>' {
        spans.push(Span {
            start,
            end,
            kind: Kind::Quote,
        });
        spans.push(Span {
            start: quote_at,
            end: quote_at + 1,
            kind: Kind::Marker,
        });
    }

    let mut index = start;
    while index < end {
        let ch = chars[index];

        if ch == '\\' {
            index += 2;
            continue;
        }

        if ch == '*' || ch == '_' {
            if let Some(next) = scan_emphasis(chars, index, end) {
                spans.extend(next.spans);
                index = next.after;
                continue;
            }
        }

        if starts_with(chars, index, end, "nostos:cite/") {
            let mut link_end = index + "nostos:cite/".chars().count();
            while link_end < end && !chars[link_end].is_whitespace() {
                link_end += 1;
            }
            spans.push(Span {
                start: index,
                end: link_end,
                kind: Kind::Cite,
            });
            index = link_end;
            continue;
        }

        index += 1;
    }
}

struct Emphasis {
    spans: Vec<Span>,
    after: usize,
}

fn scan_emphasis(chars: &[char], start: usize, end: usize) -> Option<Emphasis> {
    let marker = chars[start];

    // Underscores inside a word are not emphasis (`snake_case`).
    if marker == '_' && start > 0 && chars[start - 1].is_alphanumeric() {
        return None;
    }

    let mut run = 0usize;
    while start + run < end && chars[start + run] == marker {
        run += 1;
    }
    if run > 3 {
        return None;
    }

    // Try the longest run first so ***both*** wins over **bold**.
    for width in (1..=run.min(3)).rev() {
        if width == 1 && run >= 2 {
            continue;
        }
        let content_start = start + width;
        if content_start >= end || chars[content_start].is_whitespace() {
            continue;
        }
        if let Some(close) = find_closing(chars, content_start, end, marker, width) {
            let mut spans = Vec::new();
            if width >= 2 {
                spans.push(Span {
                    start: content_start,
                    end: close,
                    kind: Kind::Bold,
                });
                spans.push(Span {
                    start,
                    end: content_start,
                    kind: Kind::Marker,
                });
                spans.push(Span {
                    start: close,
                    end: close + width,
                    kind: Kind::Marker,
                });
            }
            if width == 1 || width == 3 {
                spans.push(Span {
                    start: content_start,
                    end: close,
                    kind: Kind::Italic,
                });
                if width == 1 {
                    spans.push(Span {
                        start,
                        end: content_start,
                        kind: Kind::Marker,
                    });
                    spans.push(Span {
                        start: close,
                        end: close + 1,
                        kind: Kind::Marker,
                    });
                }
            }
            return Some(Emphasis {
                spans,
                after: close + width,
            });
        }
    }
    None
}

fn find_closing(
    chars: &[char],
    from: usize,
    end: usize,
    marker: char,
    width: usize,
) -> Option<usize> {
    let mut index = from;
    while index + width <= end {
        if chars[index] == marker {
            let mut run = 0usize;
            while index + run < end && chars[index + run] == marker {
                run += 1;
            }
            if run >= width && index > from {
                // Use the end of the run, so `**bold**` closes on the last two.
                return Some(index + run - width);
            }
            index += run;
        } else {
            index += 1;
        }
    }
    None
}

fn starts_with(chars: &[char], start: usize, end: usize, needle: &str) -> bool {
    let needle: Vec<char> = needle.chars().collect();
    start + needle.len() <= end && chars[start..start + needle.len()] == needle[..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(Kind, String)> {
        let chars: Vec<char> = text.chars().collect();
        spans(text)
            .into_iter()
            .map(|span| {
                (
                    span.kind,
                    chars[span.start..span.end].iter().collect::<String>(),
                )
            })
            .collect()
    }

    fn has(text: &str, kind: Kind, content: &str) -> bool {
        kinds(text)
            .iter()
            .any(|(found, text)| *found == kind && text == content)
    }

    #[test]
    fn plain_text_has_no_spans() {
        assert!(spans("A quiet morning.").is_empty());
    }

    #[test]
    fn bold_and_its_markers() {
        let text = "A **quiet** morning";
        assert!(has(text, Kind::Bold, "quiet"));
        assert!(has(text, Kind::Marker, "**"));
    }

    #[test]
    fn italic_and_its_markers() {
        let text = "A *quiet* morning";
        assert!(has(text, Kind::Italic, "quiet"));
        assert!(has(text, Kind::Marker, "*"));
    }

    #[test]
    fn bold_italic() {
        let text = "***very quiet***";
        assert!(has(text, Kind::Bold, "very quiet"));
        assert!(has(text, Kind::Italic, "very quiet"));
    }

    #[test]
    fn underscores_inside_words_are_not_emphasis() {
        let text = "write snake_case names";
        assert!(spans(text).is_empty());
    }

    #[test]
    fn headings_are_larger_and_markers_dimmed() {
        let text = "## Chapter two";
        assert!(has(text, Kind::Heading(2), "## Chapter two"));
        assert!(has(text, Kind::Marker, "##"));
        assert!(!has(
            "####### too many",
            Kind::Heading(6),
            "###### # too many"
        ));
    }

    #[test]
    fn quotes_are_indented() {
        let text = "> a quoted line";
        assert!(has(text, Kind::Quote, "> a quoted line"));
        assert!(has(text, Kind::Marker, ">"));
    }

    #[test]
    fn citations_are_superscript_markers() {
        let text = "As seen.[^1](nostos:cite/abc123)";
        assert!(has(text, Kind::Cite, "nostos:cite/abc123)"));
    }

    #[test]
    fn offsets_are_characters_not_bytes() {
        let text = "é **bonjour**";
        let spans = spans(text);
        let bold = spans.iter().find(|span| span.kind == Kind::Bold).unwrap();
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(
            chars[bold.start..bold.end].iter().collect::<String>(),
            "bonjour"
        );
    }

    #[test]
    fn unmatched_markers_stay_plain() {
        assert!(spans("**not closed").is_empty());
        assert!(spans("*not closed").is_empty());
    }
}
