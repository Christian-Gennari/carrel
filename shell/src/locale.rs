//! The short lists the Settings and First run screens cycle through.
//!
//! Carrel supports a deliberately small set of languages, layouts and time
//! zones: enough for the people the device is for, few enough that everything
//! fits on one calm screen.

use crate::i18n::tr;

pub const LANGUAGES: &[&str] = &[
    "English",
    "Deutsch",
    "Français",
    "Español",
    "Italiano",
    "Português",
    "Nederlands",
    "Polski",
    "Türkçe",
];

/// Layout code and its English name; the screens use [`keyboard_label`].
pub const KEYBOARDS: &[(&str, &str)] = &[
    ("us", "US"),
    ("gb", "UK"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("br", "Brazilian"),
    ("se", "Swedish"),
    ("no", "Norwegian"),
    ("dk", "Danish"),
    ("pl", "Polish"),
    ("tr", "Turkish"),
];

pub const TIME_ZONES: &[&str] = &[
    "Local time",
    "UTC",
    "Europe/London",
    "Europe/Berlin",
    "Europe/Paris",
    "Europe/Madrid",
    "Europe/Rome",
    "Europe/Lisbon",
    "Europe/Warsaw",
    "Europe/Stockholm",
    "America/New_York",
    "America/Chicago",
    "America/Denver",
    "America/Los_Angeles",
    "America/Sao_Paulo",
    "Asia/Tokyo",
    "Asia/Shanghai",
    "Asia/Singapore",
    "Asia/Kolkata",
    "Australia/Sydney",
];

/// Move through a list, wrapping at both ends. An unknown current value
/// starts at the first entry, so a hand-edited settings file cannot break the
/// screen.
pub fn cycle(list: &[&str], current: &str, delta: i32) -> String {
    if list.is_empty() {
        return current.to_owned();
    }
    let index = list.iter().position(|item| *item == current);
    let next = match index {
        Some(index) => {
            let len = list.len() as i32;
            ((index as i32 + delta) % len + len) % len
        }
        None => 0,
    };
    list[next as usize].to_owned()
}

/// The layout's name in the current language; an unknown code shows itself.
pub fn keyboard_label(code: &str) -> &str {
    const KEYS: [&str; 13] = [
        "keyboard.us",
        "keyboard.gb",
        "keyboard.de",
        "keyboard.fr",
        "keyboard.es",
        "keyboard.it",
        "keyboard.pt",
        "keyboard.br",
        "keyboard.se",
        "keyboard.no",
        "keyboard.dk",
        "keyboard.pl",
        "keyboard.tr",
    ];
    KEYS.into_iter()
        .find(|key| key.strip_prefix("keyboard.") == Some(code))
        .map(tr)
        .unwrap_or(code)
}

/// A time zone as the screens write it. Zone names stay as they are; only
/// "Local time" is a word.
pub fn time_zone_label(zone: &str) -> &str {
    if zone == "Local time" {
        tr("timezone.local")
    } else {
        zone
    }
}

pub fn keyboard_cycle(current: &str, delta: i32) -> String {
    if KEYBOARDS.is_empty() {
        return current.to_owned();
    }
    let index = KEYBOARDS
        .iter()
        .position(|(code, _)| *code == current)
        .unwrap_or(0);
    let len = KEYBOARDS.len() as i32;
    let next = ((index as i32 + delta) % len + len) % len;
    KEYBOARDS[next as usize].0.to_owned()
}

/// Parse the active XKB layout code from an `XKB_DEFAULT_LAYOUT` value
/// (`"se"`, `"de,us"` -> `"de"`, `"de(nodeadkeys)"` -> `"de"`).
pub fn parse_xkb_layout(raw: &str) -> Option<String> {
    let first = raw.split(',').next()?.trim();
    let layout = first.split('(').next()?.trim().to_ascii_lowercase();
    if layout.is_empty() {
        None
    } else {
        Some(layout)
    }
}

/// The keyboard layout the running compositor started with, if set via
/// `XKB_DEFAULT_LAYOUT` by `/usr/lib/carrel/session-env`.
pub fn session_keyboard_from_env() -> Option<String> {
    let raw = std::env::var("XKB_DEFAULT_LAYOUT").ok()?;
    parse_xkb_layout(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_wraps_both_ways() {
        assert_eq!(cycle(&["A", "B", "C"], "A", 1), "B");
        assert_eq!(cycle(&["A", "B", "C"], "A", -1), "C");
        assert_eq!(cycle(&["A", "B", "C"], "C", 1), "A");
        assert_eq!(cycle(&["A", "B", "C"], "unknown", 1), "A");
        assert_eq!(cycle(&[], "x", 1), "x");
    }

    #[test]
    fn the_first_run_defaults_are_present() {
        assert_eq!(LANGUAGES[0], "English");
        assert_eq!(keyboard_label("us"), "US");
        assert!(TIME_ZONES.contains(&"Europe/Berlin"));
        assert!(LANGUAGES.contains(&"Türkçe"));
    }

    #[test]
    fn keyboards_cycle_by_code_and_show_a_label() {
        assert_eq!(keyboard_cycle("us", 1), "gb");
        assert_eq!(keyboard_cycle("us", -1), "tr");
        assert_eq!(keyboard_label("de"), "German");
        assert_eq!(keyboard_label("xx"), "xx");
    }

    #[test]
    fn parses_xkb_default_layout_values() {
        assert_eq!(parse_xkb_layout("se").as_deref(), Some("se"));
        assert_eq!(parse_xkb_layout(" SE , us ").as_deref(), Some("se"));
        assert_eq!(parse_xkb_layout("de(nodeadkeys)").as_deref(), Some("de"));
        assert_eq!(parse_xkb_layout(""), None);
        assert_eq!(parse_xkb_layout("  "), None);
    }
}
