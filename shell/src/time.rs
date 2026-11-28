//! Current date and time in the writer's chosen time zone.
//!
//! `Settings::time_zone` stores either `"Local time"` or an IANA identifier
//! such as `"Europe/Stockholm"` or `"UTC"`. The bottom-line clock, daily word
//! count rollover and Automatic day/night theme all read the time through this
//! module so changing the setting takes effect at once.

use gtk::glib;

/// Resolve a saved `Settings::time_zone` value to a GLib time zone.
///
/// `"Local time"` (and empty strings) use the system local zone, `"UTC"` uses
/// UTC directly, and any valid IANA identifier is loaded via
/// [`glib::TimeZone::from_identifier`]. Unknown or invalid identifiers fall
/// back to the local zone so a hand-edited settings file never breaks the
/// clock or theme.
pub fn zone(name: &str) -> glib::TimeZone {
    let name = name.trim();
    if name.is_empty() || name == "Local time" {
        return glib::TimeZone::local();
    }
    if name == "UTC" {
        return glib::TimeZone::utc();
    }
    glib::TimeZone::from_identifier(Some(name)).unwrap_or_else(glib::TimeZone::local)
}

/// The current instant in `time_zone`.
pub fn now(time_zone: &str) -> Option<glib::DateTime> {
    let tz = zone(time_zone);
    glib::DateTime::now(&tz).ok()
}

/// Convert a UTC Unix timestamp (seconds) into `time_zone`.
pub fn at_unix(unix: i64, time_zone: &str) -> Option<glib::DateTime> {
    let tz = zone(time_zone);
    glib::DateTime::from_unix_utc(unix)
        .ok()
        .and_then(|dt| dt.to_timezone(&tz).ok())
}

/// Current hour (0–23) in `time_zone`, defaulting to noon when the clock
/// cannot be read.
pub fn hour(time_zone: &str) -> u8 {
    now(time_zone)
        .map(|dt| dt.hour().clamp(0, 23) as u8)
        .unwrap_or(12)
}

/// Bottom-line clock text (`HH:MM`) in `time_zone`.
pub fn clock_text(time_zone: &str) -> Option<String> {
    now(time_zone)
        .and_then(|dt| dt.format("%H:%M").ok())
        .map(|text| text.to_string())
}

/// Calendar date (`YYYY-MM-DD`) in `time_zone`.
pub fn date(time_zone: &str) -> String {
    now(time_zone)
        .and_then(|dt| dt.format("%Y-%m-%d").ok())
        .map(|text| text.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::TIME_ZONES;
    use crate::settings::{Settings, Theme};

    // 2026-01-15 12:00:00 UTC
    const WINTER_NOON_UTC: i64 = 1_768_478_400;

    #[test]
    fn every_listed_time_zone_resolves() {
        for &entry in TIME_ZONES {
            if entry == "Local time" {
                let local = glib::DateTime::from_unix_local(WINTER_NOON_UTC).unwrap();
                let resolved = at_unix(WINTER_NOON_UTC, entry).unwrap();
                assert_eq!(resolved.utc_offset(), local.utc_offset());
            } else {
                assert!(
                    glib::TimeZone::from_identifier(Some(entry)).is_some(),
                    "TIME_ZONES entry {entry:?} must resolve via GLib"
                );
            }
        }
    }

    #[test]
    fn utc_and_iana_zones_shift_a_known_instant() {
        let cases = [
            ("UTC", 12, "12:00", "2026-01-15"),
            ("Europe/Stockholm", 13, "13:00", "2026-01-15"),
            ("Asia/Kolkata", 17, "17:30", "2026-01-15"),
            ("Asia/Tokyo", 21, "21:00", "2026-01-15"),
            ("America/Los_Angeles", 4, "04:00", "2026-01-15"),
        ];
        for (name, expected_hour, expected_clock, expected_date) in cases {
            let dt = at_unix(WINTER_NOON_UTC, name).expect("valid time zone");
            assert_eq!(dt.hour(), expected_hour, "{name}");
            assert_eq!(
                dt.format("%H:%M").unwrap().as_str(),
                expected_clock,
                "{name}"
            );
            assert_eq!(
                dt.format("%Y-%m-%d").unwrap().as_str(),
                expected_date,
                "{name}"
            );
        }
    }

    #[test]
    fn local_time_and_invalid_ids_fall_back_to_local() {
        let local = glib::DateTime::from_unix_local(WINTER_NOON_UTC).unwrap();
        for candidate in ["Local time", "", "   ", "Mars/Olympus", "Not/A_Real_Zone"] {
            let resolved = at_unix(WINTER_NOON_UTC, candidate).expect("fallback to local");
            assert_eq!(
                resolved.utc_offset(),
                local.utc_offset(),
                "candidate {candidate:?} must fall back to local offset"
            );
            assert_eq!(
                resolved.format("%Y-%m-%d %H:%M").unwrap(),
                local.format("%Y-%m-%d %H:%M").unwrap(),
                "candidate {candidate:?} must match local time"
            );
        }
    }

    #[test]
    fn now_and_clock_text_reflect_selected_zone() {
        let utc = now("UTC").expect("UTC now");
        let tokyo = now("Asia/Tokyo").expect("Tokyo now");
        // Asia/Tokyo is always UTC+9 (no DST).
        assert_eq!(utc.utc_offset().as_seconds(), 0);
        assert_eq!(tokyo.utc_offset().as_seconds(), 9 * 3600);

        let utc_clock = clock_text("UTC").expect("UTC clock");
        let tokyo_clock = clock_text("Asia/Tokyo").expect("Tokyo clock");
        assert_ne!(
            utc_clock, tokyo_clock,
            "UTC and Asia/Tokyo are 9 hours apart"
        );
    }

    #[test]
    fn automatic_theme_follows_the_chosen_zone_hour() {
        let settings = Settings::default();
        let utc_hour = at_unix(WINTER_NOON_UTC, "UTC").unwrap().hour() as u8;
        let tokyo_hour = at_unix(WINTER_NOON_UTC, "Asia/Tokyo").unwrap().hour() as u8;
        let la_hour = at_unix(WINTER_NOON_UTC, "America/Los_Angeles")
            .unwrap()
            .hour() as u8;

        assert_eq!(settings.theme_at(utc_hour), Theme::Light);
        assert_eq!(settings.theme_at(tokyo_hour), Theme::Dark);
        assert_eq!(settings.theme_at(la_hour), Theme::Dark);
    }
}
