//! The Files screen: a calm list, keyboard first, destructive actions behind a
//! confirmation dialog.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};

use crate::editor::EditorPage;
use crate::store::{is_document_name, Entry, Store};

const HINT: &str =
    "Enter open · Ctrl+N new · Ctrl+Shift+N folder · F2 rename · Del delete · Esc editor";

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryMode {
    None,
    Rename,
    NewFolder,
}

pub struct FilesPage {
    pub widget: gtk::Box,
    list: gtk::ListBox,
    list_scroller: gtk::ScrolledWindow,
    path_label: gtk::Label,
    hint: gtk::Label,
    entry_bar: gtk::Box,
    entry_label: gtk::Label,
    entry: gtk::Entry,
    mode: Cell<EntryMode>,
    rename_target: RefCell<Option<PathBuf>>,
    current_dir: RefCell<PathBuf>,
    entries: RefCell<Vec<Entry>>,
    hint_generation: Cell<u64>,
    store: Store,
    editor: Rc<EditorPage>,
    stack: gtk::Stack,
}

impl FilesPage {
    pub fn new(store: Store, editor: Rc<EditorPage>, stack: gtk::Stack) -> Rc<Self> {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_activate_on_single_click(false);
        // Keys are handled by the window controller, so the list itself never
        // needs focus; this also keeps the calm Paper look free of a blue
        // focus ring around the selected row.
        list.set_can_focus(false);
        list.add_css_class("file-list");

        let list_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(&list)
            .build();

        let path_label = gtk::Label::new(Some("Writing"));
        path_label.set_xalign(0.0);
        path_label.add_css_class("hint");

        let title = gtk::Label::new(Some("Files"));
        title.set_xalign(0.0);
        title.add_css_class("screen-title");

        let entry_label = gtk::Label::new(Some("Rename"));
        let entry = gtk::Entry::new();
        entry.set_hexpand(true);
        entry.add_css_class("renaming");
        let entry_bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        entry_bar.append(&entry_label);
        entry_bar.append(&entry);
        entry_bar.set_visible(false);

        let hint = gtk::Label::new(Some(HINT));
        hint.set_xalign(0.0);
        hint.add_css_class("hint");

        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_halign(gtk::Align::Center);
        column.set_width_request(760);
        column.set_vexpand(true);
        column.set_margin_top(52);
        column.set_margin_bottom(18);
        column.append(&title);
        column.append(&path_label);
        column.append(&list_scroller);
        column.append(&entry_bar);
        column.append(&hint);

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("carrel-page");
        widget.set_focusable(true);
        widget.append(&column);

        let page = Rc::new(Self {
            widget,
            list,
            list_scroller,
            path_label,
            hint,
            entry_bar,
            entry_label,
            entry,
            mode: Cell::new(EntryMode::None),
            rename_target: RefCell::new(None),
            current_dir: RefCell::new(PathBuf::new()),
            entries: RefCell::new(Vec::new()),
            hint_generation: Cell::new(0),
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
                page.widget.grab_focus();
            }
        });

        self.entry.connect_activate({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.commit_entry();
                }
            }
        });
    }

    pub(crate) fn handle_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        if self.mode.get() != EntryMode::None {
            match keyval {
                gdk::Key::Escape => {
                    self.cancel_entry();
                    return glib::Propagation::Stop;
                }
                gdk::Key::Return => {
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
            gdk::Key::Return => {
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
        let count = self.entries.borrow().len() as i32;
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
        let Some(bounds) = row.compute_bounds(&self.list) else {
            return;
        };
        let adjustment = self.list_scroller.vadjustment();
        let top = f64::from(bounds.y());
        let bottom = top + f64::from(bounds.height());
        let value = adjustment.value();
        let page = adjustment.page_size();
        if top < value {
            adjustment.set_value(top);
        } else if bottom > value + page {
            adjustment.set_value(bottom - page);
        }
    }

    fn select_rel(&self, rel: &Path) {
        let index = self
            .entries
            .borrow()
            .iter()
            .position(|entry| entry.rel == rel);
        if let Some(index) = index {
            self.select_index(index as i32);
        }
    }

    fn show_editor(&self) {
        self.stack.set_visible_child_name("editor");
        self.editor.focus_editor();
    }

    fn refresh(self: &Rc<Self>) {
        let dir = self.current_dir.borrow().clone();
        match self.store.list(&dir) {
            Ok(entries) => {
                while let Some(child) = self.list.first_child() {
                    self.list.remove(&child);
                }
                for entry in &entries {
                    self.list.append(&self.make_row(entry));
                }
                if entries.is_empty() {
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
                self.entries.replace(entries);
                self.path_label.set_text(&display_path(&dir));
                self.set_hint(HINT);
                if !self.entries.borrow().is_empty() {
                    self.select_index(0);
                }
            }
            Err(error) => self.flash_error(format!("Cannot list folder: {error}")),
        }
    }

    fn make_row(&self, entry: &Entry) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("file-row");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let name = gtk::Label::new(Some(&if entry.is_dir {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
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
        self.entry_label.set_text("New folder");
        self.entry.set_text("");
        self.entry_bar.set_visible(true);
        self.entry.grab_focus();
    }

    fn start_rename(self: &Rc<Self>) {
        let Some(row) = self.list.selected_row() else {
            return;
        };
        let entry = self.entries.borrow().get(row.index() as usize).cloned();
        let Some(entry) = entry else {
            return;
        };
        self.mode.set(EntryMode::Rename);
        *self.rename_target.borrow_mut() = Some(entry.rel.clone());
        self.entry_label.set_text("Rename");
        self.entry.set_text(&entry.name);
        self.entry.select_region(0, -1);
        self.entry_bar.set_visible(true);
        self.entry.grab_focus();
    }

    fn cancel_entry(&self) {
        self.mode.set(EntryMode::None);
        *self.rename_target.borrow_mut() = None;
        self.entry_bar.set_visible(false);
        self.entry.set_text("");
    }

    fn commit_entry(self: &Rc<Self>) {
        let name = self.entry.text().trim().to_string();
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
        self.entry_bar.set_visible(false);
        self.entry.set_text("");
    }

    fn confirm_delete(self: &Rc<Self>) {
        let Some(row) = self.list.selected_row() else {
            return;
        };
        let entry = self.entries.borrow().get(row.index() as usize).cloned();
        let Some(entry) = entry else {
            return;
        };
        let window = self
            .widget
            .root()
            .and_then(|root| root.downcast::<gtk::Window>().ok());
        let dialog = gtk::AlertDialog::builder()
            .modal(true)
            .message(format!("Delete “{}”?", entry.name))
            .detail("The file moves to Carrel trash inside .carrel. Nothing is erased.")
            .buttons(["Cancel", "Delete"])
            .cancel_button(0)
            .default_button(0)
            .build();

        let weak = Rc::downgrade(self);
        let rel = entry.rel.clone();
        dialog.choose(window.as_ref(), gio::Cancellable::NONE, move |answer| {
            if !matches!(answer, Ok(1)) {
                return;
            }
            let Some(page) = weak.upgrade() else {
                return;
            };
            let affected = page.editor.close_if_affected(&rel).is_some();
            match page.store.delete(&rel) {
                Ok(()) => {
                    page.refresh();
                    if affected {
                        if let Err(error) = page.editor.open_last_or_new() {
                            eprintln!("carrel: cannot open another file: {error}");
                        }
                    }
                }
                Err(error) => page.flash_error(format!("Cannot delete: {error}")),
            }
        });
    }

    fn set_hint(&self, text: &str) {
        self.hint.remove_css_class("error-text");
        self.hint.set_text(text);
    }

    fn flash_error(self: &Rc<Self>, message: impl Into<String>) {
        let message = message.into();
        let generation = self.hint_generation.get().wrapping_add(1);
        self.hint_generation.set(generation);
        self.hint.add_css_class("error-text");
        self.hint.set_text(&message);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(4), move || {
            if let Some(page) = weak.upgrade() {
                if page.hint_generation.get() == generation {
                    page.set_hint(HINT);
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
    use super::{ensure_document_extension, moved_path};
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
}
