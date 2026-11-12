//! Themes and the single CSS provider shared by every screen.

use gtk::CssProvider;

use crate::settings::Settings;

pub struct Palette {
    pub background: &'static str,
    pub text: &'static str,
    pub dim: &'static str,
    pub muted: &'static str,
    pub selection: &'static str,
    pub accent: &'static str,
    pub border: &'static str,
    pub danger: &'static str,
}

impl Palette {
    pub fn paper() -> Self {
        Self {
            background: "#f3ecdf",
            text: "#26221c",
            dim: "#9a8f7c",
            muted: "#6f6657",
            selection: "#e4d8c2",
            accent: "#7b5e3c",
            border: "#d8cbb6",
            danger: "#8c3f33",
        }
    }

    pub fn night() -> Self {
        Self {
            background: "#262320",
            text: "#e8e0d2",
            dim: "#8c8274",
            muted: "#a79c8b",
            selection: "#39342d",
            accent: "#c9ae83",
            border: "#413b33",
            danger: "#d08b7d",
        }
    }
}

pub fn palette(settings: &Settings) -> Palette {
    match settings.theme {
        crate::settings::Theme::Paper => Palette::paper(),
        crate::settings::Theme::Night => Palette::night(),
    }
}

pub fn css(settings: &Settings) -> String {
    let palette = palette(settings);
    let family = settings.font.css_family();
    let size = settings.font_size;

    format!(
        r#"
window, .stylos-page {{
    background-color: {background};
    color: {text};
    font-family: {family};
}}
.stylos-editor, .stylos-editor text {{
    background-color: {background};
    color: {text};
    font-family: {family};
    font-size: {size}pt;
    caret-color: {text};
}}
.stylos-editor selection {{
    background-color: {selection};
    color: {text};
}}
/* Typewriter scrolling makes the scrollbar unnecessary; keep the space calm. */
scrollbar {{
    opacity: 0;
    min-width: 0;
    min-height: 0;
}}
.bottom-line {{
    color: {muted};
    font-size: 10pt;
    padding: 8px 24px 14px 24px;
}}
.screen-title {{
    font-size: 24pt;
    font-weight: 600;
    color: {text};
    margin-bottom: 18px;
}}
.hint {{
    color: {muted};
    font-size: 10pt;
    padding: 12px 4px 18px 4px;
}}
.file-list {{
    background-color: transparent;
}}
.file-row {{
    padding: 10px 14px;
    border-radius: 7px;
    color: {text};
    font-size: 15pt;
}}
.file-row:hover {{
    background-color: {selection};
}}
.file-row:selected {{
    background-color: {selection};
    color: {text};
}}
.folder-name {{
    font-weight: 600;
}}
.file-name {{
    font-weight: 400;
}}
.row-note {{
    color: {dim};
    font-size: 10pt;
}}
.renaming {{
    background-color: {selection};
    color: {text};
    border-radius: 6px;
    padding: 6px 10px;
}}
.error-text {{
    color: {danger};
}}
.calm-button {{
    background-color: {selection};
    color: {text};
    border: 1px solid {border};
    border-radius: 6px;
    padding: 6px 14px;
}}
button {{
    background-color: {selection};
    color: {text};
    border: 1px solid {border};
    border-radius: 6px;
    box-shadow: none;
    padding: 6px 16px;
}}
button:hover {{
    background-color: {border};
}}
.dialog, window.dialog {{
    background-color: {background};
    color: {text};
}}
"#,
        background = palette.background,
        text = palette.text,
        dim = palette.dim,
        muted = palette.muted,
        selection = palette.selection,
        border = palette.border,
        danger = palette.danger,
        family = family,
        size = size,
    )
}

/// Load the current theme into the app-wide provider. GTK keeps the old
/// stylesheet until the new one parses, so a mistake cannot leave a blank
/// screen.
pub fn apply(provider: &CssProvider, settings: &Settings) {
    provider.load_from_string(&css(settings));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FontChoice, Settings, Theme};

    fn settings_for(theme: Theme, font: FontChoice) -> Settings {
        Settings {
            theme,
            font,
            ..Settings::default()
        }
    }

    #[test]
    fn themes_are_warm_paper_and_dark_grey_not_black_or_white() {
        for theme in [Theme::Paper, Theme::Night] {
            let sheet = css(&settings_for(theme, FontChoice::Serif));
            assert!(!sheet.to_ascii_lowercase().contains("#000000"));
            assert!(!sheet.to_ascii_lowercase().contains("#ffffff"));
            assert!(sheet.contains("font-size: 18pt"));
            assert!(sheet.contains("DejaVu Serif"));
        }
    }

    #[test]
    fn the_three_fonts_are_distinct_families() {
        let serif = css(&settings_for(Theme::Paper, FontChoice::Serif));
        let sans = css(&settings_for(Theme::Paper, FontChoice::Sans));
        let mono = css(&settings_for(Theme::Paper, FontChoice::Mono));
        assert!(serif.contains("DejaVu Serif"));
        assert!(sans.contains("DejaVu Sans"));
        assert!(mono.contains("DejaVu Sans Mono"));
    }

    #[test]
    fn the_scrollbar_is_hidden_for_typewriter_scrolling() {
        let sheet = css(&Settings::default());
        assert!(sheet.contains("scrollbar"));
        assert!(sheet.contains("opacity: 0"));
    }
}
