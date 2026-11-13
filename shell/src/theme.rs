//! Themes and the single CSS provider shared by every screen.
//!
//! Six palettes, straight from `spec/03-ui.md`: the five Nostos themes (Light,
//! Sepia, Mist, Writer's Study, Dark) so the two products feel like one
//! ecosystem, plus a High contrast theme. Each one names its paper, ink,
//! accent and border; dim text and the other quiet shades are mixed from them,
//! so there is no second accent and no pure black.

use gtk::gdk;
use gtk::glib;
use gtk::CssProvider;

use crate::settings::{Settings, Theme};

/// The one type scale for the whole interface, in pixels. Body rows follow
/// the editor's body size (spec/03-ui.md: 19 px default, never under 16), so
/// the screen and the page read at the same size. Nothing on screen is smaller
/// than a hint; the hint is the floor.
pub const TITLE_PX: u32 = 30;
pub const SECTION_PX: u32 = 15;
pub const STEP_PX: u32 = 23;
pub const SECONDARY_PX: u32 = 15;
pub const HINT_PX: u32 = 14;
pub const PREVIEW_PX: u32 = 16;
pub const MIN_ROW_PX: u32 = 16;
pub const BOTTOM_LINE_PX: u32 = 14;
pub const BUTTON_PX: u32 = 15;

/// The colours a theme names out loud. The rest is mixed from them.
#[derive(Clone, Copy)]
pub struct Palette {
    pub background: &'static str,
    pub text: &'static str,
    pub accent: &'static str,
    pub danger: &'static str,
    /// The hairline between surfaces, from the theme's own border token.
    pub border: &'static str,
}

impl Palette {
    /// Nostos Light: the Writing Studio's white page and obsidian ink, with
    /// the forest accent.
    pub fn light() -> Self {
        Self {
            background: "#FFFFFF",
            text: "#121316",
            accent: "#293E32",
            danger: "#B3261E",
            border: "#E5E7EB",
        }
    }

    /// Nostos Sepia: parchment paper, brown-black ink, the same forest accent.
    pub fn sepia() -> Self {
        Self {
            background: "#FBF4E5",
            text: "#2B2118",
            accent: "#293E32",
            danger: "#A8372A",
            border: "#E6DFD3",
        }
    }

    /// Nostos Mist: cool grey paper with slate-blue ink and accent.
    pub fn mist() -> Self {
        Self {
            background: "#1E2026",
            text: "#C8CCD4",
            accent: "#8494B0",
            danger: "#E39A9A",
            border: "#33363D",
        }
    }

    /// Nostos Writer's Study: a dark room, warm paper and an amber accent.
    pub fn study() -> Self {
        Self {
            background: "#1A1814",
            text: "#E4D5BF",
            accent: "#C9953E",
            danger: "#E3A08A",
            border: "#3A342C",
        }
    }

    /// Nostos Dark: near-black paper with silver ink and a mint accent.
    pub fn dark() -> Self {
        Self {
            background: "#121318",
            text: "#F0F1F4",
            accent: "#8FBFAE",
            danger: "#FFB4AB",
            border: "#2A2D37",
        }
    }

    /// High contrast: near-black on white, a strong blue accent and visible
    /// control edges. Body text is 18.9:1, dim text 5.9:1.
    pub fn high_contrast() -> Self {
        Self {
            background: "#FFFFFF",
            text: "#111111",
            accent: "#0043CE",
            danger: "#B00020",
            border: "#767676",
        }
    }

    pub fn of(theme: Theme) -> Self {
        match theme {
            Theme::Light => Self::light(),
            Theme::Sepia => Self::sepia(),
            Theme::Mist => Self::mist(),
            Theme::Study => Self::study(),
            Theme::Dark => Self::dark(),
            Theme::HighContrast => Self::high_contrast(),
        }
    }

    /// Markdown syntax and other quiet text: the ink stepped toward the paper
    /// until it clears the 4.5:1 text minimum (WCAG AA), with a little margin
    /// so rounding never leaves it on the floor. 55 % where the paper allows
    /// it, a little more on light paper.
    pub fn dim(&self) -> String {
        let mut alpha = 0.55;
        let mut colour = blend(self.text, self.background, alpha);
        while contrast_ratio(&colour, self.background) < DIM_CONTRAST_TARGET && alpha < 1.0 {
            alpha += 0.01;
            colour = blend(self.text, self.background, alpha);
        }
        colour
    }

    /// Secondary interface text: the bottom line, hints, values.
    pub fn muted(&self) -> String {
        blend(self.text, self.background, 0.72)
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

    /// A hint under a selected row: the ink mixed onto the row's own accent
    /// tint, so the smaller text still clears 4.5:1 there.
    pub fn selected_note(&self) -> String {
        blend(self.text, &self.selection(), 0.72)
    }

    /// The ring a focused field draws. A neutral step of the ink, never the
    /// accent, and always at least 3:1 against the paper so the edge of a
    /// control is findable.
    pub fn focus_ring(&self) -> String {
        blend(self.text, self.background, 0.70)
    }

    /// The translucent layer under a confirmation card: the theme's ink at
    /// 27 %, so the page is still the page and the card carries the weight.
    pub fn scrim(&self) -> String {
        rgba_css(self.text, 0.27)
    }

    /// The soft shadow under a confirmation card. On a day theme it is a
    /// little ink; on a night theme it is the dark paper.
    pub fn shadow(&self) -> String {
        if self.is_dark() {
            rgba_css(self.background, 0.72)
        } else {
            rgba_css(self.text, 0.18)
        }
    }

    /// Whether this palette is a night theme.
    pub fn is_dark(&self) -> bool {
        luminance(self.background) < 0.5
    }

    /// A paragraph outside the cursor, drawn at 35 % opacity.
    pub fn focus_text(&self) -> gdk::RGBA {
        rgba_color(self.text, 0.35)
    }
}

/// The palette on screen now. Automatic resolves to its day or night theme
/// for the local hour; the app re-applies it when the hour turns.
pub fn palette(settings: &Settings) -> Palette {
    Palette::of(settings.theme_at(local_hour()))
}

/// Whether GTK should be told the app is dark: true when the palette on
/// screen is a night theme.
pub fn prefers_dark(settings: &Settings) -> bool {
    settings.theme_at(local_hour()).is_dark()
}

/// Local time on the device, 0–23. Carrel has no OS preference, so Automatic
/// follows this instead (spec/03-ui.md). Noon when even the clock is unknown,
/// because a day theme is the calmer mistake.
pub fn local_hour() -> u8 {
    glib::DateTime::now_local()
        .map(|now| now.hour().clamp(0, 23) as u8)
        .unwrap_or(12)
}

/// The WCAG AA minimum for body text. Dim text aims a little above it so
/// rounding never leaves it exactly on the floor.
const DIM_CONTRAST_TARGET: f64 = 4.6;

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

/// `rgba(r, g, b, alpha)` for GTK CSS: the dim layer under a dialog, where
/// the page must still show through.
pub fn rgba_css(hex: &str, alpha: f64) -> String {
    let (red, green, blue) = parse_hex(hex);
    format!("rgba({red}, {green}, {blue}, {:.2})", alpha.clamp(0.0, 1.0))
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

/// WCAG 2.1 contrast between two solid colours, from 1:1 to 21:1.
pub fn contrast_ratio(first: &str, second: &str) -> f64 {
    let (high, low) = {
        let a = luminance(first);
        let b = luminance(second);
        if a >= b {
            (a, b)
        } else {
            (b, a)
        }
    };
    (high + 0.05) / (low + 0.05)
}

/// WCAG relative luminance: 0 for black, 1 for white.
fn luminance(hex: &str) -> f64 {
    let channel = |value: u8| {
        let value = f64::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let (red, green, blue) = parse_hex(hex);
    0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue)
}

pub fn css(settings: &Settings) -> String {
    let palette = palette(settings);
    let background = palette.background;
    let text = palette.text;
    let accent = palette.accent;
    let dim = palette.dim();
    let muted = palette.muted();
    let border = palette.border;
    let surface = palette.surface();
    let hover = palette.hover();
    let selection = palette.selection();
    let text_selection = palette.text_selection();
    let selected_note = palette.selected_note();
    let focus = palette.focus_ring();
    let scrim = palette.scrim();
    let shadow = palette.shadow();
    let overlay_dim = rgba_css(background, 0.60);
    let background_clear = rgba_css(background, 0.0);
    let family = settings.font.css_family();
    let size = settings.font_size;
    // A row reads at the editor's body size, never below the scale's floor.
    let row = size.max(MIN_ROW_PX);

    format!(
        r#"
window, .carrel-page {{
    background-color: {background};
    color: {text};
    font-family: {family};
}}
.carrel-editor, .carrel-editor text {{
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
selection, entry selection, entry:backdrop selection,
textview selection, .carrel-editor selection {{
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
.bottom-line-row {{
    padding: 8px 24px 14px 24px;
}}
.bottom-line {{
    color: {muted};
    font-size: {bottom_line}px;
    padding: 0;
}}
/* The bottom line ends with the palette's key, quiet and dim. */
.bottom-cap {{
    color: {dim};
    border: 1px solid {border};
    border-radius: 5px;
    padding: 0 6px;
    font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace;
    font-size: {hint}px;
}}
/* The one line an empty document shows, exactly where the text will begin. */
.start-hint {{
    color: {dim};
    font-size: {size}px;
}}
/* The command palette: the Quotes palette's sheet, centred near the top of
   the editor column. It carries the shared card shadow, and the page behind
   it is drawn at about 40 % (paper over the page), like focus mode. */
.overlay-dim {{
    background-color: {overlay_dim};
}}
.command-palette {{
    background-color: {background};
    color: {text};
    border: 1px solid {border};
    box-shadow: 0 18px 48px {shadow};
}}
.command-query-row {{
    padding: 14px 16px 12px 16px;
    border-bottom: 1px solid {border};
}}
.command-query {{
    color: {text};
    font-size: {size}px;
}}
.command-query.command-placeholder {{
    color: {dim};
}}
.command-list {{
    background-color: transparent;
}}
.command-row {{
    padding: 9px 14px;
    border-radius: 6px;
}}
.command-name {{
    color: {text};
    font-size: {row}px;
}}
.command-message {{
    color: {muted};
    font-size: {secondary}px;
    padding: 16px;
}}
.hint-bar.command-hint-bar {{
    padding: 9px 16px 10px 16px;
    border-top: 1px solid {border};
}}
/* The key sheet: one centred card, grouped, with the same key caps. */
.key-sheet {{
    background-color: {background};
    color: {text};
    border: 1px solid {border};
    border-radius: 12px;
    padding: 26px 30px 22px 30px;
    box-shadow: 0 18px 48px {shadow};
}}
.key-sheet-title {{
    color: {text};
    font-size: {step}px;
    font-weight: 600;
}}
.key-group {{
    color: {muted};
    font-size: {section}px;
}}
.key-word {{
    color: {text};
    font-size: {secondary}px;
}}
.screen-title {{
    color: {text};
    font-size: {title}px;
    font-weight: 600;
}}
.screen-status {{
    color: {muted};
    font-size: {secondary}px;
    margin-top: 8px;
}}
.section-title {{
    color: {muted};
    font-size: {section}px;
    margin-bottom: 2px;
}}
.step-title {{
    color: {text};
    font-size: {step}px;
    margin-bottom: 4px;
}}
/* One row style everywhere: same height, padding, radius and accent tint. */
.file-list {{
    background-color: transparent;
}}
/* A scrolled list fades into the page instead of cutting a row in half. */
.scroll-fade-top {{
    background-image: linear-gradient(to bottom, {background} 0%, {background_clear} 100%);
}}
.file-row, .setting-row, .quote-row, .ui-row {{
    padding: 10px 14px;
    border-radius: 8px;
    color: {text};
    font-size: {row}px;
}}
.file-row:hover, .setting-row:hover, .quote-row:hover, .ui-row:hover {{
    background-color: {hover};
}}
.file-row:selected, .setting-row:selected, .quote-row:selected, .ui-row:selected {{
    background-color: {selection};
    color: {text};
}}
.folder-name {{
    font-weight: 600;
}}
.file-name {{
    font-weight: 400;
}}
.setting-label {{
    color: {text};
    font-size: {row}px;
}}
.setting-value {{
    color: {muted};
    font-size: {secondary}px;
}}
.row-note {{
    color: {dim};
    font-size: {secondary}px;
}}
/* A hint under the selected row sits on that row's tint, so it takes a
   little more ink to stay readable. */
.file-row:selected .row-note,
.setting-row:selected .row-note,
.quote-row:selected .quote-meta,
.ui-row:selected .row-note {{
    color: {selected_note};
}}
/* Rename in place: the row itself becomes a field. */
.renaming {{
    background-color: {surface};
    color: {text};
    border: 1px solid {border};
    border-radius: 6px;
    padding: 2px 8px;
    font-size: {row}px;
}}
.error-text {{
    color: {danger};
}}
.calm-button {{
    background-color: {surface};
    color: {text};
    border: 1px solid {border};
    border-radius: 6px;
    padding: 7px 16px;
    font-size: {button}px;
}}
button {{
    background-color: {surface};
    color: {text};
    border: 1px solid {border};
    border-radius: 6px;
    box-shadow: none;
    padding: 7px 18px;
    font-size: {button}px;
}}
button:hover {{
    background-color: {hover};
}}
button:focus {{
    border-color: {focus};
    box-shadow: inset 0 0 0 1px {focus};
    outline-style: none;
    outline-width: 0;
}}
entry {{
    background-color: {surface};
    color: {text};
    border: 1px solid {border};
    border-radius: 6px;
    padding: 8px 12px;
    font-size: {row}px;
    caret-color: {accent};
}}
entry:focus, entry:focus-within {{
    /* A focused field draws a visible ring, neutral and never the accent. */
    border-color: {focus};
    box-shadow: inset 0 0 0 1px {focus};
    outline-style: none;
    outline-width: 0;
}}
/* A confirmation is one card over a dimmed page, never a native strip. */
.dialog-scrim {{
    background-color: {scrim};
}}
.dialog-card {{
    background-color: {background};
    color: {text};
    border: 1px solid {border};
    border-radius: 12px;
    padding: 26px 28px 22px 28px;
    box-shadow: 0 18px 48px {shadow};
}}
.dialog-title {{
    color: {text};
    font-size: {step}px;
    font-weight: 600;
}}
.dialog-text {{
    color: {muted};
    font-size: {secondary}px;
}}
/* Key caps: the one hint bar, on every screen. */
.hint-bar {{
    padding: 0;
}}
.key-cap {{
    background-color: {surface};
    color: {text};
    border: 1px solid {border};
    border-radius: 5px;
    padding: 1px 7px;
    font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace;
    font-size: {hint}px;
}}
.hint-word {{
    color: {muted};
    font-size: {hint}px;
}}
.hint-slot {{
    border-top: 1px solid {border};
    padding-top: 14px;
}}
.status-line {{
    color: {muted};
    font-size: {secondary}px;
}}
.sync-progress {{
    color: {text};
    font-size: {secondary}px;
}}
.preview-title {{
    color: {muted};
    font-size: {hint}px;
}}
.preview-text {{
    color: {text};
    font-size: {preview}px;
}}
.preview-scroller {{
    border-left: 1px solid {border};
}}
.notice-line {{
    color: {muted};
    font-size: {secondary}px;
    padding: 8px 2px 2px 2px;
}}
.notice-ok {{
    color: {text};
}}
.done-heading {{
    color: {text};
    font-size: {step}px;
    margin-top: 6px;
}}
.battery-note {{
    color: {text};
    background-color: {surface};
    border-radius: 7px;
    font-size: {hint}px;
    padding: 5px 14px;
}}
.pairing-code {{
    color: {text};
    font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace;
    font-size: 36px;
}}
.pairing-url {{
    color: {muted};
    font-size: {secondary}px;
}}
.wifi-signal, .wifi-signal-off {{
    font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace;
    font-size: {hint}px;
}}
.wifi-signal {{
    color: {dim};
}}
.wifi-signal-off {{
    color: {border};
}}
.history-time {{
    color: {text};
    font-size: {row}px;
}}
.history-delta {{
    color: {muted};
    font-size: {secondary}px;
}}
.quote-list {{
    background-color: transparent;
}}
.quote-text {{
    color: {text};
    font-size: {row}px;
}}
.quote-meta {{
    color: {dim};
    font-size: {secondary}px;
}}
.quote-message {{
    color: {muted};
    font-size: {preview}px;
    padding: 12px 16px;
}}
.quote-heading {{
    color: {dim};
    font-size: {hint}px;
    padding: 10px 16px 2px 16px;
}}
.quote-hint-bar {{
    padding: 9px 16px 10px 16px;
    border-top: 1px solid {border};
}}
/* The palette at the caret: a quiet raised sheet. The accent stays on the
   selected row only. */
popover.quote-popover > contents {{
    background-color: {background};
    color: {text};
    border: 1px solid {border};
    border-radius: 0;
    padding: 0;
    box-shadow: none;
}}
/* A QR code needs a white quiet zone on every theme to scan. */
.qr {{
    background-color: #FFFFFF;
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
        selected_note = selected_note,
        focus = focus,
        scrim = scrim,
        shadow = shadow,
        overlay_dim = overlay_dim,
        background_clear = background_clear,
        family = family,
        size = size,
        row = row,
        title = TITLE_PX,
        section = SECTION_PX,
        step = STEP_PX,
        secondary = SECONDARY_PX,
        hint = HINT_PX,
        preview = PREVIEW_PX,
        button = BUTTON_PX,
        bottom_line = BOTTOM_LINE_PX,
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
    use crate::settings::{FontChoice, Settings, Theme, ThemeChoice};

    fn settings_for(theme: Theme, font: FontChoice) -> Settings {
        Settings {
            theme: theme.into(),
            font,
            ..Settings::default()
        }
    }

    #[test]
    fn every_theme_keeps_body_text_at_seven_and_dim_text_at_four_point_five() {
        for theme in Theme::ALL {
            let palette = Palette::of(theme);
            let name = theme.label();
            let body = contrast_ratio(palette.text, palette.background);
            let dim = contrast_ratio(&palette.dim(), palette.background);
            let accent = contrast_ratio(palette.accent, palette.background);
            let danger = contrast_ratio(palette.danger, palette.background);
            let ring = contrast_ratio(&palette.focus_ring(), palette.background);
            let hint = contrast_ratio(&palette.selected_note(), &palette.selection());
            assert!(body >= 7.0, "{name}: body text is only {body:.2}:1");
            assert!(dim >= 4.5, "{name}: dim text is only {dim:.2}:1");
            assert!(accent >= 3.0, "{name}: the accent is only {accent:.2}:1");
            assert!(danger >= 4.5, "{name}: error text is only {danger:.2}:1");
            assert!(ring >= 3.0, "{name}: the focus ring is only {ring:.2}:1");
            assert!(
                hint >= 4.5,
                "{name}: a hint on a selected row is only {hint:.2}:1"
            );
        }
    }

    #[test]
    fn high_contrast_is_near_black_on_white() {
        let palette = Palette::high_contrast();
        assert_eq!(palette.background, "#FFFFFF");
        assert_eq!(palette.text, "#111111");
        assert!(contrast_ratio(palette.text, palette.background) >= 15.0);
        assert!(!Theme::HighContrast.is_dark());
        // A clearly visible accent and control edge on white.
        assert!(contrast_ratio(palette.accent, palette.background) >= 4.5);
        assert!(contrast_ratio(palette.border, palette.background) >= 3.0);
        assert!(contrast_ratio(&palette.focus_ring(), palette.background) >= 3.0);
    }

    #[test]
    fn palettes_are_exactly_the_nostos_themes() {
        let light = Palette::light();
        assert_eq!(light.background, "#FFFFFF");
        assert_eq!(light.text, "#121316");
        assert_eq!(light.accent, "#293E32");

        let sepia = Palette::sepia();
        assert_eq!(sepia.background, "#FBF4E5");
        assert_eq!(sepia.text, "#2B2118");
        assert_eq!(sepia.accent, "#293E32");

        let mist = Palette::mist();
        assert_eq!(mist.background, "#1E2026");
        assert_eq!(mist.text, "#C8CCD4");
        assert_eq!(mist.accent, "#8494B0");

        let study = Palette::study();
        assert_eq!(study.background, "#1A1814");
        assert_eq!(study.text, "#E4D5BF");
        assert_eq!(study.accent, "#C9953E");

        let dark = Palette::dark();
        assert_eq!(dark.background, "#121318");
        assert_eq!(dark.text, "#F0F1F4");
        assert_eq!(dark.accent, "#8FBFAE");
    }

    #[test]
    fn dim_text_steps_toward_the_ink_only_as_far_as_the_floor_needs() {
        // A dark paper allows the plain 55 %.
        assert_eq!(Palette::dark().dim(), blend("#F0F1F4", "#121318", 0.55));
        // Every paper ends up above the WCAG AA floor, even the tight ones.
        for theme in Theme::ALL {
            let palette = Palette::of(theme);
            assert!(
                contrast_ratio(&palette.dim(), palette.background) >= 4.5,
                "{}: dim text is {}",
                theme.label(),
                palette.dim()
            );
        }
        assert_eq!(blend("#FFFFFF", "#000000", 0.5), "#808080");
    }

    #[test]
    fn the_accent_is_only_the_caret_the_selection_and_the_hairline() {
        for theme in Theme::ALL {
            let palette = Palette::of(theme);
            let sheet = css(&settings_for(theme, FontChoice::Serif));
            assert!(sheet.contains(&format!("caret-color: {}", palette.accent)));
            let accent_lines: Vec<&str> = sheet
                .lines()
                .filter(|line| line.to_ascii_uppercase().contains(palette.accent))
                .collect();
            assert_eq!(
                accent_lines.len(),
                2,
                "{}: editor caret and entry caret",
                theme.label()
            );
            for line in accent_lines {
                assert!(
                    line.trim_start().starts_with("caret-color:"),
                    "the accent hex must never colour anything but a caret: {line}"
                );
            }
            assert!(!sheet.contains(&format!("caret-color: {}", palette.text)));
            // A selected row and selected text are mixed from the accent, never
            // a plain neutral or another theme's colour.
            assert!(sheet.contains(&palette.selection()));
            assert!(sheet.contains(&palette.text_selection()));
        }
    }

    #[test]
    fn rows_dialogs_and_key_caps_stay_readable_in_every_theme() {
        for theme in Theme::ALL {
            let palette = Palette::of(theme);
            let surface = palette.surface();
            let selection = palette.selection();
            let name = theme.label();
            let row = contrast_ratio(palette.text, &surface);
            let quiet = contrast_ratio(&palette.muted(), &surface);
            let selected = contrast_ratio(palette.text, &selection);
            assert!(row >= 7.0, "{name}: text on a raised surface is {row:.2}:1");
            assert!(
                quiet >= 4.5,
                "{name}: secondary text on a card is {quiet:.2}:1"
            );
            assert!(
                selected >= 7.0,
                "{name}: a row's text on its selection is {selected:.2}:1"
            );
            assert!(palette.scrim().starts_with("rgba("));
        }
    }

    #[test]
    fn the_dialog_is_paper_over_light_ink() {
        for theme in Theme::ALL {
            let palette = Palette::of(theme);
            assert_eq!(palette.scrim(), rgba_css(palette.text, 0.27));
            let sheet = css(&settings_for(theme, FontChoice::Serif));
            assert!(sheet.contains(&format!("box-shadow: 0 18px 48px {}", palette.shadow())));
            assert!(sheet.contains(".scroll-fade-top"));
        }
    }

    #[test]
    fn no_ui_text_is_below_the_hint_floor() {
        for theme in Theme::ALL {
            let sheet = css(&settings_for(theme, FontChoice::Serif));
            for line in sheet.lines() {
                let Some(value) = line.trim().strip_prefix("font-size: ") else {
                    continue;
                };
                let Some(px) = value.strip_suffix("px;") else {
                    continue;
                };
                let px: u32 = px.parse().expect("a pixel size");
                assert!(
                    px >= 13,
                    "{}: the stylesheet asks for {px} px text",
                    theme.label()
                );
            }
        }
    }

    #[test]
    fn the_stylesheet_carries_each_theme_whole() {
        for theme in Theme::ALL {
            let sheet = css(&settings_for(theme, FontChoice::Serif)).to_ascii_lowercase();
            assert!(
                !sheet.contains("#000000"),
                "{}: no pure black",
                theme.label()
            );
            assert!(sheet.contains("font-size: 19px"));
            assert!(sheet.contains("dejavu serif"));
            assert!(sheet.contains(&Palette::of(theme).background.to_ascii_lowercase()));
            assert!(sheet.contains(&Palette::of(theme).border.to_ascii_lowercase()));
        }
        assert!(css(&settings_for(Theme::HighContrast, FontChoice::Serif))
            .contains("background-color: #FFFFFF"));
    }

    #[test]
    fn a_fixed_choice_ignores_the_hour_and_automatic_does_not() {
        let fixed = Settings {
            theme: ThemeChoice::Mist,
            ..Settings::default()
        };
        assert_eq!(palette(&fixed).background, "#1E2026");
        let automatic = Settings::default();
        assert_eq!(
            Palette::of(automatic.theme_at(12)).background,
            "#FFFFFF",
            "the default day theme is Light"
        );
        assert_eq!(
            Palette::of(automatic.theme_at(22)).background,
            "#121318",
            "the default night theme is Dark"
        );
    }

    #[test]
    fn the_three_fonts_are_distinct_families() {
        let serif = css(&settings_for(Theme::Light, FontChoice::Serif));
        let sans = css(&settings_for(Theme::Light, FontChoice::Sans));
        let mono = css(&settings_for(Theme::Light, FontChoice::Mono));
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
        let colour = Palette::light().focus_text();
        assert!((colour.red() - 18.0 / 255.0).abs() < 1e-6);
        assert!((colour.green() - 19.0 / 255.0).abs() < 1e-6);
        assert!((colour.blue() - 22.0 / 255.0).abs() < 1e-6);
        assert!((colour.alpha() - 0.35).abs() < 1e-6);
        assert_eq!(rgb("#000000"), (0.0, 0.0, 0.0));
        let (r, g, b) = rgb("#8FBFAE");
        assert!((r - 143.0 / 255.0).abs() < f64::EPSILON);
        assert!((g - 191.0 / 255.0).abs() < f64::EPSILON);
        assert!((b - 174.0 / 255.0).abs() < f64::EPSILON);
    }
}
