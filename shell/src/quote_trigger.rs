//! The `@` trigger of the Quotes palette: pure text rules, no GTK.
//!
//! These are the rules of the Nostos Writing Studio's source palette
//! (`source-suggestion.extension.ts`), so both products open it the same way:
//!
//! * `@` opens the palette at the start of a word: after the start of a line,
//!   a space or no-break space, or one of `( [ " “ ‘ ' — –`. In the middle of
//!   a word (an email address) it does nothing.
//! * The text typed after `@` is the query. Spaces are allowed. Once it is
//!   longer than 80 characters, has more than 8 words, starts with a space or
//!   has a double space, it is plainly prose and the palette closes.
//! * Never inside a code span or a fenced code block.

/// What may stand directly before a trigger `@`, besides the start of a line.
pub const PREFIXES: [char; 10] = [' ', '\u{00A0}', '(', '[', '"', '“', '‘', '\'', '—', '–'];

/// The longest query the palette follows before it is plainly just prose.
pub const MAX_QUERY_LENGTH: usize = 80;
/// The most words the palette follows.
pub const MAX_QUERY_WORDS: usize = 8;

/// An open `@` query on one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    /// Character offset of the `@` from the start of the line.
    pub start: usize,
    /// The text after `@`, up to the caret.
    pub query: String,
}

/// Whether text typed after `@` still reads as a palette query.
pub fn is_palette_query(query: &str) -> bool {
    if query.chars().count() > MAX_QUERY_LENGTH {
        return false;
    }
    if query.chars().next().is_some_and(char::is_whitespace) {
        return false;
    }
    let mut previous_space = false;
    for character in query.chars() {
        let space = character.is_whitespace();
        if space && previous_space {
            return false;
        }
        previous_space = space;
    }
    query.split_whitespace().count() <= MAX_QUERY_WORDS
}

/// Whether a character may stand directly before the `@` of a trigger.
pub fn is_prefix(character: char) -> bool {
    PREFIXES.contains(&character)
}

/// The open query in `before`, the text of the caret's line up to the caret,
/// or `None`. `in_code` says the caret is inside a code span or block.
///
/// The trigger is the last `@` that starts a word; what follows it must still
/// be a palette query.
pub fn find(before: &str, in_code: bool) -> Option<Trigger> {
    if in_code {
        return None;
    }
    let chars: Vec<char> = before.chars().collect();
    let start = (0..chars.len())
        .rev()
        .find(|&index| chars[index] == '@' && (index == 0 || is_prefix(chars[index - 1])))?;
    let query: String = chars[start + 1..].iter().collect();
    is_palette_query(&query).then_some(Trigger { start, query })
}

/// Whether the caret, at character `offset` of `text`, is inside a fenced code
/// block or an inline code span.
///
/// Fences are lines that start (after up to three spaces) with three or more
/// backticks or tildes; a fence closes with the same character. An inline span
/// opens at a run of backticks and closes at the next run of the same length
/// on the same line. Indented code blocks are not recognised: Carrel's text is
/// prose, and a four-space indent is as likely a pasted list.
pub fn in_code(text: &str, offset: usize) -> bool {
    let mut fence: Option<(char, usize)> = None;
    let mut consumed = 0usize;
    for line in text.split('\n') {
        let length = line.chars().count();
        let caret_here = offset <= consumed + length;
        let marker = fence_marker(line);
        match (fence, marker) {
            (Some((open, open_len)), Some((found, found_len)))
                if open == found
                    && found_len >= open_len
                    && line.trim_start().chars().all(|c| c == found) =>
            {
                // The closing fence line itself is still code.
                if caret_here {
                    return true;
                }
                fence = None;
            }
            (Some(_), _) => {
                if caret_here {
                    return true;
                }
            }
            (None, Some(found)) => {
                if caret_here {
                    return true;
                }
                fence = Some(found);
            }
            (None, None) => {
                if caret_here {
                    let column = offset - consumed;
                    return inline_code_open(line, column);
                }
            }
        }
        consumed += length + 1;
    }
    false
}

/// A code fence at the start of `line`: its character and length.
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let indent = line.chars().take_while(|&c| c == ' ').count();
    if indent > 3 {
        return None;
    }
    let rest: Vec<char> = line.chars().skip(indent).collect();
    let first = *rest.first()?;
    if first != '`' && first != '~' {
        return None;
    }
    let run = rest.iter().take_while(|&&c| c == first).count();
    if run < 3 {
        return None;
    }
    // A backtick fence's info string may not contain a backtick; otherwise it
    // is an inline span (```code``` on one line).
    if first == '`' && rest[run..].contains(&'`') {
        return None;
    }
    Some((first, run))
}

/// Whether `column` of `line` sits inside an inline code span that has not
/// been closed before it.
fn inline_code_open(line: &str, column: usize) -> bool {
    let chars: Vec<char> = line.chars().take(column).collect();
    let mut open: Option<usize> = None;
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '`' {
            let run = chars[index..].iter().take_while(|&&c| c == '`').count();
            match open {
                None => open = Some(run),
                Some(length) if length == run => open = None,
                Some(_) => {}
            }
            index += run;
        } else {
            index += 1;
        }
    }
    open.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(before: &str) -> Option<String> {
        find(before, false).map(|trigger| trigger.query)
    }

    #[test]
    fn at_the_start_of_a_line_opens() {
        assert_eq!(query("@"), Some(String::new()));
        assert_eq!(query("@attention"), Some("attention".to_string()));
    }

    #[test]
    fn at_the_start_of_a_word_opens() {
        assert_eq!(query("As Mara wrote @quiet"), Some("quiet".to_string()));
        assert_eq!(
            query("As Mara wrote\u{00A0}@quiet"),
            Some("quiet".to_string())
        );
    }

    #[test]
    fn every_opening_prefix_opens() {
        for prefix in ["(", "[", "\"", "“", "‘", "'", "—", "–"] {
            let before = format!("word {prefix}@mara");
            assert_eq!(query(&before), Some("mara".to_string()), "prefix {prefix}");
        }
    }

    #[test]
    fn in_the_middle_of_a_word_does_nothing() {
        assert_eq!(query("write to me@example.com"), None);
        assert_eq!(query("me@"), None);
        assert_eq!(query("a.@b"), None);
        assert_eq!(query("a-@b"), None);
    }

    #[test]
    fn the_last_word_start_wins() {
        assert_eq!(query("see @one and @two"), Some("two".to_string()));
        assert_eq!(query("me@example.com @rea"), Some("rea".to_string()));
    }

    #[test]
    fn spaces_belong_to_the_query() {
        assert_eq!(query("@a small room"), Some("a small room".to_string()));
        assert_eq!(query("@mara "), Some("mara ".to_string()));
    }

    #[test]
    fn a_query_over_80_characters_is_prose() {
        let long = "a".repeat(80);
        assert_eq!(query(&format!("@{long}")), Some(long.clone()));
        assert_eq!(query(&format!("@{long}a")), None);
    }

    #[test]
    fn a_query_of_more_than_8_words_is_prose() {
        assert!(query("@one two three four five six seven eight").is_some());
        assert_eq!(query("@one two three four five six seven eight nine"), None);
    }

    #[test]
    fn a_query_that_starts_with_a_space_or_has_a_double_space_is_prose() {
        assert_eq!(query("@ mara"), None);
        assert_eq!(query("@mara  ellison"), None);
        assert_eq!(query("@mara \u{00A0}ellison"), None);
        assert_eq!(query("@mara ellison"), Some("mara ellison".to_string()));
    }

    #[test]
    fn the_trigger_reports_where_the_at_sign_is() {
        let trigger = find("Ünï @qu", false).unwrap();
        assert_eq!(trigger.start, 4);
        assert_eq!(trigger.query, "qu");
    }

    #[test]
    fn code_never_opens() {
        assert_eq!(find("@quiet", true), None);
    }

    #[test]
    fn an_inline_code_span_is_code_until_it_closes() {
        let text = "run `cargo @";
        assert!(in_code(text, text.chars().count()));
        let text = "run `cargo` then @";
        assert!(!in_code(text, text.chars().count()));
        let text = "run ``a ` b @";
        assert!(in_code(text, text.chars().count()));
        let text = "run ``a ` b`` then @";
        assert!(!in_code(text, text.chars().count()));
        let text = "an apostrophe isn't code @";
        assert!(!in_code(text, text.chars().count()));
    }

    #[test]
    fn a_fenced_block_is_code_between_its_fences() {
        let text = "before\n```rust\nlet a = b @\n```\nafter @";
        let inside = text.find("b @").unwrap() + 3;
        assert!(in_code(text, inside));
        assert!(!in_code(text, text.chars().count()));
        // On the opening and closing fence lines themselves.
        assert!(in_code(text, "before\n```rus".chars().count()));
        // An unclosed fence runs to the end of the document.
        let open = "```\ncode @";
        assert!(in_code(open, open.chars().count()));
    }

    #[test]
    fn tildes_and_longer_fences_work_and_a_shorter_fence_does_not_close() {
        let text = "~~~\ninside @\n~~~\nout @";
        assert!(in_code(text, "~~~\ninside @".chars().count()));
        assert!(!in_code(text, text.chars().count()));
        let text = "````\n```\nstill @\n````\nout @";
        assert!(in_code(text, "````\n```\nstill @".chars().count()));
        assert!(!in_code(text, text.chars().count()));
    }

    #[test]
    fn three_backticks_with_text_after_them_on_one_line_are_a_span() {
        let text = "```code``` then @";
        assert!(!in_code(text, text.chars().count()));
    }

    #[test]
    fn plain_prose_is_not_code() {
        let text = "One line.\n\nAnother with @";
        assert!(!in_code(text, text.chars().count()));
        assert!(!in_code("", 0));
    }
}
