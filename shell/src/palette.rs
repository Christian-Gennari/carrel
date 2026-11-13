//! The command palette (Ctrl+K) and the key sheet (Ctrl+/).
//!
//! The palette is the quotes palette's sheet, centred near the top of the
//! editor column: one line to type into, every action below it with its key as
//! key caps, and the files whose names match the query under the actions. A
//! writer never has to learn a chord or hunt a screen: Ctrl+K, a few letters,
//! Enter.
//!
//! The pure part ([`actions`], [`rows_for`], [`match_score`]) has no GTK, so
//! the ordering and the fuzzy matching are tested without a display. Typing is
//! read from the key dispatcher, like the Quotes palette: the caret never
//! leaves the editor and no widget needs focus.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::daemon::Client;
use crate::editor::EditorPage;
use crate::nav::Navigator;
use crate::quotes::QuotePalette;
use crate::settings::{FontChoice, GoalDisplay, Settings, ThemeChoice};
use crate::settings_page::{SettingRow, SettingsPage};
use crate::store::{display_path, Store};
use crate::sync::SyncPage;
use crate::ui;

/// The sheet's width: wide enough for an action name and its key caps.
const PALETTE_WIDTH: i32 = 620;
/// Near the top of the editor column, just above the first line of text.
const PALETTE_TOP: i32 = 88;
/// The tallest the list grows before it scrolls: the ten curated actions fit
/// without a scroll, and the rest of the list scrolls under them.
const LIST_MAX_HEIGHT: i32 = 424;
/// How many documents a file query lists.
const FILE_LIMIT: usize = 20;

/// The key hints under the list, in the same key-cap bar as every screen.
const PALETTE_HINTS: [(&str, &str); 3] = [("Enter", "Run"), ("↑ ↓", "Move"), ("Esc", "Close")];

/// What running a row does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    NewFile,
    OpenFiles,
    InsertQuote,
    SyncNow,
    Export,
    History,
    ToggleFocus,
    OpenTheme,
    Settings,
    KeySheet,
    LargerFont,
    SmallerFont,
    BackToEditor,
    ToggleBottomLine,
    SetGoal(GoalDisplay),
    ToggleTypewriter,
    Theme(ThemeChoice),
    Font(FontChoice),
    Wifi,
    Pair,
    Update,
    /// A document by its path relative to the writing root.
    Open(PathBuf),
}

/// One row: the action's name on the left, its key as key caps on the right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteRow {
    pub label: String,
    pub keys: &'static str,
    pub action: Action,
}

/// Every action the palette lists, most useful first. `update` is the version
/// of a ready update, if any; the Update row exists only then.
pub fn actions(settings: &Settings, update: Option<&str>) -> Vec<PaletteRow> {
    let mut rows = Vec::new();
    let mut add = |label: &str, keys: &'static str, action: Action| {
        rows.push(PaletteRow {
            label: label.to_owned(),
            keys,
            action,
        });
    };

    // The ten a writer reaches for most, in this order.
    add("New file", "Ctrl+N", Action::NewFile);
    add("Open file…", "Ctrl+O", Action::OpenFiles);
    add("Insert quote", "Ctrl+'", Action::InsertQuote);
    add("Sync now", "Ctrl+Shift+S", Action::SyncNow);
    add("Export…", "Ctrl+E", Action::Export);
    add("History", "Ctrl+H", Action::History);
    add(
        if settings.focus_mode {
            "Focus mode off"
        } else {
            "Focus mode on"
        },
        "F2",
        Action::ToggleFocus,
    );
    add("Theme…", "", Action::OpenTheme);
    add("Settings", "Ctrl+,", Action::Settings);
    add("Keyboard shortcuts", "Ctrl+/", Action::KeySheet);

    // The rest of the keys table, then the direct values.
    add("Larger text", "Ctrl+=", Action::LargerFont);
    add("Smaller text", "Ctrl+-", Action::SmallerFont);
    add("Back to the editor", "Esc", Action::BackToEditor);
    add(
        if settings.show_bottom_line {
            "Hide bottom line"
        } else {
            "Show bottom line"
        },
        "F1",
        Action::ToggleBottomLine,
    );
    add(
        "Goal progress: Hairline",
        "",
        Action::SetGoal(GoalDisplay::Hairline),
    );
    add(
        "Goal progress: Numbers",
        "",
        Action::SetGoal(GoalDisplay::Numbers),
    );
    add(
        if settings.typewriter_mode {
            "Typewriter mode off"
        } else {
            "Typewriter mode on"
        },
        "",
        Action::ToggleTypewriter,
    );
    for choice in ThemeChoice::ALL {
        add(
            &format!("Theme: {}", choice.label()),
            "",
            Action::Theme(choice),
        );
    }
    for font in [FontChoice::Serif, FontChoice::Sans, FontChoice::Mono] {
        add(&format!("Font: {}", font.label()), "", Action::Font(font));
    }
    add("Wi-Fi…", "", Action::Wifi);
    add("Pair…", "", Action::Pair);
    if let Some(version) = update {
        let version = version.trim();
        let label = if version.is_empty() {
            "Install update".to_owned()
        } else {
            format!("Install update {version}")
        };
        add(&label, "", Action::Update);
    }
    rows
}

/// The rows for `query`: actions ranked by how well they match, then the
/// documents whose names match. An empty query lists the actions only, in
/// their curated order.
pub fn rows_for(
    query: &str,
    settings: &Settings,
    update: Option<&str>,
    files: &[PathBuf],
) -> Vec<PaletteRow> {
    let query = query.trim();
    let all = actions(settings, update);
    if query.is_empty() {
        return all;
    }

    let mut scored: Vec<(i32, usize, PaletteRow)> = all
        .into_iter()
        .enumerate()
        .filter_map(|(index, row)| match_score(query, &row.label).map(|score| (score, index, row)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut rows: Vec<PaletteRow> = scored.into_iter().map(|(_, _, row)| row).collect();

    let mut matches: Vec<(i32, String, PaletteRow)> = files
        .iter()
        .filter_map(|path| {
            let shown = display_path(path);
            match_score(query, &shown).map(|score| {
                (
                    score,
                    shown.clone(),
                    PaletteRow {
                        label: format!("Open: {shown}"),
                        keys: "",
                        action: Action::Open(path.clone()),
                    },
                )
            })
        })
        .collect();
    matches.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    rows.extend(matches.into_iter().take(FILE_LIMIT).map(|(_, _, row)| row));
    rows
}

/// How well `query` matches `text`, or `None`. Case-insensitive; every word of
/// the query must match in order ("theme dark" finds "Theme: Dark"), and a
/// word that starts a word in the text beats one that is merely in it, which
/// beats a scattered subsequence ("exp" finds "Export…" at the top).
pub fn match_score(query: &str, text: &str) -> Option<i32> {
    let query = query.trim();
    if query.is_empty() {
        return Some(0);
    }
    let text = text.to_lowercase();
    let mut score = 0;
    for token in query.to_lowercase().split_whitespace() {
        score += token_score(token, &text)?;
    }
    Some(score)
}

fn token_score(token: &str, text: &str) -> Option<i32> {
    if let Some(position) = text.find(token) {
        let position = position as i32;
        if position == 0 {
            return Some(3_000);
        }
        let starts_word = text[..position as usize]
            .chars()
            .next_back()
            .is_some_and(is_word_break);
        if starts_word {
            return Some(2_000 - position.min(500));
        }
        return Some(1_000 - position.min(500));
    }
    subsequence_score(token, text)
}

/// How tightly the characters of `token` appear in `text`, in order.
fn subsequence_score(token: &str, text: &str) -> Option<i32> {
    let mut characters = text.char_indices();
    let mut first = None;
    let mut last = 0;
    for wanted in token.chars() {
        loop {
            let (index, character) = characters.next()?;
            if character == wanted {
                first.get_or_insert(index);
                last = index;
                break;
            }
        }
    }
    let span = last.saturating_sub(first?);
    Some(500 - span.min(400) as i32)
}

fn is_word_break(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            ':' | '/' | '·' | '-' | '(' | ')' | '…' | ',' | '\'' | '’'
        )
}

/// `ready:0.2.0` and `ready 0.2.0` carry a version; a bare `ready` does not.
fn ready_version(wire: &str) -> Option<String> {
    let version = wire
        .strip_prefix("ready:")
        .or_else(|| wire.strip_prefix("ready "))?
        .trim();
    Some(version.to_owned())
}

/// The palette: one sheet over the current screen, or none.
pub struct CommandPalette {
    pub widget: gtk::Box,
    query_label: gtk::Label,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    message: gtk::Label,
    rows: RefCell<Vec<PaletteRow>>,
    /// The documents on disk, read once when the palette opens so typing
    /// never walks the folder again.
    files: RefCell<Vec<PathBuf>>,
    query: RefCell<String>,
    selected: Cell<usize>,
    open: Cell<bool>,
    settings: Rc<RefCell<Settings>>,
    root: PathBuf,
    restyle: Rc<dyn Fn()>,
    editor: Rc<EditorPage>,
    nav: Rc<Navigator>,
    quotes: Rc<QuotePalette>,
    sync: Rc<SyncPage>,
    settings_page: Rc<SettingsPage>,
    key_sheet: Rc<KeySheet>,
    store: Store,
    client: Client,
    update_wire: RefCell<String>,
}

impl CommandPalette {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: Client,
        store: Store,
        settings: Rc<RefCell<Settings>>,
        restyle: Rc<dyn Fn()>,
        editor: Rc<EditorPage>,
        nav: Rc<Navigator>,
        quotes: Rc<QuotePalette>,
        sync: Rc<SyncPage>,
        settings_page: Rc<SettingsPage>,
        key_sheet: Rc<KeySheet>,
    ) -> Rc<Self> {
        let query_label = gtk::Label::new(None);
        query_label.set_xalign(0.0);
        query_label.set_hexpand(true);
        query_label.set_ellipsize(pango::EllipsizeMode::End);
        query_label.add_css_class("command-query");
        let query_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        query_row.add_css_class("command-query-row");
        query_row.append(&query_label);

        let list = ui::list();
        list.add_css_class("command-list");
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(LIST_MAX_HEIGHT)
            .child(&list)
            .build();
        scroller.set_can_focus(false);

        let message = gtk::Label::new(None);
        message.set_xalign(0.0);
        message.set_wrap(true);
        message.add_css_class("command-message");
        message.set_visible(false);

        let hint = ui::hint_bar(&PALETTE_HINTS);
        hint.add_css_class("command-hint-bar");

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("command-palette");
        widget.set_halign(gtk::Align::Center);
        widget.set_valign(gtk::Align::Start);
        widget.set_margin_top(PALETTE_TOP);
        widget.set_width_request(PALETTE_WIDTH);
        widget.append(&query_row);
        widget.append(&scroller);
        widget.append(&message);
        widget.append(&hint);
        widget.set_visible(false);

        let root = store.root().to_path_buf();
        let palette = Rc::new(Self {
            widget,
            query_label,
            list,
            scroller,
            message,
            rows: RefCell::new(Vec::new()),
            files: RefCell::new(Vec::new()),
            query: RefCell::new(String::new()),
            selected: Cell::new(0),
            open: Cell::new(false),
            settings,
            root,
            restyle,
            editor,
            nav,
            quotes,
            sync,
            settings_page,
            key_sheet,
            store,
            client,
            update_wire: RefCell::new(String::new()),
        });
        palette.render_query();
        palette
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    /// The labels of the rows on screen, for the tests.
    #[cfg(test)]
    pub(crate) fn labels(&self) -> Vec<String> {
        self.rows
            .borrow()
            .iter()
            .map(|row| row.label.clone())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn selected_label(&self) -> Option<String> {
        self.rows
            .borrow()
            .get(self.selected.get())
            .map(|row| row.label.clone())
    }

    /// Open on the current screen. The quotes palette steps aside.
    pub fn open(self: &Rc<Self>) {
        if self.open.get() {
            return;
        }
        self.quotes.close_from_outside();
        self.open.set(true);
        self.query.replace(String::new());
        self.selected.set(0);
        *self.files.borrow_mut() = self.store.document_paths();
        self.load_update();
        self.rebuild();
        self.widget.set_visible(true);
    }

    pub fn close(&self) {
        if !self.open.get() {
            return;
        }
        self.open.set(false);
        self.widget.set_visible(false);
        self.query.borrow_mut().clear();
        self.rows.borrow_mut().clear();
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
    }

    /// Keys belong to the palette while it is open. Typing is read here, so
    /// no screen behind it ever sees a letter.
    pub fn handle_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        if !self.open.get() {
            return glib::Propagation::Proceed;
        }
        let control = state.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = state.contains(gdk::ModifierType::ALT_MASK);
        if control && keyval.to_lower() == gdk::Key::k {
            self.close();
            return glib::Propagation::Stop;
        }
        match keyval {
            gdk::Key::Escape => self.close(),
            gdk::Key::Up => self.move_by(-1),
            gdk::Key::Down => self.move_by(1),
            gdk::Key::Return | gdk::Key::KP_Enter => self.run(),
            gdk::Key::BackSpace => {
                if !control {
                    self.pop_character();
                }
            }
            _ if control || alt => {}
            _ => {
                if let Some(character) = keyval.to_unicode() {
                    if !character.is_control() {
                        self.push_character(character);
                    }
                }
            }
        }
        glib::Propagation::Stop
    }

    fn push_character(self: &Rc<Self>, character: char) {
        self.query.borrow_mut().push(character);
        self.selected.set(0);
        self.refresh();
    }

    fn pop_character(self: &Rc<Self>) {
        if self.query.borrow_mut().pop().is_some() {
            self.selected.set(0);
            self.refresh();
        }
    }

    fn move_by(&self, delta: i32) {
        let len = self.rows.borrow().len() as i32;
        if len == 0 {
            return;
        }
        self.selected
            .set(((self.selected.get() as i32 + delta).rem_euclid(len)) as usize);
        let selected = self.selected.get();
        ui::select(&self.list, selected);
        if let Some(row) = self.list.row_at_index(selected as i32) {
            ui::scroll_to_row(&self.scroller, &self.list, &row);
        }
    }

    fn run(self: &Rc<Self>) {
        let action = self
            .rows
            .borrow()
            .get(self.selected.get())
            .map(|row| row.action.clone());
        let Some(action) = action else {
            return;
        };
        // The palette is gone before the action starts: never half-open.
        self.close();
        self.execute(action);
    }

    fn execute(self: &Rc<Self>, action: Action) {
        match action {
            Action::NewFile => {
                self.nav.show_editor();
                if let Err(error) = self.editor.create_new_file() {
                    self.editor.flash_note(&error, 4);
                }
            }
            Action::OpenFiles => self.nav.show("files"),
            Action::InsertQuote => {
                self.nav.show_editor();
                let quotes = Rc::clone(&self.quotes);
                glib::idle_add_local_once(move || quotes.open_at_caret());
            }
            Action::SyncNow => self.sync.sync_now(),
            Action::Export => self.nav.show("export"),
            Action::History => self.nav.show("history"),
            Action::ToggleFocus => {
                self.editor.toggle_focus_mode();
                let on = self.settings.borrow().focus_mode;
                self.editor.flash_note(
                    if on {
                        "Focus mode on"
                    } else {
                        "Focus mode off"
                    },
                    2,
                );
            }
            Action::OpenTheme => self.settings_page.open_row(SettingRow::Theme),
            Action::Settings => self.nav.show("settings"),
            Action::KeySheet => self.key_sheet.open(),
            Action::LargerFont => {
                self.editor.change_font(true);
                let size = self.settings.borrow().font_size;
                self.editor.flash_note(&format!("Text size {size}"), 2);
            }
            Action::SmallerFont => {
                self.editor.change_font(false);
                let size = self.settings.borrow().font_size;
                self.editor.flash_note(&format!("Text size {size}"), 2);
            }
            Action::BackToEditor => self.nav.show_editor(),
            Action::ToggleBottomLine => {
                self.editor.toggle_bottom_line();
                let shown = self.settings.borrow().show_bottom_line;
                self.editor.flash_note(
                    if shown {
                        "Bottom line shown"
                    } else {
                        "Bottom line hidden"
                    },
                    2,
                );
            }
            Action::SetGoal(goal) => {
                self.settings.borrow_mut().goal_display = goal;
                self.save_settings();
                (self.restyle)();
                self.editor
                    .flash_note(&format!("Goal progress: {}", goal.label()), 2);
            }
            Action::ToggleTypewriter => {
                let on = {
                    let mut settings = self.settings.borrow_mut();
                    settings.typewriter_mode = !settings.typewriter_mode;
                    settings.typewriter_mode
                };
                self.save_settings();
                (self.restyle)();
                self.editor.flash_note(
                    if on {
                        "Typewriter mode on"
                    } else {
                        "Typewriter mode off"
                    },
                    2,
                );
            }
            Action::Theme(choice) => {
                self.settings.borrow_mut().theme = choice;
                self.save_settings();
                (self.restyle)();
                self.editor
                    .flash_note(&format!("Theme: {}", choice.label()), 2);
            }
            Action::Font(font) => {
                self.settings.borrow_mut().font = font;
                self.save_settings();
                (self.restyle)();
                self.editor
                    .flash_note(&format!("Font: {}", font.label()), 2);
            }
            Action::Wifi => self.nav.show("wifi"),
            Action::Pair => self.nav.show("sync"),
            Action::Update => {
                self.settings_page.open_row(SettingRow::Update);
                self.settings_page.apply_update();
            }
            Action::Open(path) => {
                self.nav.show_editor();
                if let Err(error) = self.editor.open_rel(&path) {
                    self.editor.flash_note(&error, 4);
                }
            }
        }
    }

    fn save_settings(&self) {
        if let Err(error) = self.settings.borrow().save(&self.root) {
            eprintln!("carrel: could not save settings: {error}");
        }
    }

    fn load_update(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.client.spawn(
            |daemon| daemon.update_status(),
            move |result| {
                let Some(palette) = weak.upgrade() else {
                    return;
                };
                let wire = result.unwrap_or_default();
                *palette.update_wire.borrow_mut() = wire;
                if palette.is_open() {
                    palette.rebuild();
                }
            },
        );
    }

    fn refresh(self: &Rc<Self>) {
        self.render_query();
        self.rebuild();
    }

    fn rebuild(self: &Rc<Self>) {
        let query = self.query.borrow().clone();
        let update = ready_version(&self.update_wire.borrow());
        let rows = {
            let settings = self.settings.borrow();
            let files = if query.trim().is_empty() {
                Vec::new()
            } else {
                self.files.borrow().clone()
            };
            rows_for(&query, &settings, update.as_deref(), &files)
        };
        *self.rows.borrow_mut() = rows;
        self.render_rows();
    }

    fn render_query(self: &Rc<Self>) {
        let query = self.query.borrow().clone();
        if query.is_empty() {
            self.query_label.set_text("Type to find an action");
            self.query_label.add_css_class("command-placeholder");
        } else {
            self.query_label.set_text(&query);
            self.query_label.remove_css_class("command-placeholder");
        }
    }

    fn render_rows(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let rows = self.rows.borrow();
        for row in rows.iter() {
            self.list.append(&row_widget(row));
        }
        let empty = rows.is_empty();
        let selected = self.selected.get().min(rows.len().saturating_sub(1));
        self.selected.set(selected);
        drop(rows);

        self.scroller.set_visible(!empty);
        self.message.set_visible(empty);
        if empty {
            self.message.set_text(&format!(
                "No actions or files match “{}”.",
                self.query.borrow().trim()
            ));
            return;
        }
        ui::select(&self.list, selected);
        if let Some(row) = self.list.row_at_index(selected as i32) {
            ui::scroll_to_row(&self.scroller, &self.list, &row);
        }
    }
}

/// One row: the name, then the key caps at the right edge.
fn row_widget(row: &PaletteRow) -> gtk::ListBoxRow {
    let element = gtk::ListBoxRow::new();
    element.add_css_class("ui-row");
    element.add_css_class("command-row");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let name = gtk::Label::new(Some(&row.label));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(pango::EllipsizeMode::End);
    name.add_css_class("command-name");
    line.append(&name);
    if !row.keys.is_empty() {
        let cap = gtk::Label::new(Some(row.keys));
        cap.add_css_class("key-cap");
        cap.set_valign(gtk::Align::Center);
        line.append(&cap);
    }
    element.set_child(Some(&line));
    element
}

// ---------------------------------------------------------------------------
// The key sheet.
// ---------------------------------------------------------------------------

const WRITING: &[(&str, &str)] = &[
    ("Ctrl+K", "Everything"),
    ("Ctrl+'", "Quote"),
    ("F1", "Bottom line"),
    ("F2", "Focus mode"),
    ("Ctrl+=", "Larger text"),
    ("Ctrl+-", "Smaller text"),
    ("Ctrl+/", "This card"),
];

const FILES: &[(&str, &str)] = &[
    ("Ctrl+O", "Files"),
    ("Ctrl+N", "New file"),
    ("Enter", "Open"),
    ("F2", "Rename"),
    ("Del", "Delete"),
];

const SYNC_AND_EXPORT: &[(&str, &str)] = &[
    ("Ctrl+Shift+S", "Sync now"),
    ("Ctrl+E", "Export"),
    ("Enter", "Continue"),
];

const SCREENS: &[(&str, &str)] = &[
    ("Ctrl+H", "History"),
    ("Ctrl+,", "Settings"),
    ("Esc", "Back to the editor"),
];

/// The groups, two per column: Writing and Files on the left, Sync and export
/// and Screens on the right.
const GROUPS: [(&str, &[(&str, &str)]); 4] = [
    ("Writing", WRITING),
    ("Files", FILES),
    ("Sync and export", SYNC_AND_EXPORT),
    ("Screens", SCREENS),
];

/// The centred card listing every key.
pub struct KeySheet {
    pub widget: gtk::Box,
    open: Cell<bool>,
}

impl KeySheet {
    pub fn new() -> Rc<Self> {
        let title = gtk::Label::new(Some("Keyboard shortcuts"));
        title.set_xalign(0.0);
        title.add_css_class("key-sheet-title");

        let columns = gtk::Grid::new();
        columns.set_margin_top(18);
        columns.set_row_spacing(26);
        columns.set_column_spacing(48);
        columns.set_column_homogeneous(true);
        columns.set_hexpand(true);
        columns.attach(&group(&GROUPS[0]), 0, 0, 1, 1);
        columns.attach(&group(&GROUPS[1]), 1, 0, 1, 1);
        columns.attach(&group(&GROUPS[2]), 0, 1, 1, 1);
        columns.attach(&group(&GROUPS[3]), 1, 1, 1, 1);

        let hint = ui::hint_bar(&[("Esc", "Close")]);
        hint.set_margin_top(20);

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("key-sheet");
        widget.set_halign(gtk::Align::Center);
        widget.set_valign(gtk::Align::Center);
        widget.set_width_request(780);
        widget.append(&title);
        widget.append(&columns);
        widget.append(&hint);
        widget.set_visible(false);

        Rc::new(Self {
            widget,
            open: Cell::new(false),
        })
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    pub fn open(&self) {
        self.open.set(true);
        self.widget.set_visible(true);
    }

    pub fn close(&self) {
        self.open.set(false);
        self.widget.set_visible(false);
    }

    /// Esc or Ctrl+/ closes the card; every other key belongs to it, so the
    /// page behind cannot change while it is up.
    pub fn handle_key(&self, keyval: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        if !self.open.get() {
            return glib::Propagation::Proceed;
        }
        let control = state.contains(gdk::ModifierType::CONTROL_MASK);
        if keyval == gdk::Key::Escape || (control && keyval.to_lower() == gdk::Key::slash) {
            self.close();
        }
        glib::Propagation::Stop
    }
}

/// One group: its heading, then a cap and its meaning per row.
fn group((name, rows): &(&str, &[(&str, &str)])) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let heading = gtk::Label::new(Some(name));
    heading.set_xalign(0.0);
    heading.add_css_class("key-group");
    column.append(&heading);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(14);
    for (index, (keys, word)) in rows.iter().enumerate() {
        let cap = gtk::Label::new(Some(keys));
        cap.add_css_class("key-cap");
        cap.set_halign(gtk::Align::Start);
        cap.set_valign(gtk::Align::Center);
        let word = gtk::Label::new(Some(word));
        word.set_xalign(0.0);
        word.set_hexpand(true);
        word.add_css_class("key-word");
        grid.attach(&cap, 0, index as i32, 1, 1);
        grid.attach(&word, 1, index as i32, 1, 1);
    }
    column.append(&grid);
    column
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FontChoice, Theme};

    fn settings() -> Settings {
        Settings {
            first_run_complete: true,
            ..Settings::default()
        }
    }

    #[test]
    fn the_empty_palette_starts_with_the_ten_useful_actions() {
        let rows = actions(&settings(), None);
        let labels: Vec<&str> = rows.iter().take(10).map(|row| row.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "New file",
                "Open file…",
                "Insert quote",
                "Sync now",
                "Export…",
                "History",
                "Focus mode on",
                "Theme…",
                "Settings",
                "Keyboard shortcuts",
            ]
        );
        assert_eq!(rows[0].keys, "Ctrl+N");
        assert_eq!(rows[3].keys, "Ctrl+Shift+S");
    }

    #[test]
    fn every_theme_and_font_is_reachable_by_its_name() {
        let rows = actions(&settings(), None);
        for theme in Theme::ALL {
            let wanted = format!("Theme: {}", theme.label());
            assert!(
                rows.iter().any(|row| row.label == wanted),
                "{wanted} is missing"
            );
        }
        for font in [FontChoice::Serif, FontChoice::Sans, FontChoice::Mono] {
            let wanted = format!("Font: {}", font.label());
            assert!(rows.iter().any(|row| row.label == wanted), "{wanted}");
        }
        assert!(rows
            .iter()
            .any(|row| row.label == "Goal progress: Hairline"));
        assert!(rows.iter().any(|row| row.label == "Goal progress: Numbers"));
        assert!(rows
            .iter()
            .any(|row| row.label == "Back to the editor" && row.keys == "Esc"));
    }

    #[test]
    fn exp_finds_export_and_dark_finds_the_theme() {
        let rows = rows_for("exp", &settings(), None, &[]);
        assert_eq!(rows[0].label, "Export…");
        let rows = rows_for("dark", &settings(), None, &[]);
        assert_eq!(rows[0].label, "Theme: Dark");
        let rows = rows_for("EXP", &settings(), None, &[]);
        assert_eq!(rows[0].label, "Export…");
        let rows = rows_for("dark", &settings(), None, &[]);
        assert!(rows
            .iter()
            .all(|row| row.label.to_lowercase().contains("dark")));
    }

    #[test]
    fn a_query_can_span_words_and_prefers_word_starts() {
        let rows = rows_for("theme sepia", &settings(), None, &[]);
        assert_eq!(rows[0].label, "Theme: Sepia");
        assert!(match_score("key", "Keyboard shortcuts").unwrap() > 2_500);
        assert!(match_score("board", "Keyboard shortcuts").is_some());
        assert!(match_score("xylophone", "Keyboard shortcuts").is_none());
        assert_eq!(match_score("", "anything"), Some(0));
    }

    #[test]
    fn toggles_say_what_enter_will_do() {
        let mut settings = settings();
        let rows = actions(&settings, None);
        assert!(rows.iter().any(|row| row.label == "Focus mode on"));
        assert!(rows.iter().any(|row| row.label == "Hide bottom line"));
        settings.focus_mode = true;
        settings.show_bottom_line = false;
        settings.typewriter_mode = true;
        let rows = actions(&settings, None);
        assert!(rows.iter().any(|row| row.label == "Focus mode off"));
        assert!(rows.iter().any(|row| row.label == "Show bottom line"));
        assert!(rows.iter().any(|row| row.label == "Typewriter mode off"));
    }

    #[test]
    fn files_only_list_while_typing_and_rank_by_name() {
        let settings = settings();
        let files = vec![
            PathBuf::from("Novel/Chapter one.md"),
            PathBuf::from("Notes.md"),
            PathBuf::from("Chapter two.md"),
        ];
        assert!(rows_for("", &settings, None, &files)
            .iter()
            .all(|row| !row.label.starts_with("Open: ")));
        let rows = rows_for("chapter", &settings, None, &files);
        let opened: Vec<&str> = rows
            .iter()
            .filter(|row| row.label.starts_with("Open: "))
            .map(|row| row.label.as_str())
            .collect();
        assert_eq!(opened.len(), 2);
        assert!(opened.contains(&"Open: Novel/Chapter one"));
        assert!(opened.contains(&"Open: Chapter two"));
    }

    #[test]
    fn the_update_row_only_exists_when_an_update_is_ready() {
        let settings = settings();
        assert!(!actions(&settings, None)
            .iter()
            .any(|row| row.label.starts_with("Install update")));
        assert_eq!(ready_version("ready:0.2.0").as_deref(), Some("0.2.0"));
        assert_eq!(ready_version("ready 0.2.0").as_deref(), Some("0.2.0"));
        assert_eq!(ready_version("ready ").as_deref(), Some(""));
        assert_eq!(ready_version("ready"), None);
        assert_eq!(ready_version("none"), None);
        let rows = rows_for("update", &settings, Some("0.2.0"), &[]);
        assert_eq!(rows[0].label, "Install update 0.2.0");
    }
}
