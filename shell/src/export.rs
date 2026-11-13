//! Export and import: what, format, USB drive, progress, "Safe to remove".
//!
//! The shell never mounts or copies anything itself (rule 2 and the USB rules
//! live in the daemon). This screen only collects the three answers and shows
//! the path the daemon returns.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::daemon::{Client, Drive};
use crate::editor::EditorPage;
use crate::nav::Navigator;
use crate::store::Store;
use crate::ui;

const HINTS: [(&str, &str); 3] = [("Enter", "Next"), ("↑ ↓", "Move"), ("Esc", "Back")];
const DRIVE_HINTS: [(&str, &str); 4] = [
    ("Enter", "Export"),
    ("R", "Scan"),
    ("↑ ↓", "Move"),
    ("Esc", "Back"),
];
const IMPORT_HINTS: [(&str, &str); 4] = [
    ("Enter", "Import"),
    ("R", "Scan"),
    ("↑ ↓", "Move"),
    ("Esc", "Back"),
];
const DONE_HINTS: [(&str, &str); 2] = [("Enter", "Again"), ("Esc", "Back")];
const RUNNING_HINTS: [(&str, &str); 1] = [("Esc", "Back")];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExportFormat {
    #[default]
    Markdown,
    Text,
    Docx,
    Odt,
    Pdf,
    Html,
    Epub,
}

impl ExportFormat {
    pub const ALL: [Self; 7] = [
        Self::Markdown,
        Self::Text,
        Self::Docx,
        Self::Odt,
        Self::Pdf,
        Self::Html,
        Self::Epub,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Markdown => "Markdown",
            Self::Text => "Plain text",
            Self::Docx => "Word .docx",
            Self::Odt => "OpenDocument .odt",
            Self::Pdf => "PDF",
            Self::Html => "HTML",
            Self::Epub => "EPUB",
        }
    }

    /// The value `Export(selection, format, drive_id)` receives.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Text => "text",
            Self::Docx => "docx",
            Self::Odt => "odt",
            Self::Pdf => "pdf",
            Self::Html => "html",
            Self::Epub => "epub",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    Current,
    Folder(String),
    Everything,
    Import,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportStep {
    #[default]
    What,
    Folder,
    Format,
    Drive,
    Running,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportOutcome {
    Exported { path: String },
    Imported { paths: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    pub selection: Vec<String>,
    pub format: String,
    pub drive_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportEffect {
    ListDrives,
    Export(ExportRequest),
    Import { drive_id: String },
}

/// The pure Export screen state.
#[derive(Debug, Default)]
pub struct ExportState {
    pub step: ExportStep,
    pub selected: usize,
    pub what: Option<What>,
    pub format: ExportFormat,
    pub folders: Vec<String>,
    pub folder_selected: usize,
    pub drives: Vec<Drive>,
    pub drive_selected: usize,
    pub current_file: Option<String>,
    pub outcome: Option<ExportOutcome>,
    pub error: Option<String>,
}

impl ExportState {
    pub fn reset(&mut self) {
        self.step = ExportStep::What;
        self.selected = 0;
        self.what = None;
        self.outcome = None;
        self.error = None;
        self.drive_selected = 0;
        self.folder_selected = 0;
    }

    pub fn rows(&self) -> Vec<String> {
        match self.step {
            ExportStep::What => vec![
                match &self.current_file {
                    Some(path) => format!("Current file · {path}"),
                    None => "Current file · no file open".to_owned(),
                },
                "A folder…".to_owned(),
                "Everything".to_owned(),
                "Import from a USB drive".to_owned(),
            ],
            ExportStep::Folder => self.folder_rows(),
            ExportStep::Format => ExportFormat::ALL
                .iter()
                .map(|format| format.label().to_owned())
                .collect(),
            ExportStep::Drive => {
                if self.drives.is_empty() {
                    vec!["Insert a USB drive".to_owned()]
                } else {
                    self.drives
                        .iter()
                        .map(|drive| {
                            format!("{} · {} free", drive.label, free_space(drive.free_bytes))
                        })
                        .collect()
                }
            }
            ExportStep::Running | ExportStep::Done => Vec::new(),
        }
    }

    fn folder_rows(&self) -> Vec<String> {
        let mut rows = vec!["Writing".to_owned()];
        for folder in &self.folders {
            rows.push(folder.clone());
        }
        rows
    }

    pub fn move_by(&mut self, delta: i32) {
        let rows = self.rows().len() as i32;
        if rows == 0 {
            return;
        }
        match self.step {
            ExportStep::Folder => {
                self.folder_selected =
                    ((self.folder_selected as i32 + delta).rem_euclid(rows)) as usize;
            }
            _ => {
                self.selected = ((self.selected as i32 + delta).rem_euclid(rows)) as usize;
            }
        }
    }

    pub fn selected_row(&self) -> usize {
        match self.step {
            ExportStep::Folder => self.folder_selected,
            _ => self.selected,
        }
    }

    pub fn select(&mut self, index: usize) {
        let max = self.rows().len().saturating_sub(1);
        match self.step {
            ExportStep::Folder => self.folder_selected = index.min(max),
            _ => self.selected = index.min(max),
        }
    }

    /// Enter on the current step.
    pub fn choose(&mut self) -> Option<ExportEffect> {
        self.error = None;
        match self.step {
            ExportStep::What => {
                let what = match self.selected {
                    0 => {
                        if self.current_file.is_none() {
                            self.error = Some("Open a file first.".to_owned());
                            return None;
                        }
                        What::Current
                    }
                    1 => {
                        self.step = ExportStep::Folder;
                        self.folder_selected = 0;
                        return None;
                    }
                    2 => What::Everything,
                    _ => What::Import,
                };
                self.what = Some(what.clone());
                match what {
                    What::Import => {
                        self.step = ExportStep::Drive;
                        self.selected = 0;
                        Some(ExportEffect::ListDrives)
                    }
                    _ => {
                        self.step = ExportStep::Format;
                        self.selected = 0;
                        None
                    }
                }
            }
            ExportStep::Folder => {
                let folder = if self.folder_selected == 0 {
                    String::new()
                } else {
                    self.folders[self.folder_selected - 1].clone()
                };
                self.what = Some(What::Folder(folder));
                self.step = ExportStep::Format;
                self.selected = 0;
                None
            }
            ExportStep::Format => {
                self.format = ExportFormat::ALL[self.selected.min(ExportFormat::ALL.len() - 1)];
                self.step = ExportStep::Drive;
                self.selected = 0;
                Some(ExportEffect::ListDrives)
            }
            ExportStep::Drive => {
                if self.drives.is_empty() {
                    return Some(ExportEffect::ListDrives);
                }
                let drive = self.drives[self.drive_selected.min(self.drives.len() - 1)].clone();
                match self.what.clone() {
                    Some(What::Import) => Some(ExportEffect::Import { drive_id: drive.id }),
                    Some(what) => {
                        let request = ExportRequest {
                            selection: self.selection(&what),
                            format: self.format.wire().to_owned(),
                            drive_id: drive.id,
                        };
                        Some(ExportEffect::Export(request))
                    }
                    None => None,
                }
            }
            ExportStep::Running => None,
            ExportStep::Done => {
                self.reset();
                None
            }
        }
    }

    fn selection(&self, what: &What) -> Vec<String> {
        match what {
            What::Current => self
                .current_file
                .clone()
                .map(|path| vec![format!("current:{path}")])
                .unwrap_or_default(),
            What::Folder(path) => vec![format!("folder:{path}")],
            What::Everything => vec!["all".to_owned()],
            What::Import => Vec::new(),
        }
    }

    pub fn on_drives(&mut self, drives: Vec<Drive>) {
        self.drives = drives;
        self.drive_selected = self.drive_selected.min(self.drives.len().saturating_sub(1));
    }

    pub fn on_exported(&mut self, path: String) {
        self.step = ExportStep::Done;
        self.outcome = Some(ExportOutcome::Exported { path });
        self.error = None;
    }

    pub fn on_imported(&mut self, paths: Vec<String>) {
        self.step = ExportStep::Done;
        self.outcome = Some(ExportOutcome::Imported { paths });
        self.error = None;
    }

    pub fn on_error(&mut self, message: String) {
        self.step = ExportStep::Drive;
        self.error = Some(message);
    }

    pub fn step_title(&self) -> &'static str {
        match self.step {
            ExportStep::What => "What to export",
            ExportStep::Folder => "Choose a folder",
            ExportStep::Format => "Format",
            ExportStep::Drive => "Destination",
            ExportStep::Running => "Exporting",
            ExportStep::Done => "Finished",
        }
    }

    pub fn note(&self, dated_folder: &str) -> String {
        match self.step {
            ExportStep::What => {
                "Nothing leaves the device. A USB drive needs no network or account.".to_owned()
            }
            ExportStep::Folder => "The folder keeps its structure inside the export.".to_owned(),
            ExportStep::Format => {
                "Citations become footnotes in .docx, .odt, PDF and EPUB.".to_owned()
            }
            ExportStep::Drive => {
                if self.drives.is_empty() {
                    "Plug it in; this screen notices it and lists it here.".to_owned()
                } else if self.what == Some(What::Import) {
                    "Copies .md and .txt into Writing/Imported/.".to_owned()
                } else if self.what == Some(What::Everything) {
                    format!("Everything goes into “Carrel export {dated_folder}/”.")
                } else {
                    "The drive is unmounted and powered off when the export finishes.".to_owned()
                }
            }
            ExportStep::Running => "Writing files. Do not remove the drive.".to_owned(),
            ExportStep::Done => outcome_text(self.outcome.as_ref()),
        }
    }

    pub fn done_heading(&self) -> Option<&'static str> {
        match self.outcome {
            Some(ExportOutcome::Exported { .. }) => Some("Safe to remove"),
            Some(ExportOutcome::Imported { .. }) => Some("Import finished"),
            None => None,
        }
    }

    pub fn outcome_detail(&self) -> Option<String> {
        match &self.outcome {
            Some(ExportOutcome::Exported { path }) => Some(path.clone()),
            Some(ExportOutcome::Imported { paths }) => {
                Some(format!("{} files into Writing/Imported/", paths.len()))
            }
            None => None,
        }
    }
}

pub fn outcome_text(outcome: Option<&ExportOutcome>) -> String {
    match outcome {
        Some(ExportOutcome::Exported { path }) => {
            format!("Exported to {path}. Safe to remove the drive.")
        }
        Some(ExportOutcome::Imported { paths }) => {
            format!("Imported {} files into Writing/Imported/.", paths.len())
        }
        None => String::new(),
    }
}

fn free_space(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / GB)
    } else {
        format!("{} MB", bytes / 1_000_000)
    }
}

pub struct ExportPage {
    pub widget: gtk::Overlay,
    screen: Rc<ui::Screen>,
    list: gtk::ListBox,
    list_scroller: gtk::ScrolledWindow,
    note: gtk::Label,
    done_heading: gtk::Label,
    done_detail: gtk::Label,
    state: RefCell<ExportState>,
    client: Client,
    nav: Rc<Navigator>,
    editor: Rc<EditorPage>,
    store: Store,
}

impl ExportPage {
    pub fn new(
        client: Client,
        nav: Rc<Navigator>,
        editor: Rc<EditorPage>,
        store: Store,
    ) -> Rc<Self> {
        let screen = ui::Screen::new("Export");
        screen.set_hints(&HINTS);

        let list = ui::list();
        let list_scroller = ui::scroller(&list);
        screen.content.append(&list_scroller);

        let note = gtk::Label::new(None);
        note.set_xalign(0.0);
        note.set_wrap(true);
        note.add_css_class("notice-line");
        screen.content.append(&note);

        let done_heading = gtk::Label::new(None);
        done_heading.set_xalign(0.0);
        done_heading.add_css_class("done-heading");
        screen.content.append(&done_heading);
        let done_detail = gtk::Label::new(None);
        done_detail.set_xalign(0.0);
        done_detail.set_wrap(true);
        done_detail.add_css_class("row-note");
        done_detail.set_visible(false);
        screen.content.append(&done_detail);

        let page = Rc::new(Self {
            widget: screen.root.clone(),
            screen,
            list,
            list_scroller,
            note,
            done_heading,
            done_detail,
            state: RefCell::new(ExportState::default()),
            client,
            nav,
            editor,
            store,
        });
        page.install();
        page
    }

    fn install(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(page) = weak.upgrade() {
                page.state.borrow_mut().select(row.index().max(0) as usize);
                page.choose();
            }
        });
        let weak = Rc::downgrade(self);
        self.widget.connect_map(move |_| {
            if let Some(page) = weak.upgrade() {
                page.on_open();
                page.screen.focus_page();
            }
        });
    }

    /// Show the destination step directly, as if "Everything" and a format
    /// had been chosen. Used by the screenshot script; harmless otherwise.
    pub fn start_at_drive(self: &Rc<Self>) {
        self.nav.show("export");
        {
            let mut state = self.state.borrow_mut();
            state.reset();
            state.what = Some(What::Everything);
            state.step = ExportStep::Drive;
        }
        self.refresh();
        self.load_drives();
    }

    fn on_open(self: &Rc<Self>) {
        {
            let mut state = self.state.borrow_mut();
            state.current_file = self
                .editor
                .current_rel()
                .map(|path| path.to_string_lossy().to_string());
            state.folders = self
                .store
                .list(std::path::Path::new(""))
                .map(|entries| {
                    entries
                        .into_iter()
                        .filter(|entry| entry.is_dir)
                        .map(|entry| entry.name)
                        .collect()
                })
                .unwrap_or_default();
            if matches!(state.step, ExportStep::Done | ExportStep::Running) {
                state.reset();
            }
        }
        self.refresh();
        let step = self.state.borrow().step;
        if step == ExportStep::Drive {
            self.load_drives();
        }
    }

    fn load_drives(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.client.spawn(
            |daemon| daemon.list_drives(),
            move |result| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                match result {
                    Ok(drives) => {
                        page.state.borrow_mut().on_drives(drives);
                        page.refresh();
                        let empty = page.state.borrow().drives.is_empty();
                        let step = page.state.borrow().step;
                        if empty && step == ExportStep::Drive {
                            page.schedule_drive_rescan();
                        }
                    }
                    Err(error) => {
                        page.state.borrow_mut().error = Some(error.message);
                        page.refresh();
                    }
                }
            },
        );
    }

    fn schedule_drive_rescan(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(2), move || {
            if let Some(page) = weak.upgrade() {
                if page.state.borrow().step == ExportStep::Drive
                    && page.state.borrow().drives.is_empty()
                {
                    page.load_drives();
                }
            }
        });
    }

    fn choose(self: &Rc<Self>) {
        let effect = self.state.borrow_mut().choose();
        self.refresh();
        if let Some(effect) = effect {
            self.run_effect(effect);
        }
    }

    fn run_effect(self: &Rc<Self>, effect: ExportEffect) {
        match effect {
            ExportEffect::ListDrives => self.load_drives(),
            ExportEffect::Export(request) => {
                self.state.borrow_mut().step = ExportStep::Running;
                self.refresh();
                let weak = Rc::downgrade(self);
                self.client.spawn(
                    move |daemon| {
                        daemon.export(
                            request.selection.clone(),
                            request.format.clone(),
                            request.drive_id.clone(),
                        )
                    },
                    move |result| {
                        let Some(page) = weak.upgrade() else {
                            return;
                        };
                        match result {
                            Ok(path) => page.state.borrow_mut().on_exported(path),
                            Err(error) => page.state.borrow_mut().on_error(error.message),
                        }
                        page.refresh();
                    },
                );
            }
            ExportEffect::Import { drive_id } => {
                self.state.borrow_mut().step = ExportStep::Running;
                self.refresh();
                let weak = Rc::downgrade(self);
                self.client.spawn(
                    move |daemon| daemon.import(drive_id, Vec::new()),
                    move |result| {
                        let Some(page) = weak.upgrade() else {
                            return;
                        };
                        match result {
                            Ok(paths) => page.state.borrow_mut().on_imported(paths),
                            Err(error) => page.state.borrow_mut().on_error(error.message),
                        }
                        page.refresh();
                    },
                );
            }
        }
    }

    pub(crate) fn handle_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        if state.contains(gdk::ModifierType::CONTROL_MASK) {
            return glib::Propagation::Proceed;
        }
        let step = self.state.borrow().step;
        match keyval {
            gdk::Key::Escape => {
                self.nav.show_editor();
                glib::Propagation::Stop
            }
            _ if keyval.to_lower() == gdk::Key::r && step == ExportStep::Drive => {
                self.load_drives();
                glib::Propagation::Stop
            }
            gdk::Key::Up if !matches!(step, ExportStep::Running | ExportStep::Done) => {
                self.state.borrow_mut().move_by(-1);
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Down if !matches!(step, ExportStep::Running | ExportStep::Done) => {
                self.state.borrow_mut().move_by(1);
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                self.choose();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn refresh(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let (step, rows, selected, note, error, done_heading, done_detail) = {
            let state = self.state.borrow();
            (
                state.step,
                state.rows(),
                state.selected_row(),
                state.note(&ui::local_date()),
                state.error.clone(),
                state.done_heading(),
                state.outcome_detail(),
            )
        };
        self.screen.set_status(self.state.borrow().step_title());
        for (index, row) in rows.iter().enumerate() {
            let element = gtk::ListBoxRow::new();
            element.add_css_class("ui-row");
            element.add_css_class("file-row");
            let label = gtk::Label::new(Some(row));
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.add_css_class("file-name");
            if step == ExportStep::Drive && self.state.borrow().drives.is_empty() {
                label.add_css_class("row-note");
            }
            element.set_child(Some(&label));
            self.list.append(&element);
            let _ = index;
        }
        self.list.set_visible(!rows.is_empty());
        self.list_scroller.set_visible(!rows.is_empty());
        ui::select(&self.list, selected);
        if let Some(row) = self.list.row_at_index(selected as i32) {
            ui::scroll_to_row(&self.list_scroller, &self.list, &row);
        }
        self.note.set_text(&error.clone().unwrap_or(note));
        if error.is_some() {
            self.note.add_css_class("error-text");
        } else {
            self.note.remove_css_class("error-text");
        }
        self.done_heading.set_text(done_heading.unwrap_or(""));
        self.done_heading.set_visible(done_heading.is_some());
        self.done_detail
            .set_text(&done_detail.clone().unwrap_or_default());
        self.done_detail.set_visible(done_detail.is_some());

        match step {
            ExportStep::Drive if self.state.borrow().what == Some(What::Import) => {
                self.screen.set_hints(&IMPORT_HINTS);
            }
            ExportStep::Drive => self.screen.set_hints(&DRIVE_HINTS),
            ExportStep::Running => self.screen.set_hints(&RUNNING_HINTS),
            ExportStep::Done => self.screen.set_hints(&DONE_HINTS),
            _ => self.screen.set_hints(&HINTS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{DaemonClient, FakeDaemon};

    fn drive() -> Drive {
        Drive {
            id: "usb-1".into(),
            label: "CARREL USB".into(),
            free_bytes: 12_400_000_000,
        }
    }

    fn state_with_file() -> ExportState {
        ExportState {
            current_file: Some("The quiet morning.md".into()),
            folders: vec!["Novel".into(), "Notes".into()],
            drives: vec![drive()],
            ..ExportState::default()
        }
    }

    #[test]
    fn every_format_from_the_spec_is_listed() {
        let labels: Vec<&str> = ExportFormat::ALL
            .iter()
            .map(|format| format.label())
            .collect();
        assert_eq!(
            labels,
            vec![
                "Markdown",
                "Plain text",
                "Word .docx",
                "OpenDocument .odt",
                "PDF",
                "HTML",
                "EPUB",
            ]
        );
        assert_eq!(ExportFormat::Docx.wire(), "docx");
    }

    #[test]
    fn current_file_goes_format_drive_export() {
        let mut state = state_with_file();
        assert_eq!(state.step, ExportStep::What);
        assert_eq!(state.choose(), None);
        assert_eq!(state.step, ExportStep::Format);
        state.select(2); // Word .docx
        assert_eq!(state.choose(), Some(ExportEffect::ListDrives));
        assert_eq!(state.step, ExportStep::Drive);
        let effect = state.choose().unwrap();
        assert_eq!(
            effect,
            ExportEffect::Export(ExportRequest {
                selection: vec!["current:The quiet morning.md".into()],
                format: "docx".into(),
                drive_id: "usb-1".into(),
            })
        );
    }

    #[test]
    fn everything_is_one_selection_and_a_dated_folder() {
        let mut state = state_with_file();
        state.select(2);
        state.choose();
        state.choose(); // format -> drive
        let ExportEffect::Export(request) = state.choose().unwrap() else {
            panic!("expected an export");
        };
        assert_eq!(request.selection, vec!["all"]);
        assert!(state
            .note("2026-11-12")
            .contains("Carrel export 2026-11-12/"));
    }

    #[test]
    fn a_folder_keeps_its_path() {
        let mut state = state_with_file();
        state.select(1);
        state.choose();
        assert_eq!(state.step, ExportStep::Folder);
        assert_eq!(state.rows(), vec!["Writing", "Novel", "Notes"]);
        state.move_by(1);
        state.choose();
        assert_eq!(state.what, Some(What::Folder("Novel".into())));
        assert_eq!(state.step, ExportStep::Format);
    }

    #[test]
    fn no_drive_says_insert_a_usb_drive_and_asks_again() {
        let mut state = state_with_file();
        state.drives.clear();
        state.choose();
        state.choose();
        assert_eq!(state.step, ExportStep::Drive);
        assert_eq!(state.rows(), vec!["Insert a USB drive"]);
        assert_eq!(state.choose(), Some(ExportEffect::ListDrives));
    }

    #[test]
    fn import_asks_for_a_drive_then_reports_the_copied_files() {
        let mut state = state_with_file();
        state.select(3);
        assert_eq!(state.choose(), Some(ExportEffect::ListDrives));
        assert_eq!(state.step, ExportStep::Drive);
        assert_eq!(
            state.choose(),
            Some(ExportEffect::Import {
                drive_id: "usb-1".into()
            })
        );
        state.on_imported(vec!["Imported/one.md".into(), "Imported/two.txt".into()]);
        assert_eq!(state.step, ExportStep::Done);
        assert_eq!(
            state.outcome_detail().as_deref(),
            Some("2 files into Writing/Imported/")
        );
        assert_eq!(state.done_heading(), Some("Import finished"));
    }

    #[test]
    fn a_finished_export_shows_the_path_and_safe_to_remove() {
        let mut state = state_with_file();
        state.on_exported("/media/usb/Carrel export 2026-11-12/".into());
        assert_eq!(state.step, ExportStep::Done);
        assert_eq!(state.done_heading(), Some("Safe to remove"));
        assert!(state
            .outcome_detail()
            .unwrap()
            .contains("Carrel export 2026-11-12"));
    }

    #[test]
    fn an_export_error_returns_to_the_drive_step_with_one_line() {
        let mut state = state_with_file();
        state.step = ExportStep::Drive;
        state.on_drives(vec![drive()]);
        state.on_error("the drive was removed".into());
        assert_eq!(state.step, ExportStep::Drive);
        assert_eq!(state.error.as_deref(), Some("the drive was removed"));
    }

    #[test]
    fn the_fake_daemon_records_the_export_request() {
        let fake = FakeDaemon::new();
        let path = fake
            .export(vec!["all".into()], "epub".into(), "usb-1".into())
            .unwrap();
        assert!(path.contains("Carrel export"));
        assert_eq!(
            fake.exported(),
            vec![(vec!["all".into()], "epub".into(), "usb-1".into())]
        );
        assert_eq!(fake.list_drives().unwrap(), vec![drive()]);
    }

    #[test]
    fn free_space_is_readable() {
        assert_eq!(free_space(12_400_000_000), "12.4 GB");
        assert_eq!(free_space(620_000_000), "620 MB");
    }
}
