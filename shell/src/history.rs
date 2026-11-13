//! The History screen: versions by time with a word-change bar, a preview
//! that shows what changed against the current text, and Restore behind the
//! shared confirmation. Restoring never rewrites history: the old text is
//! applied as a new edit and committed afterwards.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::daemon::{Client, HistoryEntry};
use crate::editor::EditorPage;
use crate::nav::Navigator;
use crate::settings::Settings;
use crate::store::display_path;
use crate::theme;
use crate::ui;

/// The version list's column: a short moment, the word change and the bar.
/// Everything else on the wide frame belongs to the diff.
const HISTORY_LIST_WIDTH: i32 = 340;

const HINTS: [(&str, &str); 4] = [
    ("Enter", "Restore"),
    ("↑ ↓", "Versions"),
    ("PgUp PgDn", "Scroll"),
    ("Esc", "Back"),
];

/// The pure part: which version is selected, what the preview shows, what the
/// screen asks the daemon for next.
#[derive(Debug, Default)]
pub struct HistoryState {
    pub path: String,
    pub versions: Vec<HistoryEntry>,
    pub selected: usize,
    pub preview: Option<String>,
    pub preview_rev: Option<String>,
    pub notice: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryEffect {
    Preview { path: String, rev: String },
    Restore { path: String, rev: String },
}

impl HistoryState {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Self::default()
        }
    }

    pub fn current(&self) -> Option<&HistoryEntry> {
        self.versions.get(self.selected)
    }

    pub fn set_versions(&mut self, versions: Vec<HistoryEntry>) -> Vec<HistoryEffect> {
        self.versions = versions;
        self.selected = 0;
        self.preview = None;
        self.preview_rev = None;
        self.error = None;
        self.preview_effect().into_iter().collect()
    }

    pub fn move_by(&mut self, delta: i32) -> Vec<HistoryEffect> {
        if self.versions.is_empty() {
            return Vec::new();
        }
        let len = self.versions.len() as i32;
        self.selected = ((self.selected as i32 + delta).rem_euclid(len)) as usize;
        self.preview_effect().into_iter().collect()
    }

    pub fn select(&mut self, index: usize) -> Vec<HistoryEffect> {
        if self.versions.is_empty() {
            return Vec::new();
        }
        let index = index.min(self.versions.len() - 1);
        if index == self.selected && self.preview_rev.is_some() {
            return Vec::new();
        }
        self.selected = index;
        self.preview_effect().into_iter().collect()
    }

    fn preview_effect(&self) -> Option<HistoryEffect> {
        let entry = self.current()?;
        if self.preview_rev.as_deref() == Some(entry.rev.as_str()) {
            return None;
        }
        Some(HistoryEffect::Preview {
            path: self.path.clone(),
            rev: entry.rev.clone(),
        })
    }

    pub fn on_preview(&mut self, rev: &str, text: String) {
        if self.current().map(|entry| entry.rev.as_str()) == Some(rev) {
            self.preview = Some(text);
            self.preview_rev = Some(rev.to_owned());
            self.error = None;
        }
    }

    pub fn on_preview_error(&mut self, rev: &str, message: String) {
        if self.current().map(|entry| entry.rev.as_str()) == Some(rev) {
            self.preview = None;
            self.preview_rev = Some(rev.to_owned());
            self.error = Some(message);
        }
    }

    pub fn restore_effect(&self) -> Option<HistoryEffect> {
        let entry = self.current()?;
        Some(HistoryEffect::Restore {
            path: self.path.clone(),
            rev: entry.rev.clone(),
        })
    }

    pub fn on_restored(&mut self, when: String) {
        self.notice = format!("Restored the version from {when}. The newer text stays in history.");
        self.error = None;
    }

    pub fn delta_label(entry: &HistoryEntry) -> String {
        let delta = entry.delta;
        if delta > 0 {
            format!(
                "+{} words",
                crate::store::stats::format_number(delta as u64)
            )
        } else if delta < 0 {
            format!(
                "−{} words",
                crate::store::stats::format_number(delta.unsigned_abs())
            )
        } else {
            "no change".to_owned()
        }
    }

    /// The largest change in the list, for the little +/- bars.
    pub fn largest_delta(&self) -> u64 {
        self.versions
            .iter()
            .map(|entry| entry.delta.unsigned_abs())
            .max()
            .unwrap_or(0)
            .max(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Same,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRun {
    pub kind: DiffKind,
    pub text: String,
}

/// The largest word-diff table Carrel builds. A whole-book rewrite falls back
/// to one removed block and one added block, which still reads as a diff and
/// never stalls the screen.
const MAX_DIFF_CELLS: usize = 1_000_000;

/// A word-level diff of an old version against the text as it is now. Added
/// text exists only now; removed text exists only in the old version. Runs
/// merge so the preview has as few tags as possible.
pub fn diff_runs(old: &str, current: &str) -> Vec<DiffRun> {
    let old_tokens = tokens(old);
    let new_tokens = tokens(current);

    let prefix = old_tokens
        .iter()
        .zip(&new_tokens)
        .take_while(|(old, new)| old == new)
        .count();
    let max_suffix = old_tokens
        .len()
        .min(new_tokens.len())
        .saturating_sub(prefix);
    let suffix = old_tokens
        .iter()
        .rev()
        .zip(new_tokens.iter().rev())
        .take(max_suffix)
        .take_while(|(old, new)| old == new)
        .count();

    let mut runs = Vec::new();
    push_run(&mut runs, DiffKind::Same, &old_tokens[..prefix].concat());

    let old_middle = &old_tokens[prefix..old_tokens.len() - suffix];
    let new_middle = &new_tokens[prefix..new_tokens.len() - suffix];
    if old_middle.len().saturating_mul(new_middle.len()) > MAX_DIFF_CELLS {
        push_run(&mut runs, DiffKind::Removed, &old_middle.concat());
        push_run(&mut runs, DiffKind::Added, &new_middle.concat());
    } else {
        diff_middle(old_middle, new_middle, &mut runs);
    }

    push_run(
        &mut runs,
        DiffKind::Same,
        &old_tokens[old_tokens.len() - suffix..].concat(),
    );
    runs
}

fn diff_middle(old: &[String], new: &[String], runs: &mut Vec<DiffRun>) {
    let rows = old.len();
    let columns = new.len();
    let width = columns + 1;
    let mut table = vec![0u32; (rows + 1) * width];
    for row in (0..rows).rev() {
        for column in (0..columns).rev() {
            table[row * width + column] = if old[row] == new[column] {
                table[(row + 1) * width + column + 1] + 1
            } else {
                table[(row + 1) * width + column].max(table[row * width + column + 1])
            };
        }
    }
    let (mut row, mut column) = (0, 0);
    while row < rows && column < columns {
        if old[row] == new[column] {
            push_run(runs, DiffKind::Same, &old[row]);
            row += 1;
            column += 1;
        } else if table[(row + 1) * width + column] >= table[row * width + column + 1] {
            push_run(runs, DiffKind::Removed, &old[row]);
            row += 1;
        } else {
            push_run(runs, DiffKind::Added, &new[column]);
            column += 1;
        }
    }
    while row < rows {
        push_run(runs, DiffKind::Removed, &old[row]);
        row += 1;
    }
    while column < columns {
        push_run(runs, DiffKind::Added, &new[column]);
        column += 1;
    }
}

fn push_run(runs: &mut Vec<DiffRun>, kind: DiffKind, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.kind == kind {
            last.text.push_str(text);
            return;
        }
    }
    runs.push(DiffRun {
        kind,
        text: text.to_owned(),
    });
}

/// Words and the whitespace between them as separate runs, so a word at the
/// end of a line still matches the same word when a paragraph follows it.
fn tokens(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut whitespace: Option<bool> = None;
    for character in text.chars() {
        let is_whitespace = character.is_whitespace();
        if whitespace == Some(is_whitespace) {
            current.push(character);
        } else {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            current.push(character);
            whitespace = Some(is_whitespace);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

pub struct HistoryPage {
    pub widget: gtk::Overlay,
    screen: Rc<ui::Screen>,
    list: gtk::ListBox,
    list_scroller: gtk::ScrolledWindow,
    preview_title: gtk::Label,
    preview_legend: gtk::Label,
    preview: gtk::TextView,
    preview_scroller: gtk::ScrolledWindow,
    added_tag: gtk::TextTag,
    removed_tag: gtk::TextTag,
    notice: gtk::Label,
    /// The two panes, hidden while a state takes their place.
    columns: gtk::Box,
    /// Empty, loading and error states: one headline and one detail line.
    message: ui::State,
    state: RefCell<HistoryState>,
    client: Client,
    nav: Rc<Navigator>,
    editor: Rc<EditorPage>,
    settings: Rc<RefCell<Settings>>,
    list_generation: std::cell::Cell<u64>,
    preview_generation: std::cell::Cell<u64>,
}

impl HistoryPage {
    pub fn new(
        client: Client,
        nav: Rc<Navigator>,
        editor: Rc<EditorPage>,
        settings: Rc<RefCell<Settings>>,
    ) -> Rc<Self> {
        // History is the one two-pane screen: the wide frame keeps the title,
        // the status and the hint bar on every other screen's left edge and
        // grows to the right, so the diff reads at about 66 characters a line.
        let screen = ui::Screen::new_wide("History");
        screen.set_hints(&HINTS);

        let list = ui::list();
        // The versions stay compact (a short moment, the word change and the
        // bar); the rest of the width is the diff's.
        list.set_width_request(HISTORY_LIST_WIDTH);
        let list_scroller = ui::scroller(&list);

        let preview_title = gtk::Label::new(None);
        preview_title.set_xalign(0.0);
        preview_title.set_wrap(true);
        // A wrapping label asks for one character of width, so the pane takes
        // the space the frame leaves instead of widening the frame itself.
        preview_title.set_max_width_chars(1);
        preview_title.add_css_class("preview-title");
        let preview_legend = gtk::Label::new(None);
        preview_legend.set_xalign(0.0);
        preview_legend.set_wrap(true);
        preview_legend.set_max_width_chars(1);
        preview_legend.add_css_class("row-note");
        preview_legend.set_visible(false);

        let preview = gtk::TextView::new();
        preview.set_editable(false);
        preview.set_cursor_visible(false);
        preview.set_wrap_mode(gtk::WrapMode::WordChar);
        preview.set_can_focus(false);
        preview.set_focusable(false);
        preview.set_left_margin(0);
        preview.set_right_margin(0);
        preview.set_top_margin(0);
        preview.set_bottom_margin(0);
        preview.add_css_class("preview-text");
        preview.add_css_class("diff-view");
        let added_tag = preview
            .buffer()
            .create_tag(Some("added"), &[])
            .expect("the added tag");
        let removed_tag = preview
            .buffer()
            .create_tag(Some("removed"), &[])
            .expect("the removed tag");
        let preview_scroller = ui::scroller(&preview);

        let preview_inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
        preview_inner.set_margin_start(24);
        preview_inner.set_vexpand(true);
        preview_inner.append(&preview_title);
        preview_inner.append(&preview_legend);
        preview_inner.append(&preview_scroller);

        // The pane carries the hairline, so it runs the full height of the
        // screen beside the list.
        let preview_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        preview_pane.set_vexpand(true);
        preview_pane.add_css_class("preview-scroller");
        preview_pane.append(&preview_inner);

        let columns = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        columns.set_vexpand(true);
        // The versions keep their narrow column; the preview takes the rest of
        // the shared frame.
        let list_pane = ui::fade_top(&list_scroller);
        list_pane.set_hexpand(false);
        list_pane.set_size_request(HISTORY_LIST_WIDTH, -1);
        preview_pane.set_hexpand(true);
        columns.append(&list_pane);
        columns.append(&preview_pane);
        let message = ui::State::new();
        screen.content.append(&message.widget);
        screen.content.append(&columns);

        let notice = gtk::Label::new(None);
        notice.set_xalign(0.0);
        notice.set_wrap(true);
        notice.add_css_class("notice-line");
        notice.set_visible(false);
        screen.content.append(&notice);

        let page = Rc::new(Self {
            widget: screen.root.clone(),
            screen,
            list,
            list_scroller,
            preview_title,
            preview_legend,
            preview,
            preview_scroller,
            added_tag,
            removed_tag,
            notice,
            columns,
            message,
            state: RefCell::new(HistoryState::default()),
            client,
            nav,
            editor,
            settings,
            list_generation: std::cell::Cell::new(0),
            preview_generation: std::cell::Cell::new(0),
        });
        page.install();
        page
    }

    fn install(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(page) = weak.upgrade() {
                page.choose_index(row.index().max(0) as usize);
            }
        });
        let weak = Rc::downgrade(self);
        self.widget.connect_map(move |_| {
            if let Some(page) = weak.upgrade() {
                page.open();
                page.screen.focus_page();
            }
        });
    }

    /// Refresh from the editor's current file. Called when the screen appears.
    fn open(self: &Rc<Self>) {
        let Some(path) = self.editor.current_rel() else {
            self.screen.set_status("");
            self.show_message_state(
                "No file open",
                "Open a document to see its versions.",
                false,
            );
            return;
        };
        if let Err(error) = self.editor.flush_now() {
            self.show_message(
                &format!("Cannot save the file before history: {error}"),
                true,
            );
            return;
        }
        let path = path.to_string_lossy().to_string();
        self.screen.set_status(&display_path(Path::new(&path)));
        self.state.borrow_mut().path = path.clone();
        self.state.borrow_mut().notice.clear();
        self.state.borrow_mut().error = None;
        self.show_loading();

        let generation = self.list_generation.get().wrapping_add(1);
        self.list_generation.set(generation);
        let weak = Rc::downgrade(self);
        self.client.spawn(
            move |daemon| daemon.history(&path),
            move |result| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                if page.list_generation.get() != generation {
                    return;
                }
                match result {
                    Ok(versions) => page.set_versions(versions),
                    Err(error) => page.show_message(&error.message, true),
                }
            },
        );
    }

    fn set_versions(self: &Rc<Self>, versions: Vec<HistoryEntry>) {
        if versions.is_empty() {
            self.show_message_state(
                "No versions yet",
                "Carrel saves history after five quiet minutes.",
                false,
            );
            return;
        }
        let effects = self.state.borrow_mut().set_versions(versions);
        self.show_columns();
        self.rebuild_list();
        self.update_preview();
        self.run_effects(effects);
    }

    /// The two panes come back whenever there is something to show.
    fn show_columns(&self) {
        self.message.hide();
        self.columns.set_visible(true);
    }

    fn show_loading(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.versions.clear();
            state.preview = None;
            state.preview_rev = None;
            state.error = None;
        }
        self.clear_list();
        self.preview_title.set_text("");
        self.preview_legend.set_visible(false);
        self.preview.buffer().set_text("");
        self.update_notice();
        self.show_message_state("Loading versions…", "This takes a moment.", false);
    }

    /// A state takes the panes' place: one headline, one detail line, on the
    /// same page edge as the title. Empty and error look the same, except for
    /// the colour of the headline.
    fn show_message_state(&self, headline: &str, detail: &str, error: bool) {
        self.message.set(headline, &[detail], error);
        self.columns.set_visible(false);
    }

    fn show_message(self: &Rc<Self>, message: &str, error: bool) {
        {
            let mut state = self.state.borrow_mut();
            state.versions.clear();
            state.preview = None;
            state.preview_rev = None;
            // The state block below carries the sentence; the notice line is
            // for a restore's outcome, not for a screen with nothing on it.
            state.notice.clear();
            state.error = None;
        }
        self.clear_list();
        self.preview_title.set_text("");
        self.preview_legend.set_visible(false);
        self.preview.buffer().set_text("");
        self.update_notice();
        self.show_message_state(message, "Nothing in your writing changed.", error);
    }

    fn clear_list(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
    }

    fn rebuild_list(&self) {
        self.clear_list();
        let state = self.state.borrow();
        let largest = state.largest_delta() as f64;
        let palette = theme::palette(&self.settings.borrow());
        // The bar is drawn, not styled, so the selected row hands it the
        // selection's own colours: on High contrast the row is the accent and
        // ink on ink would disappear.
        let ink = colour(palette.text);
        let dim = colour(&palette.dim());
        let selected_ink = colour(palette.selected_text());
        let selected_dim = colour(&palette.selected_note());
        for (index, entry) in state.versions.iter().enumerate() {
            let row = gtk::ListBoxRow::new();
            row.add_css_class("ui-row");
            row.add_css_class("file-row");
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let time = gtk::Label::new(Some(&ui::format_moment_short(entry.time)));
            time.set_xalign(0.0);
            time.set_hexpand(true);
            time.set_ellipsize(gtk::pango::EllipsizeMode::End);
            time.add_css_class("history-time");
            let detail = gtk::Label::new(Some(&HistoryState::delta_label(entry)));
            detail.set_xalign(1.0);
            detail.add_css_class("history-delta");
            let (ink, dim) = if index == state.selected {
                (selected_ink, selected_dim)
            } else {
                (ink, dim)
            };
            let bar = delta_bar(entry.delta, largest, ink, dim);
            line.append(&time);
            line.append(&detail);
            line.append(&bar);
            row.set_child(Some(&line));
            self.list.append(&row);
        }
        let selected = state.selected;
        drop(state);
        ui::select(&self.list, selected);
        if let Some(row) = self.list.row_at_index(selected as i32) {
            ui::scroll_to_row(&self.list_scroller, &self.list, &row);
        }
    }

    fn choose_index(self: &Rc<Self>, index: usize) {
        let effects = self.state.borrow_mut().select(index);
        self.rebuild_list();
        self.run_effects(effects);
        self.update_preview();
    }

    fn run_effects(self: &Rc<Self>, effects: Vec<HistoryEffect>) {
        for effect in effects {
            match effect {
                HistoryEffect::Preview { path, rev } => self.load_preview(path, rev),
                HistoryEffect::Restore { .. } => {
                    // Restore is started from Enter, never from a mere move.
                }
            }
        }
    }

    fn load_preview(self: &Rc<Self>, path: String, rev: String) {
        let generation = self.preview_generation.get().wrapping_add(1);
        self.preview_generation.set(generation);
        let weak = Rc::downgrade(self);
        self.preview_title.set_text("Loading…");
        self.preview_legend.set_visible(false);
        let request_rev = rev.clone();
        self.client.spawn(
            move |daemon| daemon.restore(&path, &request_rev),
            move |result| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                if page.preview_generation.get() != generation {
                    return;
                }
                match result {
                    Ok(text) => {
                        page.state.borrow_mut().on_preview(&rev, text);
                        page.update_preview();
                    }
                    Err(error) => {
                        page.state
                            .borrow_mut()
                            .on_preview_error(&rev, error.message);
                        page.update_preview();
                    }
                }
            },
        );
    }

    fn update_preview(&self) {
        let (entry, old, error) = {
            let state = self.state.borrow();
            (
                state.current().cloned(),
                state.preview.clone(),
                state.error.clone(),
            )
        };
        let Some(entry) = entry else {
            self.update_notice();
            return;
        };
        self.preview_title.set_text(&format!(
            "{} · {} words",
            ui::format_moment(entry.time),
            crate::store::stats::format_number(u64::from(entry.words))
        ));
        if let Some(message) = error {
            self.preview_legend.set_visible(false);
            self.preview.buffer().set_text(&message);
            self.update_notice();
            return;
        }
        let Some(old) = old else {
            self.preview_legend.set_visible(false);
            self.preview.buffer().set_text("");
            self.update_notice();
            return;
        };
        self.preview_legend.set_visible(true);
        self.preview_legend
            .set_text("Compared with the current text · tinted added · struck removed");
        self.show_diff(&old, &self.current_text());
        self.update_notice();
    }

    /// Tints what exists only now and strikes what exists only in the old
    /// version, in the theme's own raised-surface and dim tokens.
    fn show_diff(&self, old: &str, current: &str) {
        let palette = theme::palette(&self.settings.borrow());
        self.added_tag
            .set_background_rgba(Some(&colour(&palette.surface())));
        self.removed_tag
            .set_foreground_rgba(Some(&colour(&palette.dim())));
        self.removed_tag.set_strikethrough(true);

        let buffer = self.preview.buffer();
        buffer.set_text("");
        let mut iter = buffer.end_iter();
        for run in diff_runs(old, current) {
            match run.kind {
                DiffKind::Same => buffer.insert(&mut iter, &run.text),
                DiffKind::Added => {
                    buffer.insert_with_tags_by_name(&mut iter, &run.text, &["added"]);
                }
                DiffKind::Removed => {
                    buffer.insert_with_tags_by_name(&mut iter, &run.text, &["removed"]);
                }
            }
        }
        self.preview_scroller.vadjustment().set_value(0.0);
    }

    fn current_text(&self) -> String {
        let buffer = self.editor.buffer();
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), true)
            .to_string()
    }

    fn update_notice(&self) {
        let state = self.state.borrow();
        if let Some(error) = &state.error {
            self.notice.remove_css_class("notice-ok");
            self.notice.add_css_class("error-text");
            self.notice.set_text(error);
            self.notice.set_visible(true);
        } else if !state.notice.is_empty() {
            self.notice.remove_css_class("error-text");
            self.notice.add_css_class("notice-ok");
            self.notice.set_text(&state.notice);
            self.notice.set_visible(true);
        } else {
            self.notice.set_visible(false);
        }
    }

    /// The restore card is open; the command palette waits for it.
    pub(crate) fn is_dialog_open(&self) -> bool {
        self.screen.is_dialog_open()
    }

    fn scroll_preview(&self, pages: f64) {
        let adjustment = self.preview_scroller.vadjustment();
        let maximum = (adjustment.upper() - adjustment.page_size()).max(0.0);
        let target = adjustment.value() + pages * adjustment.page_size() * 0.9;
        adjustment.set_value(target.clamp(0.0, maximum));
    }

    pub(crate) fn handle_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        if self.screen.is_dialog_open() {
            return self.screen.dialog_key(keyval, state);
        }
        if state.contains(gdk::ModifierType::CONTROL_MASK) {
            return glib::Propagation::Proceed;
        }
        match keyval {
            gdk::Key::Escape => {
                self.nav.show_editor();
                glib::Propagation::Stop
            }
            gdk::Key::Up => {
                self.move_selection(-1);
                glib::Propagation::Stop
            }
            gdk::Key::Down => {
                self.move_selection(1);
                glib::Propagation::Stop
            }
            gdk::Key::Page_Up => {
                self.scroll_preview(-1.0);
                glib::Propagation::Stop
            }
            gdk::Key::Page_Down => {
                self.scroll_preview(1.0);
                glib::Propagation::Stop
            }
            gdk::Key::Home => {
                self.choose_index(0);
                glib::Propagation::Stop
            }
            gdk::Key::End => {
                let last = self.state.borrow().versions.len().saturating_sub(1);
                self.choose_index(last);
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                self.confirm_restore();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn move_selection(self: &Rc<Self>, delta: i32) {
        let effects = self.state.borrow_mut().move_by(delta);
        self.rebuild_list();
        self.run_effects(effects);
        self.update_preview();
    }

    fn confirm_restore(self: &Rc<Self>) {
        let Some(entry) = self.state.borrow().current().cloned() else {
            return;
        };
        let when = ui::format_moment(entry.time);
        let weak = Rc::downgrade(self);
        // A short question, one width for every card; the moment leads the
        // sentence, where no title length can stretch the card.
        self.screen.ask(
            "Restore this version?",
            &format!("{when}. Your current text is kept in History."),
            "Restore",
            move || {
                if let Some(page) = weak.upgrade() {
                    page.restore_selected();
                }
            },
        );
    }

    fn restore_selected(self: &Rc<Self>) {
        let Some(effect) = self.state.borrow().restore_effect() else {
            return;
        };
        let HistoryEffect::Restore { path, rev } = effect else {
            return;
        };
        if let Err(error) = self.editor.flush_now() {
            self.state.borrow_mut().error = Some(format!("Cannot save before restoring: {error}"));
            self.update_notice();
            return;
        }
        let when = self
            .state
            .borrow()
            .current()
            .map(|entry| ui::format_moment(entry.time))
            .unwrap_or_default();

        let weak = Rc::downgrade(self);
        // Commit the current text first, so the version being replaced is in
        // history even when the writer restored within the five-minute window.
        self.client.spawn(
            {
                let path = path.clone();
                let rev = rev.clone();
                move |daemon| {
                    daemon.commit()?;
                    daemon.restore(&path, &rev)
                }
            },
            move |result| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                match result {
                    Ok(text) => {
                        if let Err(error) = page.editor.replace_text(&text) {
                            page.state.borrow_mut().error =
                                Some(format!("Cannot write the old version: {error}"));
                            page.update_notice();
                            return;
                        }
                        page.state.borrow_mut().on_restored(when.clone());
                        page.update_notice();
                        page.commit_and_reload();
                    }
                    Err(error) => {
                        page.state.borrow_mut().error = Some(error.message);
                        page.update_notice();
                    }
                }
            },
        );
    }

    /// Record the restored text as a new version, then show the list again.
    fn commit_and_reload(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.client.spawn(
            |daemon| daemon.commit(),
            move |result| {
                if let Some(page) = weak.upgrade() {
                    if let Err(error) = result {
                        page.state.borrow_mut().error =
                            Some(format!("Restored, but the history commit failed: {error}"));
                    }
                    let path = page.state.borrow().path.clone();
                    let generation = page.list_generation.get().wrapping_add(1);
                    page.list_generation.set(generation);
                    page.client.spawn(
                        move |daemon| daemon.history(&path),
                        move |result| {
                            if let Some(page) = weak.upgrade() {
                                if page.list_generation.get() != generation {
                                    return;
                                }
                                match result {
                                    Ok(versions) => page.set_versions(versions),
                                    Err(error) => page.show_message(&error.message, true),
                                }
                            }
                        },
                    );
                }
            },
        );
    }
}

/// A theme token as a solid GTK colour.
fn colour(hex: &str) -> gdk::RGBA {
    let (red, green, blue) = theme::rgb(hex);
    gdk::RGBA::new(red as f32, green as f32, blue as f32, 1.0)
}

/// A small bar either side of a centre tick: longer to the right for words
/// added, to the left for words removed. The delta label says the exact
/// number; the bar makes the sign readable at a glance.
fn delta_bar(delta: i64, largest: f64, ink: gdk::RGBA, dim: gdk::RGBA) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(40);
    area.set_content_height(14);
    area.set_valign(gtk::Align::Center);
    area.set_can_focus(false);
    area.set_focusable(false);
    area.set_accessible_role(gtk::AccessibleRole::Img);
    let sign = if delta > 0 {
        "words added"
    } else if delta < 0 {
        "words removed"
    } else {
        "no change"
    };
    area.update_property(&[gtk::accessible::Property::Label(sign)]);
    let largest = largest.max(1.0);
    area.set_draw_func(move |_, context, width, height| {
        let centre = f64::from(width) / 2.0;
        let middle = f64::from(height) / 2.0;
        let bar = (f64::from(height) * 0.42).max(3.0);
        context.set_source_rgba(
            f64::from(dim.red()),
            f64::from(dim.green()),
            f64::from(dim.blue()),
            0.55,
        );
        context.rectangle(centre - 0.5, middle - bar / 2.0, 1.0, bar);
        let _ = context.fill();
        if delta == 0 {
            return;
        }
        let magnitude = (delta.unsigned_abs() as f64 / largest)
            .sqrt()
            .clamp(0.0, 1.0);
        let length = 3.0 + magnitude * (centre - 3.0);
        let colour = if delta > 0 { ink } else { dim };
        context.set_source_rgba(
            f64::from(colour.red()),
            f64::from(colour.green()),
            f64::from(colour.blue()),
            1.0,
        );
        if delta > 0 {
            context.rectangle(centre + 1.0, middle - bar / 2.0, length, bar);
        } else {
            context.rectangle(centre - 1.0 - length, middle - bar / 2.0, length, bar);
        }
        let _ = context.fill();
    });
    area
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{DaemonClient, FakeDaemon};
    use std::sync::Arc;

    fn entry(rev: &str, delta: i64, words: u32) -> HistoryEntry {
        HistoryEntry {
            rev: rev.into(),
            time: 1_760_000_000,
            words,
            delta,
        }
    }

    fn same_text(runs: &[DiffRun]) -> String {
        runs.iter()
            .filter(|run| run.kind != DiffKind::Removed)
            .map(|run| run.text.as_str())
            .collect()
    }

    fn old_text(runs: &[DiffRun]) -> String {
        runs.iter()
            .filter(|run| run.kind != DiffKind::Added)
            .map(|run| run.text.as_str())
            .collect()
    }

    #[test]
    fn deltas_read_as_words_not_arithmetic() {
        assert_eq!(HistoryState::delta_label(&entry("a", 86, 10)), "+86 words");
        assert_eq!(HistoryState::delta_label(&entry("a", -18, 10)), "−18 words");
        assert_eq!(HistoryState::delta_label(&entry("a", 0, 10)), "no change");
        assert_eq!(
            HistoryState::delta_label(&entry("a", 1_204, 1_204)),
            "+1,204 words"
        );
    }

    #[test]
    fn the_list_knows_the_largest_change_for_its_bars() {
        let mut state = HistoryState::new("draft.md");
        state.set_versions(vec![entry("a", 86, 10), entry("b", -1_204, 10)]);
        assert_eq!(state.largest_delta(), 1_204);
        state.set_versions(Vec::new());
        assert_eq!(state.largest_delta(), 1);
    }

    #[test]
    fn selecting_a_version_asks_for_its_preview_once() {
        let fake = FakeDaemon::new();
        let versions = fake.history("draft.md").unwrap();
        let mut state = HistoryState::new("draft.md");
        let effects = state.set_versions(versions.clone());
        assert_eq!(
            effects,
            vec![HistoryEffect::Preview {
                path: "draft.md".into(),
                rev: versions[0].rev.clone()
            }]
        );
        let HistoryEffect::Preview { path, rev } = effects[0].clone() else {
            unreachable!()
        };
        let text = fake.restore(&path, &rev).unwrap();
        state.on_preview(&rev, text.clone());
        assert_eq!(state.preview.as_deref(), Some(text.as_str()));
        assert!(state
            .move_by(1)
            .iter()
            .all(|effect| matches!(effect, HistoryEffect::Preview { .. })));
        assert_eq!(state.current().unwrap().rev, versions[1].rev);
    }

    #[test]
    fn restore_points_at_the_selected_version() {
        let fake = FakeDaemon::new();
        let versions = fake.history("draft.md").unwrap();
        let mut state = HistoryState::new("draft.md");
        state.set_versions(versions.clone());
        state.move_by(2);
        let HistoryEffect::Restore { path, rev } = state.restore_effect().unwrap() else {
            unreachable!()
        };
        assert_eq!(path, "draft.md");
        assert_eq!(rev, versions[2].rev);
        state.on_restored("Today, 14:32".into());
        assert!(state.notice.contains("The newer text stays in history"));
    }

    #[test]
    fn the_daemon_is_the_source_of_versions() {
        let fake = Arc::new(FakeDaemon::new());
        let versions = fake.history("draft.md").unwrap();
        assert!(versions[0].time > versions[1].time);
        // A missing file has no versions; the screen then explains why.
        let empty = FakeDaemon::new();
        empty.set_versions(Vec::new());
        assert!(empty.history("draft.md").unwrap().is_empty());
    }

    #[test]
    fn identical_text_is_one_unchanged_run() {
        let text = "The kettle clicks off at six.\n\n";
        let runs = diff_runs(text, text);
        assert_eq!(
            runs,
            vec![DiffRun {
                kind: DiffKind::Same,
                text: text.to_owned()
            }]
        );
    }

    #[test]
    fn a_changed_word_is_removed_then_added() {
        let old = "The kettle clicks off at five.";
        let current = "The kettle clicks off at six.";
        let runs = diff_runs(old, current);
        assert_eq!(
            runs,
            vec![
                DiffRun {
                    kind: DiffKind::Same,
                    text: "The kettle clicks off at ".into()
                },
                DiffRun {
                    kind: DiffKind::Removed,
                    text: "five.".into()
                },
                DiffRun {
                    kind: DiffKind::Added,
                    text: "six.".into()
                },
            ]
        );
        assert_eq!(same_text(&runs), current);
        assert_eq!(old_text(&runs), old);
    }

    #[test]
    fn an_added_paragraph_is_tinted_and_a_deleted_one_is_struck() {
        let old = "First line.\n\nSecond line.\n";
        let current = "First line.\n\nSecond line.\n\nA new paragraph.\n";
        let runs = diff_runs(old, current);
        assert!(runs
            .iter()
            .any(|run| run.kind == DiffKind::Added && run.text.contains("A new paragraph.")));
        assert_eq!(same_text(&runs), current);
        assert_eq!(old_text(&runs), old);

        let runs = diff_runs(current, old);
        assert!(runs
            .iter()
            .any(|run| run.kind == DiffKind::Removed && run.text.contains("A new paragraph.")));
        assert_eq!(same_text(&runs), old);
        assert_eq!(old_text(&runs), current);
    }

    #[test]
    fn empty_text_on_either_side_reads_as_a_whole_change() {
        let added = diff_runs("", "A new file.\n");
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].kind, DiffKind::Added);
        let removed = diff_runs("An old file.\n", "");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].kind, DiffKind::Removed);
        assert!(diff_runs("", "").is_empty());
    }

    #[test]
    fn a_huge_rewrite_still_yields_a_readable_diff() {
        let old = "old words here. ".repeat(8_000);
        let current = "new words there. ".repeat(8_000);
        let runs = diff_runs(&old, &current);
        assert!(runs.iter().any(|run| run.kind == DiffKind::Removed));
        assert!(runs.iter().any(|run| run.kind == DiffKind::Added));
        assert_eq!(same_text(&runs), current);
        assert_eq!(old_text(&runs), old);
    }
}
