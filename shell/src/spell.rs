//! What Spellcheck looks at: the dictionary for the writer's language, and the
//! words of a Markdown line that are prose. Pure, so it is tested without GTK.

/// The dictionary for a Settings > Language value (`locale::LANGUAGES`). Each
/// one is shipped by a `hunspell-*` package in `image/mkosi.conf`.
pub fn dictionary_code(language: &str) -> Option<&'static str> {
    Some(match language {
        "English" => "en_US",
        "Deutsch" => "de_DE",
        "Français" => "fr_FR",
        "Español" => "es_ES",
        "Italiano" => "it_IT",
        "Português" => "pt_PT",
        "Nederlands" => "nl_NL",
        "Polski" => "pl_PL",
        "Türkçe" => "tr_TR",
        _ => return None,
    })
}

/// A word of one line, with its character range (what GTK buffers count).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub start: usize,
    pub end: usize,
    /// The word as the dictionary knows it: `’` is written `'`.
    pub text: String,
}

/// The words to check in `line`, a line outside any fenced code block. Code
/// spans, link targets, URLs, addresses, file names, tags and anything with a
/// digit or an underscore are not prose.
pub fn words(line: &str) -> Vec<Word> {
    let mut chars: Vec<char> = line.chars().collect();
    blank_code_spans(&mut chars);
    blank_link_targets(&mut chars);
    blank_tags(&mut chars);
    blank_addresses(&mut chars);

    let mut words = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if !is_token(chars[index]) {
            index += 1;
            continue;
        }
        let mut start = index;
        while index < chars.len() && is_token(chars[index]) {
            index += 1;
        }
        let mut end = index;
        // Apostrophes at the edges are quotation marks, underscores emphasis.
        while start < end && !chars[start].is_alphanumeric() {
            start += 1;
        }
        while end > start && !chars[end - 1].is_alphanumeric() && !is_mark(chars[end - 1]) {
            end -= 1;
        }
        let token = &chars[start..end];
        if token.len() < 2 || token.iter().any(|&c| c.is_numeric() || c == '_') {
            continue;
        }
        words.push(Word {
            start,
            end,
            text: token
                .iter()
                .map(|&c| if c == '’' { '\'' } else { c })
                .collect(),
        });
    }
    words
}

/// Combining accents, for text that arrives decomposed.
fn is_mark(c: char) -> bool {
    ('\u{0300}'..='\u{036F}').contains(&c)
}

fn is_token(c: char) -> bool {
    c.is_alphanumeric() || is_mark(c) || matches!(c, '\'' | '’' | '_')
}

fn blank(chars: &mut [char]) {
    chars.fill(' ');
}

/// A span opens at a run of backticks and closes at the next run of the same
/// length on the same line (as `quote_trigger::in_code` reads it).
fn blank_code_spans(chars: &mut [char]) {
    let run_at = |chars: &[char], at: usize| chars[at..].iter().take_while(|&&c| c == '`').count();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '`' {
            index += 1;
            continue;
        }
        let length = run_at(chars, index);
        let mut close = index + length;
        let mut closed = None;
        while close < chars.len() {
            if chars[close] != '`' {
                close += 1;
                continue;
            }
            let found = run_at(chars, close);
            if found == length {
                closed = Some(close + found);
                break;
            }
            close += found;
        }
        match closed {
            Some(end) => {
                blank(&mut chars[index..end]);
                index = end;
            }
            None => index += length,
        }
    }
}

/// `](target)` and the `[^label]` of a footnote.
fn blank_link_targets(chars: &mut [char]) {
    let mut index = 0;
    while index + 1 < chars.len() {
        let close = match (chars[index], chars[index + 1]) {
            (']', '(') => ')',
            ('[', '^') => ']',
            _ => {
                index += 1;
                continue;
            }
        };
        let mut depth = 0usize;
        let mut end = index + 2;
        while end < chars.len() && !(chars[end] == close && depth == 0) {
            match chars[end] {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            end += 1;
        }
        blank(&mut chars[index + 1..end]);
        index = end;
    }
}

/// HTML tags, comments on one line and `<autolinks>`.
fn blank_tags(chars: &mut [char]) {
    let mut index = 0;
    while index + 1 < chars.len() {
        let next = chars[index + 1];
        if chars[index] == '<' && (next.is_ascii_alphabetic() || next == '/' || next == '!') {
            if let Some(length) = chars[index..].iter().position(|&c| c == '>') {
                blank(&mut chars[index..=index + length]);
                index += length;
            }
        }
        index += 1;
    }
}

/// Whole space-separated chunks that are a URL, an address, a file name or a
/// domain.
fn blank_addresses(chars: &mut [char]) {
    for chunk in chars.split_mut(|c| c.is_whitespace()) {
        let text: String = chunk.iter().collect();
        let dotted = chunk.windows(4).any(|w| {
            // `name.md`, `example.com`; not the abbreviation `e.g.`.
            w[0].is_alphanumeric()
                && w[1].is_alphanumeric()
                && w[2] == '.'
                && w[3].is_ascii_lowercase()
        });
        if dotted
            || text.contains("://")
            || text.contains("www.")
            || text.contains("nostos:")
            || text.contains("mailto:")
            || text.contains('@')
        {
            blank(chunk);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::LANGUAGES;

    fn texts(line: &str) -> Vec<String> {
        words(line).into_iter().map(|word| word.text).collect()
    }

    #[test]
    fn every_offered_language_has_its_dictionary() {
        let codes: Vec<_> = LANGUAGES
            .iter()
            .map(|language| dictionary_code(language))
            .collect();
        assert_eq!(
            codes,
            ["en_US", "de_DE", "fr_FR", "es_ES", "it_IT", "pt_PT", "nl_NL", "pl_PL", "tr_TR"]
                .map(Some)
        );
        assert_eq!(dictionary_code("Klingon"), None);
    }

    #[test]
    fn prose_words_keep_their_character_ranges() {
        let found = words("# Żółć isn’t 'quoted', _soft_ e.g. here.");
        assert_eq!(
            found.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["Żółć", "isn't", "quoted", "soft", "here"]
        );
        assert_eq!((found[0].start, found[0].end), (2, 6));
        assert_eq!((found[1].start, found[1].end), (7, 12));
        assert_eq!(texts("well-known end.The"), ["well", "known", "end", "The"]);
    }

    #[test]
    fn code_links_and_addresses_are_not_checked() {
        assert_eq!(
            texts("run `qwzx` or ``a ` qwzx`` now"),
            ["run", "or", "now"]
        );
        assert_eq!(texts("a lone ` stays prose"), ["lone", "stays", "prose"]);
        assert_eq!(
            texts("see [the page](qwzx/pa_(ge).md) and[^note1] too"),
            ["see", "the", "page", "and", "too"]
        );
        assert_eq!(
            texts("at https://qwzx.example/a www.qwzx.se me@qwzx.com notes.md"),
            ["at"]
        );
        assert_eq!(
            texts("<span class=\"qwzx\">bold</span> <!-- no -->"),
            ["bold"]
        );
        assert_eq!(
            texts("nostos:cite/abc 3rd h2o snake_case"),
            Vec::<String>::new()
        );
    }
}
