//! The History screen: versions by time with word-count deltas, a preview,
//! and Restore behind a confirmation. Restoring never rewrites history: the
//! old text is applied as a new edit and committed afterwards.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::daemon::{Client, HistoryEntry};
use crate::editor::EditorPage;
use crate::nav::Navigator;
use crate::ui;

const HINTS: [(&str, &str); 3] = [("Enter", "Restore"), ("↑ ↓", "Versions"), ("Esc", "Back")];

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
}

pub struct HistoryPage {
    pub widget: gtk::Overlay,
    screen: Rc<ui::Screen>,
    list: gtk::ListBox,
    list_scroller: gtk::ScrolledWindow,
    preview_title: gtk::Label,
    preview: gtk::Label,
    notice: gtk::Label,
    state: RefCell<HistoryState>,
    client: Client,
    nav: Rc<Navigator>,
    editor: Rc<EditorPage>,
    list_generation: std::cell::Cell<u64>,
    preview_generation: std::cell::Cell<u64>,
}

impl HistoryPage {
    pub fn new(client: Client, nav: Rc<Navigator>, editor: Rc<EditorPage>) -> Rc<Self> {
        let screen = ui::Screen::new("History");
        screen.set_hints(&HINTS);

        let list = ui::list();
        list.set_width_request(340);
        let list_scroller = ui::scroller(&list);

        let preview_title = gtk::Label::new(None);
        preview_title.set_xalign(0.0);
        preview_title.add_css_class("preview-title");
        let preview = gtk::Label::new(None);
        preview.set_xalign(0.0);
        preview.set_yalign(0.0);
        preview.set_wrap(true);
        preview.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        preview.set_selectable(false);
        preview.add_css_class("preview-text");
        let preview_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
        preview_box.set_hexpand(true);
        preview_box.set_margin_start(24);
        preview_box.append(&preview_title);
        preview_box.append(&preview);
        let preview_scroller = ui::scroller(&preview_box);
        preview_scroller.add_css_class("preview-scroller");

        let columns = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        columns.set_vexpand(true);
        columns.append(&list_scroller);
        columns.append(&preview_scroller);
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
            preview,
            notice,
            state: RefCell::new(HistoryState::default()),
            client,
            nav,
            editor,
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
            self.show_message("Open a file to see its history.", false);
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
        self.screen.set_status(&path);
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
            self.show_message(
                "No versions yet. Carrel saves history after five quiet minutes.",
                false,
            );
            return;
        }
        let effects = self.state.borrow_mut().set_versions(versions);
        self.rebuild_list();
        self.update_preview();
        self.run_effects(effects);
    }

    fn show_loading(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.versions.clear();
            state.preview = None;
            state.preview_rev = None;
            state.error = None;
        }
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.preview_title.set_text("Loading…");
        self.preview.set_text("");
        self.update_notice();
    }

    fn show_message(&self, message: &str, error: bool) {
        {
            let mut state = self.state.borrow_mut();
            state.versions.clear();
            state.preview = None;
            state.preview_rev = None;
            state.notice.clear();
            state.error = error.then(|| message.to_owned());
            if !error {
                state.notice = message.to_owned();
            }
        }
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.preview_title.set_text("");
        self.preview.set_text(message);
        self.update_notice();
    }

    fn rebuild_list(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let state = self.state.borrow();
        for entry in &state.versions {
            let row = gtk::ListBoxRow::new();
            row.add_css_class("ui-row");
            row.add_css_class("file-row");
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let time = gtk::Label::new(Some(&ui::format_moment(entry.time)));
            time.set_xalign(0.0);
            time.set_hexpand(true);
            time.set_ellipsize(gtk::pango::EllipsizeMode::End);
            time.add_css_class("history-time");
            let detail = gtk::Label::new(Some(&HistoryState::delta_label(entry)));
            detail.set_xalign(1.0);
            detail.add_css_class("history-delta");
            line.append(&time);
            line.append(&detail);
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
        let state = self.state.borrow();
        let Some(entry) = state.current() else {
            return;
        };
        self.preview_title.set_text(&format!(
            "{} · {} words",
            ui::format_moment(entry.time),
            crate::store::stats::format_number(u64::from(entry.words))
        ));
        match &state.preview {
            Some(text) => self.preview.set_text(text),
            None => self.preview.set_text(""),
        }
        drop(state);
        self.update_notice();
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
        self.screen.ask(
            "Restore this version?",
            &format!(
                "The text from {when} replaces the current file, and the current text stays in history."
            ),
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
}
