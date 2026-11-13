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
//! * `-`, `*`, `+` and `1.` list markers (dimmed)
//! * `nostos:cite/…` links (the scheme is hidden and the id is a small,
//!   raised, dimmed marker)
//! * `[^1](nostos:cite/…)` and `[text](nostos:cite/…)` links: the label is a
//!   small raised marker and the rest of the link syntax is hidden, unless the
//!   cursor is inside the link

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bold,
    Italic,
    Heading(u8),
    Quote,
    /// The label of a `[label](nostos:cite/…)` link when it is short, like
    /// `1` in `[^1](…)`, or the id of a bare `nostos:cite/…` link: a small
    /// raised marker. Never the raw URL.
    CiteMarker,
    /// The label of a longer citation link (quoted words): normal size.
    CiteText,
    /// The link syntax around a label: `[`, `[^` and `](nostos:cite/…)`.
    /// Hidden while the cursor is outside the link.
    CiteHidden,
    /// The whole `[label](nostos:cite/…)` range. It is never drawn; the editor
    /// uses it to show the raw text while the cursor is inside.
    CiteLink,
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

    // List markers, dimmed like every other piece of Markdown syntax. Up to
    // three leading spaces are allowed, as CommonMark does. A `*` or `_` that
    // starts emphasis has no space after it, so it is not mistaken for a
    // bullet.
    if let Some((marker_start, marker_end)) = list_marker(chars, start, end) {
        spans.push(Span {
            start: marker_start,
            end: marker_end,
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

        if ch == '[' {
            if let Some(link_end) = scan_cite_link(chars, index, end, spans) {
                index = link_end;
                continue;
            }
            // A broken citation link stays completely plain: its target must
            // not become a bare marker. Skip the target as a unit.
            if let Some(link_end) = broken_cite_link_end(chars, index, end) {
                index = link_end;
                continue;
            }
        }

        if ch == '*' || ch == '_' {
            if let Some(next) = scan_emphasis(chars, index, end) {
                spans.extend(next.spans);
                index = next.after;
                continue;
            }
        }

        if starts_with(chars, index, end, "nostos:cite/") {
            // A bare target has no label: hide the scheme and keep the id as
            // the quiet marker. The buffer text itself is never touched, so
            // the file on disk stays exactly as it was stored.
            let scheme_end = index + "nostos:cite/".chars().count();
            let mut link_end = scheme_end;
            // Ids are slugs. Stopping at the first other character keeps
            // sentence punctuation and the `)` of a broken link outside the
            // marker, and leaves an empty target alone.
            while link_end < end
                && (chars[link_end].is_alphanumeric() || matches!(chars[link_end], '-' | '_'))
            {
                link_end += 1;
            }
            if link_end > scheme_end {
                spans.push(Span {
                    start: index,
                    end: link_end,
                    kind: Kind::CiteLink,
                });
                spans.push(Span {
                    start: index,
                    end: scheme_end,
                    kind: Kind::CiteHidden,
                });
                spans.push(Span {
                    start: scheme_end,
                    end: link_end,
                    kind: Kind::CiteMarker,
                });
                index = link_end;
                continue;
            }
        }

        index += 1;
    }
}

/// The end of a list marker starting at `start`: `- `, `* `, `+ `, or up to
/// nine digits followed by `. ` or `) `. `None` when the line does not start
/// with a list item. The trailing space is part of the marker so the dimmed
/// range ends where the item's text begins.
fn list_marker(chars: &[char], start: usize, end: usize) -> Option<(usize, usize)> {
    let mut at = start;
    while at < end && at - start < 3 && chars[at] == ' ' {
        at += 1;
    }
    if at >= end {
        return None;
    }
    match chars[at] {
        '-' | '*' | '+' => {
            let after = at + 1;
            (after < end && chars[after] == ' ').then_some((at, after + 1))
        }
        '0'..='9' => {
            let mut digits = at;
            while digits < end && digits - at < 9 && chars[digits].is_ascii_digit() {
                digits += 1;
            }
            let punctuation = digits;
            if punctuation >= end || !matches!(chars[punctuation], '.' | ')') {
                return None;
            }
            let after = punctuation + 1;
            (after < end && chars[after] == ' ').then_some((at, after + 1))
        }
        _ => None,
    }
}

/// The end of a malformed `[label](nostos:cite/…)` link starting at `start`:
/// the link's syntax stays plain, and the target inside it is not mistaken
/// for a bare marker. `None` when the text does not look like such a link.
fn broken_cite_link_end(chars: &[char], start: usize, end: usize) -> Option<usize> {
    const TARGET: &str = "](nostos:cite/";
    let mut close = start + 1;
    while close < end && chars[close] != ']' {
        if chars[close] == '[' {
            return None;
        }
        close += 1;
    }
    if close >= end || !starts_with(chars, close, end, TARGET) {
        return None;
    }
    let mut at = close + TARGET.chars().count();
    while at < end && !chars[at].is_whitespace() && chars[at] != ')' {
        at += 1;
    }
    Some(at)
}

/// A label of at most this many characters, without spaces, is drawn as a
/// raised marker. Anything longer is quoted text and keeps its size.
const MARKER_LABEL_MAX: usize = 8;

/// `[label](nostos:cite/id)` or `[^n](nostos:cite/id)` starting at `start`.
/// Pushes the spans and returns the index after the closing `)`.
fn scan_cite_link(
    chars: &[char],
    start: usize,
    end: usize,
    spans: &mut Vec<Span>,
) -> Option<usize> {
    const TARGET: &str = "](nostos:cite/";
    let mut close = start + 1;
    while close < end && chars[close] != ']' {
        if chars[close] == '[' {
            return None;
        }
        close += 1;
    }
    if close >= end || !starts_with(chars, close, end, TARGET) {
        return None;
    }
    let id_start = close + TARGET.chars().count();
    let mut link_end = id_start;
    while link_end < end && chars[link_end] != ')' && !chars[link_end].is_whitespace() {
        link_end += 1;
    }
    if link_end >= end || chars[link_end] != ')' || link_end == id_start {
        return None;
    }
    link_end += 1;

    let caret = chars.get(start + 1) == Some(&'^');
    let label_start = start + 1 + usize::from(caret);
    if label_start >= close {
        return None;
    }
    let label = &chars[label_start..close];
    let short = label.len() <= MARKER_LABEL_MAX && !label.iter().any(|ch| ch.is_whitespace());

    spans.push(Span {
        start,
        end: link_end,
        kind: Kind::CiteLink,
    });
    spans.push(Span {
        start,
        end: label_start,
        kind: Kind::CiteHidden,
    });
    spans.push(Span {
        start: label_start,
        end: close,
        kind: if short {
            Kind::CiteMarker
        } else {
            Kind::CiteText
        },
    });
    spans.push(Span {
        start: close,
        end: link_end,
        kind: Kind::CiteHidden,
    });
    Some(link_end)
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
    fn list_markers_are_dimmed() {
        for (text, marker) in [
            ("- buy ink", "- "),
            ("* buy ink", "* "),
            ("+ buy ink", "+ "),
            ("1. buy ink", "1. "),
        ] {
            assert!(has(text, Kind::Marker, marker), "{text:?}");
        }
        // Three leading spaces are a list; four are a code block.
        assert!(has("   - nested", Kind::Marker, "- "));
        assert!(!has("    - code", Kind::Marker, "- "));
        // Emphasis at the start of a line is not a bullet.
        assert!(!has("**bold**", Kind::Marker, "* "));
        assert!(!has("*not closed", Kind::Marker, "* "));
        // An ordered marker keeps its punctuation and the space.
        assert!(has("12) an item", Kind::Marker, "12) "));
    }

    #[test]
    fn bare_citation_targets_hide_the_scheme_and_keep_a_marker() {
        let text = "As seen. nostos:cite/abc123";
        assert!(has(text, Kind::CiteMarker, "abc123"));
        assert!(has(text, Kind::CiteHidden, "nostos:cite/"));
        assert!(has(text, Kind::CiteLink, "nostos:cite/abc123"));
        // Sentence punctuation stays outside the marker.
        let quoted = "As seen nostos:cite/abc123, and again.";
        assert!(has(quoted, Kind::CiteMarker, "abc123"));
        assert!(!has(quoted, Kind::CiteMarker, "abc123,"));
    }

    #[test]
    fn footnote_citation_links_become_a_marker_with_hidden_syntax() {
        let text = "As seen.[^1](nostos:cite/abc123) Next.";
        assert!(has(text, Kind::CiteMarker, "1"));
        assert!(has(text, Kind::CiteHidden, "[^"));
        assert!(has(text, Kind::CiteHidden, "](nostos:cite/abc123)"));
        assert!(has(text, Kind::CiteLink, "[^1](nostos:cite/abc123)"));
        // The target is not also a bare citation marker.
        assert!(!has(text, Kind::CiteMarker, "nostos:cite/abc123"));
        assert_eq!(
            kinds(text)
                .iter()
                .filter(|(kind, _)| *kind == Kind::CiteMarker)
                .count(),
            1
        );
    }

    #[test]
    fn short_labels_are_markers_and_quoted_words_keep_their_size() {
        assert!(has("x[3](nostos:cite/a)", Kind::CiteMarker, "3"));
        let quote = "[a quiet morning](nostos:cite/a)";
        assert!(has(quote, Kind::CiteText, "a quiet morning"));
        assert!(has(quote, Kind::CiteHidden, "["));
        assert!(!kinds(quote)
            .iter()
            .any(|(kind, _)| *kind == Kind::CiteMarker));
    }

    #[test]
    fn broken_citation_links_stay_plain() {
        for text in [
            "[^1](https://example.com)",
            "[^1](nostos:cite/)",
            "[^1](nostos:cite/abc",
            "[](nostos:cite/abc)",
            "[^](nostos:cite/abc)",
        ] {
            assert!(
                !kinds(text).iter().any(|(kind, _)| matches!(
                    kind,
                    Kind::CiteLink | Kind::CiteHidden | Kind::CiteMarker | Kind::CiteText
                )),
                "{text}"
            );
        }
    }

    #[test]
    fn citation_after_emphasis_and_with_a_long_id() {
        let text = "Some mornings the words come *slowly*.[^1](nostos:cite/quiet-mornings-42)";
        assert!(has(text, Kind::CiteMarker, "1"));
        assert!(has(
            text,
            Kind::CiteHidden,
            "](nostos:cite/quiet-mornings-42)"
        ));
    }

    #[test]
    fn citation_offsets_are_characters() {
        let text = "é.[^2](nostos:cite/x)";
        let link = spans(text)
            .into_iter()
            .find(|span| span.kind == Kind::CiteLink)
            .unwrap();
        assert_eq!((link.start, link.end), (2, text.chars().count()));
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
