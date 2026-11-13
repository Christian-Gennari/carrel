//! The Files screen: a calm list, keyboard first, destructive actions behind a
//! confirmation dialog, and rename in place.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::editor::EditorPage;
use crate::store::{display_name, is_document_name, Entry, Store};
use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryMode {
    None,
    Rename,
    NewFolder,
}

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
    status_generation: Cell<u64>,
    store: Store,
    editor: Rc<EditorPage>,
    stack: gtk::Stack,
}

impl FilesPage {
    pub fn new(store: Store, editor: Rc<EditorPage>, stack: gtk::Stack) -> Rc<Self> {
        let screen = ui::Screen::new("Files");
        screen.set_hints(&[
            ("Enter", "Open"),
            ("F2", "Rename"),
            ("Del", "Delete"),
            ("Ctrl+N", "New"),
            ("Esc", "Back"),
        ]);

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
            status_generation: Cell::new(0),
            store,
            editor,
            stack,
        });
        page.install();
        page.refresh();
        page
    }

    fn install(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(page) = weak.upgrade() {
                page.open_index(row.index());
            }
        });

        // Refresh whenever the screen is shown, so changes made elsewhere are
        // visible without a separate notification path.
        let weak = Rc::downgrade(self);
        self.widget.connect_map(move |_| {
            if let Some(page) = weak.upgrade() {
                page.refresh();
                if page.mode.get() == EntryMode::None {
                    page.screen.focus_page();
                }
            }
        });
    }

    pub(crate) fn handle_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        if self.screen.is_dialog_open() {
            return self.screen.dialog_key(keyval, state);
        }

        if self.mode.get() != EntryMode::None {
            match keyval {
                gdk::Key::Escape => {
                    self.cancel_entry();
                    return glib::Propagation::Stop;
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    self.commit_entry();
                    return glib::Propagation::Stop;
                }
                _ => return glib::Propagation::Proceed,
            }
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
                self.show_editor();
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
                self.go_up();
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
                self.select_index(0);
                glib::Propagation::Stop
            }
            gdk::Key::End => {
                self.select_index(i32::MAX);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn move_selection(&self, delta: i32) {
        let current = self
            .list
            .selected_row()
            .map(|row| row.index())
            .unwrap_or(-1);
        self.select_index(current.saturating_add(delta));
    }

    fn select_index(&self, index: i32) {
        let count = self.list.observe_children().n_items() as i32;
        if count == 0 {
            return;
        }
        let index = index.clamp(0, count - 1);
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
            .entries
            .borrow()
            .iter()
            .position(|entry| entry.rel == rel);
        if let Some(index) = index {
            let offset = i32::from(self.mode.get() == EntryMode::NewFolder);
            self.select_index(index as i32 + offset);
        }
    }

    fn selected_rel(&self) -> Option<PathBuf> {
        let index = self.list.selected_row()?.index().max(0) as usize;
        if self.mode.get() == EntryMode::NewFolder {
            index.checked_sub(1)
        } else {
            Some(index)
        }
        .and_then(|index| {
            self.entries
                .borrow()
                .get(index)
                .map(|entry| entry.rel.clone())
        })
    }

    fn show_editor(&self) {
        self.stack.set_visible_child_name("editor");
        self.editor.focus_editor();
    }

    fn refresh(self: &Rc<Self>) {
        let dir = self.current_dir.borrow().clone();
        let previous = self.selected_rel();
        match self.store.list(&dir) {
            Ok(entries) => {
                self.entries.replace(entries);
                self.screen.set_status(&display_path(&dir));
                let selected = previous
                    .and_then(|rel| {
                        self.entries
                            .borrow()
                            .iter()
                            .position(|entry| entry.rel == rel)
                    })
                    .unwrap_or(0);
                self.rebuild(selected);
            }
            Err(error) => self.flash_error(format!("Cannot list folder: {error}")),
        }
    }

    /// Draw the list. In rename mode the selected row itself becomes a field,
    /// so the name is edited where the writer sees it.
    fn rebuild(self: &Rc<Self>, selected: usize) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let mode = self.mode.get();
        let target = self.rename_target.borrow().clone();
        let entries = self.entries.borrow().clone();

        let mut entry_row = None;
        if mode == EntryMode::NewFolder {
            let row = gtk::ListBoxRow::new();
            row.add_css_class("ui-row");
            row.add_css_class("file-row");
            let field = self.make_entry("", Some("Folder name"));
            row.set_child(Some(&field));
            self.list.append(&row);
            entry_row = Some((0, field));
        }

        for entry in &entries {
            if mode == EntryMode::Rename && target.as_deref() == Some(entry.rel.as_path()) {
                let row = gtk::ListBoxRow::new();
                row.add_css_class("ui-row");
                row.add_css_class("file-row");
                let field = self.make_entry(&editable_name(entry), None);
                row.set_child(Some(&field));
                self.list.append(&row);
                entry_row = Some((row.index(), field));
            } else {
                self.list.append(&self.make_row(entry));
            }
        }

        if entries.is_empty() && mode != EntryMode::NewFolder {
            let row = gtk::ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);
            let label = gtk::Label::new(Some("No files in this folder yet"));
            label.set_xalign(0.0);
            label.add_css_class("row-note");
            label.set_margin_top(10);
            label.set_margin_bottom(10);
            label.set_margin_start(12);
            row.set_child(Some(&label));
            self.list.append(&row);
        }

        let row_index = if mode == EntryMode::NewFolder {
            0
        } else {
            selected.min(entries.len().saturating_sub(1))
        };
        ui::select(&self.list, row_index);
        if let Some(row) = self.list.row_at_index(row_index as i32) {
            self.scroll_to_row(&row);
        }

        if let Some((_, field)) = entry_row {
            *self.entry.borrow_mut() = Some(field.clone());
            field.grab_focus();
            field.select_region(0, -1);
        } else {
            *self.entry.borrow_mut() = None;
        }
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

    fn make_row(self: &Rc<Self>, entry: &Entry) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("ui-row");
        row.add_css_class("file-row");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let name = gtk::Label::new(Some(&if entry.is_dir {
            format!("{}/", entry.name)
        } else {
            display_name(&entry.name)
        }));
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_ellipsize(pango::EllipsizeMode::End);
        name.add_css_class(if entry.is_dir {
            "folder-name"
        } else {
            "file-name"
        });
        line.append(&name);
        if entry.is_dir {
            let note = gtk::Label::new(Some("folder"));
            note.add_css_class("row-note");
            line.append(&note);
        }
        row.set_child(Some(&line));
        row
    }

    fn open_index(self: &Rc<Self>, index: i32) {
        let offset = i32::from(self.mode.get() == EntryMode::NewFolder);
        let index = index - offset;
        if index < 0 {
            return;
        }
        let entry = self.entries.borrow().get(index as usize).cloned();
        let Some(entry) = entry else {
            return;
        };
        if entry.is_dir {
            *self.current_dir.borrow_mut() = entry.rel;
            self.refresh();
            return;
        }
        if let Err(error) = self.editor.open_rel(&entry.rel) {
            self.flash_error(error);
            return;
        }
        self.show_editor();
    }

    fn activate_selected(self: &Rc<Self>) {
        if let Some(row) = self.list.selected_row() {
            self.open_index(row.index());
        }
    }

    fn go_up(self: &Rc<Self>) {
        let parent = self
            .current_dir
            .borrow()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        if parent == *self.current_dir.borrow() {
            return;
        }
        *self.current_dir.borrow_mut() = parent;
        self.refresh();
    }

    fn new_file(self: &Rc<Self>) {
        let dir = self.current_dir.borrow().clone();
        match self.editor.create_new_file_in(&dir) {
            Ok(()) => self.show_editor(),
            Err(error) => self.flash_error(error),
        }
    }

    fn start_new_folder(self: &Rc<Self>) {
        self.mode.set(EntryMode::NewFolder);
        *self.rename_target.borrow_mut() = None;
        let selected = self.list.selected_row().map(|row| row.index()).unwrap_or(0);
        self.rebuild(selected.max(0) as usize);
    }

    fn start_rename(self: &Rc<Self>) {
        let Some(row) = self.list.selected_row() else {
            return;
        };
        let index = row.index().max(0) as usize;
        let entry = self.entries.borrow().get(index).cloned();
        let Some(entry) = entry else {
            return;
        };
        self.mode.set(EntryMode::Rename);
        *self.rename_target.borrow_mut() = Some(entry.rel.clone());
        self.rebuild(index);
    }

    fn cancel_entry(self: &Rc<Self>) {
        let mode = self.mode.get();
        self.finish_entry();
        if mode == EntryMode::NewFolder {
            self.refresh();
        } else {
            let selected = self.list.selected_row().map(|row| row.index()).unwrap_or(0);
            self.rebuild(selected.max(0) as usize);
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
            EntryMode::None => {}
            EntryMode::NewFolder => {
                let rel = self.current_dir.borrow().join(&name);
                match self.store.create_dir(&rel) {
                    Ok(()) => {
                        self.finish_entry();
                        self.refresh();
                        self.select_rel(&rel);
                    }
                    Err(error) => self.flash_error(format!("Cannot create folder: {error}")),
                }
            }
            EntryMode::Rename => {
                let Some(source) = self.rename_target.borrow_mut().take() else {
                    return;
                };
                let source_is_file = self.store.root().join(&source).is_file();
                let new_name = ensure_document_extension(name, &source, source_is_file);
                let target = source
                    .parent()
                    .map_or_else(|| PathBuf::from(&new_name), |parent| parent.join(&new_name));
                if target == source {
                    self.finish_entry();
                    let selected = self.list.selected_row().map(|row| row.index()).unwrap_or(0);
                    self.rebuild(selected.max(0) as usize);
                    return;
                }

                let reopen = self.editor.close_if_affected(&source);
                match self.store.rename(&source, &target) {
                    Ok(()) => {
                        self.finish_entry();
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
        let Some(row) = self.list.selected_row() else {
            return;
        };
        let offset = i32::from(self.mode.get() == EntryMode::NewFolder);
        let entry = self
            .entries
            .borrow()
            .get((row.index() - offset).max(0) as usize)
            .cloned();
        let Some(entry) = entry else {
            return;
        };
        let sentence = if entry.is_dir {
            "The folder and its files move to the device's trash, so nothing is erased."
        } else {
            "It moves to the device's trash, so nothing is erased."
        };
        let weak = Rc::downgrade(self);
        let rel = entry.rel.clone();
        self.screen.ask(
            &format!("Delete “{}”?", display_name(&entry.name)),
            sentence,
            "Delete",
            move || {
                if let Some(page) = weak.upgrade() {
                    page.delete_rel(&rel);
                }
            },
        );
    }

    fn delete_rel(self: &Rc<Self>, rel: &Path) {
        let affected = self.editor.close_if_affected(rel).is_some();
        match self.store.delete(rel) {
            Ok(()) => {
                self.refresh();
                if affected {
                    if let Err(error) = self.editor.open_last_or_new() {
                        eprintln!("carrel: cannot open another file: {error}");
                    }
                }
            }
            Err(error) => self.flash_error(format!("Cannot delete: {error}")),
        }
    }

    fn flash_error(self: &Rc<Self>, message: impl Into<String>) {
        let message = message.into();
        let generation = self.status_generation.get().wrapping_add(1);
        self.status_generation.set(generation);
        self.screen.set_status_error(&message);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(4), move || {
            if let Some(page) = weak.upgrade() {
                if page.status_generation.get() == generation {
                    page.screen
                        .set_status(&display_path(&page.current_dir.borrow()));
                }
            }
        });
    }
}

fn display_path(dir: &Path) -> String {
    if dir.as_os_str().is_empty() {
        "Writing".to_owned()
    } else {
        format!("Writing / {}", dir.display())
    }
}

/// A document is renamed by its name, not its extension: "Notes.md" shows
/// "Notes" in the row, and the extension comes back on save.
fn editable_name(entry: &Entry) -> String {
    if entry.is_dir {
        return entry.name.clone();
    }
    display_name(&entry.name)
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
    use super::{editable_name, ensure_document_extension, moved_path};
    use crate::store::Entry;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry {
            name: name.to_owned(),
            rel: PathBuf::from(name),
            is_dir,
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        }
    }

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
        assert_eq!(editable_name(&entry("Notes.md", false)), "Notes");
        assert_eq!(
            editable_name(&entry("The quiet morning.md", false)),
            "The quiet morning"
        );
        assert_eq!(editable_name(&entry("Novel", true)), "Novel");
    }
}
