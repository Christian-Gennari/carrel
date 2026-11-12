//! Themes and the single CSS provider shared by every screen.
//!
//! One palette per theme, straight from `spec/03-ui.md`: Paper is a warm
//! off-white with near-black ink, Night is a dark grey with soft white text.
//! Everything else is derived from those two colours, so there is no second
//! accent and no pure black or white.

use gtk::gdk;
use gtk::CssProvider;

use crate::settings::Settings;

/// The four colours a person can name. The rest is mixed from them.
pub struct Palette {
    pub background: &'static str,
    pub text: &'static str,
    pub accent: &'static str,
    pub danger: &'static str,
}

impl Palette {
    pub fn paper() -> Self {
        Self {
            background: "#F6F4EE",
            text: "#262523",
            accent: "#C85A32",
            danger: "#8C3F33",
        }
    }

    pub fn night() -> Self {
        Self {
            background: "#1E1C1A",
            text: "#DCDAD5",
            accent: "#E5A93B",
            danger: "#D08B7D",
        }
    }

    /// Markdown syntax and other quiet text: ink at 55 % (spec/03-ui.md).
    pub fn dim(&self) -> String {
        blend(self.text, self.background, 0.55)
    }

    /// Secondary interface text: the bottom line, hints, values.
    pub fn muted(&self) -> String {
        blend(self.text, self.background, 0.72)
    }

    pub fn border(&self) -> String {
        blend(self.text, self.background, 0.16)
    }

    /// A quiet raised surface for entries and buttons. Neutral, not accent:
    /// the accent belongs to the caret, the selection and the hairline only.
    pub fn surface(&self) -> String {
        blend(self.text, self.background, 0.06)
    }

    /// The row under the pointer. A hover is not a selection, so it stays
    /// neutral.
    pub fn hover(&self) -> String {
        blend(self.text, self.background, 0.10)
    }

    /// The row behind a selected item: the one place a list uses the accent.
    pub fn selection(&self) -> String {
        blend(self.accent, self.background, 0.20)
    }

    /// Selected text inside the editor and the entries.
    pub fn text_selection(&self) -> String {
        blend(self.accent, self.background, 0.30)
    }

    /// A paragraph outside the cursor, drawn at 35 % opacity.
    pub fn focus_text(&self) -> gdk::RGBA {
        rgba_color(self.text, 0.35)
    }
}

pub fn palette(settings: &Settings) -> Palette {
    match settings.theme {
        crate::settings::Theme::Paper => Palette::paper(),
        crate::settings::Theme::Night => Palette::night(),
    }
}

/// `#RRGGBB` to `(r, g, b)` in the 0..1 range Cairo wants.
pub fn rgb(hex: &str) -> (f64, f64, f64) {
    let (r, g, b) = parse_hex(hex);
    (
        f64::from(r) / 255.0,
        f64::from(g) / 255.0,
        f64::from(b) / 255.0,
    )
}

/// `over` drawn at `alpha` on top of `base`, as `#RRGGBB`. The result is a
/// solid colour so GTK and Pango both draw it the same way.
pub fn blend(over: &str, base: &str, alpha: f64) -> String {
    let (or, og, ob) = parse_hex(over);
    let (br, bg, bb) = parse_hex(base);
    let alpha = alpha.clamp(0.0, 1.0);
    let mix = |front: u8, back: u8| {
        (f64::from(front) * alpha + f64::from(back) * (1.0 - alpha)).round() as u8
    };
    format!("#{:02X}{:02X}{:02X}", mix(or, br), mix(og, bg), mix(ob, bb))
}

/// `rgba(r, g, b, alpha)` for a text tag that must stay translucent.
pub fn rgba_color(hex: &str, alpha: f64) -> gdk::RGBA {
    let (red, green, blue) = rgb(hex);
    gdk::RGBA::new(
        red as f32,
        green as f32,
        blue as f32,
        alpha.clamp(0.0, 1.0) as f32,
    )
}

fn parse_hex(hex: &str) -> (u8, u8, u8) {
    let hex = hex.trim_start_matches('#');
    let value = u32::from_str_radix(hex, 16).unwrap_or(0);
    (
        ((value >> 16) & 0xFF) as u8,
        ((value >> 8) & 0xFF) as u8,
        (value & 0xFF) as u8,
    )
}

pub fn css(settings: &Settings) -> String {
    let palette = palette(settings);
    let background = palette.background;
    let text = palette.text;
    let accent = palette.accent;
    let dim = palette.dim();
    let muted = palette.muted();
    let border = palette.border();
    let surface = palette.surface();
    let hover = palette.hover();
    let selection = palette.selection();
    let text_selection = palette.text_selection();
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
    /* Body text is measured in pixels (spec/03-ui.md): 19 px by default. */
    font-size: {size}px;
    /* The accent is the caret, and nothing else in the editor. */
    caret-color: {accent};
}}
/* One selection colour everywhere: no default GTK blue-grey in the editor,
   the Quotes search field or any list. */
selection, entry selection, textview selection, .stylos-editor selection {{
    background-color: {text_selection};
    color: {text};
}}
/* Typewriter scrolling makes the scrollbar unnecessary; keep the space calm. */
scrollbar {{
    opacity: 0;
    min-width: 0;
    min-height: 0;
}}
/* The hairline goal line along the bottom window edge. */
.goal-hairline {{
    background-color: transparent;
    min-height: 2px;
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
    background-color: {hover};
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
    background-color: {surface};
    color: {text};
    border-radius: 6px;
    padding: 6px 10px;
}}
.error-text {{
    color: {danger};
}}
.calm-button {{
    background-color: {surface};
    color: {text};
    border: 1px solid {border};
    border-radius: 6px;
    padding: 6px 14px;
}}
button {{
    background-color: {surface};
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
entry {{
    background-color: {surface};
    color: {text};
    border: 1px solid {border};
    border-radius: 6px;
    padding: 7px 11px;
    font-size: 13pt;
    caret-color: {accent};
}}
entry:focus, entry:focus-within {{
    border-color: {muted};
    outline-style: none;
    outline-width: 0;
    box-shadow: none;
}}
.setting-row {{
    padding: 8px 14px;
    border-radius: 7px;
}}
.setting-row:selected {{
    background-color: {selection};
}}
.setting-label {{
    color: {text};
    font-size: 14pt;
}}
.setting-value {{
    color: {muted};
    font-size: 12pt;
}}
.section-title {{
    color: {muted};
    font-size: 12pt;
    margin-bottom: 6px;
}}
.step-title {{
    color: {text};
    font-size: 18pt;
    margin-top: 20px;
    margin-bottom: 6px;
}}
.sync-progress {{
    color: {text};
    font-size: 13pt;
    padding: 6px 0 0 0;
}}
.preview-title {{
    color: {muted};
    font-size: 10pt;
}}
.preview-text {{
    color: {text};
    font-size: 12pt;
}}
.preview-scroller {{
    border-left: 1px solid {border};
}}
.notice-line {{
    color: {muted};
    font-size: 10pt;
    padding: 8px 2px 2px 2px;
}}
.notice-ok {{
    color: {text};
}}
.done-heading {{
    color: {text};
    font-size: 18pt;
    margin-top: 14px;
}}
.battery-note {{
    color: {text};
    background-color: {surface};
    border-radius: 7px;
    font-size: 10pt;
    padding: 5px 14px;
}}
.pairing-code {{
    color: {text};
    font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace;
    font-size: 30pt;
}}
.pairing-url {{
    color: {muted};
    font-size: 11pt;
}}
.wifi-signal {{
    color: {dim};
    font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace;
    font-size: 11pt;
}}
.wifi-signal-off {{
    color: {border};
    font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace;
    font-size: 11pt;
}}
.history-time {{
    color: {text};
    font-size: 13pt;
}}
.quote-search {{
    font-size: 14pt;
}}
.filter-chip {{
    background-color: transparent;
    color: {muted};
    border: 1px solid {border};
    border-radius: 14px;
    padding: 3px 12px;
    font-size: 10pt;
}}
.filter-chip:hover {{
    background-color: {hover};
}}
.filter-chip-active {{
    color: {text};
    border-color: {muted};
}}
.quote-list {{
    background-color: transparent;
}}
.quote-row {{
    padding: 10px 14px;
    border-radius: 7px;
}}
.quote-row:hover {{
    background-color: {hover};
}}
.quote-row:selected {{
    background-color: {selection};
}}
.quote-text {{
    color: {text};
    font-size: 13pt;
}}
.quote-meta {{
    color: {dim};
    font-size: 10pt;
}}
.quote-message {{
    color: {muted};
    font-size: 12pt;
    padding: 10px 2px 2px 2px;
}}
/* A QR code needs a light quiet zone on both themes to scan. */
.qr {{
    background-color: #FBF7EF;
    padding: 10px;
    border-radius: 6px;
}}
"#,
        background = background,
        text = text,
        accent = accent,
        danger = palette.danger,
        dim = dim,
        muted = muted,
        border = border,
        surface = surface,
        hover = hover,
        selection = selection,
        text_selection = text_selection,
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
    fn palette_is_exactly_the_specs_colours() {
        let paper = Palette::paper();
        assert_eq!(paper.background, "#F6F4EE");
        assert_eq!(paper.text, "#262523");
        assert_eq!(paper.accent, "#C85A32");

        let night = Palette::night();
        assert_eq!(night.background, "#1E1C1A");
        assert_eq!(night.text, "#DCDAD5");
        assert_eq!(night.accent, "#E5A93B");
    }

    #[test]
    fn dim_text_is_ink_at_fifty_five_percent() {
        assert_eq!(blend("#262523", "#F6F4EE", 0.55), "#84827E");
        assert_eq!(blend("#DCDAD5", "#1E1C1A", 0.55), "#878581");
        assert_eq!(Palette::paper().dim(), "#84827E");
        assert_eq!(Palette::night().dim(), "#878581");
    }

    #[test]
    fn the_accent_is_only_the_caret_the_selection_and_the_hairline() {
        let sheet = css(&settings_for(Theme::Paper, FontChoice::Serif));
        assert!(sheet.contains("caret-color: #C85A32"));
        let accent_lines: Vec<&str> = sheet
            .lines()
            .filter(|line| line.to_ascii_uppercase().contains("#C85A32"))
            .collect();
        assert_eq!(accent_lines.len(), 2, "editor caret and entry carets");
        for line in accent_lines {
            assert!(
                line.trim_start().starts_with("caret-color:"),
                "the accent hex must never colour anything but a caret: {line}"
            );
        }
        assert!(!sheet.contains("caret-color: #262523"));
        // A selected row and selected text are mixed from the accent, never
        // a plain neutral or the theme's own selection colour.
        assert!(sheet.contains("#EDD5C8"));
        assert!(sheet.contains("#E8C6B6"));
    }

    #[test]
    fn themes_are_warm_paper_and_dark_grey_not_black_or_white() {
        for theme in [Theme::Paper, Theme::Night] {
            let sheet = css(&settings_for(theme, FontChoice::Serif)).to_ascii_lowercase();
            assert!(!sheet.contains("#000000"));
            assert!(!sheet.contains("#ffffff"));
            assert!(sheet.contains("font-size: 19px"));
            assert!(sheet.contains("dejavu serif"));
        }
        assert!(css(&settings_for(Theme::Night, FontChoice::Serif))
            .contains("background-color: #1E1C1A"));
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

    #[test]
    fn focus_text_is_ink_at_thirty_five_percent() {
        let colour = Palette::paper().focus_text();
        assert!((colour.red() - 38.0 / 255.0).abs() < 1e-6);
        assert!((colour.green() - 37.0 / 255.0).abs() < 1e-6);
        assert!((colour.blue() - 35.0 / 255.0).abs() < 1e-6);
        assert!((colour.alpha() - 0.35).abs() < 1e-6);
        assert_eq!(blend("#FFFFFF", "#000000", 0.5), "#808080");
        assert_eq!(rgb("#000000"), (0.0, 0.0, 0.0));
        let (r, g, b) = rgb("#E5A93B");
        assert!((r - 229.0 / 255.0).abs() < f64::EPSILON);
        assert!((g - 169.0 / 255.0).abs() < f64::EPSILON);
        assert!((b - 59.0 / 255.0).abs() < f64::EPSILON);
    }
}
