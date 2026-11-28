//! One key dispatcher for the window and the UI tests.
//!
//! The screens never depend on which widget has focus: a window-level capture
//! controller sends every key to the visible page. The tests drive the same
//! entry point, so a counted key press and a real one take the same path.
//!
//! `CARREL_SHOT_KEYS` uses it too. The screenshot script names a few keys
//! (`Down,Down,Return`) to reach a state that only the keyboard opens; the
//! keys are dispatched through [`Keys::handle`] as if the writer pressed them.

use std::rc::Rc;

use gtk::prelude::DisplayExtManual;
use gtk::{gdk, glib};

use crate::editor::EditorPage;
use crate::export::ExportPage;
use crate::files::FilesPage;
use crate::first_run::FirstRunPage;
use crate::history::HistoryPage;
use crate::nav::Navigator;
use crate::palette::{CommandPalette, KeySheet, RenameSheet};
use crate::quotes::QuotePalette;
use crate::settings_page::SettingsPage;
use crate::sync::SyncPage;
use crate::wifi::WifiPage;

/// Every page the dispatcher can reach, plus the stack that says which one is
/// visible.
pub struct Keys {
    stack: gtk::Stack,
    nav: Rc<Navigator>,
    editor: Rc<EditorPage>,
    files: Rc<FilesPage>,
    history: Rc<HistoryPage>,
    quotes: Rc<QuotePalette>,
    sync: Rc<SyncPage>,
    settings_page: Rc<SettingsPage>,
    wifi: Rc<WifiPage>,
    export: Rc<ExportPage>,
    first_run: Rc<FirstRunPage>,
    palette: Rc<CommandPalette>,
    key_sheet: Rc<KeySheet>,
    rename_sheet: Rc<RenameSheet>,
}

impl Keys {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        stack: gtk::Stack,
        nav: Rc<Navigator>,
        editor: Rc<EditorPage>,
        files: Rc<FilesPage>,
        history: Rc<HistoryPage>,
        quotes: Rc<QuotePalette>,
        sync: Rc<SyncPage>,
        settings_page: Rc<SettingsPage>,
        wifi: Rc<WifiPage>,
        export: Rc<ExportPage>,
        first_run: Rc<FirstRunPage>,
        palette: Rc<CommandPalette>,
        key_sheet: Rc<KeySheet>,
        rename_sheet: Rc<RenameSheet>,
    ) -> Rc<Self> {
        Rc::new(Self {
            stack,
            nav,
            editor,
            files,
            history,
            quotes,
            sync,
            settings_page,
            wifi,
            export,
            first_run,
            palette,
            key_sheet,
            rename_sheet,
        })
    }

    /// The name of the visible page: `editor`, `files`, `first_run`, …
    pub fn visible(&self) -> String {
        self.stack
            .visible_child_name()
            .map(|name| name.to_string())
            .unwrap_or_default()
    }

    /// A confirmation card is modal: the palette waits until it is answered.
    fn dialog_open(&self) -> bool {
        match self.visible().as_str() {
            "files" => self.files.is_dialog_open(),
            "history" => self.history.is_dialog_open(),
            "sync" => self.sync.is_dialog_open(),
            "wifi" => self.wifi.is_dialog_open(),
            _ => false,
        }
    }

    pub fn handle(&self, keyval: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        self.handle_keycode(keyval, 0, state)
    }

    pub fn handle_keycode(
        &self,
        keyval: gdk::Key,
        keycode: u32,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        let visible = self.visible();
        let control = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let key = keyval.to_lower();

        // The key sheet is on top of everything until Esc. First run is a
        // closed flow: the palette and the sheet do not exist there.
        if self.key_sheet.handle_key(keyval, keycode, state) == glib::Propagation::Stop {
            return glib::Propagation::Stop;
        }
        if visible == "first_run" {
            return self.first_run.handle_key(keyval, state);
        }

        // While the rename card is open, Enter saves, Esc cancels, and all
        // other keys propagate straight to its focused Entry without firing
        // global shortcuts behind it.
        if self.rename_sheet.is_open() {
            return self.rename_sheet.handle_key(keyval, state);
        }

        // While the palette is open every key belongs to it, so no letter
        // reaches the editor or a screen behind it.
        if self.palette.handle_key(keyval, state) == glib::Propagation::Stop {
            return glib::Propagation::Stop;
        }

        let punctuation = punctuation_shortcut(keyval, keycode, state);

        // A modal confirmation card owns the keyboard until it is answered;
        // global chords must never switch screens behind an open dialog.
        if control && !self.dialog_open() {
            // Ctrl+K: the same palette from the editor and every screen.
            if key == gdk::Key::k {
                self.palette.open();
                return glib::Propagation::Stop;
            }
            // Ctrl+/: the key sheet. `?` stays a normal character in the
            // editor, so the sheet has a chord of its own.
            if punctuation == Some(PunctuationShortcut::KeySheet) {
                self.key_sheet.open();
                return glib::Propagation::Stop;
            }
            // The global keys from spec/03-ui.md.
            if key == gdk::Key::o && visible != "files" {
                self.nav.show("files");
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::h {
                self.nav.show("history");
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::s && shift {
                self.sync.sync_now();
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::r && shift {
                self.rename_sheet.open();
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::e {
                // Ctrl+E on the finished result returns to the three rows,
                // with the choices Carrel remembers, ready for Enter again.
                if visible == "export" {
                    self.export.reopen();
                }
                self.nav.show("export");
                return glib::Propagation::Stop;
            }
            if punctuation == Some(PunctuationShortcut::Settings) {
                self.nav.show("settings");
                return glib::Propagation::Stop;
            }
        }

        match visible.as_str() {
            "files" => self.files.handle_key(keyval, state),
            "history" => self.history.handle_key(keyval, state),
            "sync" => self.sync.handle_key(keyval, state),
            "settings" => self.settings_page.handle_key(keyval, state),
            "wifi" => self.wifi.handle_key(keyval, state),
            "export" => self.export.handle_key(keyval, state),
            // First run returns above; this arm is never reached.
            "first_run" => self.first_run.handle_key(keyval, state),
            // The Quotes palette (Ctrl+' and the `@` trigger) lives in the
            // editor, so it sees the editor's keys first.
            _ => match self.quotes.handle_key(keyval, keycode, state) {
                glib::Propagation::Stop => glib::Propagation::Stop,
                glib::Propagation::Proceed => self.editor.handle_key(keyval, keycode, state),
            },
        }
    }
}

/// Hardware keycodes (XKB = Linux evdev + 8) of the US punctuation keys and
/// keypad operators behind the shortcuts.
pub(crate) const KEYCODE_MINUS: u32 = 20; // KEY_MINUS (12) + 8
pub(crate) const KEYCODE_EQUAL: u32 = 21; // KEY_EQUAL (13) + 8
pub(crate) const KEYCODE_APOSTROPHE: u32 = 48; // KEY_APOSTROPHE (40) + 8
pub(crate) const KEYCODE_COMMA: u32 = 59; // KEY_COMMA (51) + 8
pub(crate) const KEYCODE_SLASH: u32 = 61; // KEY_SLASH (53) + 8
pub(crate) const KEYCODE_KP_SUBTRACT: u32 = 82; // KEY_KPMINUS (74) + 8
pub(crate) const KEYCODE_KP_ADD: u32 = 86; // KEY_KPPLUS (78) + 8
pub(crate) const KEYCODE_KP_DIVIDE: u32 = 106; // KEY_KPSLASH (98) + 8

/// Every `Ctrl` + punctuation shortcut. One matcher serves them all, so that a
/// symbol the layout really produces always beats another shortcut's hardware
/// position: on Swedish and German the US `/` key types `-`, so it zooms out,
/// and the key sheet is `Ctrl+Shift+7` (their `/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PunctuationShortcut {
    KeySheet,
    Settings,
    Quotes,
    ZoomIn,
    ZoomOut,
}

fn punctuation_for_keyval(keyval: gdk::Key) -> Option<PunctuationShortcut> {
    use PunctuationShortcut::*;
    match keyval {
        gdk::Key::slash | gdk::Key::KP_Divide => Some(KeySheet),
        gdk::Key::comma => Some(Settings),
        gdk::Key::apostrophe => Some(Quotes),
        gdk::Key::equal | gdk::Key::plus | gdk::Key::KP_Add | gdk::Key::KP_Equal => Some(ZoomIn),
        gdk::Key::minus | gdk::Key::underscore | gdk::Key::KP_Subtract => Some(ZoomOut),
        _ => None,
    }
}

fn punctuation_for_keycode(keycode: u32, keyval: gdk::Key) -> Option<PunctuationShortcut> {
    use PunctuationShortcut::*;
    // Dvorak and others put letters on the last three keys: `Ctrl+Z` on the
    // US `/` key must stay Undo.
    let letter = keyval.to_unicode().is_some_and(|c| c.is_ascii_alphabetic());
    match keycode {
        KEYCODE_EQUAL | KEYCODE_KP_ADD => Some(ZoomIn),
        KEYCODE_MINUS | KEYCODE_KP_SUBTRACT => Some(ZoomOut),
        KEYCODE_KP_DIVIDE => Some(KeySheet),
        KEYCODE_SLASH if !letter => Some(KeySheet),
        KEYCODE_COMMA if !letter => Some(Settings),
        KEYCODE_APOSTROPHE if !letter => Some(Quotes),
        _ => None,
    }
}

/// Match a `Ctrl` + punctuation shortcut, in this order:
///
/// 1. the layout-translated `keyval`, with or without Shift;
/// 2. with Shift held, `alt_keyval`: the other Shift level of the same key.
///    That is level 0 for a shifted symbol (US `Ctrl+Shift+/` arrives as
///    `?`, as does Swedish `Ctrl+Shift++`), and level 1 when the symbol
///    stayed unshifted (Swedish `Ctrl+Shift+7` arriving as `7`, its `/`);
/// 3. the hardware keycode, for layouts that type something else there (a
///    dead key, `ä`, `ß`).
pub(crate) fn match_punctuation(
    keyval: gdk::Key,
    keycode: u32,
    state: gdk::ModifierType,
    alt_keyval: Option<gdk::Key>,
) -> Option<PunctuationShortcut> {
    if !state.contains(gdk::ModifierType::CONTROL_MASK)
        || state.contains(gdk::ModifierType::ALT_MASK)
    {
        return None;
    }
    let alt = alt_keyval.filter(|_| state.contains(gdk::ModifierType::SHIFT_MASK));
    punctuation_for_keyval(keyval)
        .or_else(|| alt.and_then(punctuation_for_keyval))
        .or_else(|| punctuation_for_keycode(keycode, keyval))
}

/// [`match_punctuation`] with the other Shift level read from the display's keymap.
pub(crate) fn punctuation_shortcut(
    keyval: gdk::Key,
    keycode: u32,
    state: gdk::ModifierType,
) -> Option<PunctuationShortcut> {
    let alt = if state.contains(gdk::ModifierType::SHIFT_MASK) {
        shift_alternate_keyval(keycode, keyval)
    } else {
        None
    };
    match_punctuation(keyval, keycode, state, alt)
}

/// Look up the other Shift level of `keycode` in the layout group that
/// produced `keyval`: level 1 if `keyval` is the unshifted symbol, else level
/// 0. Matching the group ensures multi-layout sessions (or synthetic tests on
/// a US display) never substitute a different layout's symbol.
fn shift_alternate_keyval(keycode: u32, keyval: gdk::Key) -> Option<gdk::Key> {
    if keycode == 0 {
        return None;
    }
    let display = gdk::Display::default()?;
    let entries = display.map_keycode(keycode)?;
    let (matched, _) = entries.iter().find(|(_, candidate)| *candidate == keyval)?;
    let level = if matched.level() == 0 { 1 } else { 0 };
    entries
        .iter()
        .find(|(keymap_key, _)| {
            keymap_key.group() == matched.group() && keymap_key.level() == level
        })
        .map(|(_, key)| *key)
}

/// Parse the `CARREL_SHOT_KEYS` script: comma-separated key names, each with
/// optional `ctrl+`, `shift+` or `alt+` prefixes. An unknown name is skipped.
///
/// ```text
/// Down,Down,Return
/// ctrl+e,Return,Down,Down,Return
/// ```
pub fn parse_script(script: &str) -> Vec<(gdk::Key, gdk::ModifierType)> {
    let mut keys = Vec::new();
    for token in script.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let mut parts: Vec<&str> = token.split('+').collect();
        let mut state = gdk::ModifierType::empty();
        while parts.len() > 1 {
            let modifier = parts[0].to_ascii_lowercase();
            let flag = match modifier.as_str() {
                "ctrl" | "control" => gdk::ModifierType::CONTROL_MASK,
                "shift" => gdk::ModifierType::SHIFT_MASK,
                "alt" => gdk::ModifierType::ALT_MASK,
                _ => break,
            };
            state |= flag;
            parts.remove(0);
        }
        let name = parts.join("+");
        let key =
            gdk::Key::from_name(&name).or_else(|| gdk::Key::from_name(name.to_ascii_lowercase()));
        if let Some(key) = key {
            keys.push((key, state));
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::PunctuationShortcut::*;
    use super::*;

    const CTRL: gdk::ModifierType = gdk::ModifierType::CONTROL_MASK;
    const SHIFT: gdk::ModifierType = gdk::ModifierType::SHIFT_MASK;

    #[test]
    fn scripts_name_keys_and_chords() {
        let keys = parse_script(" Down, Down ,Return,ctrl+e,shift+ctrl+s ");
        assert_eq!(keys.len(), 5);
        assert_eq!(keys[0].0, gdk::Key::Down);
        assert_eq!(keys[1].0, gdk::Key::Down);
        assert_eq!(keys[2].0, gdk::Key::Return);
        assert_eq!(keys[3].0, gdk::Key::e);
        assert_eq!(keys[3].1, CTRL);
        assert_eq!(keys[4].0, gdk::Key::s);
        assert_eq!(keys[4].1, CTRL | SHIFT);
    }

    #[test]
    fn unknown_keys_are_skipped() {
        assert!(parse_script("Down,NotAKey,Return").len() == 2);
        assert!(parse_script("").is_empty());
    }

    /// `(keyval, keycode, shift held, level-0 keyval, expected)`.
    type Case = (
        gdk::Key,
        u32,
        bool,
        Option<gdk::Key>,
        Option<PunctuationShortcut>,
    );

    fn check(cases: &[Case]) {
        for &(keyval, keycode, shift, base, expected) in cases {
            let state = if shift { CTRL | SHIFT } else { CTRL };
            assert_eq!(
                match_punctuation(keyval, keycode, state, base),
                expected,
                "{keyval:?} on keycode {keycode}, shift {shift}, base {base:?}"
            );
        }
    }

    #[test]
    fn punctuation_matches_us_and_keypad_keyvals() {
        use gdk::Key;
        check(&[
            (Key::slash, KEYCODE_SLASH, false, None, Some(KeySheet)),
            (Key::comma, KEYCODE_COMMA, false, None, Some(Settings)),
            (
                Key::apostrophe,
                KEYCODE_APOSTROPHE,
                false,
                None,
                Some(Quotes),
            ),
            (Key::equal, KEYCODE_EQUAL, false, None, Some(ZoomIn)),
            (Key::plus, KEYCODE_EQUAL, true, None, Some(ZoomIn)),
            (Key::KP_Add, KEYCODE_KP_ADD, false, None, Some(ZoomIn)),
            (Key::KP_Add, KEYCODE_KP_ADD, true, None, Some(ZoomIn)),
            (Key::minus, KEYCODE_MINUS, false, None, Some(ZoomOut)),
            (Key::underscore, KEYCODE_MINUS, true, None, Some(ZoomOut)),
            (
                Key::KP_Subtract,
                KEYCODE_KP_SUBTRACT,
                false,
                None,
                Some(ZoomOut),
            ),
        ]);
    }

    #[test]
    fn shift_uses_the_level_zero_symbol_of_the_same_key() {
        use gdk::Key;
        check(&[
            // US `?`, `"` and `<`.
            (
                Key::question,
                KEYCODE_SLASH,
                true,
                Some(Key::slash),
                Some(KeySheet),
            ),
            (
                Key::quotedbl,
                KEYCODE_APOSTROPHE,
                true,
                Some(Key::apostrophe),
                Some(Quotes),
            ),
            (
                Key::less,
                KEYCODE_COMMA,
                true,
                Some(Key::comma),
                Some(Settings),
            ),
            // Swedish `Ctrl+Shift++` is `?` on the US `-` key: `+` wins over
            // that position's zoom out.
            (
                Key::question,
                KEYCODE_MINUS,
                true,
                Some(Key::plus),
                Some(ZoomIn),
            ),
            // Without Shift the base symbol is ignored.
            (Key::_7, 16, false, Some(Key::slash), None),
        ]);
    }

    #[test]
    fn keypad_slash_opens_the_key_sheet() {
        use gdk::Key;
        check(&[
            (
                Key::KP_Divide,
                KEYCODE_KP_DIVIDE,
                false,
                None,
                Some(KeySheet),
            ),
            (
                Key::KP_Divide,
                KEYCODE_KP_DIVIDE,
                true,
                None,
                Some(KeySheet),
            ),
            (
                Key::VoidSymbol,
                KEYCODE_KP_DIVIDE,
                false,
                None,
                Some(KeySheet),
            ),
        ]);
    }

    #[test]
    fn shift_uses_level_one_when_the_symbol_stays_unshifted() {
        use gdk::Key;
        check(&[
            // Swedish/German `Ctrl+Shift+7` arriving as `7`.
            (Key::_7, 16, true, Some(Key::slash), Some(KeySheet)),
            // A level 1 that is no shortcut changes nothing.
            (Key::_8, 17, true, Some(Key::parenleft), None),
        ]);
    }

    #[test]
    fn swedish_and_german_symbols_beat_us_positions() {
        use gdk::Key;
        check(&[
            // The US `/` key types `-` and `_`: zoom out, not the key sheet.
            (Key::minus, KEYCODE_SLASH, false, None, Some(ZoomOut)),
            (
                Key::underscore,
                KEYCODE_SLASH,
                true,
                Some(Key::minus),
                Some(ZoomOut),
            ),
            // `/` is Shift+7.
            (Key::slash, 16, true, Some(Key::_7), Some(KeySheet)),
            // `'` is next to Enter: plain on Swedish, shifted on German.
            (Key::apostrophe, 51, false, None, Some(Quotes)),
            (
                Key::apostrophe,
                51,
                true,
                Some(Key::numbersign),
                Some(Quotes),
            ),
            // `+` is on the US `-` key, `=` is Shift+0.
            (Key::plus, KEYCODE_MINUS, false, None, Some(ZoomIn)),
            (Key::equal, 19, true, Some(Key::_0), Some(ZoomIn)),
            // Spanish `'` and Turkish `-` on other US positions.
            (Key::apostrophe, KEYCODE_MINUS, false, None, Some(Quotes)),
            (Key::minus, KEYCODE_EQUAL, false, None, Some(ZoomOut)),
        ]);
    }

    #[test]
    fn other_symbols_fall_back_to_the_hardware_keycode() {
        use gdk::Key;
        check(&[
            // French `!` and `;`.
            (Key::exclam, KEYCODE_SLASH, false, None, Some(KeySheet)),
            (Key::semicolon, KEYCODE_COMMA, false, None, Some(Settings)),
            // Swedish `ä` and US-International dead acute.
            (
                Key::adiaeresis,
                KEYCODE_APOSTROPHE,
                false,
                None,
                Some(Quotes),
            ),
            (
                Key::dead_acute,
                KEYCODE_APOSTROPHE,
                false,
                None,
                Some(Quotes),
            ),
            // Swedish dead acute and German `ß`.
            (Key::dead_acute, KEYCODE_EQUAL, false, None, Some(ZoomIn)),
            (Key::ssharp, KEYCODE_MINUS, false, None, Some(ZoomOut)),
            (Key::VoidSymbol, KEYCODE_KP_ADD, false, None, Some(ZoomIn)),
            (
                Key::VoidSymbol,
                KEYCODE_KP_SUBTRACT,
                false,
                None,
                Some(ZoomOut),
            ),
        ]);
    }

    #[test]
    fn letters_and_unrelated_keys_stay_free() {
        use gdk::Key;
        check(&[
            // Dvorak `z` and `w` on the US `/` and `,` keys.
            (Key::z, KEYCODE_SLASH, false, None, None),
            (Key::Z, KEYCODE_SLASH, true, Some(Key::z), None),
            (Key::w, KEYCODE_COMMA, false, None, None),
            (Key::a, 38, false, None, None),
            (Key::_0, 19, false, None, None),
        ]);
    }

    #[test]
    fn punctuation_needs_ctrl_without_alt() {
        let alt = CTRL | gdk::ModifierType::ALT_MASK;
        for state in [gdk::ModifierType::empty(), SHIFT, alt] {
            for keyval in [gdk::Key::slash, gdk::Key::apostrophe, gdk::Key::plus] {
                assert_eq!(match_punctuation(keyval, KEYCODE_SLASH, state, None), None);
            }
        }
    }
}
