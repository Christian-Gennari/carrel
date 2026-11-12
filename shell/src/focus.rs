//! Focus mode: which characters are dimmed so only the current paragraph
//! stays at full strength (spec/03-ui.md).
//!
//! A paragraph is a run of non-blank lines: the Markdown meaning, and the way
//! people type in this editor. The computation is pure so it can be tested
//! without a display; the editor supplies the buffer through [`Lines`].
//!
//! The editor never retags the whole document on a keystroke. It remembers
//! the range that was left at full strength and only touches that range plus
//! the paragraph the cursor is in now, which keeps focus mode cheap on long
//! documents.

/// Character offsets (the units GtkTextBuffer uses) for the logical lines of
/// a document. The editor implements this over a GtkTextBuffer; the tests
/// implement it over a `&str`.
pub trait Lines {
    fn count(&self) -> i32;
    /// True for an empty line or a line of whitespace only.
    fn is_blank(&self, line: i32) -> bool;
    /// Offset of the first character on `line`.
    fn start(&self, line: i32) -> i32;
    /// Offset of the line break after `line`, or of the last character at the
    /// end of the buffer.
    fn end(&self, line: i32) -> i32;
}

/// The paragraph containing `line`, as `(start, end)` character offsets.
/// A blank line is its own (empty) paragraph.
pub fn paragraph_range(lines: &impl Lines, line: i32) -> Option<(i32, i32)> {
    if line < 0 || line >= lines.count() {
        return None;
    }
    if lines.is_blank(line) {
        return Some((lines.start(line), lines.end(line)));
    }
    let mut first = line;
    while first > 0 && !lines.is_blank(first - 1) {
        first -= 1;
    }
    let mut last = line;
    while last + 1 < lines.count() && !lines.is_blank(last + 1) {
        last += 1;
    }
    let start = lines.start(first);
    let end = lines.end(last);
    (start < end).then_some((start, end))
}

/// The paragraph that should stay at full strength in focus mode.
///
/// A blank line has no text of its own, so the nearest paragraph above (or,
/// at the top of a document, below) stays bright while the writer sits on the
/// empty line between paragraphs.
pub fn focused_paragraph(lines: &impl Lines, line: i32) -> Option<(i32, i32)> {
    if line >= 0 && line < lines.count() && !lines.is_blank(line) {
        return paragraph_range(lines, line);
    }
    let mut above = line - 1;
    while above >= 0 && lines.is_blank(above) {
        above -= 1;
    }
    if above >= 0 {
        return paragraph_range(lines, above);
    }
    let mut below = line + 1;
    while below < lines.count() && lines.is_blank(below) {
        below += 1;
    }
    if below < lines.count() {
        return paragraph_range(lines, below);
    }
    paragraph_range(lines, line)
}

/// The work one cursor move or edit needs in the tag table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retag {
    /// Range to dim: the paragraph that was left, plus any text inserted
    /// before the paragraph the cursor is in now.
    pub dim: Option<(i32, i32)>,
    /// Range to clear: the paragraph under the cursor. Always done last, so
    /// a merge after Backspace or Delete ends up at full strength.
    pub clear: Option<(i32, i32)>,
}

/// The tag operations for moving from `previous` to `current`.
///
/// Only the paragraph that was left and the one entered are touched, never
/// the whole document. Dimming is skipped when the paragraph start did not
/// move: typing inside the paragraph, or Delete joining the next one, where
/// clearing the cursor's paragraph is enough.
pub fn retag(previous: Option<(i32, i32)>, current: (i32, i32)) -> Retag {
    let clear = (current.0 < current.1).then_some(current);
    let Some((old_start, old_end)) = previous else {
        return Retag { dim: None, clear };
    };
    if old_start == current.0 {
        return Retag { dim: None, clear };
    }
    // `old_end.max(current.0)` keeps the old paragraph dimmed when the cursor
    // moved up, and reaches the new paragraph start after Enter or a paste.
    let end = old_end.max(current.0);
    let dim = (old_start < end).then_some((old_start, end));
    Retag { dim, clear }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test lines over a text, with the same blank-line rule as the editor.
    struct TextLines {
        lines: Vec<(i32, i32, bool)>,
    }

    impl TextLines {
        fn new(text: &str) -> Self {
            let chars: Vec<char> = text.chars().collect();
            let mut lines = Vec::new();
            let mut start = 0usize;
            loop {
                let end = chars[start..]
                    .iter()
                    .position(|&ch| ch == '\n')
                    .map_or(chars.len(), |offset| start + offset);
                let blank = chars[start..end].iter().all(|ch| ch.is_whitespace());
                lines.push((start as i32, end as i32, blank));
                if end == chars.len() {
                    break;
                }
                start = end + 1;
            }
            Self { lines }
        }

        fn find(&self, needle: &str, text: &str) -> i32 {
            let chars: Vec<char> = text.chars().collect();
            let needle: Vec<char> = needle.chars().collect();
            (0..chars.len().saturating_sub(needle.len()) + 1)
                .find(|&index| chars[index..index + needle.len()] == needle[..])
                .map(|index| index as i32)
                .unwrap_or(0)
        }
    }

    impl Lines for TextLines {
        fn count(&self) -> i32 {
            self.lines.len() as i32
        }

        fn is_blank(&self, line: i32) -> bool {
            self.lines[line as usize].2
        }

        fn start(&self, line: i32) -> i32 {
            self.lines[line as usize].0
        }

        fn end(&self, line: i32) -> i32 {
            self.lines[line as usize].1
        }
    }

    const ESSAY: &str = "First paragraph\nstill first\n\nSecond **bold** paragraph\n\nThird";

    fn range_at(text: &str, needle: &str) -> (i32, i32) {
        let lines = TextLines::new(text);
        let offset = lines.find(needle, text);
        let line = (0..lines.count())
            .rev()
            .find(|&line| lines.start(line) <= offset)
            .unwrap();
        paragraph_range(&lines, line).unwrap()
    }

    /// A document where every character remembers whether it is dimmed. The
    /// tests apply [`retag`] exactly as the editor applies the text tags.
    struct FocusState {
        dimmed: Vec<bool>,
        current: Option<(i32, i32)>,
    }

    impl FocusState {
        fn new(len: usize, current: (i32, i32)) -> Self {
            let mut dimmed = vec![true; len];
            for flag in &mut dimmed[current.0 as usize..current.1 as usize] {
                *flag = false;
            }
            Self {
                dimmed,
                current: Some(current),
            }
        }

        fn apply(&mut self, current: (i32, i32)) {
            let ops = retag(self.current, current);
            if let Some((start, end)) = ops.dim {
                for flag in &mut self.dimmed[start as usize..end as usize] {
                    *flag = true;
                }
            }
            if let Some((start, end)) = ops.clear {
                for flag in &mut self.dimmed[start as usize..end as usize] {
                    *flag = false;
                }
            }
            self.current = Some(current);
        }

        fn assert_only_clear(&self, clear: (i32, i32)) {
            for (index, dim) in self.dimmed.iter().enumerate() {
                let inside = index >= clear.0 as usize && index < clear.1.max(clear.0) as usize;
                assert_eq!(*dim, !inside, "character {index}");
            }
        }
    }

    #[test]
    fn a_paragraph_is_a_run_of_non_blank_lines() {
        let lines = TextLines::new(ESSAY);
        assert_eq!(paragraph_range(&lines, 0), Some((0, 27)));
        assert_eq!(paragraph_range(&lines, 3), Some((29, 54)));
        assert_eq!(paragraph_range(&lines, 5), Some((56, 61)));
    }

    #[test]
    fn blank_lines_are_their_own_empty_paragraph() {
        let lines = TextLines::new(ESSAY);
        assert_eq!(paragraph_range(&lines, 2), Some((28, 28)));
        assert!(paragraph_range(&lines, 99).is_none());
        assert!(paragraph_range(&lines, -1).is_none());
    }

    #[test]
    fn a_caret_on_a_blank_line_keeps_the_paragraph_above_it_bright() {
        let lines = TextLines::new(ESSAY);
        // Line 4 is the blank line before "Third"; the paragraph above is
        // the one the writer just finished.
        assert_eq!(focused_paragraph(&lines, 4), Some((29, 54)));
    }

    #[test]
    fn a_caret_at_the_top_blank_line_keeps_the_first_paragraph_bright() {
        let lines = TextLines::new("\nFirst\n");
        assert_eq!(focused_paragraph(&lines, 0), Some((1, 6)));
    }

    #[test]
    fn an_empty_document_has_an_empty_focused_paragraph() {
        let lines = TextLines::new("");
        assert_eq!(focused_paragraph(&lines, 0), Some((0, 0)));
    }

    #[test]
    fn moving_down_dims_the_paragraph_that_was_left() {
        let first = range_at(ESSAY, "First");
        let second = range_at(ESSAY, "Second");
        assert_eq!(first, (0, 27));
        assert_eq!(second, (29, 54));
        let ops = retag(Some(first), second);
        assert_eq!(ops.dim, Some((0, 29)));
        assert_eq!(ops.clear, Some(second));
    }

    #[test]
    fn moving_up_dims_the_paragraph_that_was_left_and_clears_the_one_entered() {
        let first = range_at(ESSAY, "First");
        let second = range_at(ESSAY, "Second");
        let ops = retag(Some(second), first);
        assert_eq!(ops.dim, Some(second));
        assert_eq!(ops.clear, Some(first));
    }

    #[test]
    fn enter_at_the_end_dims_the_line_that_was_left() {
        let first = range_at(ESSAY, "still");
        // Pressing Enter at the end of the paragraph: the new, empty
        // paragraph starts after the inserted line break.
        let new = (first.1 + 1, first.1 + 1);
        let ops = retag(Some(first), new);
        assert_eq!(ops.dim, Some((0, 28)));
        assert_eq!(ops.clear, None);
    }

    #[test]
    fn enter_inside_a_paragraph_dims_only_the_part_before_the_break() {
        let first = range_at(ESSAY, "First");
        // The writer pressed Enter before "paragraph" (offset 6); the new
        // paragraph starts at 7 and must stay at full strength, so the dim
        // range is applied first and the new paragraph cleared after it.
        let new = (7, first.1);
        let ops = retag(Some(first), new);
        assert_eq!(ops.dim, Some((0, 27)));
        let mut state = FocusState::new(ESSAY.chars().count(), first);
        state.apply(new);
        state.assert_only_clear(new);
    }

    #[test]
    fn backspace_joining_two_paragraphs_leaves_one_clear_paragraph() {
        let second = range_at(ESSAY, "Second");
        let merged = (second.0 - 1, second.1);
        let ops = retag(Some(second), merged);
        // The old paragraph is dimmed and then the merged paragraph, which
        // contains it, is cleared in the same step.
        assert_eq!(ops.dim, Some(second));
        let mut state = FocusState::new(ESSAY.chars().count(), second);
        state.apply(merged);
        state.assert_only_clear(merged);
    }

    #[test]
    fn a_typing_keystroke_inside_the_paragraph_does_not_dim_anything() {
        let first = range_at(ESSAY, "First");
        let grown = (first.0, first.1 + 1);
        assert_eq!(retag(Some(first), grown).dim, None);
        assert_eq!(retag(None, grown).dim, None);
    }

    #[test]
    fn pasting_a_block_dims_every_paragraph_before_the_cursor() {
        let first = range_at(ESSAY, "First");
        let pasted_second = first.1 + 20;
        let current = (pasted_second, pasted_second + 10);
        assert_eq!(retag(Some(first), current).dim, Some((0, 47)));
    }

    #[test]
    fn the_invariant_holds_after_a_walk_through_the_document() {
        let first = range_at(ESSAY, "First");
        let second = range_at(ESSAY, "Second");
        let third = range_at(ESSAY, "Third");
        let mut state = FocusState::new(ESSAY.chars().count(), first);
        for next in [second, third, second, first] {
            state.apply(next);
            state.assert_only_clear(next);
        }
    }
}
