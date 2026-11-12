//! Editing a `String` by character offset.
//!
//! GTK text buffers count in characters, not bytes, so the journal stores
//! character offsets too. This module is the only place that translates
//! between the two, and it keeps a cached character length so appending at
//! the end of a document stays cheap.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditOutOfRange {
    pub offset: usize,
    pub delete_len: usize,
    pub text_len: usize,
}

impl fmt::Display for EditOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "edit at character {} deleting {} is outside a {}-character document",
            self.offset, self.delete_len, self.text_len
        )
    }
}

impl std::error::Error for EditOutOfRange {}

/// Byte offset of a character offset, or `None` past the end.
pub fn char_to_byte(text: &str, char_offset: usize) -> Option<usize> {
    if char_offset == 0 {
        return Some(0);
    }
    text.char_indices()
        .nth(char_offset)
        .map(|(byte, _)| byte)
        .or_else(|| {
            // `nth(char_offset)` fails exactly when the offset is the length
            // of the string. Count once to distinguish that from too far.
            (text.chars().count() == char_offset).then_some(text.len())
        })
}

/// Apply one journal operation in place.
pub fn apply_edit(
    text: &mut String,
    char_len: &mut usize,
    offset: usize,
    delete_len: usize,
    insert: &str,
) -> Result<(), EditOutOfRange> {
    let end = offset.checked_add(delete_len).ok_or(EditOutOfRange {
        offset,
        delete_len,
        text_len: *char_len,
    })?;
    if end > *char_len {
        return Err(EditOutOfRange {
            offset,
            delete_len,
            text_len: *char_len,
        });
    }

    // The common case: typing at the end.
    if delete_len == 0 && offset == *char_len {
        text.push_str(insert);
        *char_len += insert.chars().count();
        return Ok(());
    }

    let start_byte = if offset == *char_len {
        text.len()
    } else {
        char_to_byte(text, offset).ok_or(EditOutOfRange {
            offset,
            delete_len,
            text_len: *char_len,
        })?
    };
    let end_byte = if end == *char_len {
        text.len()
    } else {
        char_to_byte(text, end).ok_or(EditOutOfRange {
            offset,
            delete_len,
            text_len: *char_len,
        })?
    };

    text.replace_range(start_byte..end_byte, insert);
    *char_len = *char_len - delete_len + insert.chars().count();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_to_byte_handles_multibyte() {
        let text = "aé—z";
        assert_eq!(char_to_byte(text, 0), Some(0));
        assert_eq!(char_to_byte(text, 1), Some(1));
        assert_eq!(char_to_byte(text, 2), Some(3));
        assert_eq!(char_to_byte(text, 4), Some(text.len()));
        assert_eq!(char_to_byte(text, 5), None);
    }

    #[test]
    fn appending_is_cheap_and_correct() {
        let mut text = String::from("hello");
        let mut len = 5;
        apply_edit(&mut text, &mut len, 5, 0, " world").unwrap();
        assert_eq!(text, "hello world");
        assert_eq!(len, 11);
    }

    #[test]
    fn replace_middle() {
        let mut text = String::from("one TWO three");
        let mut len = 13;
        apply_edit(&mut text, &mut len, 4, 3, "two").unwrap();
        assert_eq!(text, "one two three");
        assert_eq!(len, 13);
    }

    #[test]
    fn unicode_offsets() {
        let mut text = String::from("héllo");
        let mut len = 5;
        apply_edit(&mut text, &mut len, 1, 1, "a").unwrap();
        assert_eq!(text, "hallo");
        apply_edit(&mut text, &mut len, 5, 0, " — ok").unwrap();
        assert_eq!(text, "hallo — ok");
        assert_eq!(len, 10);
    }

    #[test]
    fn rejects_out_of_range() {
        let mut text = String::from("abc");
        let mut len = 3;
        assert!(apply_edit(&mut text, &mut len, 2, 5, "").is_err());
        assert_eq!(text, "abc");
        assert_eq!(len, 3);
    }
}
