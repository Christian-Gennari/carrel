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
//! * `> quotes` (indented)
//! * `-`, `*`, `+` and `1.` list markers (always dimmed)
//!
//! Every other piece of syntax (`**`, `_`, `# `, `> `) is a `Kind::Marker`:
//! the editor shows it dimmed on the caret's line and hides it elsewhere.
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
    /// Markdown syntax itself (`**`, `_`, `# `, `> `): dimmed on the line
    /// the caret is on, hidden on every other line, so the page reads as
    /// prose and the line being edited shows exactly what is stored.
    Marker,
    /// A list marker (`- `, `1. `): part of how a list reads, so it stays
    /// visible and dimmed on every line.
    ListMarker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

const CITE_SCHEME: &[char] = &['n', 'o', 's', 't', 'o', 's', ':', 'c', 'i', 't', 'e', '/'];
const CITE_LINK_TARGET: &[char] = &[
    ']', '(', 'n', 'o', 's', 't', 'o', 's', ':', 'c', 'i', 't', 'e', '/',
];

/// Cached Markdown parse + word count for a single logical line (excluding its
/// trailing line break).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedLine {
    pub char_len: usize,
    /// Character length of the trailing line break in `GtkTextBuffer`
    /// (`2` for `\r\n`, `1` for `\n`, `\r`, or `\u{2029}`, `0` at EOF).
    pub break_len: usize,
    pub words: u64,
    /// Line-relative spans (`0..=char_len`).
    pub spans: Vec<Span>,
    /// Line-relative `(start, end)` character ranges of `Kind::CiteLink`.
    pub cite_links: Vec<(usize, usize)>,
}

/// Iterator over `(line_text, break_char_len)` pairs matching `GtkTextBuffer`
/// line boundaries (`\r\n`, `\r`, `\n`, and `\u{2029}`).
///
/// Always yields at least one line (even for `""`), and yields a trailing empty
/// line when `text` ends with a line break, matching `GtkTextBuffer::line_count()`.
struct GtkLines<'a> {
    rest: &'a str,
    done: bool,
}

impl<'a> GtkLines<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            rest: text,
            done: false,
        }
    }
}

impl<'a> Iterator for GtkLines<'a> {
    type Item = (&'a str, usize);

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        for (idx, ch) in self.rest.char_indices() {
            match ch {
                '\r' => {
                    let line = &self.rest[..idx];
                    if self.rest[idx + 1..].starts_with('\n') {
                        self.rest = &self.rest[idx + 2..];
                        return Some((line, 2));
                    } else {
                        self.rest = &self.rest[idx + 1..];
                        return Some((line, 1));
                    }
                }
                '\n' | '\u{2029}' => {
                    let line = &self.rest[..idx];
                    self.rest = &self.rest[idx + ch.len_utf8()..];
                    return Some((line, 1));
                }
                _ => {}
            }
        }
        self.done = true;
        Some((self.rest, 0))
    }
}

/// Per-line cache for incremental Markdown styling and O(1) document word count.
#[derive(Debug, Default, Clone)]
pub struct LineCache {
    pub lines: Vec<CachedLine>,
    pub total_words: u64,
}

impl LineCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rebuild(&mut self, text: &str) {
        self.lines.clear();
        self.total_words = 0;
        let mut scratch = Vec::new();
        for (line, break_len) in GtkLines::new(text) {
            let cached = scan_cached_line(line, break_len, &mut scratch);
            self.total_words += cached.words;
            self.lines.push(cached);
        }
    }

    /// Replace `old_start_line..=old_end_line` with the lines in `new_text`
    /// (split on GTK line breaks `\r\n`, `\r`, `\n`, `\u{2029}`) and return the
    /// count of newly inserted lines. `trailing_break_len` is applied to the
    /// final inserted line.
    pub fn replace_lines(
        &mut self,
        old_start_line: usize,
        old_end_line: usize,
        new_text: &str,
        trailing_break_len: usize,
    ) -> usize {
        for line in &self.lines[old_start_line..=old_end_line] {
            self.total_words = self.total_words.saturating_sub(line.words);
        }
        let mut scratch = Vec::new();
        let mut replacement = Vec::new();
        for (line, break_len) in GtkLines::new(new_text) {
            let cached = scan_cached_line(line, break_len, &mut scratch);
            self.total_words += cached.words;
            replacement.push(cached);
        }
        if let Some(last) = replacement.last_mut() {
            last.break_len = trailing_break_len;
        }
        let count = replacement.len();
        self.lines
            .splice(old_start_line..=old_end_line, replacement);
        count
    }
}

fn scan_cached_line(line: &str, break_len: usize, scratch: &mut Vec<char>) -> CachedLine {
    scratch.clear();
    scratch.extend(line.chars());
    let char_len = scratch.len();
    let mut spans = Vec::new();
    scan_line(scratch, 0, char_len, &mut spans);
    spans.retain(|span| span.start < span.end);
    let cite_links = spans
        .iter()
        .filter(|span| span.kind == Kind::CiteLink)
        .map(|span| (span.start, span.end))
        .collect();
    let words = crate::store::stats::count_words(line);
    CachedLine {
        char_len,
        break_len,
        words,
        spans,
        cite_links,
    }
}

/// Character ranges for `text`. Offsets count characters, not bytes, because
/// that is what GTK text buffers use.
pub fn spans(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut scratch = Vec::new();
    let mut line_start = 0usize;

    for (line, break_len) in GtkLines::new(text) {
        scratch.clear();
        scratch.extend(line.chars());
        let line_len = scratch.len();
        let before = spans.len();
        scan_line(&scratch, 0, line_len, &mut spans);
        if line_start != 0 {
            for span in &mut spans[before..] {
                span.start += line_start;
                span.end += line_start;
            }
        }
        line_start += line_len + break_len;
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
        // The space after the hashes belongs to the marker, so a hidden
        // marker leaves no gap before the heading's first word. A lone `#`
        // stays visible: hiding it would leave an empty-looking line.
        if start + hashes < end && start + hashes + 1 < end {
            spans.push(Span {
                start,
                end: start + hashes + 1,
                kind: Kind::Marker,
            });
        } else {
            spans.push(Span {
                start,
                end: start + hashes,
                kind: Kind::ListMarker,
            });
        }
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
        // Likewise `> `: the space after `>` hides with it.
        let marker_end = if quote_at + 1 < end && chars[quote_at + 1] == ' ' {
            quote_at + 2
        } else {
            quote_at + 1
        };
        spans.push(Span {
            start: quote_at,
            end: marker_end,
            kind: if marker_end < end {
                Kind::Marker
            } else {
                Kind::ListMarker
            },
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
            kind: Kind::ListMarker,
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
            if let Some(after) = scan_emphasis(chars, index, end, spans) {
                index = after;
                continue;
            }
        }

        if ch == 'n' && starts_with_chars(chars, index, end, CITE_SCHEME) {
            // A bare target has no label: hide the scheme and keep the id as
            // the quiet marker. The buffer text itself is never touched, so
            // the file on disk stays exactly as it was stored.
            let scheme_end = index + CITE_SCHEME.len();
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
    let mut close = start + 1;
    while close < end && chars[close] != ']' {
        if chars[close] == '[' {
            return None;
        }
        close += 1;
    }
    if close >= end || !starts_with_chars(chars, close, end, CITE_LINK_TARGET) {
        return None;
    }
    let mut at = close + CITE_LINK_TARGET.len();
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
    let mut close = start + 1;
    while close < end && chars[close] != ']' {
        if chars[close] == '[' {
            return None;
        }
        close += 1;
    }
    if close >= end || !starts_with_chars(chars, close, end, CITE_LINK_TARGET) {
        return None;
    }
    let id_start = close + CITE_LINK_TARGET.len();
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

fn scan_emphasis(chars: &[char], start: usize, end: usize, spans: &mut Vec<Span>) -> Option<usize> {
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
            return Some(close + width);
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

#[inline]
fn starts_with_chars(chars: &[char], start: usize, end: usize, needle: &[char]) -> bool {
    start + needle.len() <= end && chars[start..start + needle.len()] == *needle
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
        assert!(has(text, Kind::Marker, "## "));
        // A lone hash is not hidden: the line would look empty.
        assert!(has("#", Kind::ListMarker, "#"));
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
        assert!(has(text, Kind::Marker, "> "));
    }

    #[test]
    fn list_markers_are_dimmed() {
        for (text, marker) in [
            ("- buy ink", "- "),
            ("* buy ink", "* "),
            ("+ buy ink", "+ "),
            ("1. buy ink", "1. "),
        ] {
            assert!(has(text, Kind::ListMarker, marker), "{text:?}");
        }
        // Three leading spaces are a list; four are a code block.
        assert!(has("   - nested", Kind::ListMarker, "- "));
        assert!(!has("    - code", Kind::ListMarker, "- "));
        // Emphasis at the start of a line is not a bullet.
        assert!(!has("**bold**", Kind::ListMarker, "* "));
        assert!(!has("*not closed", Kind::ListMarker, "* "));
        // An ordered marker keeps its punctuation and the space.
        assert!(has("12) an item", Kind::ListMarker, "12) "));
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

    fn flatten_cache_spans(cache: &LineCache) -> Vec<Span> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        for line in &cache.lines {
            for span in &line.spans {
                out.push(Span {
                    start: offset + span.start,
                    end: offset + span.end,
                    kind: span.kind,
                });
            }
            offset += line.char_len + line.break_len;
        }
        out
    }

    #[test]
    fn line_cache_matches_full_scan_across_edits() {
        let initial =
            "# Title\n\nPlain line with **bold**.\n> quoted line\nSee [^1](nostos:cite/a1).\n";
        let mut cache = LineCache::new();
        cache.rebuild(initial);

        assert_eq!(cache.total_words, crate::store::stats::count_words(initial));
        assert_eq!(flatten_cache_spans(&cache), spans(initial));

        // Replace one line (line 2) with two lines (line split).
        let inserted = cache.replace_lines(2, 2, "Plain line\nwith *italic* and _more_.", 1);
        assert_eq!(inserted, 2);
        let after_split =
            "# Title\n\nPlain line\nwith *italic* and _more_.\n> quoted line\nSee [^1](nostos:cite/a1).\n";
        assert_eq!(
            cache.total_words,
            crate::store::stats::count_words(after_split)
        );
        assert_eq!(flatten_cache_spans(&cache), spans(after_split));

        // Merge lines 2..=3 back into a single line with UTF-8 + bare citation.
        let inserted = cache.replace_lines(2, 3, "Café nostos:cite/bar-9.", 1);
        assert_eq!(inserted, 1);
        let after_merge =
            "# Title\n\nCafé nostos:cite/bar-9.\n> quoted line\nSee [^1](nostos:cite/a1).\n";
        assert_eq!(
            cache.total_words,
            crate::store::stats::count_words(after_merge)
        );
        assert_eq!(flatten_cache_spans(&cache), spans(after_merge));
    }

    #[test]
    fn line_cache_handles_cr_crlf_and_paragraph_separator() {
        let initial = "# Head\r\nPlain\r**bold** middle\u{2029}*soft* end";
        let mut cache = LineCache::new();
        cache.rebuild(initial);

        assert_eq!(cache.lines.len(), 4);
        assert_eq!(cache.lines[0].break_len, 2);
        assert_eq!(cache.lines[1].break_len, 1);
        assert_eq!(cache.lines[2].break_len, 1);
        assert_eq!(cache.lines[3].break_len, 0);
        assert_eq!(cache.total_words, crate::store::stats::count_words(initial));
        assert_eq!(flatten_cache_spans(&cache), spans(initial));

        // Replace line 1 ("Plain") with three lines separated by \r, \r\n, \u{2029}.
        let inserted = cache.replace_lines(1, 1, "one\r**two**\r\n_three_\u{2029}four", 1);
        assert_eq!(inserted, 4);
        let updated =
            "# Head\r\none\r**two**\r\n_three_\u{2029}four\r**bold** middle\u{2029}*soft* end";
        assert_eq!(cache.lines.len(), 7);
        assert_eq!(cache.total_words, crate::store::stats::count_words(updated));
        assert_eq!(flatten_cache_spans(&cache), spans(updated));
    }

    #[test]
    fn bare_citation_and_non_citation_n_words() {
        let text = "note nothing nostos:cite nostos:cite/ nostos:cite/ok-1.";
        assert!(has(text, Kind::CiteMarker, "ok-1"));
        assert!(has(text, Kind::CiteHidden, "nostos:cite/"));
        assert!(!has(text, Kind::CiteMarker, "nostos:cite"));
    }
}
