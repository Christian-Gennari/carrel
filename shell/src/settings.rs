//! User settings stored at `<writing root>/.carrel/settings.json`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::atomic_write;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Paper,
    Night,
}

impl Theme {
    pub fn other(self) -> Self {
        match self {
            Self::Paper => Self::Night,
            Self::Night => Self::Paper,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Paper => "Paper",
            Self::Night => "Night",
        }
    }
}

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
    pub theme: Theme,
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
            theme: Theme::Paper,
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
        assert_eq!(settings.theme, Theme::Paper);
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
            br#"{"theme":"night","font":"mono","font_size":99,"daily_goal":0}"#,
        )
        .unwrap();
        let settings = Settings::load(dir.path());
        assert_eq!(settings.theme, Theme::Night);
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
