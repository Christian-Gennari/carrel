//! Small, dependency-free Markdown handling for the formats that do not go
//! through pandoc: plain-text conversion and finding citation link ids.

/// Prefix of a library citation link (`spec/02-sync-protocol.md`).
pub(crate) const CITATION_PREFIX: &str = "nostos:cite/";

/// Ids of every `nostos:cite/<id>` link in `text`, in order, without repeats.
pub(crate) fn citation_ids(text: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(position) = rest.find(CITATION_PREFIX) {
        let after = &rest[position + CITATION_PREFIX.len()..];
        if after.is_empty() {
            break;
        }
        let end = after
            .find(|c: char| {
                c.is_whitespace() || matches!(c, ')' | '>' | '"' | '\'' | ']' | '#' | '?' | '<')
            })
            .unwrap_or(after.len());
        if end > 0 {
            let id = &after[..end];
            if !ids.iter().any(|seen| seen == id) {
                ids.push(id.to_string());
            }
        }
        rest = &after[end.max(1)..];
    }
    ids
}

/// Turn Markdown into readable plain text for the `.txt` export.
///
/// This is intentionally a converter, not a renderer: headings lose their
/// `#`, emphasis and code markers disappear, links keep their label, block
/// quotes lose their `>` and fenced code keeps its contents.
pub(crate) fn markdown_to_plain(markdown: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut in_fence = false;

    for raw in markdown.lines() {
        let trimmed = raw.trim();
        if is_fence(trimmed) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            lines.push(raw.trim_end().to_string());
            continue;
        }
        if trimmed.is_empty() {
            lines.push(String::new());
            continue;
        }
        if is_underline(trimmed) {
            // A `===`/`---` line under text is a setext heading: drop the
            // underline. On its own it is a horizontal rule: keep a blank.
            if lines.last().is_some_and(|line| !line.is_empty()) {
                continue;
            }
            lines.push(String::new());
            continue;
        }
        lines.push(clean_line(trimmed));
    }

    let mut output = String::new();
    let mut pending_blank = false;
    for line in lines {
        if line.is_empty() {
            pending_blank = true;
            continue;
        }
        if !output.is_empty() {
            output.push('\n');
            if pending_blank {
                output.push('\n');
            }
        }
        output.push_str(&line);
        pending_blank = false;
    }
    if !output.is_empty() {
        output.push('\n');
    }
    output
}

fn is_fence(line: &str) -> bool {
    line.starts_with("```") || line.starts_with("~~~")
}

fn is_underline(line: &str) -> bool {
    (line.len() >= 2 && line.chars().all(|c| c == '='))
        || (line.len() >= 3 && line.chars().all(|c| c == '-'))
}

fn clean_line(line: &str) -> String {
    let mut text = line.to_string();

    // Block quotes, including nested ones.
    while let Some(rest) = text.strip_prefix('>') {
        text = rest
            .strip_prefix(' ')
            .unwrap_or(rest)
            .trim_start()
            .to_string();
    }

    // ATX headings: one to six `#` followed by a space (or the end).
    let hashes = text.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &text[hashes..];
        if rest.is_empty() || rest.starts_with(' ') {
            text = rest
                .trim_start()
                .trim_end_matches('#')
                .trim_end()
                .to_string();
        }
    }

    // Normalize list bullets and drop task-list checkboxes.
    if let Some(rest) = text.strip_prefix("* ").or_else(|| text.strip_prefix("+ ")) {
        text = format!("- {rest}");
    }
    for checkbox in ["- [ ] ", "- [x] ", "- [X] "] {
        if let Some(rest) = text.strip_prefix(checkbox) {
            text = format!("- {rest}");
            break;
        }
    }

    text = strip_links(&text);
    text = strip_inline(&text);
    text.trim_end().to_string()
}

/// Replace `[label](url)` and `![alt](url)` (and reference links) with their
/// label.
fn strip_links(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '!' && chars.get(index + 1) == Some(&'[') {
            if let Some(next) = link_at(&chars, index + 1, &mut out) {
                index = next;
                continue;
            }
        }
        if chars[index] == '[' {
            if let Some(next) = link_at(&chars, index, &mut out) {
                index = next;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

/// If a link begins at `start` (the `[`), append its label to `out` and return
/// the index just past the link.
fn link_at(chars: &[char], start: usize, out: &mut String) -> Option<usize> {
    let close = (start + 1..chars.len()).find(|&i| chars[i] == ']')?;
    let label: String = chars[start + 1..close].iter().collect();
    let after = close + 1;
    match chars.get(after) {
        Some('(') => {
            let end = (after + 1..chars.len()).find(|&i| chars[i] == ')')?;
            out.push_str(&strip_inline(&label));
            Some(end + 1)
        }
        Some('[') => {
            let end = (after + 1..chars.len()).find(|&i| chars[i] == ']')?;
            out.push_str(&strip_inline(&label));
            Some(end + 1)
        }
        _ => None,
    }
}

/// Drop emphasis, code and strikethrough markers. Single underscores are kept
/// so `snake_case` survives.
fn strip_inline(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '`' => index += 1,
            marker @ ('*' | '~' | '_') => {
                let run = chars[index..]
                    .iter()
                    .take_while(|character| **character == marker)
                    .count();
                if run >= 2 {
                    index += run;
                } else if marker == '~' {
                    // A lone tilde is usually "approximately", not emphasis.
                    out.push(marker);
                    index += 1;
                } else {
                    // Keep a lone marker used as prose ("5 * 3"), remove one
                    // that hugs a word on just one side (`*emphasis*`).
                    let before = index
                        .checked_sub(1)
                        .map(|position| chars[position])
                        .unwrap_or(' ');
                    let after = chars.get(index + 1).copied().unwrap_or(' ');
                    let before_is_boundary = !before.is_alphanumeric();
                    let after_is_boundary = !after.is_alphanumeric();
                    if before_is_boundary != after_is_boundary {
                        index += 1;
                    } else {
                        out.push(marker);
                        index += 1;
                    }
                }
            }
            '\\' if index + 1 < chars.len() => {
                let next = chars[index + 1];
                if "\\`*_{}[]()#+-.!>~".contains(next) {
                    out.push(next);
                    index += 2;
                } else {
                    out.push('\\');
                    index += 1;
                }
            }
            character => {
                out.push(character);
                index += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_citation_ids_once() {
        let text = "One [label](nostos:cite/quote-1) and [another](nostos:cite/quote-2).\n\
                    Again [same](nostos:cite/quote-1) and [other](nostos:cite/quote-3#note).";
        assert_eq!(citation_ids(text), ["quote-1", "quote-2", "quote-3"]);
        assert!(citation_ids("no links here").is_empty());
        assert!(citation_ids("empty target nostos:cite/ end").is_empty());
    }

    #[test]
    fn converts_markdown_to_readable_text() {
        let markdown = "# Title\n\nSome **bold** and *italic* text with a [link](https://example.com) and `code`.\n\n> A quote\n> second line\n\n- item one\n- item two\n\n```rust\nfn main() {}\n```\n";
        let plain = markdown_to_plain(markdown);
        assert_eq!(
            plain,
            "Title\n\nSome bold and italic text with a link and code.\n\nA quote\nsecond line\n\n- item one\n- item two\n\nfn main() {}\n"
        );
    }

    #[test]
    fn converts_headings_lists_and_unicode() {
        let markdown = "## Überlegungen ##\n\nSee [Marilynne Robinson, *Gilead*, p. 12](nostos:cite/quote-1).\n\n1. first\n2. second\n\n---\n\nDone.\n";
        let plain = markdown_to_plain(markdown);
        assert!(plain.starts_with("Überlegungen\n\n"));
        assert!(plain.contains("Marilynne Robinson, Gilead, p. 12."));
        assert!(plain.contains("1. first\n2. second"));
        assert!(!plain.contains("nostos:cite"));
        assert!(plain.ends_with("Done.\n"));
    }

    #[test]
    fn keeps_lone_markers_used_as_prose() {
        let markdown = "Multiply 5 * 3 and wait ~5 minutes for snake_case names.\n";
        assert_eq!(
            markdown_to_plain(markdown),
            "Multiply 5 * 3 and wait ~5 minutes for snake_case names.\n"
        );
    }

    #[test]
    fn keeps_fenced_code_verbatim() {
        let markdown = "Text.\n\n```\n# not a heading\n**not bold**\n```\n";
        assert_eq!(
            markdown_to_plain(markdown),
            "Text.\n\n# not a heading\n**not bold**\n"
        );
    }

    #[test]
    fn empty_input_gives_empty_output() {
        assert_eq!(markdown_to_plain(""), "");
        assert_eq!(markdown_to_plain("\n\n\n"), "");
    }
}
