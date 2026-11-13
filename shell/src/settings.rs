//! User settings stored at `<writing root>/.carrel/settings.json`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::atomic_write;

/// One of the six colours a person can wear (issue #21): the five Nostos
/// themes, so the two products feel like one ecosystem, and a High contrast
/// theme for bright rooms and weak eyes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    #[default]
    Light,
    Sepia,
    Mist,
    Study,
    Dark,
    HighContrast,
}

impl Theme {
    /// Lightest to darkest, with High contrast last: the order the picker
    /// walks in.
    pub const ALL: [Self; 6] = [
        Self::Light,
        Self::Sepia,
        Self::Mist,
        Self::Study,
        Self::Dark,
        Self::HighContrast,
    ];

    /// The themes Automatic may wear by day (07:00–19:00).
    pub const DAY: [Self; 3] = [Self::Light, Self::Sepia, Self::HighContrast];

    /// The themes Automatic may wear at night (19:00–07:00).
    pub const NIGHT: [Self; 3] = [Self::Mist, Self::Study, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Sepia => "Sepia",
            Self::Mist => "Mist",
            Self::Study => "Writer's Study",
            Self::Dark => "Dark",
            Self::HighContrast => "High contrast",
        }
    }

    /// Whether this theme is a night theme: the only ones the night slot and
    /// GTK's dark preference accept.
    pub fn is_dark(self) -> bool {
        match self {
            Self::Light | Self::Sepia | Self::HighContrast => false,
            Self::Mist | Self::Study | Self::Dark => true,
        }
    }
}

/// What the person picked on the Theme row: one theme always, or Automatic,
/// which follows the clock (`spec/03-ui.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice {
    #[default]
    Automatic,
    Light,
    /// `paper` is what settings files written before issue #21 called Sepia.
    #[serde(alias = "paper")]
    Sepia,
    Mist,
    /// `night` is what settings files written before issue #21 called
    /// Writer's Study, the closest theme to the old Night.
    #[serde(alias = "night")]
    Study,
    Dark,
    HighContrast,
}

impl ThemeChoice {
    /// The seven choices in the order the picker walks in.
    pub const ALL: [Self; 7] = [
        Self::Automatic,
        Self::Light,
        Self::Sepia,
        Self::Mist,
        Self::Study,
        Self::Dark,
        Self::HighContrast,
    ];

    pub fn label(self) -> &'static str {
        match self.theme() {
            Some(theme) => theme.label(),
            None => "Automatic",
        }
    }

    pub fn is_automatic(self) -> bool {
        self == Self::Automatic
    }

    /// The fixed theme, or `None` for Automatic.
    pub fn theme(self) -> Option<Theme> {
        match self {
            Self::Automatic => None,
            Self::Light => Some(Theme::Light),
            Self::Sepia => Some(Theme::Sepia),
            Self::Mist => Some(Theme::Mist),
            Self::Study => Some(Theme::Study),
            Self::Dark => Some(Theme::Dark),
            Self::HighContrast => Some(Theme::HighContrast),
        }
    }

    pub fn cycled(self, delta: i32) -> Self {
        cycle(&Self::ALL, self, delta)
    }
}

impl From<Theme> for ThemeChoice {
    fn from(theme: Theme) -> Self {
        match theme {
            Theme::Light => Self::Light,
            Theme::Sepia => Self::Sepia,
            Theme::Mist => Self::Mist,
            Theme::Study => Self::Study,
            Theme::Dark => Self::Dark,
            Theme::HighContrast => Self::HighContrast,
        }
    }
}

/// Move `delta` places through `list`, wrapping. Zero stays put.
fn cycle<T: Copy + PartialEq>(list: &[T], current: T, delta: i32) -> T {
    let len = list.len() as i32;
    let index = list.iter().position(|item| *item == current).unwrap_or(0) as i32;
    list[((index + delta).rem_euclid(len)) as usize]
}

/// Automatic wears the day theme from 07:00 and the night theme from 19:00
/// (spec/03-ui.md). Carrel has no OS preference, so local time decides.
pub const DAY_START_HOUR: u8 = 7;
pub const DAY_END_HOUR: u8 = 19;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontChoice {
    #[default]
    Serif,
    Sans,
    Mono,
}

impl FontChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Serif => "Serif",
            Self::Sans => "Sans",
            Self::Mono => "Mono",
        }
    }

    /// Bundled faces first, then whatever the system has. The image will get
    /// Source Serif 4, Inter and JetBrains Mono later; DejaVu is today's
    /// fallback so a build always looks right.
    pub fn families(self) -> &'static [&'static str] {
        match self {
            Self::Serif => &[
                "Source Serif 4",
                "Source Serif Pro",
                "Charter",
                "DejaVu Serif",
            ],
            Self::Sans => &["Inter", "DejaVu Sans"],
            Self::Mono => &["JetBrains Mono", "DejaVu Sans Mono"],
        }
    }

    pub fn css_family(self) -> String {
        let generic = match self {
            Self::Serif => "serif",
            Self::Sans => "sans-serif",
            Self::Mono => "monospace",
        };
        let named: Vec<String> = self
            .families()
            .iter()
            .map(|family| format!("\"{family}\""))
            .collect();
        format!("{}, {generic}", named.join(", "))
    }
}

/// Body text is measured in pixels (spec/03-ui.md): 19 px by default, and
/// Ctrl+= / Ctrl+- walk one pixel at a time from 16 to 24.
pub const MIN_FONT_SIZE: u32 = 16;
pub const MAX_FONT_SIZE: u32 = 24;
pub const FONT_SIZE_STEP: u32 = 1;

/// How the daily goal appears in the editor (spec/03-ui.md).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalDisplay {
    /// Words today / goal on the bottom line.
    #[default]
    Numbers,
    /// A 2 px accent line along the bottom window edge.
    Hairline,
}

impl GoalDisplay {
    pub fn label(self) -> &'static str {
        match self {
            Self::Numbers => "Numbers",
            Self::Hairline => "Hairline",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Numbers => Self::Hairline,
            Self::Hairline => Self::Numbers,
        }
    }
}

pub const MIN_DAILY_GOAL: u32 = 100;
pub const MAX_DAILY_GOAL: u32 = 2_000;
pub const DAILY_GOAL_STEP: u32 = 100;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// One theme, or Automatic (the default): a day theme and a night theme.
    pub theme: ThemeChoice,
    /// The theme Automatic wears from 07:00 to 19:00.
    pub day_theme: Theme,
    /// The theme Automatic wears from 19:00 to 07:00.
    pub night_theme: Theme,
    pub font: FontChoice,
    /// Body text size in CSS pixels.
    pub font_size: u32,
    pub daily_goal: u32,
    /// How the daily goal is drawn: numbers or the hairline.
    pub goal_display: GoalDisplay,
    /// Dim every paragraph but the one with the cursor (F2).
    pub focus_mode: bool,
    pub show_bottom_line: bool,
    pub typewriter_mode: bool,
    pub spellcheck: bool,
    pub clock: bool,
    /// Display language (see `locale::LANGUAGES`).
    pub language: String,
    /// Keyboard layout code (see `locale::KEYBOARDS`).
    pub keyboard: String,
    /// IANA time zone, or `Local time`.
    pub time_zone: String,
    /// Sync once more before shutting down, if a server is paired.
    pub sync_at_shutdown: bool,
    /// False until the First run flow reaches the editor.
    pub first_run_complete: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::Automatic,
            day_theme: Theme::Light,
            night_theme: Theme::Dark,
            font: FontChoice::Serif,
            font_size: 19,
            daily_goal: 500,
            goal_display: GoalDisplay::Numbers,
            focus_mode: false,
            show_bottom_line: true,
            typewriter_mode: false,
            spellcheck: false,
            clock: false,
            language: "English".to_owned(),
            keyboard: "us".to_owned(),
            time_zone: "Local time".to_owned(),
            sync_at_shutdown: false,
            first_run_complete: false,
        }
    }
}

impl Settings {
    pub fn load(root: &Path) -> Self {
        let path = settings_path(root);
        let mut settings = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
            .unwrap_or_default();
        settings.normalize();
        settings
    }

    pub fn save(&self, root: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        atomic_write(&settings_path(root), &json)
    }

    pub fn normalize(&mut self) {
        // The day slot holds a day theme and the night slot a night one, even
        // if the JSON was edited by hand: a light theme at 23:00 is not
        // automatic, it is a mistake.
        if self.day_theme.is_dark() {
            self.day_theme = Theme::Light;
        }
        if !self.night_theme.is_dark() {
            self.night_theme = Theme::Dark;
        }
        self.font_size = self
            .font_size
            .clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
            .next_multiple_of(FONT_SIZE_STEP);
        if self.daily_goal == 0 {
            self.daily_goal = 500;
        }
        self.daily_goal = self.daily_goal.clamp(MIN_DAILY_GOAL, MAX_DAILY_GOAL);
        if self.language.is_empty() {
            self.language = "English".to_owned();
        }
        if self.keyboard.is_empty() {
            self.keyboard = "us".to_owned();
        }
        if self.time_zone.is_empty() {
            self.time_zone = "Local time".to_owned();
        }
    }

    /// The theme on screen at `hour` (local time, 0–23): a fixed choice is
    /// always worn, Automatic wears its day theme from 07:00 and its night
    /// theme from 19:00.
    pub fn theme_at(&self, hour: u8) -> Theme {
        match self.theme.theme() {
            Some(theme) => theme,
            None if (DAY_START_HOUR..DAY_END_HOUR).contains(&hour) => self.day_theme,
            None => self.night_theme,
        }
    }

    pub fn larger_font(&mut self) -> bool {
        let next = (self.font_size + FONT_SIZE_STEP).min(MAX_FONT_SIZE);
        let changed = next != self.font_size;
        self.font_size = next;
        changed
    }

    pub fn smaller_font(&mut self) -> bool {
        let next = self
            .font_size
            .saturating_sub(FONT_SIZE_STEP)
            .max(MIN_FONT_SIZE);
        let changed = next != self.font_size;
        self.font_size = next;
        changed
    }
}

pub fn settings_path(root: &Path) -> PathBuf {
    root.join(".carrel").join("settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_match_the_spec() {
        let settings = Settings::default();
        assert_eq!(settings.theme, ThemeChoice::Automatic);
        assert_eq!(settings.day_theme, Theme::Light);
        assert_eq!(settings.night_theme, Theme::Dark);
        assert_eq!(settings.font, FontChoice::Serif);
        assert_eq!(settings.font_size, 19);
        assert_eq!(settings.daily_goal, 500);
        assert_eq!(settings.goal_display, GoalDisplay::Numbers);
        assert!(!settings.focus_mode);
        assert!(settings.show_bottom_line);
        assert!(!settings.typewriter_mode);
        assert!(!settings.spellcheck);
        assert!(!settings.clock);
    }

    #[test]
    fn automatic_wears_the_day_theme_from_seven_and_the_night_theme_from_nineteen() {
        let mut settings = Settings::default();
        assert_eq!(settings.theme_at(6), Theme::Dark);
        assert_eq!(settings.theme_at(DAY_START_HOUR), Theme::Light);
        assert_eq!(settings.theme_at(12), Theme::Light);
        assert_eq!(settings.theme_at(DAY_END_HOUR - 1), Theme::Light);
        assert_eq!(settings.theme_at(DAY_END_HOUR), Theme::Dark);
        assert_eq!(settings.theme_at(23), Theme::Dark);

        // The slots are settings, not constants.
        settings.day_theme = Theme::Sepia;
        settings.night_theme = Theme::Study;
        assert_eq!(settings.theme_at(9), Theme::Sepia);
        assert_eq!(settings.theme_at(21), Theme::Study);

        // A fixed choice ignores the clock.
        settings.theme = Theme::Mist.into();
        assert_eq!(settings.theme_at(9), Theme::Mist);
        assert_eq!(settings.theme_at(21), Theme::Mist);
    }

    #[test]
    fn old_paper_and_night_settings_migrate() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".carrel")).unwrap();

        // A settings file written before issue #21 keeps its similar look.
        fs::write(settings_path(dir.path()), br#"{"theme":"paper"}"#).unwrap();
        assert_eq!(Settings::load(dir.path()).theme, ThemeChoice::Sepia);
        fs::write(settings_path(dir.path()), br#"{"theme":"night"}"#).unwrap();
        assert_eq!(Settings::load(dir.path()).theme, ThemeChoice::Study);

        // Saving once writes the new name, so the migration happens once.
        Settings::load(dir.path()).save(dir.path()).unwrap();
        let saved = fs::read_to_string(settings_path(dir.path())).unwrap();
        assert!(saved.contains("\"study\""));
        assert!(!saved.contains("\"night\""));

        // Files without a theme get the new default.
        fs::write(settings_path(dir.path()), br#"{"font":"mono"}"#).unwrap();
        assert_eq!(Settings::load(dir.path()).theme, ThemeChoice::Automatic);
    }

    #[test]
    fn theme_choices_and_slots_cycle_within_their_modes() {
        assert_eq!(ThemeChoice::ALL.len(), 7);
        assert_eq!(ThemeChoice::Automatic.cycled(1), ThemeChoice::Light);
        assert_eq!(ThemeChoice::HighContrast.cycled(1), ThemeChoice::Automatic);
        assert_eq!(ThemeChoice::Automatic.cycled(-1), ThemeChoice::HighContrast);
        assert_eq!(ThemeChoice::Sepia.cycled(0), ThemeChoice::Sepia);

        assert_eq!(Theme::DAY.len(), 3);
        assert_eq!(Theme::NIGHT.len(), 3);
        for theme in Theme::DAY {
            assert!(!theme.is_dark(), "{} must be a day theme", theme.label());
        }
        for theme in Theme::NIGHT {
            assert!(theme.is_dark(), "{} must be a night theme", theme.label());
        }
        assert_eq!(Theme::Study.label(), "Writer's Study");
        assert_eq!(Theme::HighContrast.label(), "High contrast");
        assert_eq!(ThemeChoice::Automatic.label(), "Automatic");
    }

    #[test]
    fn normalize_keeps_the_day_and_night_slots_in_their_mode() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".carrel")).unwrap();
        fs::write(
            settings_path(dir.path()),
            br#"{"day_theme":"dark","night_theme":"sepia"}"#,
        )
        .unwrap();
        let settings = Settings::load(dir.path());
        assert_eq!(settings.day_theme, Theme::Light);
        assert_eq!(settings.night_theme, Theme::Dark);
    }

    #[test]
    fn load_missing_or_broken_returns_defaults() {
        let dir = tempdir().unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
        fs::create_dir_all(dir.path().join(".carrel")).unwrap();
        fs::write(settings_path(dir.path()), b"{not json").unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
    }

    #[test]
    fn roundtrips_and_clamps_font_size() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".carrel")).unwrap();
        fs::write(
            settings_path(dir.path()),
            br#"{"theme":"study","font":"mono","font_size":99,"daily_goal":0}"#,
        )
        .unwrap();
        let settings = Settings::load(dir.path());
        assert_eq!(settings.theme, ThemeChoice::Study);
        assert_eq!(settings.font, FontChoice::Mono);
        assert_eq!(settings.font_size, MAX_FONT_SIZE);
        assert_eq!(settings.daily_goal, 500);
        settings.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), settings);
    }

    #[test]
    fn font_size_steps_and_stops() {
        let mut settings = Settings::default();
        assert!(settings.larger_font());
        assert_eq!(settings.font_size, 20);
        for _ in 0..20 {
            settings.larger_font();
        }
        assert_eq!(settings.font_size, MAX_FONT_SIZE);
        assert!(!settings.larger_font());
        for _ in 0..30 {
            settings.smaller_font();
        }
        assert_eq!(settings.font_size, MIN_FONT_SIZE);
        assert!(!settings.smaller_font());
        assert_eq!((MIN_FONT_SIZE, MAX_FONT_SIZE), (16, 24));
    }

    #[test]
    fn focus_mode_and_goal_display_persist() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".carrel")).unwrap();
        let mut settings = Settings {
            focus_mode: true,
            goal_display: GoalDisplay::Hairline,
            ..Settings::default()
        };
        settings.save(dir.path()).unwrap();
        let loaded = Settings::load(dir.path());
        assert!(loaded.focus_mode);
        assert_eq!(loaded.goal_display, GoalDisplay::Hairline);
        assert_eq!(loaded, settings);

        // Old settings files predate both fields: the spec defaults apply.
        fs::write(
            settings_path(dir.path()),
            br#"{"theme":"paper","focus_mode":false}"#,
        )
        .unwrap();
        let old = Settings::load(dir.path());
        assert_eq!(old.goal_display, GoalDisplay::Numbers);
        assert!(!old.focus_mode);

        settings.focus_mode = false;
        settings.goal_display = GoalDisplay::Numbers;
        settings.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), settings);
    }

    #[test]
    fn goal_display_labels_and_toggle() {
        assert_eq!(GoalDisplay::Numbers.label(), "Numbers");
        assert_eq!(GoalDisplay::Hairline.label(), "Hairline");
        assert_eq!(GoalDisplay::Numbers.other(), GoalDisplay::Hairline);
        assert_eq!(GoalDisplay::Hairline.other(), GoalDisplay::Numbers);
    }
}
