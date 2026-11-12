//! User settings stored at `<writing root>/.stylos/settings.json`.

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
    pub fn css_family(self) -> &'static str {
        match self {
            Self::Serif => {
                "\"Source Serif 4\", \"Source Serif Pro\", \"Charter\", \"DejaVu Serif\", serif"
            }
            Self::Sans => "\"Inter\", \"DejaVu Sans\", sans-serif",
            Self::Mono => "\"JetBrains Mono\", \"DejaVu Sans Mono\", monospace",
        }
    }
}

pub const MIN_FONT_SIZE: u32 = 12;
pub const MAX_FONT_SIZE: u32 = 36;
pub const FONT_SIZE_STEP: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    pub font: FontChoice,
    /// Points.
    pub font_size: u32,
    pub daily_goal: u32,
    pub show_bottom_line: bool,
    pub typewriter_mode: bool,
    pub spellcheck: bool,
    pub clock: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Paper,
            font: FontChoice::Serif,
            font_size: 18,
            daily_goal: 500,
            show_bottom_line: true,
            typewriter_mode: false,
            spellcheck: false,
            clock: false,
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
    root.join(".stylos").join("settings.json")
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
        assert_eq!(settings.font_size, 18);
        assert_eq!(settings.daily_goal, 500);
        assert!(settings.show_bottom_line);
        assert!(!settings.typewriter_mode);
        assert!(!settings.spellcheck);
        assert!(!settings.clock);
    }

    #[test]
    fn load_missing_or_broken_returns_defaults() {
        let dir = tempdir().unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
        fs::create_dir_all(dir.path().join(".stylos")).unwrap();
        fs::write(settings_path(dir.path()), b"{not json").unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
    }

    #[test]
    fn roundtrips_and_clamps_font_size() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".stylos")).unwrap();
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
    }
}
