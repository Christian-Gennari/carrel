//! The Files screen: a calm list, keyboard first, destructive actions behind a
//! confirmation dialog, and rename in place.
//!
//! Typing letters searches every document under the writing root at once, so a
//! writer never has to find a field first. Without a query, the last documents
//! opened sit above the folder tree. Conflict copies made by a sync are marked
//! and can be resolved from the shared card.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::editor::EditorPage;
use crate::files_conflict::{self, ConflictCopy};
use crate::store::{display_name, is_document_name, Entry, Store};
use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryMode {
    None,
    Rename,
    NewFolder,
    /// A short list of destination folders for the selected document.
    Move,
}

/// What a list row acts on. Header and message rows map to `None`.
#[derive(Clone, PartialEq, Eq)]
enum Target {
    Document(PathBuf),
    Folder(PathBuf),
    NewFolder,
}

/// The five hints spec/03-ui.md asks for, most useful first. Esc is implied.
const HINTS: [(&str, &str); 5] = [
    ("Enter", "Open"),
    ("Type", "Search"),
    ("F2", "Rename"),
    ("M", "Move"),
    ("Del", "Delete"),
];
const MOVE_HINTS: [(&str, &str); 3] =
    [("Enter", "Move here"), ("↑ ↓", "Choose"), ("Esc", "Cancel")];
const RENAME_HINTS: [(&str, &str); 2] = [("Enter", "Save"), ("Esc", "Cancel")];
const FOLDER_HINTS: [(&str, &str); 2] = [("Enter", "Create"), ("Esc", "Cancel")];

pub struct FilesPage {
    pub widget: gtk::Overlay,
    screen: Rc<ui::Screen>,
    list: gtk::ListBox,
    list_scroller: gtk::ScrolledWindow,
    entry: RefCell<Option<gtk::Entry>>,
    mode: Cell<EntryMode>,
    rename_target: RefCell<Option<PathBuf>>,
    current_dir: RefCell<PathBuf>,
    entries: RefCell<Vec<Entry>>,
    /// One slot per list row, parallel to the ListBox children.
    rows: RefCell<Vec<Option<Target>>>,
    query: RefCell<String>,
    move_source: RefCell<Option<PathBuf>>,
    move_dirs: RefCell<Vec<PathBuf>>,
    /// The rename entry is drawn once even when a recent row and a folder row
    /// name the same document.
    rename_row: Cell<bool>,
    /// How many list rows the Recent group occupies (header included). The
    /// folder tree starts after them.
    recent_rows: Cell<usize>,
    /// False until this page first appears. The first appearance starts on
    /// the newest recent document, so Ctrl+O then Enter reopens it.
    shown: Cell<bool>,
    status_generation: Cell<u64>,
    store: Store,
    editor: Rc<EditorPage>,
    stack: gtk::Stack,
}

impl FilesPage {
    pub fn new(store: Store, editor: Rc<EditorPage>, stack: gtk::Stack) -> Rc<Self> {
        let screen = ui::Screen::new("Files");
        screen.set_hints(&HINTS);

        let list = ui::list();
        let list_scroller = ui::scroller(&list);
        screen.content.append(&ui::fade_top(&list_scroller));

        let page = Rc::new(Self {
            widget: screen.root.clone(),
            screen,
            list,
            list_scroller,
            entry: RefCell::new(None),
            mode: Cell::new(EntryMode::None),
            rename_target: RefCell::new(None),
            current_dir: RefCell::new(PathBuf::new()),
            entries: RefCell::new(Vec::new()),
            rows: RefCell::new(Vec::new()),
            query: RefCell::new(String::new()),
            move_source: RefCell::new(None),
            move_dirs: RefCell::new(Vec::new()),
            rename_row: Cell::new(false),
            recent_rows: Cell::new(0),
            shown: Cell::new(false),
            status_generation: Cell::new(0),
            store,
            editor,
            stack,
        });
        page.install();
        page.update_status();
        page.refresh();
        page
    }

    fn install(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(page) = weak.upgrade() {
                page.activate_index(row.index());
            }
        });

        // Refresh whenever the screen is shown, so changes made elsewhere are
        // visible without a separate notification path.
        let weak = Rc::downgrade(self);
        self.widget.connect_map(move |_| {
            if let Some(page) = weak.upgrade() {
                page.refresh();
                if !page.shown.replace(true)
                    && page.query.borrow().is_empty()
                    && matches!(page.mode.get(), EntryMode::None | EntryMode::Move)
                {
                    page.select_first();
                }
                if !matches!(page.mode.get(), EntryMode::Rename | EntryMode::NewFolder) {
                    page.screen.focus_page();
                }
            }
        });
    }

    /// A delete or conflict card is open; the command palette waits for it.
    pub(crate) fn is_dialog_open(&self) -> bool {
        self.screen.is_dialog_open()
    }

    pub(crate) fn handle_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        if self.screen.is_dialog_open() {
            return self.screen.dialog_key(keyval, state);
        }

        match self.mode.get() {
            EntryMode::Rename | EntryMode::NewFolder => {
                return match keyval {
                    gdk::Key::Escape => {
                        self.cancel_entry();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Return | gdk::Key::KP_Enter => {
                        self.commit_entry();
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                };
            }
            EntryMode::Move => return self.handle_move_key(keyval, state),
            EntryMode::None => {}
        }

        let control = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let key = keyval.to_lower();

        if control && key == gdk::Key::n {
            if shift {
                self.start_new_folder();
            } else {
                self.new_file();
            }
            return glib::Propagation::Stop;
        }
        if control && key == gdk::Key::o {
            self.show_editor();
            return glib::Propagation::Stop;
        }

        match keyval {
            gdk::Key::Escape => {
                if self.query.borrow().is_empty() {
                    self.show_editor();
                } else {
                    self.clear_query();
                }
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                self.activate_selected();
                glib::Propagation::Stop
            }
            gdk::Key::Delete => {
                self.confirm_delete();
                glib::Propagation::Stop
            }
            gdk::Key::F2 => {
                self.start_rename();
                glib::Propagation::Stop
            }
            gdk::Key::BackSpace => {
                if !self.pop_query() {
                    self.go_up();
                }
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
                self.select_first();
                glib::Propagation::Stop
            }
            gdk::Key::End => {
                self.select_last();
                glib::Propagation::Stop
            }
            _ => {
                // Plain letters search; the capital M moves, so every letter
                // (including a lowercase m) can start a query.
                if shift && key == gdk::Key::m {
                    self.start_move();
                    return glib::Propagation::Stop;
                }
                if let Some(character) = typed_character(keyval, state) {
                    self.push_query(character);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        }
    }

    /// Move mode has no search and no rename: it is a short choice of folders.
    fn handle_move_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        let control = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        if control && shift && keyval.to_lower() == gdk::Key::n {
            self.start_move_new_folder();
            return glib::Propagation::Stop;
        }
        match keyval {
            gdk::Key::Escape => self.cancel_move(),
            gdk::Key::Return | gdk::Key::KP_Enter => self.activate_selected(),
            gdk::Key::Up => self.move_selection(-1),
            gdk::Key::Down => self.move_selection(1),
            gdk::Key::Home => self.select_first(),
            gdk::Key::End => self.select_last(),
            _ => {}
        }
        glib::Propagation::Stop
    }

    fn push_query(self: &Rc<Self>, character: char) {
        if self.query.borrow().is_empty() && character.is_whitespace() {
            return;
        }
        {
            let mut query = self.query.borrow_mut();
            if query.chars().count() >= 80 {
                return;
            }
            query.push(character);
        }
        self.refresh_first();
    }

    /// Returns false when there was nothing to erase, so Backspace can go up a
    /// folder instead. Esc clears the query first and leaves on the next press.
    fn pop_query(self: &Rc<Self>) -> bool {
        {
            let mut query = self.query.borrow_mut();
            if query.pop().is_none() {
                return false;
            }
        }
        self.refresh_first();
        true
    }

    fn clear_query(self: &Rc<Self>) {
        self.query.borrow_mut().clear();
        self.refresh_first();
    }

    fn move_selection(&self, delta: i32) {
        let current = self
            .list
            .selected_row()
            .map(|row| row.index())
            .unwrap_or(-1);
        let next = {
            let rows = self.rows.borrow();
            let count = rows.len() as i32;
            let mut index = current;
            loop {
                index += delta;
                if index < 0 || index >= count {
                    break None;
                }
                if rows[index as usize].is_some() {
                    break Some(index);
                }
            }
        };
        if let Some(index) = next {
            self.select_index(index);
        }
    }

    fn select_first(&self) {
        let first = self.rows.borrow().iter().position(Option::is_some);
        if let Some(index) = first {
            self.select_index(index as i32);
        }
    }

    /// After opening a folder, land on its first row, not on the Recent group
    /// above it. An empty folder leaves nothing selected; the message says why.
    fn select_first_entry(&self) {
        let start = self.recent_rows.get();
        let first = self
            .rows
            .borrow()
            .iter()
            .skip(start)
            .position(Option::is_some)
            .map(|index| index + start);
        match first {
            Some(index) => self.select_index(index as i32),
            None => self.list.unselect_all(),
        }
    }

    fn select_last(&self) {
        let last = self.rows.borrow().iter().rposition(Option::is_some);
        if let Some(index) = last {
            self.select_index(index as i32);
        }
    }

    fn select_index(&self, index: i32) {
        let count = self.list.observe_children().n_items() as i32;
        if count == 0 {
            return;
        }
        let index = index.clamp(0, count - 1);
        if self
            .rows
            .borrow()
            .get(index as usize)
            .is_none_or(|target| target.is_none())
        {
            return;
        }
        if let Some(row) = self.list.row_at_index(index) {
            self.list.select_row(Some(&row));
            self.scroll_to_row(&row);
        }
    }

    fn scroll_to_row(&self, row: &gtk::ListBoxRow) {
        ui::scroll_to_row(&self.list_scroller, &self.list, row);
    }

    fn select_rel(&self, rel: &Path) {
        let index = self
            .rows
            .borrow()
            .iter()
            .position(|target| target_path(target) == Some(rel));
        if let Some(index) = index {
            self.select_index(index as i32);
        }
    }

    fn selected_target(&self) -> Option<Target> {
        let index = self.list.selected_row()?.index().max(0) as usize;
        self.rows.borrow().get(index).cloned().flatten()
    }

    fn show_editor(&self) {
        self.stack.set_visible_child_name("editor");
        self.editor.focus_editor();
    }

    fn refresh(self: &Rc<Self>) {
        let previous = self
            .selected_target()
            .and_then(|target| target_path(&Some(target)).map(Path::to_path_buf));
        self.reload(previous);
    }

    /// Rebuild after a query edit: the first and best match is selected, not
    /// whichever row happened to be selected under the old query.
    fn refresh_first(self: &Rc<Self>) {
        self.reload(None);
    }

    fn reload(self: &Rc<Self>, keep: Option<PathBuf>) {
        let dir = self.current_dir.borrow().clone();
        match self.store.list(&dir) {
            Ok(entries) => {
                self.entries.replace(entries);
                self.update_status();
                self.rebuild(keep);
            }
            Err(error) => self.flash_error(format!("Cannot list folder: {error}")),
        }
    }

    fn update_status(&self) {
        let query = self.query.borrow().trim().to_string();
        let source = self.move_source.borrow().clone();
        if let Some(source) = source {
            let name = source
                .file_name()
                .map(|name| display_name(&name.to_string_lossy()))
                .unwrap_or_default();
            if self.mode.get() == EntryMode::NewFolder {
                self.screen.set_status(&format!("New folder for “{name}”"));
            } else {
                self.screen.set_status(&format!("Move “{name}” to…"));
            }
        } else if !query.is_empty() {
            self.screen.set_status(&format!("Search “{query}”"));
        } else {
            self.screen
                .set_status(&display_path(&self.current_dir.borrow()));
        }
    }

    fn sync_hints(&self) {
        let hints: &[(&str, &str)] = match self.mode.get() {
            EntryMode::Move => &MOVE_HINTS,
            EntryMode::Rename => &RENAME_HINTS,
            EntryMode::NewFolder => &FOLDER_HINTS,
            EntryMode::None => &HINTS,
        };
        self.screen.set_hints(hints);
    }

    /// Draw the list. In rename mode the selected row itself becomes a field,
    /// so the name is edited where the writer sees it.
    fn rebuild(self: &Rc<Self>, keep: Option<PathBuf>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.rows.borrow_mut().clear();
        self.rename_row.set(false);
        self.recent_rows.set(0);
        *self.entry.borrow_mut() = None;

        let mode = self.mode.get();
        let query = self.query.borrow().trim().to_string();
        let entries = self.entries.borrow().clone();
        let moving_folder = mode == EntryMode::NewFolder && self.move_source.borrow().is_some();

        if mode == EntryMode::NewFolder {
            let row = self.row_shell();
            let field = self.make_entry("", Some("Folder name"));
            row.set_child(Some(&field));
            self.push_row(row, Some(Target::NewFolder));
            *self.entry.borrow_mut() = Some(field);
        }

        if mode == EntryMode::Move {
            self.build_move_rows(true);
        } else if moving_folder {
            self.build_move_rows(false);
        } else if !query.is_empty() {
            self.build_search_rows(&query);
        } else {
            self.build_normal_rows(&entries);
        }

        let selected = if mode == EntryMode::NewFolder {
            Some(0)
        } else {
            self.selection_index(mode, keep.as_deref())
        };
        if let Some(index) = selected {
            ui::select(&self.list, index);
            if let Some(row) = self.list.row_at_index(index as i32) {
                self.scroll_to_row(&row);
            }
        }

        if let Some(field) = self.entry.borrow().clone() {
            field.grab_focus();
            field.select_region(0, -1);
        } else {
            self.screen.focus_page();
        }
    }

    fn selection_index(&self, mode: EntryMode, keep: Option<&Path>) -> Option<usize> {
        let target = if mode == EntryMode::Rename {
            self.rename_target.borrow().clone()
        } else {
            keep.map(Path::to_path_buf)
        };
        if let Some(target) = target {
            if let Some(index) = self
                .rows
                .borrow()
                .iter()
                .position(|row| target_path(row) == Some(target.as_path()))
            {
                return Some(index);
            }
        }
        self.rows.borrow().iter().position(Option::is_some)
    }

    fn build_normal_rows(self: &Rc<Self>, entries: &[Entry]) {
        let recent = self.store.recent_files();
        if !recent.is_empty() {
            self.push_header("Recent");
            for rel in &recent {
                self.push_document(rel, true);
            }
            // The folder tree starts here.
            self.recent_rows.set(self.rows.borrow().len());
        }
        for entry in entries {
            if entry.is_dir {
                self.push_folder(&entry.rel, &entry.name);
            } else {
                self.push_document(&entry.rel, false);
            }
        }
        if entries.is_empty() {
            self.push_state("No files in this folder yet", "Ctrl+N starts one here.");
        }
    }

    fn build_search_rows(self: &Rc<Self>, query: &str) {
        let mut documents = Vec::new();
        collect_documents(&self.store, Path::new(""), &mut documents);
        let mut hits: Vec<(PathBuf, i64)> = documents
            .into_iter()
            .filter_map(|rel| {
                let name = rel.file_name()?.to_string_lossy().to_string();
                // The full name includes the conflict suffix, so typing
                // "conflict" finds a copy even though the row shows its base.
                let key = display_name(&name);
                fuzzy_score(&key, query).map(|score| (rel, score))
            })
            .collect();
        hits.sort_by(|(left_path, left_score), (right_path, right_score)| {
            left_score.cmp(right_score).then_with(|| {
                left_path
                    .to_string_lossy()
                    .to_lowercase()
                    .cmp(&right_path.to_string_lossy().to_lowercase())
            })
        });
        if hits.is_empty() {
            self.push_state(
                &format!("No documents match “{query}”"),
                "Esc clears the search.",
            );
            return;
        }
        for (rel, _) in hits {
            self.push_document(&rel, true);
        }
    }

    fn build_move_rows(self: &Rc<Self>, include_new_folder: bool) {
        let dirs = self.move_dirs.borrow().clone();
        for dir in dirs {
            let (row, _) = self.make_destination_row(&dir);
            self.push_row(row, Some(Target::Folder(dir)));
        }
        if !include_new_folder {
            return;
        }
        let row = self.row_shell();
        let label = gtk::Label::new(Some("New folder…"));
        label.set_xalign(0.0);
        label.add_css_class("file-name");
        label.set_hexpand(true);
        label.set_ellipsize(pango::EllipsizeMode::End);
        row.set_child(Some(&label));
        self.push_row(row, Some(Target::NewFolder));
    }

    fn push_header(self: &Rc<Self>, text: &str) {
        self.push_row(ui::group_row(text), None);
    }

    /// An empty list is a state, not a sentence in a row: one headline and one
    /// small detail line, the same shape the finished Export uses. It sits on
    /// the page's own left edge, like the title above it.
    fn push_state(self: &Rc<Self>, headline: &str, detail: &str) {
        let state = ui::State::new();
        state.set(headline, &[detail], false);
        let row = gtk::ListBoxRow::new();
        row.set_selectable(false);
        row.set_activatable(false);
        row.set_can_focus(false);
        state.widget.set_margin_bottom(10);
        row.set_child(Some(&state.widget));
        self.push_row(row, None);
    }

    fn push_document(self: &Rc<Self>, rel: &Path, show_path: bool) {
        let (row, field) = self.make_document_row(rel, show_path);
        if let Some(field) = field {
            *self.entry.borrow_mut() = Some(field);
        }
        self.push_row(row, Some(Target::Document(rel.to_path_buf())));
    }

    fn push_folder(self: &Rc<Self>, rel: &Path, name: &str) {
        let (row, field) = self.make_folder_row(rel, name);
        if let Some(field) = field {
            *self.entry.borrow_mut() = Some(field);
        }
        self.push_row(row, Some(Target::Folder(rel.to_path_buf())));
    }

    fn push_row(self: &Rc<Self>, row: gtk::ListBoxRow, target: Option<Target>) {
        self.rows.borrow_mut().push(target);
        self.list.append(&row);
    }

    fn row_shell(&self) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("ui-row");
        row.add_css_class("file-row");
        row
    }

    fn is_renaming(&self, rel: &Path) -> bool {
        if self.rename_row.get() {
            return false;
        }
        if self.mode.get() == EntryMode::Rename
            && self.rename_target.borrow().as_deref() == Some(rel)
        {
            self.rename_row.set(true);
            true
        } else {
            false
        }
    }

    fn make_document_row(
        self: &Rc<Self>,
        rel: &Path,
        show_path: bool,
    ) -> (gtk::ListBoxRow, Option<gtk::Entry>) {
        let name = rel
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let copy = files_conflict::parse(&name);
        let shown = copy
            .as_ref()
            .map(|copy| copy.base.clone())
            .unwrap_or_else(|| display_name(&name));
        let row = self.row_shell();
        if self.is_renaming(rel) {
            let field = self.make_entry(&editable_name(rel), None);
            row.set_child(Some(&field));
            return (row, Some(field));
        }

        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let label = gtk::Label::new(Some(&shown));
        label.set_xalign(0.0);
        label.set_ellipsize(pango::EllipsizeMode::End);
        label.add_css_class("file-name");
        line.append(&label);
        let mut expanded = false;
        if show_path {
            if let Some(parent) = rel.parent() {
                let path = folder_label(parent);
                if !path.is_empty() {
                    let note = gtk::Label::new(Some(&path));
                    note.set_xalign(0.0);
                    note.set_hexpand(true);
                    note.set_ellipsize(pango::EllipsizeMode::End);
                    note.add_css_class("row-note");
                    line.append(&note);
                    expanded = true;
                }
            }
        }
        if !expanded {
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_hexpand(true);
            line.append(&spacer);
        }
        if copy.is_some() {
            let tag = gtk::Label::new(Some("Conflict"));
            tag.set_valign(gtk::Align::Center);
            tag.add_css_class("conflict-tag");
            line.append(&tag);
        }
        row.set_child(Some(&line));
        (row, None)
    }

    fn make_folder_row(
        self: &Rc<Self>,
        rel: &Path,
        name: &str,
    ) -> (gtk::ListBoxRow, Option<gtk::Entry>) {
        let row = self.row_shell();
        if self.is_renaming(rel) {
            let field = self.make_entry(name, None);
            row.set_child(Some(&field));
            return (row, Some(field));
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let label = gtk::Label::new(Some(&format!("{name}/")));
        label.set_xalign(0.0);
        label.set_ellipsize(pango::EllipsizeMode::End);
        label.add_css_class("folder-name");
        line.append(&label);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        line.append(&spacer);
        row.set_child(Some(&line));
        (row, None)
    }

    fn make_destination_row(self: &Rc<Self>, dir: &Path) -> (gtk::ListBoxRow, Option<gtk::Entry>) {
        let row = self.row_shell();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let name = if dir.as_os_str().is_empty() {
            "Writing".to_owned()
        } else {
            format!(
                "{}/",
                dir.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default()
            )
        };
        let label = gtk::Label::new(Some(&name));
        label.set_xalign(0.0);
        label.set_ellipsize(pango::EllipsizeMode::End);
        label.add_css_class("folder-name");
        line.append(&label);
        let note_text = match dir.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => folder_label(parent),
            _ if dir.as_os_str().is_empty() => "top level".to_owned(),
            _ => String::new(),
        };
        let note = gtk::Label::new(Some(&note_text));
        note.set_xalign(0.0);
        note.set_hexpand(true);
        note.set_ellipsize(pango::EllipsizeMode::End);
        note.add_css_class("row-note");
        line.append(&note);
        row.set_child(Some(&line));
        (row, None)
    }

    fn make_entry(self: &Rc<Self>, text: &str, placeholder: Option<&str>) -> gtk::Entry {
        let entry = gtk::Entry::new();
        entry.set_text(text);
        entry.set_placeholder_text(placeholder);
        entry.set_hexpand(true);
        entry.add_css_class("renaming");
        let weak = Rc::downgrade(self);
        entry.connect_activate(move |_| {
            if let Some(page) = weak.upgrade() {
                page.commit_entry();
            }
        });
        entry
    }

    fn activate_selected(self: &Rc<Self>) {
        if let Some(row) = self.list.selected_row() {
            self.activate_index(row.index());
        }
    }

    fn activate_index(self: &Rc<Self>, index: i32) {
        // While a name is being edited, a stray double click must not leave
        // the screen or open a folder behind the entry.
        if matches!(self.mode.get(), EntryMode::Rename | EntryMode::NewFolder) {
            return;
        }
        let target = self
            .rows
            .borrow()
            .get(index.max(0) as usize)
            .cloned()
            .flatten();
        let Some(target) = target else {
            return;
        };
        match target {
            Target::Document(rel) => self.open_document(&rel),
            Target::Folder(rel) => {
                if self.mode.get() == EntryMode::Move {
                    self.move_selected_to(&rel);
                } else {
                    *self.current_dir.borrow_mut() = rel;
                    self.refresh_first();
                    self.select_first_entry();
                }
            }
            Target::NewFolder => {
                if self.mode.get() == EntryMode::Move {
                    self.start_move_new_folder();
                } else {
                    self.start_new_folder();
                }
            }
        }
    }

    fn open_document(self: &Rc<Self>, rel: &Path) {
        let name = rel
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if !is_document_name(&name) {
            return;
        }
        if files_conflict::parse(&name).is_some() {
            self.ask_conflict(rel);
            return;
        }
        if let Err(error) = self.editor.open_rel(rel) {
            self.flash_error(error);
            return;
        }
        self.show_editor();
    }

    fn go_up(self: &Rc<Self>) {
        let child = self.current_dir.borrow().clone();
        let parent = child.parent().map(Path::to_path_buf).unwrap_or_default();
        if parent == *self.current_dir.borrow() {
            return;
        }
        *self.current_dir.borrow_mut() = parent;
        // The folder that was open stays selected in its parent.
        self.reload(Some(child));
    }

    fn new_file(self: &Rc<Self>) {
        let dir = match self.selected_target() {
            Some(Target::Folder(rel)) => rel,
            Some(Target::Document(rel)) => rel
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(PathBuf::new),
            _ => self.current_dir.borrow().clone(),
        };
        match self.editor.create_new_file_in(&dir) {
            Ok(()) => self.show_editor(),
            Err(error) => self.flash_error(error),
        }
    }

    fn start_new_folder(self: &Rc<Self>) {
        self.query.borrow_mut().clear();
        *self.move_source.borrow_mut() = None;
        self.mode.set(EntryMode::NewFolder);
        *self.rename_target.borrow_mut() = None;
        self.sync_hints();
        self.reload(None);
    }

    fn start_rename(self: &Rc<Self>) {
        let Some(target) = self.selected_target() else {
            return;
        };
        let rel = match target {
            Target::Document(rel) | Target::Folder(rel) => rel,
            Target::NewFolder => return,
        };
        self.mode.set(EntryMode::Rename);
        *self.rename_target.borrow_mut() = Some(rel.clone());
        self.sync_hints();
        self.rebuild(Some(rel));
    }

    fn cancel_entry(self: &Rc<Self>) {
        let mode = self.mode.get();
        let keep = self.rename_target.borrow().clone();
        if mode == EntryMode::NewFolder && self.move_source.borrow().is_some() {
            self.finish_entry();
            self.mode.set(EntryMode::Move);
            self.sync_hints();
            self.update_status();
            self.rebuild(None);
            return;
        }
        self.finish_entry();
        self.sync_hints();
        self.update_status();
        if mode == EntryMode::Rename {
            self.rebuild(keep);
        } else {
            self.refresh();
        }
    }

    fn commit_entry(self: &Rc<Self>) {
        let Some(field) = self.entry.borrow().clone() else {
            return;
        };
        let name = field.text().trim().to_string();
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
            self.flash_error("Choose a name without / or a leading dot.");
            return;
        }

        match self.mode.get() {
            EntryMode::None | EntryMode::Move => {}
            EntryMode::NewFolder => {
                let rel = self.current_dir.borrow().join(&name);
                match self.store.create_dir(&rel) {
                    Ok(()) => {
                        let moving = self.move_source.borrow().clone();
                        self.finish_entry();
                        self.sync_hints();
                        if let Some(source) = moving {
                            if !self.move_document(&source, &rel) {
                                self.mode.set(EntryMode::Move);
                                self.sync_hints();
                                self.update_status();
                                self.rebuild(None);
                            }
                        } else {
                            self.refresh();
                            self.select_rel(&rel);
                        }
                    }
                    Err(error) => self.flash_error(format!("Cannot create folder: {error}")),
                }
            }
            EntryMode::Rename => {
                let Some(source) = self.rename_target.borrow().clone() else {
                    return;
                };
                let source_is_file = self.store.root().join(&source).is_file();
                let new_name = ensure_document_extension(name, &source, source_is_file);
                let target = source
                    .parent()
                    .map_or_else(|| PathBuf::from(&new_name), |parent| parent.join(&new_name));
                if target == source {
                    self.finish_entry();
                    self.sync_hints();
                    self.rebuild(Some(target));
                    return;
                }

                let reopen = self.editor.close_if_affected(&source);
                match self.store.rename(&source, &target) {
                    Ok(()) => {
                        self.finish_entry();
                        self.sync_hints();
                        self.refresh();
                        self.select_rel(&target);
                        if let Some(old) = reopen {
                            let moved = moved_path(&old, &source, &target);
                            if let Err(error) = self.editor.open_rel(&moved) {
                                eprintln!("carrel: cannot reopen {}: {error}", moved.display());
                            }
                        }
                    }
                    Err(error) => {
                        if let Some(old) = reopen {
                            let _ = self.editor.open_rel(&old);
                        }
                        self.flash_error(format!("Cannot rename: {error}"));
                    }
                }
            }
        }
    }

    fn finish_entry(&self) {
        self.mode.set(EntryMode::None);
        *self.rename_target.borrow_mut() = None;
        *self.entry.borrow_mut() = None;
    }

    fn confirm_delete(self: &Rc<Self>) {
        let Some(target) = self.selected_target() else {
            return;
        };
        let (rel, is_dir) = match target {
            Target::Document(rel) => (rel, false),
            Target::Folder(rel) => (rel, true),
            Target::NewFolder => return,
        };
        let name = rel
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let conflict = files_conflict::parse(&name);
        let title = match &conflict {
            Some(copy) => format!("Delete the conflict copy of “{}”?", copy.base),
            None => format!("Delete “{}”?", display_name(&name)),
        };
        let sentence = if is_dir {
            "The folder and its files move to the device's trash, so nothing is erased."
        } else if conflict.is_some() {
            "It moves to the device's trash; the other version stays where it is."
        } else {
            "It moves to the device's trash, so nothing is erased."
        };
        let weak = Rc::downgrade(self);
        self.screen.ask(&title, sentence, "Delete", move || {
            if let Some(page) = weak.upgrade() {
                page.delete_rel(&rel);
            }
        });
    }

    fn delete_rel(self: &Rc<Self>, rel: &Path) {
        let neighbor = self.neighbor_after(rel);
        let affected = self.editor.close_if_affected(rel).is_some();
        match self.store.delete(rel) {
            Ok(()) => {
                self.refresh();
                if let Some(neighbor) = neighbor {
                    self.select_rel(&neighbor);
                }
                if affected {
                    if let Err(error) = self.editor.open_last_or_new() {
                        eprintln!("carrel: cannot open another file: {error}");
                    }
                }
            }
            Err(error) => self.flash_error(format!("Cannot delete: {error}")),
        }
    }

    /// The row that takes the deleted row's place: the next entry in the same
    /// folder, else the previous one. `None` when the row was elsewhere.
    fn neighbor_after(&self, rel: &Path) -> Option<PathBuf> {
        let entries = self.entries.borrow();
        let index = entries
            .iter()
            .position(|entry| entry.rel.as_path() == rel)?;
        entries
            .get(index + 1)
            .or_else(|| index.checked_sub(1).and_then(|index| entries.get(index)))
            .map(|entry| entry.rel.clone())
    }

    /// The short list of places the selected document can go. Its current
    /// folder is not offered: moving a file to where it already is is not a
    /// choice worth a row.
    fn start_move(self: &Rc<Self>) {
        let Some(Target::Document(source)) = self.selected_target() else {
            self.flash_message("Choose a document to move.");
            return;
        };
        let parent = source
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(PathBuf::new);
        let mut dirs = vec![PathBuf::new()];
        collect_dirs(&self.store, Path::new(""), &mut dirs);
        dirs.retain(|dir| dir != &parent);
        self.move_dirs.replace(dirs);
        *self.move_source.borrow_mut() = Some(source);
        self.mode.set(EntryMode::Move);
        self.sync_hints();
        self.update_status();
        self.rebuild(None);
    }

    fn start_move_new_folder(self: &Rc<Self>) {
        self.mode.set(EntryMode::NewFolder);
        self.sync_hints();
        self.update_status();
        self.rebuild(None);
    }

    fn cancel_move(self: &Rc<Self>) {
        *self.move_source.borrow_mut() = None;
        self.mode.set(EntryMode::None);
        self.sync_hints();
        self.update_status();
        self.refresh();
    }

    fn move_selected_to(self: &Rc<Self>, dest_dir: &Path) {
        let Some(source) = self.move_source.borrow().clone() else {
            return;
        };
        if !self.move_document(&source, dest_dir) {
            self.rebuild(None);
        }
    }

    /// Move a document and reopen it if the editor was showing it. Returns
    /// false and shows the reason when the move failed.
    fn move_document(self: &Rc<Self>, source: &Path, dest_dir: &Path) -> bool {
        let Some(file_name) = source.file_name().map(PathBuf::from) else {
            return false;
        };
        let target = dest_dir.join(file_name);
        if target == source {
            return true;
        }
        let reopen = self.editor.close_if_affected(source);
        match self.store.rename(source, &target) {
            Ok(()) => {
                *self.move_source.borrow_mut() = None;
                self.mode.set(EntryMode::None);
                self.sync_hints();
                self.update_status();
                self.refresh();
                self.select_rel(&target);
                if let Some(old) = reopen {
                    let moved = moved_path(&old, source, &target);
                    if let Err(error) = self.editor.open_rel(&moved) {
                        eprintln!("carrel: cannot reopen {}: {error}", moved.display());
                    }
                }
                true
            }
            Err(error) => {
                if let Some(old) = reopen {
                    let _ = self.editor.open_rel(&old);
                }
                self.flash_error(format!("Cannot move: {error}"));
                false
            }
        }
    }

    /// A conflict copy is never opened as a normal document: Enter offers the
    /// three ways out, and Esc leaves both files alone.
    fn ask_conflict(self: &Rc<Self>, rel: &Path) {
        let name = rel
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(copy) = files_conflict::parse(&name) else {
            return;
        };
        let Some(original) = files_conflict::original_rel(rel) else {
            return;
        };
        let source = if copy.source.trim().is_empty() {
            "another device".to_owned()
        } else {
            copy.source.clone()
        };
        let title = format!("Two versions of “{}”", copy.base);
        let sentence =
            format!("The version from {source} and yours both changed. Keep one, or keep both.");
        let weak = Rc::downgrade(self);
        let rel = rel.to_path_buf();
        self.screen.ask_choices(
            &title,
            &sentence,
            &["Keep mine", "Keep theirs", "Keep both"],
            2,
            move |choice| {
                if let Some(page) = weak.upgrade() {
                    page.resolve_conflict(&rel, &original, &copy, choice);
                }
            },
        );
    }

    fn resolve_conflict(
        self: &Rc<Self>,
        copy: &Path,
        original: &Path,
        info: &ConflictCopy,
        choice: usize,
    ) {
        match choice {
            // Keep mine: the local document stays, the other side's copy goes.
            0 => {
                let affected = self.editor.close_if_affected(copy).is_some();
                match self.store.delete(copy) {
                    Ok(()) => {
                        self.refresh();
                        if affected {
                            if let Err(error) = self.editor.open_last_or_new() {
                                eprintln!("carrel: cannot open another file: {error}");
                            }
                        }
                    }
                    Err(error) => self.flash_error(format!("Cannot delete the copy: {error}")),
                }
            }
            // Keep theirs: the other side's copy becomes the document.
            1 => {
                let affected = self.editor.close_if_affected(copy).is_some()
                    || self.editor.close_if_affected(original).is_some();
                let original_path = self.store.root().join(original);
                let result = if original_path.exists() {
                    self.store
                        .delete(original)
                        .and_then(|()| self.store.rename(copy, original))
                } else {
                    self.store.rename(copy, original)
                };
                match result {
                    Ok(()) => {
                        self.refresh();
                        self.select_rel(original);
                        if affected {
                            if let Err(error) = self.editor.open_rel(original) {
                                eprintln!("carrel: cannot reopen {}: {error}", original.display());
                            }
                        }
                    }
                    Err(error) => {
                        if affected {
                            let _ = self.editor.open_last_or_new();
                        }
                        self.flash_error(format!("Cannot keep the other version: {error}"));
                    }
                }
            }
            // Keep both: the copy gets an ordinary name and stays a document.
            _ => {
                let parent = copy.parent().map(Path::to_path_buf).unwrap_or_default();
                let extension = copy
                    .extension()
                    .map(|extension| extension.to_string_lossy().to_string())
                    .unwrap_or_else(|| "md".to_owned());
                let target = self.store.unique_rel_path(&parent, &info.base, &extension);
                let target = match target {
                    Ok(target) => target,
                    Err(error) => {
                        self.flash_error(format!("Cannot keep both: {error}"));
                        return;
                    }
                };
                let affected = self.editor.close_if_affected(copy).is_some();
                match self.store.rename(copy, &target) {
                    Ok(()) => {
                        self.refresh();
                        self.select_rel(&target);
                        if affected {
                            if let Err(error) = self.editor.open_rel(&target) {
                                eprintln!("carrel: cannot reopen {}: {error}", target.display());
                            }
                        }
                    }
                    Err(error) => {
                        if affected {
                            let _ = self.editor.open_last_or_new();
                        }
                        self.flash_error(format!("Cannot keep both: {error}"));
                    }
                }
            }
        }
    }

    fn flash_error(self: &Rc<Self>, message: impl Into<String>) {
        self.flash_status(message, true);
    }

    fn flash_message(self: &Rc<Self>, message: impl Into<String>) {
        self.flash_status(message, false);
    }

    fn flash_status(self: &Rc<Self>, message: impl Into<String>, error: bool) {
        let message = message.into();
        let generation = self.status_generation.get().wrapping_add(1);
        self.status_generation.set(generation);
        if error {
            self.screen.set_status_error(&message);
        } else {
            self.screen.set_status(&message);
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(4), move || {
            if let Some(page) = weak.upgrade() {
                if page.status_generation.get() == generation {
                    page.update_status();
                }
            }
        });
    }
}

fn target_path(target: &Option<Target>) -> Option<&Path> {
    match target {
        Some(Target::Document(path) | Target::Folder(path)) => Some(path.as_path()),
        _ => None,
    }
}

/// A lowercase, fuzzy match: a contiguous hit wins, and a subsequence still
/// finds "tqm" in "The quiet morning". Returns a lower-is-better score.
fn fuzzy_score(name: &str, query: &str) -> Option<i64> {
    let name = name.to_lowercase();
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return None;
    }
    let mut score = if let Some(position) = name.find(&query) {
        position as i64
    } else {
        let mut at = 0;
        let mut last_end = None;
        let mut score = 100;
        for needle in query.chars() {
            let (position, _) = name
                .char_indices()
                .skip_while(|(index, _)| *index < at)
                .find(|(_, character)| *character == needle)?;
            if last_end == Some(position) {
                score -= 3;
            }
            score += position.saturating_sub(at) as i64;
            at = position + needle.len_utf8();
            last_end = Some(at);
        }
        score
    };
    // Among equally good hits, a shorter name is the likelier one.
    score += name.chars().count() as i64 / 8;
    Some(score)
}

/// Every document under `dir`, relative to the writing root. Hidden entries
/// are skipped by `Store::list`, and only documents are returned.
fn collect_documents(store: &Store, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = store.list(dir) else {
        return;
    };
    for entry in entries {
        if entry.is_dir {
            collect_documents(store, &entry.rel, out);
        } else {
            out.push(entry.rel);
        }
    }
}

/// Every folder under `dir`, in the same order the folder tree shows them.
fn collect_dirs(store: &Store, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = store.list(dir) else {
        return;
    };
    for entry in entries {
        if entry.is_dir {
            out.push(entry.rel.clone());
            collect_dirs(store, &entry.rel, out);
        }
    }
}

fn folder_label(dir: &Path) -> String {
    if dir.as_os_str().is_empty() {
        String::new()
    } else {
        format!("{}/", dir.display())
    }
}

/// The printable character a key press means, or `None` for a command chord,
/// a function key or an editing key. Shift is allowed: it is how the writer
/// types a capital in the query (and capital M is Move).
fn typed_character(keyval: gdk::Key, state: gdk::ModifierType) -> Option<char> {
    if state.intersects(
        gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK,
    ) {
        return None;
    }
    let character = keyval.to_unicode()?;
    if character.is_control() {
        return None;
    }
    if state.contains(gdk::ModifierType::SHIFT_MASK) && character.is_ascii_lowercase() {
        Some(character.to_ascii_uppercase())
    } else {
        Some(character)
    }
}

fn display_path(dir: &Path) -> String {
    if dir.as_os_str().is_empty() {
        "Writing".to_owned()
    } else {
        format!("Writing / {}", dir.display())
    }
}

/// A document is edited by its name, not its extension: "Notes.md" shows
/// "Notes" in the row, and the extension comes back on save.
fn editable_name(rel: &Path) -> String {
    rel.file_name()
        .map(|name| display_name(&name.to_string_lossy()))
        .unwrap_or_default()
}

/// Where an open document moves to when its file or parent folder is renamed.
fn moved_path(current: &Path, from: &Path, to: &Path) -> PathBuf {
    if current == from {
        to.to_path_buf()
    } else if let Ok(suffix) = current.strip_prefix(from) {
        if suffix.as_os_str().is_empty() {
            to.to_path_buf()
        } else {
            to.join(suffix)
        }
    } else {
        current.to_path_buf()
    }
}

/// A renamed document keeps being a document the Files screen can show.
fn ensure_document_extension(name: String, source: &Path, source_is_file: bool) -> String {
    if !source_is_file || is_document_name(&name) {
        return name;
    }
    let extension = source
        .extension()
        .filter(|extension| !extension.is_empty())
        .map(|extension| extension.to_string_lossy().to_string())
        .unwrap_or_else(|| "md".to_owned());
    format!("{name}.{extension}")
}

#[cfg(test)]
mod tests {
    use super::{
        editable_name, ensure_document_extension, fuzzy_score, moved_path, typed_character,
    };
    use gtk::gdk;
    use std::path::{Path, PathBuf};

    #[test]
    fn renames_keep_the_open_file() {
        assert_eq!(
            moved_path(Path::new("a.md"), Path::new("a.md"), Path::new("b.md")),
            PathBuf::from("b.md")
        );
    }

    #[test]
    fn folder_renames_move_nested_files() {
        assert_eq!(
            moved_path(
                Path::new("Novel/one.md"),
                Path::new("Novel"),
                Path::new("Book")
            ),
            PathBuf::from("Book/one.md")
        );
    }

    #[test]
    fn rename_keeps_a_markdown_extension() {
        assert_eq!(
            ensure_document_extension("Notes".to_owned(), Path::new("old.md"), true),
            "Notes.md"
        );
        assert_eq!(
            ensure_document_extension("Notes".to_owned(), Path::new("old.txt"), true),
            "Notes.txt"
        );
        assert_eq!(
            ensure_document_extension("Notes.doc".to_owned(), Path::new("old.md"), true),
            "Notes.doc.md"
        );
        assert_eq!(
            ensure_document_extension("Notes.markdown".to_owned(), Path::new("old.md"), true),
            "Notes.markdown"
        );
        assert_eq!(
            ensure_document_extension("Book".to_owned(), Path::new("Novel"), false),
            "Book"
        );
    }

    #[test]
    fn rename_shows_the_name_without_the_extension() {
        assert_eq!(editable_name(Path::new("Notes.md")), "Notes");
        assert_eq!(
            editable_name(Path::new("The quiet morning.md")),
            "The quiet morning"
        );
        assert_eq!(editable_name(Path::new("Novel")), "Novel");
    }

    #[test]
    fn search_is_fuzzy_and_case_insensitive() {
        assert!(fuzzy_score("The quiet morning", "quiet").is_some());
        assert!(fuzzy_score("The quiet morning", "QUIET").is_some());
        assert!(fuzzy_score("The quiet morning", "tqm").is_some());
        assert!(fuzzy_score("Chapter one", "chapter").is_some());
        assert!(fuzzy_score("Chapter one", "zzz").is_none());
        // A contiguous hit sorts before a scattered one.
        assert!(
            fuzzy_score("The quiet morning", "quiet") < fuzzy_score("The quiet morning", "qit")
        );
    }

    #[test]
    fn plain_letters_are_typed_and_commands_are_not() {
        assert_eq!(
            typed_character(gdk::Key::q, gdk::ModifierType::empty()),
            Some('q')
        );
        assert_eq!(
            typed_character(gdk::Key::space, gdk::ModifierType::empty()),
            Some(' ')
        );
        assert_eq!(
            typed_character(gdk::Key::q, gdk::ModifierType::CONTROL_MASK),
            None
        );
        assert_eq!(
            typed_character(gdk::Key::F2, gdk::ModifierType::empty()),
            None
        );
        assert_eq!(
            typed_character(gdk::Key::Return, gdk::ModifierType::empty()),
            None
        );
    }
}
