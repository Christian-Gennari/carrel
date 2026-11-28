//! The dictionary behind Settings > Spellcheck: libspelling (enchant and the
//! image's hunspell dictionaries, all on the device).
//!
//! Only libspelling's checker is used. Its `TextBufferAdapter` also computes
//! corrections on the main thread each time the caret rests on a word it does
//! not know, which takes 30–270 ms per pause and Carrel shows no corrections.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::spell;

/// Answers are remembered, so a repeated word costs a lookup. Cleared when it
/// grows past this, which no ordinary manuscript does.
const REMEMBERED_WORDS: usize = 50_000;

struct Dictionary {
    checker: spelling::Checker,
    known: RefCell<HashMap<String, bool>>,
}

#[derive(Default)]
pub struct Speller {
    wanted: Cell<Option<&'static str>>,
    dictionary: RefCell<Option<Dictionary>>,
}

impl Speller {
    /// Ask for the dictionary `code`, or none. True when that is a change: the
    /// old dictionary is gone and [`Self::load`] brings the new one.
    pub fn want(&self, code: Option<&'static str>) -> bool {
        if self.wanted.replace(code) == code {
            return false;
        }
        self.dictionary.replace(None);
        true
    }

    /// Load `code` if it is still the one wanted. A dictionary this system
    /// does not have is silently no spellcheck.
    pub fn load(&self, code: Option<&'static str>) -> bool {
        let Some(code) = code.filter(|_| self.wanted.get() == code) else {
            return false;
        };
        let provider = spelling::Provider::default();
        if !provider.supports_language(code) {
            return false;
        }
        let checker = spelling::Checker::new(Some(&provider), Some(code));
        self.dictionary.replace(Some(Dictionary {
            checker,
            known: RefCell::default(),
        }));
        true
    }

    pub fn ready(&self) -> bool {
        self.dictionary.borrow().is_some()
    }

    /// The dictionary code in use.
    #[cfg(test)]
    pub(crate) fn language(&self) -> Option<&'static str> {
        self.wanted.get().filter(|_| self.ready())
    }

    /// Character ranges of the words in `line` the dictionary does not know.
    pub fn misspelled(&self, line: &str) -> Vec<(usize, usize)> {
        let dictionary = self.dictionary.borrow();
        let Some(dictionary) = dictionary.as_ref() else {
            return Vec::new();
        };
        let mut known = dictionary.known.borrow_mut();
        if known.len() > REMEMBERED_WORDS {
            known.clear();
        }
        spell::words(line)
            .into_iter()
            .filter(|word| {
                !*known
                    .entry(word.text.clone())
                    .or_insert_with(|| dictionary.checker.check_word(&word.text))
            })
            .map(|word| (word.start, word.end))
            .collect()
    }
}
