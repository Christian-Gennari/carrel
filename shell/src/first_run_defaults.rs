//! The sensible first-run defaults: what the image already knows about the
//! writer's language, keyboard and time zone.
//!
//! First run shows these three answers already filled in, so a writer can
//! press Enter twice and start. The probe is best effort: an image that sets
//! `XKBLAYOUT`, `LANG` or `/etc/timezone` gets those values, and anything
//! unknown falls back to English, US and local time. No GTK here, so the
//! parsing is unit tested without a display.

use std::fs;

use crate::locale;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defaults {
    pub language: String,
    pub keyboard: String,
    pub time_zone: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            language: "English".to_owned(),
            keyboard: "us".to_owned(),
            time_zone: "Local time".to_owned(),
        }
    }
}

/// What the image says, or the safe fallback when it says nothing.
pub fn detect() -> Defaults {
    let layout = fs::read_to_string("/etc/default/keyboard")
        .ok()
        .and_then(|text| layout_in_file(&text));
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .ok();
    let zone = fs::read_to_string("/etc/timezone")
        .ok()
        .or_else(|| {
            fs::read_link("/etc/localtime")
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
        })
        .and_then(|text| zone_in_text(&text));
    from_sources(layout.as_deref(), locale.as_deref(), zone.as_deref())
}

/// The same choice from three already-read values. Kept separate from
/// [`detect`] so the rules can be tested without touching the machine.
pub fn from_sources(layout: Option<&str>, locale: Option<&str>, zone: Option<&str>) -> Defaults {
    let code = keyboard_from_locale(locale);
    let keyboard = layout
        .map(|layout| layout.trim().to_ascii_lowercase())
        .filter(|layout| is_keyboard(layout))
        .or_else(|| code.clone())
        .unwrap_or_else(|| "us".to_owned());
    Defaults {
        language: language_from_locale(locale).to_owned(),
        keyboard,
        time_zone: zone
            .and_then(zone_in_text)
            .unwrap_or_else(|| "Local time".to_owned()),
    }
}

fn is_keyboard(code: &str) -> bool {
    locale::KEYBOARDS.iter().any(|(known, _)| *known == code)
}

/// `XKBLAYOUT="de"` (the first layout, when several are listed).
fn layout_in_file(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix("XKBLAYOUT=")?;
        let value = value.trim().trim_matches('"').trim_matches('\'');
        value
            .split(',')
            .next()
            .map(|first| first.trim().to_ascii_lowercase())
            .filter(|first| !first.is_empty())
    })
}

/// Split a POSIX locale (`de_DE.UTF-8`, `pt_BR`, `fr-FR@euro`) into its
/// lower-case language and region parts.
fn locale_parts(locale: Option<&str>) -> (String, Option<String>) {
    let Some(locale) = locale else {
        return (String::new(), None);
    };
    let base = locale
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .replace('-', "_");
    let mut parts = base.split('_');
    let language = parts.next().unwrap_or_default().to_ascii_lowercase();
    let region = parts.next().map(|part| part.to_ascii_lowercase());
    (language, region)
}

fn language_from_locale(locale: Option<&str>) -> &'static str {
    let (language, _) = locale_parts(locale);
    match language.as_str() {
        "de" => "Deutsch",
        "fr" => "Français",
        "es" => "Español",
        "it" => "Italiano",
        "pt" => "Português",
        "nl" => "Nederlands",
        "pl" => "Polski",
        "tr" => "Türkçe",
        _ => "English",
    }
}

fn keyboard_from_locale(locale: Option<&str>) -> Option<String> {
    let (language, region) = locale_parts(locale);
    let code = match (language.as_str(), region.as_deref()) {
        ("en", Some("gb")) => "gb",
        ("en", _) => "us",
        ("de", _) => "de",
        ("fr", _) => "fr",
        ("es", _) => "es",
        ("it", _) => "it",
        ("pt", Some("br")) => "br",
        ("pt", _) => "pt",
        ("pl", _) => "pl",
        ("tr", _) => "tr",
        ("sv", _) => "se",
        ("nb" | "nn" | "no", _) => "no",
        ("da", _) => "dk",
        _ => return None,
    };
    Some(code.to_owned())
}

/// A zone name from `/etc/timezone` or the `/etc/localtime` symlink, but only
/// one of the zones the picker offers; the system's `Etc/UTC` is spelled
/// "UTC".
fn zone_in_text(text: &str) -> Option<String> {
    let text = text.trim().trim_end_matches('/');
    let name = text
        .rsplit_once("zoneinfo/")
        .map(|(_, name)| name)
        .unwrap_or(text);
    if name.is_empty() {
        return None;
    }
    if name == "Etc/UTC" || name == "Etc/GMT" || name == "GMT" || name == "UTC" {
        return Some("UTC".to_owned());
    }
    locale::TIME_ZONES
        .iter()
        .find(|zone| **zone == name)
        .map(|zone| (*zone).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_german_image_fills_all_three_answers() {
        let defaults = from_sources(Some("de"), Some("de_DE.UTF-8"), Some("Europe/Berlin"));
        assert_eq!(defaults.language, "Deutsch");
        assert_eq!(defaults.keyboard, "de");
        assert_eq!(defaults.time_zone, "Europe/Berlin");
    }

    #[test]
    fn the_layout_file_wins_over_the_locale() {
        let defaults = from_sources(Some("fr"), Some("de_DE.UTF-8"), None);
        assert_eq!(defaults.keyboard, "fr");
        assert_eq!(defaults.language, "Deutsch");
        assert_eq!(defaults.time_zone, "Local time");
    }

    #[test]
    fn unknown_or_missing_values_fall_back_to_english_us_and_local_time() {
        let defaults = from_sources(None, None, None);
        assert_eq!(defaults, Defaults::default());
        let defaults = from_sources(Some("zz"), Some("C.UTF-8"), Some("Etc/UTC"));
        assert_eq!(defaults.language, "English");
        assert_eq!(defaults.keyboard, "us");
        assert_eq!(defaults.time_zone, "UTC");
    }

    #[test]
    fn a_british_and_a_brazilian_locale_choose_their_keyboard() {
        assert_eq!(from_sources(None, Some("en_GB.UTF-8"), None).keyboard, "gb");
        assert_eq!(from_sources(None, Some("pt_BR.UTF-8"), None).keyboard, "br");
        assert_eq!(
            from_sources(None, Some("pt_PT.UTF-8"), None).language,
            "Português"
        );
    }

    #[test]
    fn the_keyboard_file_parser_takes_the_first_layout() {
        let text = "# keyboard\nXKBLAYOUT=\"de,us\"\nXKBVARIANT=\"nodeadkeys\"\n";
        assert_eq!(layout_in_file(text).as_deref(), Some("de"));
        assert_eq!(layout_in_file("XKBLAYOUT=fr\n").as_deref(), Some("fr"));
        assert_eq!(layout_in_file("XKBVARIANT=\"x\"\n"), None);
    }

    #[test]
    fn time_zone_names_are_read_from_the_file_or_the_symlink() {
        assert_eq!(
            zone_in_text("Europe/Rome\n").as_deref(),
            Some("Europe/Rome")
        );
        assert_eq!(
            zone_in_text("/usr/share/zoneinfo/America/New_York").as_deref(),
            Some("America/New_York")
        );
        assert_eq!(zone_in_text("Etc/UTC").as_deref(), Some("UTC"));
        assert_eq!(zone_in_text("Europe/Unknown"), None);
    }
}
