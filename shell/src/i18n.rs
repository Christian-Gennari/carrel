//! The words on the screens, in every language Carrel offers.
//!
//! One table per language (`shell/i18n/<code>.lang`, `key = value` lines) is
//! compiled in, so the device needs no `.mo` files at run time. English is the
//! source and the fallback for a missing key. Key caps (Enter, Esc, Ctrl+O),
//! file names and log lines are never translated. No GTK here.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Language name (as in [`crate::locale::LANGUAGES`]) and its table.
const SOURCES: [(&str, &str); 9] = [
    ("English", include_str!("../i18n/en.lang")),
    ("Deutsch", include_str!("../i18n/de.lang")),
    ("Français", include_str!("../i18n/fr.lang")),
    ("Español", include_str!("../i18n/es.lang")),
    ("Italiano", include_str!("../i18n/it.lang")),
    ("Português", include_str!("../i18n/pt.lang")),
    ("Nederlands", include_str!("../i18n/nl.lang")),
    ("Polski", include_str!("../i18n/pl.lang")),
    ("Türkçe", include_str!("../i18n/tr.lang")),
];

type Table = HashMap<&'static str, &'static str>;

fn tables() -> &'static [Table] {
    static TABLES: OnceLock<Vec<Table>> = OnceLock::new();
    TABLES.get_or_init(|| SOURCES.iter().map(|(_, source)| parse(source)).collect())
}

fn parse(source: &'static str) -> Table {
    source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| {
            let value = value.trim();
            let value: &'static str = if value.contains("\\n") {
                Box::leak(value.replace("\\n", "\n").into_boxed_str())
            } else {
                value
            };
            (key.trim(), value)
        })
        .collect()
}

// The shell has one language at a time. Tests run side by side on their own
// threads, so there each thread keeps its own and one test cannot change the
// words under another.
#[cfg(not(test))]
mod current {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CURRENT: AtomicUsize = AtomicUsize::new(0);

    pub fn get() -> usize {
        CURRENT.load(Ordering::Relaxed)
    }

    pub fn set(index: usize) {
        CURRENT.store(index, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod current {
    use std::cell::Cell;

    thread_local!(static CURRENT: Cell<usize> = const { Cell::new(0) });

    pub fn get() -> usize {
        CURRENT.with(Cell::get)
    }

    pub fn set(index: usize) {
        CURRENT.with(|current| current.set(index));
    }
}

/// Use `name` (a `settings.language` value) from now on. Anything unknown is
/// English.
pub fn set_language(name: &str) {
    let index = SOURCES
        .iter()
        .position(|(language, _)| *language == name)
        .unwrap_or(0);
    current::set(index);
}

fn lookup(key: &str) -> Option<&'static str> {
    let tables = tables();
    tables[current::get()]
        .get(key)
        .or_else(|| tables[0].get(key))
        .copied()
}

/// The text for `key` in the current language.
pub fn tr(key: &'static str) -> &'static str {
    let text = lookup(key);
    debug_assert!(text.is_some(), "no English text for {key}");
    text.unwrap_or(key)
}

/// [`tr`] with its `{name}` placeholders filled in.
pub fn trf(key: &'static str, values: &[(&str, &str)]) -> String {
    fill(tr(key), values)
}

/// Fill the placeholders in one pass, so a value that contains braces (a file
/// name, an error) is never filled a second time.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let value = rest.find('}').and_then(|end| {
            values
                .iter()
                .find(|(name, _)| *name == &rest[1..end])
                .map(|(_, value)| (*value, end))
        });
        match value {
            Some((value, end)) => {
                out.push_str(value);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('{');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A counted phrase: `<key>.one`, `.few` (Polish) or `.other`, with `{count}`
/// filled by `shown` (the count as the screen writes it).
pub fn trn(key: &str, count: u64, shown: &str, values: &[(&str, &str)]) -> String {
    let form = plural(SOURCES[current::get()].0, count);
    let text = lookup(&format!("{key}.{form}"))
        .or_else(|| lookup(&format!("{key}.other")))
        .unwrap_or_default();
    debug_assert!(!text.is_empty(), "no English text for {key}");
    let mut all = vec![("count", shown)];
    all.extend_from_slice(values);
    fill(text, &all)
}

fn plural(language: &str, count: u64) -> &'static str {
    match language {
        "Polski" if count == 1 => "one",
        "Polski" if (2..=4).contains(&(count % 10)) && !(12..=14).contains(&(count % 100)) => "few",
        "Polski" => "other",
        "Français" if count < 2 => "one",
        _ if count == 1 => "one",
        _ => "other",
    }
}

/// A sentence the daemon sends in English, in the writer's language when it is
/// one the shell knows. Anything else (an error chain) is shown as it came.
pub fn daemon_message(message: &str) -> String {
    const FIXED: [&str; 8] = [
        "note.shutdown_sync",
        "sync.syncing",
        "daemon.not_synced",
        "daemon.up_to_date",
        "daemon.offline_pair",
        "daemon.no_wifi",
        "wifi.portal",
        "quotes.unavailable",
    ];
    let english = &tables()[0];
    if let Some(key) = FIXED.iter().find(|key| english.get(*key) == Some(&message)) {
        return tr(key).to_owned();
    }
    if let Some(name) = message
        .strip_prefix("Paired with ")
        .and_then(|rest| rest.strip_suffix('.'))
    {
        return trf("daemon.paired_with", &[("name", name)]);
    }
    message.to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::locale::LANGUAGES;

    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}'))
            .map(|(name, _)| name)
            .collect()
    }

    #[test]
    fn every_offered_language_has_a_table() {
        let names: Vec<&str> = SOURCES.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, LANGUAGES);
    }

    #[test]
    fn no_table_repeats_a_key() {
        for (name, source) in SOURCES {
            let lines = source
                .lines()
                .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
                .count();
            assert_eq!(parse(source).len(), lines, "{name}");
        }
    }

    #[test]
    fn every_table_has_every_english_key_and_placeholder() {
        let english = &tables()[0];
        let mut problems = Vec::new();
        for ((name, _), table) in SOURCES.iter().zip(tables()) {
            let mut wanted: BTreeSet<String> = english.keys().map(|key| key.to_string()).collect();
            // Polish counts 2–4 differently: every plural needs a `.few`.
            if *name == "Polski" {
                wanted.extend(
                    english
                        .keys()
                        .filter_map(|key| key.strip_suffix(".one"))
                        .map(|stem| format!("{stem}.few")),
                );
            }
            let have: BTreeSet<String> = table.keys().map(|key| key.to_string()).collect();
            for key in wanted.difference(&have) {
                problems.push(format!("{name}: missing {key}"));
            }
            for key in have.difference(&wanted) {
                problems.push(format!("{name}: extra {key}"));
            }
            for (key, value) in table {
                if value.is_empty() {
                    problems.push(format!("{name}: empty {key}"));
                }
                let source = english
                    .get(key)
                    .copied()
                    .or_else(|| {
                        let other = format!("{}.other", key.strip_suffix(".few")?);
                        english.get(other.as_str()).copied()
                    })
                    .unwrap_or_default();
                if placeholders(value) != placeholders(source) {
                    problems.push(format!("{name}: placeholders of {key}"));
                }
            }
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }

    /// Every key the code asks for exists, so a typo cannot reach the screen.
    #[test]
    fn every_key_in_the_source_exists() {
        let english = &tables()[0];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![src];
        let mut missing = Vec::new();
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(std::fs::read_dir(path).unwrap().map(|e| e.unwrap().path()));
                continue;
            }
            if path.file_name().is_some_and(|name| name == "i18n.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (call, suffix) in [("tr(", ""), ("trf(", ""), ("trn(", ".other")] {
                for (at, _) in text.match_indices(call) {
                    // Not the tail of another name (`from_str(`).
                    if text[..at]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_')
                    {
                        continue;
                    }
                    let rest = text[at + call.len()..].trim_start();
                    if let Some((key, _)) = rest.strip_prefix('"').and_then(|r| r.split_once('"')) {
                        let key = format!("{key}{suffix}");
                        if !english.contains_key(key.as_str()) {
                            missing.push(key);
                        }
                    }
                }
            }
        }
        assert!(missing.is_empty(), "{missing:#?}");
    }

    #[test]
    fn the_daemons_shutdown_sync_line_is_translated() {
        let english = daemon_message(crate::daemon::SHUTDOWN_SYNC_MESSAGE);
        assert_eq!(english, "Syncing before shutting down…");
        assert_eq!(
            crate::daemon::SHUTDOWN_SYNC_MESSAGE,
            carreld::SHUTDOWN_SYNC_MESSAGE
        );
        set_language("Deutsch");
        assert_eq!(
            daemon_message(crate::daemon::SHUTDOWN_SYNC_MESSAGE),
            "Synchronisiere vor dem Herunterfahren…"
        );
        set_language("English");
    }

    #[test]
    fn switching_changes_the_words_and_unknown_is_english() {
        assert_eq!(tr("settings.title"), "Settings");
        set_language("Deutsch");
        assert_eq!(tr("settings.title"), "Einstellungen");
        assert_eq!(
            daemon_message("Paired with Studio."),
            "Mit Studio gekoppelt."
        );
        set_language("Klingon");
        assert_eq!(tr("settings.title"), "Settings");
    }

    #[test]
    fn plurals_follow_the_language_and_values_are_filled_once() {
        let words = |n: u64| trn("editor.words", n, &n.to_string(), &[]);
        assert_eq!(words(1), "1 word");
        assert_eq!(words(2), "2 words");
        set_language("Polski");
        assert_eq!(words(1), "1 słowo");
        assert_eq!(words(3), "3 słowa");
        assert_eq!(words(5), "5 słów");
        assert_eq!(words(12), "12 słów");
        assert_eq!(words(22), "22 słowa");
        set_language("English");
        assert_eq!(
            trf("editor.cannot_open", &[("path", "{error}"), ("error", "x")]),
            "Cannot open {error}: x"
        );
    }
}
