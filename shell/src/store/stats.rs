//! Daily word count for the bottom line.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyStats {
    /// Local date, `YYYY-MM-DD`.
    pub date: String,
    pub words: u64,
}

/// Count words in prose. Standalone Markdown markers do not count.
pub fn count_words(text: &str) -> u64 {
    text.split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count() as u64
}

/// Add a signed delta without going below zero.
pub fn add_delta(words: u64, delta: i64) -> u64 {
    if delta >= 0 {
        words.saturating_add(delta as u64)
    } else {
        words.saturating_sub(delta.unsigned_abs())
    }
}

/// Group digits with a comma: `1234` → `1,234`.
pub fn format_number(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_prose_not_markers() {
        assert_eq!(count_words(""), 0);
        assert_eq!(count_words("# Chapter one\n\nA **quiet** morning."), 5);
        assert_eq!(count_words("---\n> quote"), 1);
        assert_eq!(count_words("héllo — 世界"), 2);
    }

    #[test]
    fn delta_does_not_underflow() {
        assert_eq!(add_delta(10, 5), 15);
        assert_eq!(add_delta(3, -5), 0);
    }

    #[test]
    fn formats_thousands() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1234567), "1,234,567");
    }
}
