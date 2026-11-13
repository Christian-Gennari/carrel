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

use gtk::{gdk, glib};

use crate::editor::EditorPage;
use crate::export::ExportPage;
use crate::files::FilesPage;
use crate::first_run::FirstRunPage;
use crate::history::HistoryPage;
use crate::nav::Navigator;
use crate::palette::{CommandPalette, KeySheet};
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
        let visible = self.visible();
        let control = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let key = keyval.to_lower();

        // The key sheet is on top of everything until Esc. First run is a
        // closed flow: the palette and the sheet do not exist there.
        if self.key_sheet.handle_key(keyval, state) == glib::Propagation::Stop {
            return glib::Propagation::Stop;
        }
        if visible == "first_run" {
            return self.first_run.handle_key(keyval, state);
        }

        // While the palette is open every key belongs to it, so no letter
        // reaches the editor or a screen behind it.
        if self.palette.handle_key(keyval, state) == glib::Propagation::Stop {
            return glib::Propagation::Stop;
        }

        if control {
            // Ctrl+K: the same palette from the editor and every screen.
            if key == gdk::Key::k && !self.dialog_open() {
                self.palette.open();
                return glib::Propagation::Stop;
            }
            // Ctrl+/: the key sheet. `?` stays a normal character in the
            // editor, so the sheet has a chord of its own.
            if key == gdk::Key::slash && !self.dialog_open() {
                self.key_sheet.open();
                return glib::Propagation::Stop;
            }
        }

        // The global keys from spec/03-ui.md.
        if control {
            if key == gdk::Key::h {
                self.nav.show("history");
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::s && shift {
                self.sync.sync_now();
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::e {
                self.nav.show("export");
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::comma {
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
            _ => match self.quotes.handle_key(keyval, state) {
                glib::Propagation::Stop => glib::Propagation::Stop,
                glib::Propagation::Proceed => self.editor.handle_key(keyval, state),
            },
        }
    }
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
    use super::parse_script;
    use gtk::gdk;

    #[test]
    fn scripts_name_keys_and_chords() {
        let keys = parse_script(" Down, Down ,Return,ctrl+e,shift+ctrl+s ");
        assert_eq!(keys.len(), 5);
        assert_eq!(keys[0].0, gdk::Key::Down);
        assert_eq!(keys[1].0, gdk::Key::Down);
        assert_eq!(keys[2].0, gdk::Key::Return);
        assert_eq!(keys[3].0, gdk::Key::e);
        assert_eq!(keys[3].1, gdk::ModifierType::CONTROL_MASK);
        assert_eq!(keys[4].0, gdk::Key::s);
        assert_eq!(
            keys[4].1,
            gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK
        );
    }

    #[test]
    fn unknown_keys_are_skipped() {
        assert!(parse_script("Down,NotAKey,Return").len() == 2);
        assert!(parse_script("").is_empty());
    }
}
