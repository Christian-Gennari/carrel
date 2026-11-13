//! Export and import: one screen with what, format and destination, one
//! action, then progress and the result in the same frame.
//!
//! The shell never mounts or copies anything itself (rule 2 and the USB rules
//! live in the daemon). This screen only collects the three answers, remembers
//! them, and shows the path the daemon returns.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use serde::{Deserialize, Serialize};

use crate::daemon::{Client, Drive};
use crate::editor::EditorPage;
use crate::nav::Navigator;
use crate::store::{atomic_write, display_path, Store};
use crate::ui;

const EXPORT_HINTS: [(&str, &str); 4] = [
    ("Enter", "Export"),
    ("← →", "Change"),
    ("↑ ↓", "Row"),
    ("Esc", "Back"),
];
const EXPORT_NO_DRIVE_HINTS: [(&str, &str); 3] =
    [("← →", "Change"), ("↑ ↓", "Row"), ("Esc", "Back")];
const IMPORT_HINTS: [(&str, &str); 2] = [("Enter", "Import"), ("Esc", "Back")];
const IMPORT_NO_DRIVE_HINTS: [(&str, &str); 1] = [("Esc", "Back")];
const RUNNING_HINTS: [(&str, &str); 1] = [("Esc", "Back")];
const DONE_EXPORT_HINTS: [(&str, &str); 3] = [
    ("Enter", "Done"),
    ("E", "Export something else"),
    ("Esc", "Back"),
];
const DONE_IMPORT_HINTS: [(&str, &str); 3] = [
    ("Enter", "Done"),
    ("I", "Import something else"),
    ("Esc", "Back"),
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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

    /// The extension of the file the writer finds on the drive.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Text => "txt",
            Self::Docx => "docx",
            Self::Odt => "odt",
            Self::Pdf => "pdf",
            Self::Html => "html",
            Self::Epub => "epub",
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

/// The answer to "What": one file, one folder, or everything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportTarget {
    #[default]
    Current,
    Folder,
    Everything,
}

impl ExportTarget {
    pub const ALL: [Self; 3] = [Self::Current, Self::Folder, Self::Everything];
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExportMode {
    #[default]
    Export,
    Import,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExportStep {
    #[default]
    Configure,
    Running,
    Done,
}

/// One of the screen's compact rows. The Folder row appears only when What is
/// "A folder…", so the other choices stay three rows tall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportRow {
    What,
    Folder,
    Format,
    To,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportOutcome {
    Exported {
        path: String,
        drive: String,
        file: Option<String>,
    },
    Imported {
        paths: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    pub selection: Vec<String>,
    pub format: String,
    pub drive_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportEffect {
    Export {
        request: ExportRequest,
        drive: String,
        file: Option<String>,
    },
    Import {
        drive_id: String,
        drive: String,
    },
}

/// The choices Carrel remembers between exports, in `.carrel/export.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportPrefs {
    pub what: ExportTarget,
    pub folder: String,
    pub format: ExportFormat,
    pub drive_id: String,
}

impl Default for ExportPrefs {
    fn default() -> Self {
        Self {
            what: ExportTarget::Current,
            folder: String::new(),
            format: ExportFormat::Markdown,
            drive_id: String::new(),
        }
    }
}

impl ExportPrefs {
    pub fn load(root: &Path) -> Self {
        std::fs::read(prefs_path(root))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, root: &Path) {
        let Ok(json) = serde_json::to_vec_pretty(self) else {
            return;
        };
        if let Err(error) = atomic_write(&prefs_path(root), &json) {
            eprintln!("carrel: could not remember the export choices: {error}");
        }
    }
}

fn prefs_path(root: &Path) -> PathBuf {
    root.join(".carrel").join("export.json")
}

/// The pure Export screen state.
#[derive(Debug, Default)]
pub struct ExportState {
    pub mode: ExportMode,
    pub step: ExportStep,
    pub row: usize,
    pub target: ExportTarget,
    pub format: ExportFormat,
    pub folder: String,
    pub folders: Vec<String>,
    pub drives: Vec<Drive>,
    pub drive_id: String,
    pub current_file: Option<String>,
    pub running_drive: String,
    pub outcome: Option<ExportOutcome>,
    pub error: Option<String>,
}

impl ExportState {
    /// Show the screen with the choices Carrel remembers.
    pub fn open(
        &mut self,
        prefs: &ExportPrefs,
        current_file: Option<String>,
        folders: Vec<String>,
    ) {
        self.mode = ExportMode::Export;
        self.step = ExportStep::Configure;
        self.row = 0;
        self.target = prefs.what;
        self.folder = prefs.folder.clone();
        self.folders = folders;
        self.format = prefs.format;
        self.drive_id = prefs.drive_id.clone();
        self.current_file = current_file;
        self.running_drive.clear();
        self.outcome = None;
        self.error = None;
        self.normalize();
    }

    pub fn prefs(&self) -> ExportPrefs {
        ExportPrefs {
            what: self.target,
            folder: self.folder.clone(),
            format: self.format,
            drive_id: self.drive_id.clone(),
        }
    }

    fn normalize(&mut self) {
        if !self.folders.iter().any(|folder| folder == &self.folder) {
            self.folder.clear();
        }
    }

    pub fn rows(&self) -> Vec<ExportRow> {
        if self.mode == ExportMode::Import {
            return vec![ExportRow::To];
        }
        let mut rows = vec![ExportRow::What];
        if self.target == ExportTarget::Folder {
            rows.push(ExportRow::Folder);
        }
        rows.push(ExportRow::Format);
        rows.push(ExportRow::To);
        rows
    }

    pub fn selected_row(&self) -> ExportRow {
        self.rows()
            .get(self.row)
            .copied()
            .unwrap_or(ExportRow::What)
    }

    pub fn select(&mut self, index: usize) {
        self.row = index.min(self.rows().len().saturating_sub(1));
    }

    pub fn move_by(&mut self, delta: i32) {
        let rows = self.rows().len() as i32;
        if rows == 0 {
            return;
        }
        self.row = ((self.row as i32 + delta).rem_euclid(rows)) as usize;
    }

    /// Change the selected row's value. Left/Right in the screen.
    pub fn cycle(&mut self, delta: i32) {
        match self.selected_row() {
            ExportRow::What => {
                self.target = cycle(&ExportTarget::ALL, &self.target, delta);
                self.normalize();
                self.row = self.row.min(self.rows().len().saturating_sub(1));
            }
            ExportRow::Folder => {
                let choices = self.folder_choices();
                let current = self.folder.clone();
                self.folder = cycle(&choices, &current, delta);
            }
            ExportRow::Format => {
                self.format = cycle(&ExportFormat::ALL, &self.format, delta);
            }
            ExportRow::To => self.cycle_drive(delta),
        }
    }

    /// "Writing" for the root, then one entry per folder. Stored as the folder
    /// relative path; the empty string is the whole writing folder.
    pub fn folder_choices(&self) -> Vec<String> {
        let mut choices = vec![String::new()];
        choices.extend(self.folders.iter().cloned());
        choices
    }

    pub fn folder_label(&self) -> String {
        if self.folder.is_empty() {
            "Writing".to_owned()
        } else {
            self.folder.clone()
        }
    }

    pub fn current_drive(&self) -> Option<&Drive> {
        self.drives
            .iter()
            .find(|drive| drive.id == self.drive_id)
            .or_else(|| self.drives.first())
    }

    fn cycle_drive(&mut self, delta: i32) {
        if self.drives.len() < 2 {
            return;
        }
        let current = self
            .current_drive()
            .map(|drive| drive.id.clone())
            .unwrap_or_default();
        let ids: Vec<String> = self.drives.iter().map(|drive| drive.id.clone()).collect();
        self.drive_id = cycle(&ids, &current, delta);
    }

    pub fn on_drives(&mut self, drives: Vec<Drive>) {
        self.drives = drives;
        // The remembered drive keeps its turn while it is unplugged; the
        // first drive answers until it comes back.
    }

    /// The action is enabled exactly when there is somewhere to write.
    pub fn can_export(&self) -> bool {
        self.step == ExportStep::Configure && self.current_drive().is_some()
    }

    /// Enter on the configured screen.
    pub fn choose(&mut self) -> Option<ExportEffect> {
        if !self.can_export() {
            return None;
        }
        self.error = None;
        let drive = self.current_drive()?.clone();
        if self.mode == ExportMode::Import {
            return Some(ExportEffect::Import {
                drive_id: drive.id,
                drive: drive.label,
            });
        }
        if self.target == ExportTarget::Current && self.current_file.is_none() {
            self.error = Some("Open a file first.".to_owned());
            return None;
        }
        Some(ExportEffect::Export {
            request: ExportRequest {
                selection: self.selection(),
                format: self.format.wire().to_owned(),
                drive_id: drive.id,
            },
            drive: drive.label,
            file: self.exported_file(),
        })
    }

    fn selection(&self) -> Vec<String> {
        match self.target {
            ExportTarget::Current => self
                .current_file
                .clone()
                .map(|path| vec![format!("current:{path}")])
                .unwrap_or_default(),
            ExportTarget::Folder => vec![format!("folder:{}", self.folder)],
            ExportTarget::Everything => vec!["all".to_owned()],
        }
    }

    /// The name of the file the writer will find on the drive, with its new
    /// extension. Nothing for a folder or everything.
    pub fn exported_file(&self) -> Option<String> {
        if self.target != ExportTarget::Current {
            return None;
        }
        let path = self.current_file.as_ref()?;
        let stem = Path::new(path).file_stem()?.to_string_lossy();
        Some(format!("{stem}.{}", self.format.extension()))
    }

    pub fn on_running(&mut self, drive: String) {
        self.step = ExportStep::Running;
        self.running_drive = drive;
        self.error = None;
    }

    pub fn on_exported(&mut self, path: String, drive: String, file: Option<String>) {
        self.step = ExportStep::Done;
        self.outcome = Some(ExportOutcome::Exported { path, drive, file });
        self.error = None;
    }

    pub fn on_imported(&mut self, paths: Vec<String>) {
        self.step = ExportStep::Done;
        self.outcome = Some(ExportOutcome::Imported { paths });
        self.error = None;
    }

    pub fn on_error(&mut self, message: String) {
        self.step = ExportStep::Configure;
        self.error = Some(message);
    }

    /// The one-line status under the title, or the error in its place.
    pub fn status(&self, dated_folder: &str) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        match self.step {
            ExportStep::Configure if self.current_drive().is_none() => {
                "Plug in a drive; this screen notices it as soon as it appears.".to_owned()
            }
            ExportStep::Configure if self.mode == ExportMode::Import => {
                "Copies .md and .txt files from the USB drive into Imported/.".to_owned()
            }
            ExportStep::Configure => match self.target {
                ExportTarget::Current => {
                    "Nothing leaves the device. A USB drive needs no network or account.".to_owned()
                }
                ExportTarget::Folder => {
                    format!(
                        "“{}” keeps its structure inside the export.",
                        self.folder_label()
                    )
                }
                ExportTarget::Everything => {
                    format!("Everything goes into “Carrel export {dated_folder}/”.")
                }
            },
            ExportStep::Running | ExportStep::Done => String::new(),
        }
    }

    pub fn progress_heading(&self) -> String {
        if self.mode == ExportMode::Import {
            format!("Copying from {}…", self.running_drive)
        } else {
            format!("Exporting to {}…", self.running_drive)
        }
    }

    pub fn progress_line(&self) -> &'static str {
        if self.mode == ExportMode::Import {
            "Copying files. Do not remove the drive."
        } else {
            "Writing files. Do not remove the drive."
        }
    }

    pub fn outcome_heading(&self) -> Option<String> {
        match &self.outcome {
            Some(ExportOutcome::Exported { drive, .. }) => Some(format!("Exported to {drive}")),
            // The import result is one line; it is not repeated as a heading.
            Some(ExportOutcome::Imported { .. }) | None => None,
        }
    }

    /// The result, one line per fact: where it went, then the file and the
    /// safe-to-remove sentence. The import result is said once.
    pub fn outcome_lines(&self) -> Vec<String> {
        match &self.outcome {
            Some(ExportOutcome::Exported { path, file, .. }) => {
                let mut lines = Vec::new();
                if let Some(folder) = export_location(path) {
                    lines.push(folder);
                }
                lines.push(match file {
                    Some(file) => format!("{file}. Safe to remove."),
                    None => "Safe to remove.".to_owned(),
                });
                lines
            }
            Some(ExportOutcome::Imported { paths }) => vec![imported_text(paths.len())],
            None => Vec::new(),
        }
    }

    /// What a finished export says it did, for tests and the result screen.
    pub fn is_import(&self) -> bool {
        self.mode == ExportMode::Import
            || matches!(self.outcome, Some(ExportOutcome::Imported { .. }))
    }
}

fn cycle<T: Clone + PartialEq>(list: &[T], current: &T, delta: i32) -> T {
    if list.is_empty() {
        return current.clone();
    }
    let len = list.len() as i32;
    let index = list.iter().position(|item| item == current).unwrap_or(0) as i32;
    list[((index + delta).rem_euclid(len)) as usize].clone()
}

/// The folder a finished export went into. The daemon may return the folder
/// itself or the file it wrote; both read as the last real folder name.
pub fn export_location(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let name = trimmed.rsplit('/').next()?;
    let looks_like_file = name
        .rsplit_once('.')
        .map(|(stem, extension)| {
            !stem.is_empty()
                && !extension.is_empty()
                && extension.len() <= 6
                && !extension.contains(' ')
        })
        .unwrap_or(false);
    let folder = if looks_like_file {
        trimmed
            .rsplit_once('/')
            .map(|(parent, _)| parent.rsplit('/').next().unwrap_or_default())?
    } else {
        name
    };
    if folder.is_empty() || folder == "media" || folder == "carrel" {
        None
    } else {
        Some(folder.to_owned())
    }
}

pub fn imported_text(files: usize) -> String {
    match files {
        0 => "No .md or .txt files were found on the drive.".to_owned(),
        1 => "1 file imported into Imported.".to_owned(),
        count => format!("{count} files imported into Imported."),
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
    rows: gtk::ListBox,
    configure: gtk::Box,
    action: gtk::Button,
    action_cap: gtk::Label,
    action_word: gtk::Label,
    quiet: gtk::Button,
    quiet_cap: gtk::Label,
    quiet_word: gtk::Label,
    progress: gtk::Box,
    progress_heading: gtk::Label,
    progress_line: gtk::Label,
    result: gtk::Box,
    result_heading: gtk::Label,
    result_location: gtk::Label,
    result_detail: gtk::Label,
    state: RefCell<ExportState>,
    prefs: RefCell<ExportPrefs>,
    client: Client,
    nav: Rc<Navigator>,
    editor: Rc<EditorPage>,
    store: Store,
    updating: Cell<bool>,
}

impl ExportPage {
    pub fn new(
        client: Client,
        nav: Rc<Navigator>,
        editor: Rc<EditorPage>,
        store: Store,
    ) -> Rc<Self> {
        let screen = ui::Screen::new("Export");
        screen.set_hints(&EXPORT_HINTS);

        let rows = ui::list();
        rows.set_selection_mode(gtk::SelectionMode::Single);

        let (action, action_cap, action_word) =
            action_button("Enter", "Export", "export-action", "action-word");
        action.set_margin_top(22);
        let (quiet, quiet_cap, quiet_word) =
            action_button("I", "Import from a USB drive", "quiet-action", "quiet-word");
        quiet.set_margin_top(12);

        let configure = gtk::Box::new(gtk::Orientation::Vertical, 0);
        configure.append(&rows);
        configure.append(&action);
        configure.append(&quiet);
        screen.content.append(&configure);

        let progress_heading = gtk::Label::new(None);
        progress_heading.set_xalign(0.0);
        progress_heading.set_wrap(true);
        progress_heading.add_css_class("step-title");
        let progress_line = gtk::Label::new(None);
        progress_line.set_xalign(0.0);
        progress_line.set_wrap(true);
        progress_line.add_css_class("row-note");
        let progress = gtk::Box::new(gtk::Orientation::Vertical, 8);
        progress.set_margin_top(22);
        progress.append(&progress_heading);
        progress.append(&progress_line);
        screen.content.append(&progress);

        let result_heading = gtk::Label::new(None);
        result_heading.set_xalign(0.0);
        result_heading.set_wrap(true);
        result_heading.add_css_class("step-title");
        let result_location = gtk::Label::new(None);
        result_location.set_xalign(0.0);
        result_location.set_wrap(true);
        result_location.add_css_class("row-note");
        let result_detail = gtk::Label::new(None);
        result_detail.set_xalign(0.0);
        result_detail.set_wrap(true);
        result_detail.add_css_class("file-name");
        let result = gtk::Box::new(gtk::Orientation::Vertical, 8);
        result.set_margin_top(22);
        result.append(&result_heading);
        result.append(&result_location);
        result.append(&result_detail);
        screen.content.append(&result);

        let prefs = ExportPrefs::load(store.root());
        let page = Rc::new(Self {
            widget: screen.root.clone(),
            screen,
            rows,
            configure,
            action,
            action_cap,
            action_word,
            quiet,
            quiet_cap,
            quiet_word,
            progress,
            progress_heading,
            progress_line,
            result,
            result_heading,
            result_location,
            result_detail,
            state: RefCell::new(ExportState::default()),
            prefs: RefCell::new(prefs),
            client,
            nav,
            editor,
            store,
            updating: Cell::new(false),
        });
        page.install();
        page
    }

    fn install(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.rows.connect_row_selected(move |_, row| {
            if let Some(page) = weak.upgrade() {
                if page.updating.get() {
                    return;
                }
                if let Some(row) = row {
                    page.state.borrow_mut().select(row.index().max(0) as usize);
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.rows.connect_row_activated(move |_, row| {
            if let Some(page) = weak.upgrade() {
                page.state.borrow_mut().select(row.index().max(0) as usize);
                page.state.borrow_mut().cycle(1);
                page.refresh();
            }
        });
        let weak = Rc::downgrade(self);
        self.action.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.choose();
            }
        });
        let weak = Rc::downgrade(self);
        self.quiet.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.quiet_action();
            }
        });
        let weak = Rc::downgrade(self);
        self.widget.connect_map(move |_| {
            if let Some(page) = weak.upgrade() {
                page.on_open();
                page.screen.focus_page();
            }
        });

        // A drive can arrive at any moment. Two quiet checks a second are
        // enough for the To row to turn into the drive while the writer
        // watches, and they stop as soon as the screen is hidden.
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(2, move || {
            let Some(page) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if page.widget.is_mapped() && page.state.borrow().step == ExportStep::Configure {
                page.load_drives();
            }
            glib::ControlFlow::Continue
        });
    }

    /// Open the screen with the choices it remembers. Used when Ctrl+E is
    /// pressed while the finished result is already on screen.
    pub fn reopen(self: &Rc<Self>) {
        if self.state.borrow().step == ExportStep::Running {
            return;
        }
        self.prefs.borrow_mut().what = self.state.borrow().target;
        self.prefs.borrow_mut().folder = self.state.borrow().folder.clone();
        self.prefs.borrow_mut().format = self.state.borrow().format;
        {
            let mut state = self.state.borrow_mut();
            state.mode = ExportMode::Export;
            state.step = ExportStep::Configure;
            state.row = 0;
            state.outcome = None;
            state.error = None;
            state.normalize();
        }
        self.refresh();
        self.load_drives();
    }

    /// Show the destination directly, as if "Everything" had been chosen.
    /// Used by the screenshot script; harmless otherwise.
    pub fn start_at_drive(self: &Rc<Self>) {
        self.nav.show("export");
        {
            let mut state = self.state.borrow_mut();
            state.open(
                &self.prefs.borrow(),
                self.state_current_file(),
                self.state_folders(),
            );
            state.target = ExportTarget::Everything;
        }
        self.refresh();
        self.load_drives();
    }

    fn on_open(self: &Rc<Self>) {
        let current_file = self
            .editor
            .current_rel()
            .map(|path| path.to_string_lossy().to_string());
        let folders = self
            .store
            .list(Path::new(""))
            .map(|entries| {
                entries
                    .into_iter()
                    .filter(|entry| entry.is_dir)
                    .map(|entry| entry.name)
                    .collect()
            })
            .unwrap_or_default();
        {
            let mut state = self.state.borrow_mut();
            // A running export keeps its progress screen if the writer comes
            // back to it; anything else starts fresh with the remembered
            // choices.
            if state.step != ExportStep::Running {
                state.open(&self.prefs.borrow(), current_file, folders);
            }
        }
        self.refresh();
        self.load_drives();
    }

    fn state_current_file(&self) -> Option<String> {
        self.editor
            .current_rel()
            .map(|path| path.to_string_lossy().to_string())
    }

    fn state_folders(&self) -> Vec<String> {
        self.store
            .list(Path::new(""))
            .map(|entries| {
                entries
                    .into_iter()
                    .filter(|entry| entry.is_dir)
                    .map(|entry| entry.name)
                    .collect()
            })
            .unwrap_or_default()
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
                    Ok(drives) => page.state.borrow_mut().on_drives(drives),
                    Err(error) => page.state.borrow_mut().error = Some(error.message),
                }
                page.refresh();
            },
        );
    }

    fn choose(self: &Rc<Self>) {
        let effect = self.state.borrow_mut().choose();
        if effect.is_none() {
            self.refresh();
            return;
        }
        let prefs = self.state.borrow().prefs();
        prefs.save(self.store.root());
        *self.prefs.borrow_mut() = prefs;
        if let Some(effect) = effect {
            self.run_effect(effect);
        }
    }

    fn quiet_action(self: &Rc<Self>) {
        if self.state.borrow().step == ExportStep::Running {
            return;
        }
        let to_import = self.state.borrow().mode == ExportMode::Export;
        self.show_mode(if to_import {
            ExportMode::Import
        } else {
            ExportMode::Export
        });
    }

    /// One screen, two jobs: the three export rows or the import row.
    fn show_mode(self: &Rc<Self>, mode: ExportMode) {
        {
            let mut state = self.state.borrow_mut();
            state.mode = mode;
            state.step = ExportStep::Configure;
            state.row = 0;
            state.outcome = None;
            state.error = None;
        }
        self.refresh();
        self.load_drives();
    }

    fn run_effect(self: &Rc<Self>, effect: ExportEffect) {
        match effect {
            ExportEffect::Export {
                request,
                drive,
                file,
            } => {
                {
                    let mut state = self.state.borrow_mut();
                    state.on_running(drive.clone());
                }
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
                            Ok(path) => {
                                page.state.borrow_mut().on_exported(
                                    path,
                                    drive.clone(),
                                    file.clone(),
                                );
                            }
                            Err(error) => page.state.borrow_mut().on_error(error.message),
                        }
                        page.refresh();
                    },
                );
            }
            ExportEffect::Import { drive_id, drive } => {
                {
                    let mut state = self.state.borrow_mut();
                    state.on_running(drive.clone());
                }
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

    /// Whether the action is enabled, for the GTK scenario that plugs a drive
    /// in after the screen opened.
    #[cfg(test)]
    pub(crate) fn can_export(&self) -> bool {
        self.state.borrow().can_export()
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
            gdk::Key::Up if step == ExportStep::Configure => {
                self.state.borrow_mut().move_by(-1);
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Down if step == ExportStep::Configure => {
                self.state.borrow_mut().move_by(1);
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Left if step == ExportStep::Configure => {
                self.state.borrow_mut().cycle(-1);
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Right if step == ExportStep::Configure => {
                self.state.borrow_mut().cycle(1);
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                match step {
                    ExportStep::Configure => self.choose(),
                    ExportStep::Done => self.nav.show_editor(),
                    ExportStep::Running => {}
                }
                glib::Propagation::Stop
            }
            _ if keyval.to_lower() == gdk::Key::i && step != ExportStep::Running => {
                // I is always "Import": the quiet action on the export rows,
                // and "Import something else" on the finished result.
                self.show_mode(ExportMode::Import);
                glib::Propagation::Stop
            }
            _ if keyval.to_lower() == gdk::Key::e && step != ExportStep::Running => {
                // E is always "Export": the quiet action while importing, and
                // "Export something else" on the finished result.
                self.show_mode(ExportMode::Export);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn refresh(self: &Rc<Self>) {
        let (step, error, import) = {
            let state = self.state.borrow();
            (state.step, state.error.clone(), state.is_import())
        };
        self.screen
            .set_title(if import { "Import" } else { "Export" });
        let status = self.state.borrow().status(&ui::local_date());
        if error.is_some() {
            self.screen.set_status_error(&status);
        } else {
            self.screen.set_status(&status);
        }

        self.configure.set_visible(step == ExportStep::Configure);
        self.progress.set_visible(step == ExportStep::Running);
        self.result.set_visible(step == ExportStep::Done);

        match step {
            ExportStep::Configure => {
                self.rebuild_rows();
                let state = self.state.borrow();
                let exporting = state.mode == ExportMode::Export;
                self.action_cap.set_text("Enter");
                self.action_word
                    .set_text(if exporting { "Export" } else { "Import" });
                let can = state.can_export();
                self.action.set_sensitive(can);
                if can {
                    self.action.remove_css_class("action-disabled");
                } else {
                    self.action.add_css_class("action-disabled");
                }
                self.quiet_cap.set_text(if exporting { "I" } else { "E" });
                self.quiet_word.set_text(if exporting {
                    "Import from a USB drive"
                } else {
                    "Export instead"
                });
                let hints = match (exporting, can) {
                    (true, true) => &EXPORT_HINTS[..],
                    (true, false) => &EXPORT_NO_DRIVE_HINTS[..],
                    (false, true) => &IMPORT_HINTS[..],
                    (false, false) => &IMPORT_NO_DRIVE_HINTS[..],
                };
                drop(state);
                self.screen.set_hints(hints);
            }
            ExportStep::Running => {
                self.progress_heading
                    .set_text(&self.state.borrow().progress_heading());
                self.progress_line
                    .set_text(self.state.borrow().progress_line());
                self.screen.set_hints(&RUNNING_HINTS);
            }
            ExportStep::Done => {
                let state = self.state.borrow();
                let heading = state.outcome_heading().unwrap_or_default();
                let lines = state.outcome_lines();
                self.result_heading.set_text(&heading);
                self.result_heading.set_visible(!heading.is_empty());
                let (location, detail) = match lines.as_slice() {
                    [] => (None, None),
                    [only] => (None, Some(only.as_str())),
                    [location, detail, ..] => (Some(location.as_str()), Some(detail.as_str())),
                };
                self.result_location.set_text(location.unwrap_or_default());
                self.result_location.set_visible(location.is_some());
                self.result_detail.set_text(detail.unwrap_or_default());
                self.result_detail.set_visible(detail.is_some());
                let hints = if state.is_import() {
                    &DONE_IMPORT_HINTS[..]
                } else {
                    &DONE_EXPORT_HINTS[..]
                };
                drop(state);
                self.screen.set_hints(hints);
            }
        }
    }

    fn rebuild_rows(self: &Rc<Self>) {
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        self.updating.set(true);
        let (row, target, format, folder, drive, has_file, current_path, folder_count, drive_count) = {
            let state = self.state.borrow();
            (
                state.row,
                state.target,
                state.format,
                state.folder_label(),
                state.current_drive().map(|drive| {
                    format!("{} · {} free", drive.label, free_space(drive.free_bytes))
                }),
                state.current_file.is_some(),
                state.current_file.clone(),
                state.folders.len(),
                state.drives.len(),
            )
        };
        for kind in self.state.borrow().rows() {
            let (name, value, dim, changeable) = match kind {
                ExportRow::What => (
                    "What",
                    match target {
                        ExportTarget::Current if has_file => format!(
                            "Current file · {}",
                            display_path(Path::new(current_path.as_deref().unwrap_or_default()))
                        ),
                        ExportTarget::Current => "Current file · no file open".to_owned(),
                        ExportTarget::Folder => "A folder…".to_owned(),
                        ExportTarget::Everything => "Everything".to_owned(),
                    },
                    target == ExportTarget::Current && !has_file,
                    true,
                ),
                ExportRow::Folder => ("Folder", folder.clone(), false, folder_count > 0),
                ExportRow::Format => ("Format", format.label().to_owned(), false, true),
                ExportRow::To => (
                    "To",
                    drive
                        .clone()
                        .unwrap_or_else(|| "Insert a USB drive".to_owned()),
                    drive.is_none(),
                    drive_count > 1,
                ),
            };
            let element = gtk::ListBoxRow::new();
            element.add_css_class("ui-row");
            element.add_css_class("file-row");
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let label = gtk::Label::new(Some(name));
            label.set_xalign(0.0);
            label.set_size_request(86, -1);
            label.add_css_class("setting-value");
            let value = gtk::Label::new(Some(&value));
            value.set_xalign(0.0);
            value.set_hexpand(true);
            value.set_ellipsize(gtk::pango::EllipsizeMode::End);
            value.add_css_class(if dim { "row-note" } else { "file-name" });
            let arrows = gtk::Label::new(Some("‹ ›"));
            arrows.add_css_class("row-note");
            arrows.set_visible(changeable);
            line.append(&label);
            line.append(&value);
            line.append(&arrows);
            element.set_child(Some(&line));
            self.rows.append(&element);
        }
        ui::select(&self.rows, row);
        self.updating.set(false);
    }
}

/// A button that reads as a key cap and its action, not as chrome.
fn action_button(
    cap: &str,
    word: &str,
    class: &str,
    word_class: &str,
) -> (gtk::Button, gtk::Label, gtk::Label) {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.set_halign(gtk::Align::Start);
    let cap = gtk::Label::new(Some(cap));
    cap.add_css_class("key-cap");
    cap.set_valign(gtk::Align::Center);
    let word = gtk::Label::new(Some(word));
    word.add_css_class(word_class);
    word.set_valign(gtk::Align::Center);
    content.append(&cap);
    content.append(&word);
    let button = gtk::Button::new();
    button.set_child(Some(&content));
    button.add_css_class(class);
    // The page owns the keyboard; a click still works.
    button.set_focusable(false);
    button.set_can_focus(false);
    (button, cap, word)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{DaemonClient, FakeDaemon};
    use tempfile::tempdir;

    fn drive() -> Drive {
        Drive {
            id: "usb-1".into(),
            label: "CARREL USB".into(),
            free_bytes: 12_400_000_000,
        }
    }

    fn state_with_file() -> ExportState {
        let mut state = ExportState::default();
        state.open(
            &ExportPrefs::default(),
            Some("The quiet morning.md".into()),
            vec!["Novel".into(), "Notes".into()],
        );
        state.on_drives(vec![drive()]);
        state
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
        assert_eq!(ExportFormat::Docx.extension(), "docx");
        assert_eq!(ExportFormat::Text.extension(), "txt");
    }

    #[test]
    fn the_screen_opens_as_one_compact_set_of_rows() {
        let state = state_with_file();
        assert_eq!(
            state.rows(),
            vec![ExportRow::What, ExportRow::Format, ExportRow::To]
        );
        assert_eq!(state.selected_row(), ExportRow::What);
        assert_eq!(state.target, ExportTarget::Current);
        assert_eq!(state.format, ExportFormat::Markdown);
        assert_eq!(state.current_drive().unwrap().label, "CARREL USB");
    }

    #[test]
    fn a_folder_adds_one_row_and_keeps_its_name() {
        let mut state = state_with_file();
        state.cycle(1);
        assert_eq!(state.target, ExportTarget::Folder);
        assert_eq!(
            state.rows(),
            vec![
                ExportRow::What,
                ExportRow::Folder,
                ExportRow::Format,
                ExportRow::To
            ]
        );
        state.move_by(1);
        assert_eq!(state.selected_row(), ExportRow::Folder);
        state.cycle(1);
        assert_eq!(state.folder, "Novel");
        assert_eq!(state.folder_label(), "Novel");
        state.cycle(-1);
        assert_eq!(state.folder, "");
        assert_eq!(state.folder_label(), "Writing");
    }

    #[test]
    fn left_and_right_change_format_and_drive_without_a_list_screen() {
        let mut state = state_with_file();
        state.move_by(1);
        assert_eq!(state.selected_row(), ExportRow::Format);
        state.cycle(1);
        assert_eq!(state.format, ExportFormat::Text);
        state.cycle(1);
        assert_eq!(state.format, ExportFormat::Docx);
        state.cycle(-1);
        assert_eq!(state.format, ExportFormat::Text);
        state.cycle(3);
        assert_eq!(state.format, ExportFormat::Pdf);

        state.on_drives(vec![
            drive(),
            Drive {
                id: "usb-2".into(),
                label: "ARCHIVE".into(),
                free_bytes: 2_000_000_000,
            },
        ]);
        state.move_by(1);
        assert_eq!(state.selected_row(), ExportRow::To);
        state.cycle(1);
        assert_eq!(state.current_drive().unwrap().id, "usb-2");
        state.cycle(1);
        assert_eq!(state.current_drive().unwrap().id, "usb-1");
    }

    #[test]
    fn enter_exports_the_current_file_in_the_chosen_format() {
        let mut state = state_with_file();
        state.move_by(1);
        state.cycle(1);
        state.cycle(1); // Word .docx
        let effect = state.choose().unwrap();
        assert_eq!(
            effect,
            ExportEffect::Export {
                request: ExportRequest {
                    selection: vec!["current:The quiet morning.md".into()],
                    format: "docx".into(),
                    drive_id: "usb-1".into(),
                },
                drive: "CARREL USB".into(),
                file: Some("The quiet morning.docx".into()),
            }
        );
    }

    #[test]
    fn everything_is_one_selection_and_a_dated_folder() {
        let mut state = state_with_file();
        state.cycle(1);
        state.cycle(1); // Everything
        assert_eq!(state.target, ExportTarget::Everything);
        let ExportEffect::Export { request, file, .. } = state.choose().unwrap() else {
            panic!("expected an export");
        };
        assert_eq!(request.selection, vec!["all"]);
        assert_eq!(file, None);
        assert!(state
            .status("2026-11-12")
            .contains("Carrel export 2026-11-12/"));
    }

    #[test]
    fn a_folder_keeps_its_path_in_the_selection() {
        let mut state = state_with_file();
        state.cycle(1);
        state.move_by(1);
        state.cycle(1);
        let ExportEffect::Export { request, .. } = state.choose().unwrap() else {
            panic!("expected an export");
        };
        assert_eq!(request.selection, vec!["folder:Novel"]);
    }

    #[test]
    fn no_drive_says_insert_and_disables_the_action() {
        let mut state = state_with_file();
        state.on_drives(Vec::new());
        assert!(!state.can_export());
        assert_eq!(state.choose(), None);
        assert!(state.status("2026-11-13").contains("Plug in a drive"));
    }

    #[test]
    fn a_drive_that_appears_updates_the_state_without_moving_the_file() {
        let mut state = state_with_file();
        state.on_drives(Vec::new());
        assert!(!state.can_export());
        state.on_drives(vec![drive()]);
        assert!(state.can_export());
        assert_eq!(state.current_drive().unwrap().label, "CARREL USB");
    }

    #[test]
    fn the_result_names_the_drive_the_folder_and_the_file() {
        let mut state = state_with_file();
        state.on_exported(
            "/media/carrel/usb-1/Carrel export 2026-11-13/".into(),
            "CARREL USB".into(),
            Some("The quiet morning.docx".into()),
        );
        assert_eq!(
            state.outcome_heading().as_deref(),
            Some("Exported to CARREL USB")
        );
        assert_eq!(
            state.outcome_lines(),
            vec![
                "Carrel export 2026-11-13".to_owned(),
                "The quiet morning.docx. Safe to remove.".to_owned()
            ]
        );
        assert_eq!(state.step, ExportStep::Done);
    }

    #[test]
    fn a_file_path_still_yields_the_folder_above_it() {
        assert_eq!(
            export_location("/media/carrel/usb-1/Carrel export 2026-11-13/").as_deref(),
            Some("Carrel export 2026-11-13")
        );
        assert_eq!(
            export_location("/media/carrel/usb-1/The quiet morning.docx").as_deref(),
            Some("usb-1")
        );
        assert_eq!(export_location(""), None);
    }

    #[test]
    fn the_import_result_is_said_once() {
        let mut state = state_with_file();
        state.mode = ExportMode::Import;
        assert_eq!(state.rows(), vec![ExportRow::To]);
        let effect = state.choose().unwrap();
        assert_eq!(
            effect,
            ExportEffect::Import {
                drive_id: "usb-1".into(),
                drive: "CARREL USB".into(),
            }
        );
        state.on_imported(vec!["Imported/one.md".into(), "Imported/two.txt".into()]);
        assert_eq!(
            state.outcome_heading(),
            None,
            "the import result is one line"
        );
        assert_eq!(
            state.outcome_lines(),
            vec!["2 files imported into Imported.".to_owned()]
        );
        assert!(state.is_import());
        assert_eq!(imported_text(1), "1 file imported into Imported.");
        assert_eq!(
            imported_text(0),
            "No .md or .txt files were found on the drive."
        );
    }

    #[test]
    fn an_export_error_returns_to_the_configured_screen_with_one_line() {
        let mut state = state_with_file();
        state.on_running("CARREL USB".into());
        state.on_error("the drive was removed".into());
        assert_eq!(state.step, ExportStep::Configure);
        assert_eq!(state.error.as_deref(), Some("the drive was removed"));
        assert_eq!(state.status("2026-11-13"), "the drive was removed");
    }

    #[test]
    fn the_choices_are_remembered_in_the_writing_folder() {
        let dir = tempdir().unwrap();
        let prefs = ExportPrefs {
            what: ExportTarget::Everything,
            folder: "Novel".into(),
            format: ExportFormat::Epub,
            drive_id: "usb-1".into(),
        };
        prefs.save(dir.path());
        assert_eq!(ExportPrefs::load(dir.path()), prefs);

        let mut state = ExportState::default();
        state.open(&prefs, Some("Draft.md".into()), vec!["Novel".into()]);
        assert_eq!(state.target, ExportTarget::Everything);
        assert_eq!(state.format, ExportFormat::Epub);
        assert_eq!(state.drive_id, "usb-1");
        state.on_drives(vec![drive()]);
        assert_eq!(state.current_drive().unwrap().id, "usb-1");

        // A missing file is simply the spec defaults.
        let empty = tempdir().unwrap();
        assert_eq!(ExportPrefs::load(empty.path()), ExportPrefs::default());
    }

    #[test]
    fn a_remembered_folder_that_is_gone_falls_back_to_writing() {
        let prefs = ExportPrefs {
            what: ExportTarget::Folder,
            folder: "Gone".into(),
            ..ExportPrefs::default()
        };
        let mut state = ExportState::default();
        state.open(&prefs, Some("Draft.md".into()), Vec::new());
        assert_eq!(state.target, ExportTarget::Folder);
        assert_eq!(state.folder, "");
        assert_eq!(state.folder_label(), "Writing");
    }

    #[test]
    fn open_a_file_first_is_the_only_error_left_on_the_screen() {
        let mut state = ExportState::default();
        state.open(&ExportPrefs::default(), None, Vec::new());
        state.on_drives(vec![drive()]);
        assert!(state.can_export());
        assert_eq!(state.choose(), None);
        assert_eq!(state.error.as_deref(), Some("Open a file first."));
    }

    #[test]
    fn the_remembered_drive_is_used_again_when_it_comes_back() {
        let prefs = ExportPrefs {
            drive_id: "usb-2".into(),
            ..ExportPrefs::default()
        };
        let mut state = ExportState::default();
        state.open(&prefs, Some("Draft.md".into()), Vec::new());
        state.on_drives(vec![drive()]);
        assert_eq!(
            state.current_drive().unwrap().id,
            "usb-1",
            "the first drive answers while the remembered one is away"
        );
        state.on_drives(vec![
            drive(),
            Drive {
                id: "usb-2".into(),
                label: "ARCHIVE".into(),
                free_bytes: 2_000_000_000,
            },
        ]);
        assert_eq!(state.current_drive().unwrap().label, "ARCHIVE");
    }

    #[test]
    fn free_space_is_readable() {
        assert_eq!(free_space(12_400_000_000), "12.4 GB");
        assert_eq!(free_space(620_000_000), "620 MB");
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
}
