//! The Sync screen: the state is the hero, the one action that helps comes
//! first, and the services are choices with one line each.
//!
//! The daemon owns the network and the radio; this screen only starts calls
//! and draws the `SyncStatus` / `PairStatus` signals it receives. Two things it
//! remembers itself, because the daemon reports a sync but not its
//! configuration: which services were paired through this screen
//! (`.carrel/sync-ui.json`), and whether the Wi-Fi screen was opened to fix a
//! sync error (see [`crate::handoff`]).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use serde::{Deserialize, Serialize};

use crate::daemon::{Backend, Client, PairingInfo, SyncInfo};
use crate::handoff::WifiHandoff;
use crate::nav::Navigator;
use crate::ui;

const OVERVIEW_HINTS: [(&str, &str); 3] = [("Enter", "Choose"), ("↑ ↓", "Move"), ("Esc", "Back")];
const URL_HINTS: [(&str, &str); 2] = [("Enter", "Pair"), ("Esc", "Back")];
const PAIRING_HINTS: [(&str, &str); 1] = [("Esc", "Cancel")];

/// The beat between "Paired." and the Sync overview: long enough to read the
/// word, short enough not to hold the writer. A pause, not an animation.
const PAIRED_BEAT: std::time::Duration = std::time::Duration::from_millis(2_000);

/// The sentence under the one action an error offers.
const REMEDY_NOTE: &str = "Your writing stays here. Carrel syncs when you are back online.";

/// The one line an error leaves the writer with. It is derived from the
/// daemon's message, which is a sentence, not a code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// No working network: choose one, then Sync continues by itself.
    Wifi,
    /// Nothing is paired: the three services are the next step.
    Pair,
    /// The service could not be reached: try again.
    Retry,
}

/// Classify the daemon's error line. Anything unrecognised is a plain retry,
/// which is always a safe thing to offer.
pub fn remedy_for(message: &str) -> Remedy {
    let message = message.to_ascii_lowercase();
    const UNPAIRED: [&str; 4] = [
        "not paired",
        "no sync server",
        "connect a service",
        "unpaired",
    ];
    const NETWORK: [&str; 9] = [
        "wi-fi",
        "wifi",
        "no network",
        "network is off",
        "network is unreachable",
        "no route",
        "dns",
        "name resolution",
        "offline",
    ];
    if UNPAIRED.iter().any(|phrase| message.contains(phrase)) {
        Remedy::Pair
    } else if NETWORK.iter().any(|phrase| message.contains(phrase)) {
        Remedy::Wifi
    } else {
        Remedy::Retry
    }
}

/// One row of the overview. Only actions are rows the keyboard lands on; the
/// "Connections" label between them is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// The error's one useful action: open Wi-Fi, then come back and sync.
    ChooseNetwork,
    SyncNow,
    Pair(Backend),
    Unpair,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncEffect {
    Sync,
    /// Open the Wi-Fi screen and come back here when a network is up.
    ChooseNetwork,
    Pair {
        backend: Backend,
        url: String,
    },
    Unpair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncView {
    #[default]
    Overview,
    SelfHostedUrl,
    Pairing,
}

/// The pure Sync screen state.
#[derive(Debug, Default)]
pub struct SyncState {
    pub info: SyncInfo,
    pub view: SyncView,
    /// Index into [`SyncState::rows`].
    pub selected: usize,
    pub backend: Option<Backend>,
    pub code: String,
    pub approval_url: String,
    /// True while the daemon polls for approval.
    pub waiting: bool,
    /// True for the beat between "Paired." and the overview.
    pub paired: bool,
    pub notice: String,
    pub error: Option<String>,
    /// The services paired through this screen, newest last.
    pub paired_services: Vec<Backend>,
    /// True once the shell has a pairing memory file (even an empty one), so
    /// it stops guessing from `last_sync`.
    pub memory_known: bool,
}

impl SyncState {
    /// Whether a service is connected. The daemon reports the last sync but
    /// not the configuration, so an old install with no memory file is given
    /// the benefit of the doubt when it has synced at least once.
    pub fn is_paired(&self) -> bool {
        !self.paired_services.is_empty() || (!self.memory_known && self.info.last_sync > 0)
    }

    pub fn remedy(&self) -> Option<Remedy> {
        if self.info.state != "error" {
            return None;
        }
        Some(remedy_for(&self.info.message))
    }

    /// The large one-line state at the top of the screen.
    pub fn hero(&self) -> String {
        match self.info.state.as_str() {
            "syncing" => {
                let message = self.info.message.trim();
                if message.is_empty() {
                    "Syncing…".to_owned()
                } else {
                    single_line(message)
                }
            }
            "error" => match self.remedy() {
                Some(Remedy::Wifi) | Some(Remedy::Retry) => match self.service_name() {
                    Some(name) => format!("Could not reach {name}"),
                    None => "Could not sync".to_owned(),
                },
                _ => "Not connected yet".to_owned(),
            },
            _ if !self.is_paired() => "Not connected yet".to_owned(),
            _ if self.info.last_sync > 0 => "Up to date".to_owned(),
            _ => "Ready for your first sync".to_owned(),
        }
    }

    /// The last sync time, or the reassurance that nothing has left the device.
    pub fn last_sync_line(&self) -> String {
        if self.info.last_sync > 0 {
            format!("Last sync: {}", ui::format_moment(self.info.last_sync))
        } else if self.is_paired() {
            "Not synced yet. Everything is on this device.".to_owned()
        } else {
            "Never synced. Everything is on this device.".to_owned()
        }
    }

    /// The service(s) this device is connected to, named.
    pub fn connected_line(&self) -> String {
        match self.paired_services.as_slice() {
            [] => String::new(),
            [one] => format!("Connected to {}", one.connected_label()),
            [first, second] => format!(
                "Connected to {} and {}",
                first.connected_label(),
                second.connected_label()
            ),
            _ => format!(
                "Connected to {}, {} and {}",
                self.paired_services[0].connected_label(),
                self.paired_services[1].connected_label(),
                self.paired_services[2].connected_label()
            ),
        }
    }

    /// The daemon's own sentence, shown only when it adds something: a server
    /// failure the screen cannot explain better.
    pub fn detail(&self) -> String {
        match self.remedy() {
            Some(Remedy::Retry) => single_line(&self.info.message),
            _ => String::new(),
        }
    }

    fn service_name(&self) -> Option<&'static str> {
        match self.paired_services.as_slice() {
            [one] => Some(one.connected_label()),
            _ => None,
        }
    }

    fn unconnected(&self) -> Vec<Backend> {
        if self.paired_services.is_empty() {
            return Vec::new();
        }
        Backend::ALL
            .iter()
            .copied()
            .filter(|backend| !self.paired_services.contains(backend))
            .collect()
    }

    /// The overview's rows, in screen order. The keyboard only lands on these.
    pub fn rows(&self) -> Vec<Row> {
        if !self.is_paired() {
            return Backend::ALL.iter().copied().map(Row::Pair).collect();
        }
        let mut rows = Vec::new();
        if self.remedy() == Some(Remedy::Wifi) {
            rows.push(Row::ChooseNetwork);
        }
        rows.push(Row::SyncNow);
        rows.extend(self.unconnected().into_iter().map(Row::Pair));
        rows.push(Row::Unpair);
        rows
    }

    /// Where the quiet "Connections" label goes, when there is a group to show.
    pub fn connections_at(&self) -> Option<usize> {
        if !self.is_paired() || self.unconnected().is_empty() {
            return None;
        }
        let before = if self.remedy() == Some(Remedy::Wifi) {
            2
        } else {
            1
        };
        Some(before)
    }

    pub fn action(&self) -> Option<Row> {
        self.rows().get(self.selected).copied()
    }

    pub fn move_by(&mut self, delta: i32) {
        let len = self.rows().len() as i32;
        if len == 0 {
            return;
        }
        self.selected = ((self.selected as i32 + delta).rem_euclid(len)) as usize;
    }

    pub fn row_label(&self, row: Row) -> String {
        match row {
            Row::SyncNow => "Sync now".to_owned(),
            Row::ChooseNetwork => "No Wi-Fi. Choose a network".to_owned(),
            Row::Pair(backend) if self.is_paired() => format!("Pair with {}", backend.pair_label()),
            Row::Pair(backend) => backend.label().to_owned(),
            Row::Unpair => self.unpair_label(),
        }
    }

    pub fn row_note(&self, row: Row) -> &'static str {
        match row {
            Row::Pair(backend) if !self.is_paired() => backend.promise(),
            _ => "",
        }
    }

    fn unpair_label(&self) -> String {
        match self.paired_services.as_slice() {
            [] => "Unpair".to_owned(),
            [one] => format!("Unpair {}", one.connected_label()),
            _ => "Unpair all services".to_owned(),
        }
    }

    /// Enter on the overview. Pairing a self-hosted server needs the URL first,
    /// so it only switches the view.
    pub fn choose(&mut self) -> Option<SyncEffect> {
        match self.action()? {
            Row::SyncNow => Some(SyncEffect::Sync),
            Row::ChooseNetwork => Some(SyncEffect::ChooseNetwork),
            Row::Pair(Backend::SelfHosted) => {
                self.view = SyncView::SelfHostedUrl;
                self.error = None;
                None
            }
            Row::Pair(backend) => {
                self.begin_pairing(backend);
                Some(SyncEffect::Pair {
                    backend,
                    url: String::new(),
                })
            }
            Row::Unpair => Some(SyncEffect::Unpair),
        }
    }

    pub fn submit_server_url(&mut self, url: &str) -> Result<SyncEffect, String> {
        let url = url.trim();
        if url.is_empty() {
            return Err("Type the address of your Carrel server.".to_owned());
        }
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err("The address must start with https:// or http://.".to_owned());
        }
        self.begin_pairing(Backend::SelfHosted);
        Ok(SyncEffect::Pair {
            backend: Backend::SelfHosted,
            url: url.to_owned(),
        })
    }

    fn begin_pairing(&mut self, backend: Backend) {
        self.view = SyncView::Pairing;
        self.backend = Some(backend);
        self.code.clear();
        self.approval_url.clear();
        self.waiting = true;
        self.paired = false;
        self.error = None;
        self.notice.clear();
    }

    pub fn on_pair_info(&mut self, info: PairingInfo) {
        self.view = SyncView::Pairing;
        self.backend = Backend::ALL
            .iter()
            .copied()
            .find(|backend| backend.wire() == info.backend)
            .or(self.backend);
        self.code = info.code;
        self.approval_url = info.approval_url;
        // Approval can arrive before the start call returns; it wins.
        if !self.paired {
            self.waiting = true;
            self.error = None;
        }
    }

    /// `approved` stops at "Paired." for a beat (the page returns afterwards);
    /// anything else returns to the overview with one plain line.
    pub fn on_pair_status(&mut self, state: &str, _code: &str, message: String) {
        self.waiting = false;
        if state != "approved" {
            self.view = SyncView::Overview;
            self.paired = false;
            self.error = Some(message);
            return;
        }
        if let Some(backend) = self.backend {
            if !self.paired_services.contains(&backend) {
                self.paired_services.push(backend);
            }
        }
        // The failure that sent the writer here described the pairing they
        // just replaced.
        self.info = SyncInfo::default();
        self.paired = true;
        self.notice = message;
        self.error = None;
    }

    pub fn on_sync(&mut self, info: SyncInfo) {
        self.info = info;
    }

    /// Backspace or Esc in a flow goes back one step, not to the editor.
    pub fn back(&mut self) -> bool {
        match self.view {
            SyncView::Overview => false,
            SyncView::SelfHostedUrl | SyncView::Pairing => {
                self.view = SyncView::Overview;
                self.paired = false;
                self.waiting = false;
                self.error = None;
                true
            }
        }
    }

    pub fn qr_data(&self) -> String {
        if self.approval_url.is_empty() {
            return String::new();
        }
        match self.backend {
            Some(Backend::GoogleDrive) => self.approval_url.clone(),
            _ => format!("{}?code={}", self.approval_url, self.code),
        }
    }

    /// The one sentence above the code's companion QR, telling the writer what
    /// to do on the phone.
    pub fn pairing_sentence(&self) -> String {
        if self.approval_url.is_empty() {
            return "Starting pairing…".to_owned();
        }
        match self.backend {
            Some(Backend::Nostos) => {
                "On your phone, scan the code or open app.nostos.page/pair and enter the code."
                    .to_owned()
            }
            Some(Backend::SelfHosted) => format!(
                "On your phone, scan the code or open {} and enter the code.",
                self.approval_url
            ),
            Some(Backend::GoogleDrive) => {
                "On your phone, open google.com/device and enter the code.".to_owned()
            }
            None => {
                "On your phone, scan the code or open the address and enter the code.".to_owned()
            }
        }
    }

    pub fn pairing_note(&self) -> &'static str {
        self.backend.map(Backend::promise).unwrap_or("")
    }

    /// What the live line says under the code.
    pub fn pairing_live(&self) -> &'static str {
        if self.paired {
            "Paired."
        } else {
            "Waiting for approval…"
        }
    }
}

impl Backend {
    /// "Nostos", "a self-hosted server", "Google Drive": the service after
    /// "Pair with".
    fn pair_label(self) -> &'static str {
        match self {
            Self::Nostos => "Nostos",
            Self::SelfHosted => "a self-hosted server",
            Self::GoogleDrive => "Google Drive",
        }
    }

    /// The service after "Connected to".
    fn connected_label(self) -> &'static str {
        match self {
            Self::Nostos => "Nostos",
            Self::SelfHosted => "your server",
            Self::GoogleDrive => "Google Drive",
        }
    }

    /// What the writer gets, in one line: the service choices and the pairing
    /// screen both use it.
    fn promise(self) -> &'static str {
        match self {
            Self::Nostos => "Your files go to your Nostos Writing Studio drafts.",
            Self::SelfHosted => "Your files stay on your own server. Nothing is kept in the cloud.",
            Self::GoogleDrive => "A one-way copy in your Drive. Edits made there do not come back.",
        }
    }
}

/// One line, no newlines: the daemon's `{:#}` error can carry a chain.
fn single_line(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

// ---------------------------------------------------------------------------
// The pairing memory: which services this screen paired.
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct PairingMemory {
    paired: Vec<String>,
}

fn memory_path(root: &Path) -> PathBuf {
    root.join(".carrel").join("sync-ui.json")
}

/// `None` when this device has never written a memory file: an older install
/// whose pairing the shell has not seen yet.
fn load_memory(root: &Path) -> Option<Vec<Backend>> {
    let bytes = std::fs::read(memory_path(root)).ok()?;
    let memory: PairingMemory = serde_json::from_slice(&bytes).ok()?;
    Some(
        memory
            .paired
            .iter()
            .filter_map(|wire| Backend::ALL.iter().copied().find(|it| it.wire() == wire))
            .collect(),
    )
}

fn save_memory(root: &Path, services: &[Backend]) {
    let memory = PairingMemory {
        paired: services.iter().map(|it| it.wire().to_owned()).collect(),
    };
    if let Ok(json) = serde_json::to_vec_pretty(&memory) {
        let _ = crate::store::atomic_write(&memory_path(root), &json);
    }
}

// ---------------------------------------------------------------------------
// The screen.
// ---------------------------------------------------------------------------

pub struct SyncPage {
    pub widget: gtk::Overlay,
    screen: Rc<ui::Screen>,
    stack: gtk::Stack,
    hero: gtk::Label,
    meta: gtk::Label,
    service: gtk::Label,
    detail: gtk::Label,
    list: gtk::ListBox,
    notice: gtk::Label,
    url_entry: gtk::Entry,
    url_error: gtk::Label,
    code: gtk::Label,
    sentence: gtk::Label,
    live: gtk::Label,
    pairing_note: gtk::Label,
    qr: gtk::Picture,
    /// The row indexes that hold an action, in `SyncState::rows` order.
    action_rows: RefCell<Vec<i32>>,
    state: RefCell<SyncState>,
    /// Bumped by every `SyncStatus` signal, so a slower `SyncInfo` snapshot
    /// cannot overwrite a newer state with an older one.
    info_generation: Cell<u64>,
    client: Client,
    nav: Rc<Navigator>,
    handoff: WifiHandoff,
    root: PathBuf,
}

impl SyncPage {
    pub fn new(
        client: Client,
        nav: Rc<Navigator>,
        handoff: WifiHandoff,
        root: PathBuf,
    ) -> Rc<Self> {
        let screen = ui::Screen::new("Sync");
        screen.set_hints(&OVERVIEW_HINTS);

        let hero = gtk::Label::new(None);
        hero.set_xalign(0.0);
        hero.set_hexpand(true);
        hero.set_single_line_mode(true);
        hero.set_ellipsize(pango::EllipsizeMode::End);
        hero.set_margin_bottom(5);
        hero.add_css_class("sync-state");

        let meta = gtk::Label::new(None);
        meta.set_xalign(0.0);
        meta.set_wrap(true);
        meta.add_css_class("row-note");

        let service = gtk::Label::new(None);
        service.set_xalign(0.0);
        service.set_wrap(true);
        service.add_css_class("row-note");

        let detail = gtk::Label::new(None);
        detail.set_xalign(0.0);
        detail.set_single_line_mode(true);
        detail.set_ellipsize(pango::EllipsizeMode::End);
        detail.add_css_class("row-note");

        let list = ui::list();
        list.set_margin_top(16);

        let notice = gtk::Label::new(None);
        notice.set_xalign(0.0);
        notice.set_wrap(true);
        notice.add_css_class("notice-line");

        let overview = gtk::Box::new(gtk::Orientation::Vertical, 3);
        overview.append(&hero);
        overview.append(&meta);
        overview.append(&service);
        overview.append(&detail);
        overview.append(&list);
        overview.append(&notice);

        // The self-hosted server asks for its address first.
        let url_entry = gtk::Entry::new();
        url_entry.set_placeholder_text(Some("https://studio.local:8443"));
        url_entry.set_hexpand(true);
        url_entry.add_css_class("calm-entry");
        let url_error = gtk::Label::new(None);
        url_error.set_xalign(0.0);
        url_error.set_wrap(true);
        url_error.add_css_class("error-text");
        let url_hint = gtk::Label::new(Some(
            "The server prints its address when it starts. A LAN name or a Tailscale name both work.",
        ));
        url_hint.set_xalign(0.0);
        url_hint.set_wrap(true);
        url_hint.add_css_class("row-note");
        let url_box = gtk::Box::new(gtk::Orientation::Vertical, 14);
        url_box.set_margin_top(10);
        url_box.append(&url_entry);
        url_box.append(&url_error);
        url_box.append(&url_hint);

        // Code on the left, QR beside it, live line under the code.
        let code = gtk::Label::new(None);
        code.set_xalign(0.0);
        code.add_css_class("pairing-code");
        let sentence = gtk::Label::new(None);
        sentence.set_xalign(0.0);
        sentence.set_wrap(true);
        sentence.set_max_width_chars(38);
        sentence.add_css_class("pairing-sentence");
        let live = gtk::Label::new(None);
        live.set_xalign(0.0);
        live.add_css_class("setting-label");
        let pairing_note = gtk::Label::new(None);
        pairing_note.set_xalign(0.0);
        pairing_note.set_wrap(true);
        pairing_note.set_max_width_chars(38);
        pairing_note.add_css_class("row-note");

        let qr = gtk::Picture::new();
        qr.set_size_request(216, 216);
        qr.set_can_shrink(true);
        qr.set_content_fit(gtk::ContentFit::Contain);
        let qr_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        qr_frame.add_css_class("qr");
        qr_frame.set_size_request(236, 236);
        qr_frame.set_halign(gtk::Align::End);
        qr_frame.set_valign(gtk::Align::Start);
        qr_frame.append(&qr);

        let pairing_left = gtk::Box::new(gtk::Orientation::Vertical, 12);
        pairing_left.set_hexpand(true);
        pairing_left.set_valign(gtk::Align::Start);
        pairing_left.append(&code);
        pairing_left.append(&sentence);
        pairing_left.append(&live);
        pairing_left.append(&pairing_note);
        let pairing_box = gtk::Box::new(gtk::Orientation::Horizontal, 36);
        pairing_box.set_margin_top(8);
        pairing_box.append(&pairing_left);
        pairing_box.append(&qr_frame);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.set_margin_top(2);
        stack.add_named(&overview, Some("overview"));
        stack.add_named(&url_box, Some("url"));
        stack.add_named(&pairing_box, Some("pairing"));
        stack.set_visible_child_name("overview");
        screen.content.append(&stack);

        let remembered = load_memory(&root);
        let state = SyncState {
            paired_services: remembered.clone().unwrap_or_default(),
            memory_known: remembered.is_some(),
            ..SyncState::default()
        };

        let page = Rc::new(Self {
            widget: screen.root.clone(),
            screen,
            stack,
            hero,
            meta,
            service,
            detail,
            list,
            notice,
            url_entry,
            url_error,
            code,
            sentence,
            live,
            pairing_note,
            qr,
            action_rows: RefCell::new(Vec::new()),
            state: RefCell::new(state),
            info_generation: Cell::new(0),
            client,
            nav,
            handoff,
            root,
        });
        page.install();
        page.refresh();
        page
    }

    fn install(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            let Some(page) = weak.upgrade() else {
                return;
            };
            let index = row.index();
            let position = page
                .action_rows
                .borrow()
                .iter()
                .position(|candidate| *candidate == index);
            if let Some(position) = position {
                page.state.borrow_mut().selected = position;
                page.choose();
            }
        });

        let weak = Rc::downgrade(self);
        self.url_entry.connect_activate(move |_| {
            if let Some(page) = weak.upgrade() {
                page.submit_url();
            }
        });

        let weak = Rc::downgrade(self);
        self.widget.connect_map(move |_| {
            let Some(page) = weak.upgrade() else {
                return;
            };
            page.load_sync_info();
            page.refresh();
            page.screen.focus_page();
            // Wi-Fi connected for a sync that could not reach the network:
            // pick the thread up where it stopped.
            if page.handoff.resume() {
                page.run_effect(SyncEffect::Sync);
            }
        });
    }

    fn load_sync_info(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let generation = self.info_generation.get();
        self.client.spawn(
            |daemon| daemon.sync_info(),
            move |result| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                // A `SyncStatus` signal arrived while this snapshot was being
                // read: the signal is newer, so the snapshot is dropped.
                if page.info_generation.get() != generation {
                    return;
                }
                match result {
                    Ok(info) => page.state.borrow_mut().on_sync(info),
                    Err(error) => page.state.borrow_mut().error = Some(error.message),
                }
                page.refresh();
            },
        );
    }

    /// The one-line state, for the tests.
    pub fn hero(&self) -> String {
        self.hero.text().to_string()
    }

    /// The unpair card is open; the command palette waits for it.
    pub(crate) fn is_dialog_open(&self) -> bool {
        self.screen.is_dialog_open()
    }

    /// The rows the keyboard can land on, for the tests.
    pub fn actions(&self) -> Vec<Row> {
        self.state.borrow().rows()
    }

    pub fn view(&self) -> SyncView {
        self.state.borrow().view
    }

    pub fn pairing_code(&self) -> String {
        self.code.text().to_string()
    }

    /// Open the Sync screen and begin pairing. Used by First run and by the
    /// screenshot flow; self-hosted servers need a URL first.
    pub fn open_pairing(self: &Rc<Self>, backend: Backend) {
        self.nav.show("sync");
        {
            let mut state = self.state.borrow_mut();
            state.notice.clear();
            state.error = None;
            match backend {
                Backend::SelfHosted => state.view = SyncView::SelfHostedUrl,
                _ => state.begin_pairing(backend),
            }
        }
        self.refresh();
        match backend {
            Backend::SelfHosted => {
                self.url_entry.set_text("");
                self.url_entry.grab_focus();
            }
            _ => self.start_pair(backend, String::new()),
        }
    }

    /// Pair with an explicit URL. The self-hosted flow normally asks for the
    /// address first; the screenshot script uses this to show the code step.
    pub fn open_pairing_url(self: &Rc<Self>, backend: Backend, url: &str) {
        self.nav.show("sync");
        self.state.borrow_mut().begin_pairing(backend);
        self.refresh();
        self.start_pair(backend, url.to_owned());
    }

    /// `Ctrl+Shift+S`: show the screen and start a sync now. When nothing is
    /// paired the screen itself is the next step, so the daemon's "not paired"
    /// answer lands on the three services rather than a dead end.
    pub fn sync_now(self: &Rc<Self>) {
        self.nav.show("sync");
        {
            let mut state = self.state.borrow_mut();
            state.view = SyncView::Overview;
            state.notice.clear();
            state.error = None;
            state.selected = 0;
        }
        self.refresh();
        self.run_effect(SyncEffect::Sync);
    }

    pub fn on_sync(self: &Rc<Self>, info: SyncInfo) {
        self.info_generation.set(self.info_generation.get() + 1);
        self.state.borrow_mut().on_sync(info);
        self.refresh();
    }

    pub fn on_pair_status(self: &Rc<Self>, state: &str, code: &str, message: String) {
        let (was_pairing, newly_paired) = {
            let mut current = self.state.borrow_mut();
            let was_pairing = current.view == SyncView::Pairing;
            let was_paired = current.paired;
            current.on_pair_status(state, code, message);
            (was_pairing, was_pairing && current.paired && !was_paired)
        };
        if !was_pairing {
            // A pairing that finished after the writer left the code screen:
            // keep the overview and just say so.
            let mut current = self.state.borrow_mut();
            current.view = SyncView::Overview;
            current.paired = false;
        }
        self.remember();
        self.refresh();
        if newly_paired {
            self.schedule_return();
        }
    }

    /// "Paired." is read, then the screen goes back to Sync by itself and syncs.
    fn schedule_return(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(PAIRED_BEAT, move || {
            let Some(page) = weak.upgrade() else {
                return;
            };
            {
                let mut state = page.state.borrow_mut();
                if state.view != SyncView::Pairing {
                    return;
                }
                state.view = SyncView::Overview;
                state.paired = false;
                state.selected = 0;
            }
            page.refresh();
            page.run_effect(SyncEffect::Sync);
        });
    }

    /// Persist which services are paired, so the screen is right after a reboot.
    fn remember(self: &Rc<Self>) {
        let services = self.state.borrow().paired_services.clone();
        save_memory(&self.root, &services);
        self.state.borrow_mut().memory_known = true;
    }

    fn choose(self: &Rc<Self>) {
        let effect = self.state.borrow_mut().choose();
        self.refresh();
        if let Some(effect) = effect {
            self.run_effect(effect);
        } else {
            // The URL view needs keyboard focus.
            self.url_entry.grab_focus();
        }
    }

    fn submit_url(self: &Rc<Self>) {
        let url = self.url_entry.text().to_string();
        let effect = self.state.borrow_mut().submit_server_url(&url);
        match effect {
            Ok(effect) => {
                self.url_error.set_text("");
                self.refresh();
                self.run_effect(effect);
            }
            Err(message) => {
                self.url_error.set_text(&message);
            }
        }
    }

    fn run_effect(self: &Rc<Self>, effect: SyncEffect) {
        match effect {
            SyncEffect::Sync => {
                let weak = Rc::downgrade(self);
                self.client.spawn(
                    |daemon| daemon.sync(),
                    move |result| {
                        if let Some(page) = weak.upgrade() {
                            if let Err(error) = result {
                                page.state.borrow_mut().error = Some(error.message);
                                page.refresh();
                            }
                        }
                    },
                );
            }
            SyncEffect::ChooseNetwork => {
                self.handoff.ask();
                self.nav.show("wifi");
            }
            SyncEffect::Pair { backend, url } => self.start_pair(backend, url),
            SyncEffect::Unpair => self.confirm_unpair(),
        }
    }

    fn start_pair(self: &Rc<Self>, backend: Backend, url: String) {
        self.refresh();
        let weak = Rc::downgrade(self);
        self.client.spawn(
            move |daemon| daemon.pair(backend.wire(), &url),
            move |result| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                match result {
                    Ok(info) => {
                        page.state.borrow_mut().on_pair_info(info);
                        page.refresh();
                    }
                    Err(error) => {
                        let mut state = page.state.borrow_mut();
                        state.view = SyncView::Overview;
                        state.waiting = false;
                        state.paired = false;
                        state.error = Some(error.message);
                        drop(state);
                        page.refresh();
                    }
                }
            },
        );
    }

    fn confirm_unpair(self: &Rc<Self>) {
        let title = match self.state.borrow().paired_services.as_slice() {
            [] => "Unpair?".to_owned(),
            [one] => format!("Unpair {}?", one.connected_label()),
            _ => "Unpair every service?".to_owned(),
        };
        let weak = Rc::downgrade(self);
        self.screen.ask(
            &title,
            "Carrel forgets the service on this device. Your files stay here and in your account.",
            "Unpair",
            move || {
                if let Some(page) = weak.upgrade() {
                    page.unpair();
                }
            },
        );
    }

    fn unpair(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.client.spawn(
            |daemon| daemon.unpair(),
            move |result| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                match result {
                    Ok(()) => {
                        {
                            let mut state = page.state.borrow_mut();
                            state.paired_services.clear();
                            state.paired = false;
                            state.selected = 0;
                            state.notice =
                                "Unpaired. The files on this device are untouched.".to_owned();
                            state.error = None;
                        }
                        page.remember();
                    }
                    Err(error) => page.state.borrow_mut().error = Some(error.message),
                }
                page.refresh();
            },
        );
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
        let view = self.state.borrow().view;
        match view {
            // The URL entry and the code screen only take Esc; every other key
            // belongs to the entry or does nothing.
            SyncView::SelfHostedUrl => {
                if keyval == gdk::Key::Escape {
                    self.state.borrow_mut().back();
                    self.refresh();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
            SyncView::Pairing => {
                if keyval == gdk::Key::Escape {
                    self.state.borrow_mut().back();
                    self.refresh();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
            SyncView::Overview => match keyval {
                // Esc leaves the screen; the pairing step is left above.
                gdk::Key::Escape => {
                    self.nav.show_editor();
                    glib::Propagation::Stop
                }
                gdk::Key::Up => {
                    self.state.borrow_mut().move_by(-1);
                    self.refresh();
                    glib::Propagation::Stop
                }
                gdk::Key::Down => {
                    self.state.borrow_mut().move_by(1);
                    self.refresh();
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    self.choose();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            },
        }
    }

    fn refresh(self: &Rc<Self>) {
        self.forget_stale_pairing();
        {
            // The rows change with the state (an error adds an action, unpair
            // removes some), so the cursor always lands on a real row.
            let mut state = self.state.borrow_mut();
            let len = state.rows().len();
            if len > 0 && state.selected >= len {
                state.selected = len - 1;
            }
        }
        let state = self.state.borrow();
        let view = state.view;
        self.screen.set_title(match view {
            SyncView::Overview => "Sync",
            SyncView::SelfHostedUrl => "Pair with your server",
            SyncView::Pairing => match state.backend {
                Some(Backend::Nostos) => "Pair with Nostos",
                Some(Backend::SelfHosted) => "Pair with your server",
                Some(Backend::GoogleDrive) => "Pair with Google Drive",
                None => "Pair",
            },
        });
        match view {
            SyncView::Overview => {
                self.stack.set_visible_child_name("overview");
                self.screen.set_status("");
                self.screen.set_hints(&OVERVIEW_HINTS);
                self.render_overview(&state);
            }
            SyncView::SelfHostedUrl => {
                self.stack.set_visible_child_name("url");
                self.screen
                    .set_status("Type the address your Carrel server printed.");
                self.screen.set_hints(&URL_HINTS);
            }
            SyncView::Pairing => {
                self.stack.set_visible_child_name("pairing");
                self.screen.set_status("");
                self.screen.set_hints(&PAIRING_HINTS);
                self.render_pairing(&state);
            }
        }
    }

    /// The daemon is the truth about pairing: when it says nothing is paired,
    /// forget what this screen remembered.
    fn forget_stale_pairing(self: &Rc<Self>) {
        let stale = {
            let state = self.state.borrow();
            state.remedy() == Some(Remedy::Pair) && !state.paired_services.is_empty()
        };
        if stale {
            self.state.borrow_mut().paired_services.clear();
            save_memory(&self.root, &[]);
            self.state.borrow_mut().memory_known = true;
        }
    }

    fn render_overview(self: &Rc<Self>, state: &SyncState) {
        self.hero.set_text(&state.hero());
        if matches!(state.remedy(), Some(Remedy::Wifi) | Some(Remedy::Retry)) {
            self.hero.add_css_class("error-text");
        } else {
            self.hero.remove_css_class("error-text");
        }
        set_line(&self.meta, &state.last_sync_line());
        set_line(&self.service, &state.connected_line());
        set_line(&self.detail, &state.detail());

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let rows = state.rows();
        let connections = state.connections_at();
        let mut action_rows = Vec::with_capacity(rows.len());
        let mut actions = Vec::with_capacity(rows.len());
        let mut selected_row = None;
        for (index, row) in rows.iter().enumerate() {
            if Some(index) == connections {
                self.list.append(&group_row("Connections"));
            }
            let widget = match row {
                // "Sync now" is the one prominent action only when nothing
                // else needs the writer first.
                Row::SyncNow => action_row("Sync now", "", state.remedy().is_none()),
                Row::ChooseNetwork => remedy_row(&state.row_label(Row::ChooseNetwork), REMEDY_NOTE),
                _ => action_row(&state.row_label(*row), state.row_note(*row), false),
            };
            self.list.append(&widget);
            action_rows.push(self.list.observe_children().n_items() as i32 - 1);
            actions.push(*row);
            if actions.len() - 1 == state.selected {
                selected_row = Some(widget);
            }
        }
        *self.action_rows.borrow_mut() = action_rows;
        if let Some(row) = selected_row {
            self.list.select_row(Some(&row));
        }

        if let Some(error) = &state.error {
            self.notice.add_css_class("error-text");
            self.notice.remove_css_class("notice-ok");
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

    fn render_pairing(&self, state: &SyncState) {
        let code = if state.code.is_empty() {
            "· · ·".to_owned()
        } else {
            state.code.clone()
        };
        self.code.set_text(&code);
        self.sentence.set_text(&state.pairing_sentence());
        self.live.set_text(state.pairing_live());
        if state.paired {
            self.live.add_css_class("sync-now");
        } else {
            self.live.remove_css_class("sync-now");
        }
        self.pairing_note.set_text(state.pairing_note());
        let data = state.qr_data();
        if data.is_empty() {
            self.qr.set_paintable(None::<&gtk::gdk::Paintable>);
        } else {
            self.qr.set_paintable(Some(&qr_texture(&data)));
        }
    }
}

fn set_line(label: &gtk::Label, text: &str) {
    if label.text() != text {
        label.set_text(text);
    }
    label.set_visible(!text.is_empty());
}

/// A row the keyboard lands on: the label at the row size, an optional note
/// under it. `strong` marks the one prominent action ("Sync now").
fn action_row(title: &str, note: &str, strong: bool) -> gtk::ListBoxRow {
    if !note.is_empty() {
        return ui::value_row(title, "", note);
    }
    let row = ui::plain_row(title);
    if strong {
        if let Some(label) = row
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        {
            label.add_css_class("sync-now");
        }
    }
    row
}

/// The error's one action, with the key that takes it written in the row:
/// "No Wi-Fi. [Enter] choose a network".
fn remedy_row(label: &str, note: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("ui-row");
    row.add_css_class("setting-row");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 5);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line.set_valign(gtk::Align::Center);

    let (lead, verb) = match label.split_once("Choose a network") {
        Some((lead, _)) => (lead.trim_end().to_owned(), "Choose a network".to_owned()),
        None => (label.to_owned(), String::new()),
    };
    let lead = gtk::Label::new(Some(&lead));
    lead.set_xalign(0.0);
    lead.set_valign(gtk::Align::Center);
    lead.add_css_class("setting-label");
    line.append(&lead);
    let cap = gtk::Label::new(Some("Enter"));
    cap.add_css_class("key-cap");
    cap.set_valign(gtk::Align::Center);
    line.append(&cap);
    if !verb.is_empty() {
        let verb = gtk::Label::new(Some(&verb));
        verb.set_xalign(0.0);
        verb.set_valign(gtk::Align::Center);
        verb.add_css_class("setting-label");
        line.append(&verb);
    }
    column.append(&line);
    let note = gtk::Label::new(Some(note));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.add_css_class("row-note");
    column.append(&note);
    row.set_child(Some(&column));
    row
}

/// The quiet group label above the pairing rows: not a choice, never selected.
fn group_row(text: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    row.set_can_focus(false);
    // The same padding as an action row, so the label starts where the rows do.
    row.add_css_class("ui-row");
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_margin_top(4);
    label.add_css_class("section-title");
    row.set_child(Some(&label));
    row
}

/// A QR as a GDK texture: dark modules on warm white, with the four-module
/// quiet zone scanners need. The texture always has the same 216-pixel canvas,
/// so every pairing card is the same size; modules are drawn at an integer
/// scale, centred, and never resampled.
fn qr_texture(data: &str) -> gtk::gdk::Texture {
    const CANVAS: i32 = 216;
    let dark = [0x26u8, 0x22, 0x1c, 0xff];
    let light = [0xfb, 0xf7, 0xef, 0xff];
    let quiet = 4i32;

    let Ok(code) = qrcode::QrCode::new(data.as_bytes()) else {
        let pixels = vec![light; (CANVAS * CANVAS) as usize];
        return texture_from(pixels, CANVAS);
    };
    let modules = code.width() as i32;
    let total = modules + quiet * 2;
    let scale = (CANVAS / total).max(1);
    let offset = (CANVAS - total * scale) / 2;

    let mut pixels = vec![light; (CANVAS * CANVAS) as usize];
    for (index, color) in code.to_colors().iter().enumerate() {
        if *color != qrcode::Color::Dark {
            continue;
        }
        let x = offset + (index as i32 % modules + quiet) * scale;
        let y = offset + (index as i32 / modules + quiet) * scale;
        for dy in 0..scale {
            for dx in 0..scale {
                let pixel = ((y + dy) * CANVAS + (x + dx)) as usize;
                pixels[pixel] = dark;
            }
        }
    }
    texture_from(pixels, CANVAS)
}

fn texture_from(pixels: Vec<[u8; 4]>, side: i32) -> gtk::gdk::Texture {
    let bytes: Vec<u8> = pixels.into_iter().flatten().collect();
    gtk::gdk::MemoryTexture::new(
        side,
        side,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from(&bytes),
        side as usize * 4,
    )
    .upcast()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{DaemonClient, FakeDaemon};

    fn synced(services: &[Backend]) -> SyncState {
        SyncState {
            info: SyncInfo {
                state: "idle".into(),
                message: "Already up to date.".into(),
                last_sync: 1_760_000_000,
            },
            paired_services: services.to_vec(),
            memory_known: true,
            ..SyncState::default()
        }
    }

    #[test]
    fn a_paired_device_shows_up_to_date_and_sync_now_only_among_the_mains() {
        let state = synced(&[Backend::Nostos]);
        assert_eq!(state.hero(), "Up to date");
        assert!(state.connected_line().contains("Nostos"));
        assert_eq!(
            state.rows(),
            vec![
                Row::SyncNow,
                Row::Pair(Backend::SelfHosted),
                Row::Pair(Backend::GoogleDrive),
                Row::Unpair,
            ]
        );
        assert_eq!(state.connections_at(), Some(1));
        assert_eq!(state.row_label(Row::Unpair), "Unpair Nostos");
        assert_eq!(
            state.row_label(Row::Pair(Backend::GoogleDrive)),
            "Pair with Google Drive"
        );
    }

    #[test]
    fn an_unpaired_device_offers_the_three_services_with_one_line_each() {
        let state = SyncState::default();
        assert_eq!(state.hero(), "Not connected yet");
        assert_eq!(
            state.rows(),
            vec![
                Row::Pair(Backend::Nostos),
                Row::Pair(Backend::SelfHosted),
                Row::Pair(Backend::GoogleDrive),
            ]
        );
        assert_eq!(state.row_label(Row::Pair(Backend::Nostos)), "Nostos");
        assert!(state
            .row_note(Row::Pair(Backend::Nostos))
            .contains("Nostos Writing Studio"));
        assert!(state
            .row_note(Row::Pair(Backend::GoogleDrive))
            .contains("do not come back"));
        assert_eq!(state.connections_at(), None);
        assert_eq!(
            state.last_sync_line(),
            "Never synced. Everything is on this device."
        );
    }

    #[test]
    fn the_three_remedies_come_from_the_daemons_sentence() {
        assert_eq!(
            remedy_for("No Wi-Fi. Open Wi-Fi to choose a network."),
            Remedy::Wifi
        );
        assert_eq!(remedy_for("error sending request: dns error"), Remedy::Wifi);
        assert_eq!(
            remedy_for("this device is not paired with a sync server or Google Drive"),
            Remedy::Pair
        );
        assert_eq!(remedy_for("Cannot reach the studio server."), Remedy::Retry);
    }

    #[test]
    fn a_no_wifi_error_puts_the_network_first_and_says_what_to_do() {
        let state = SyncState {
            info: SyncInfo {
                state: "error".into(),
                message: "No Wi-Fi network. Open Wi-Fi to choose one.".into(),
                last_sync: 1_760_000_000,
            },
            ..synced(&[Backend::Nostos])
        };
        assert_eq!(state.hero(), "Could not reach Nostos");
        assert_eq!(state.remedy(), Some(Remedy::Wifi));
        assert_eq!(state.rows()[0], Row::ChooseNetwork);
        assert_eq!(state.rows()[1], Row::SyncNow);
        assert!(state
            .row_label(Row::ChooseNetwork)
            .contains("Choose a network"));
        assert_eq!(state.detail(), "");
    }

    #[test]
    fn a_server_error_keeps_the_daemons_sentence_and_offers_sync_now() {
        let state = SyncState {
            info: SyncInfo {
                state: "error".into(),
                message: "Cannot reach the studio server.\nNothing was lost; try again.".into(),
                last_sync: 1_760_000_000,
            },
            ..synced(&[Backend::Nostos])
        };
        assert_eq!(state.hero(), "Could not reach Nostos");
        assert_eq!(state.rows()[0], Row::SyncNow);
        assert_eq!(
            state.detail(),
            "Cannot reach the studio server. Nothing was lost; try again."
        );
    }

    #[test]
    fn an_unpaired_error_falls_back_to_the_services() {
        let mut state = synced(&[Backend::Nostos]);
        state.info = SyncInfo {
            state: "error".into(),
            message: "this device is not paired with a sync server or Google Drive".into(),
            last_sync: 0,
        };
        assert_eq!(state.remedy(), Some(Remedy::Pair));
        assert_eq!(state.hero(), "Not connected yet");
    }

    #[test]
    fn syncing_shows_the_daemons_progress_line() {
        let mut state = synced(&[Backend::Nostos]);
        state.info = SyncInfo {
            state: "syncing".into(),
            message: "Syncing… 3 of 12 files".into(),
            last_sync: 1_760_000_000,
        };
        assert_eq!(state.hero(), "Syncing… 3 of 12 files");
        assert_eq!(state.remedy(), None);
    }

    #[test]
    fn a_paired_device_that_never_synced_says_so() {
        let state = SyncState {
            info: SyncInfo::default(),
            paired_services: vec![Backend::Nostos],
            memory_known: true,
            ..SyncState::default()
        };
        assert_eq!(state.hero(), "Ready for your first sync");
        assert_eq!(
            state.last_sync_line(),
            "Not synced yet. Everything is on this device."
        );
    }

    #[test]
    fn an_old_install_with_no_memory_is_paired_once_it_has_synced() {
        let state = SyncState {
            info: SyncInfo {
                state: "idle".into(),
                message: "Already up to date.".into(),
                last_sync: 1_760_000_000,
            },
            ..SyncState::default()
        };
        assert!(state.is_paired());
        assert_eq!(state.hero(), "Up to date");
        assert_eq!(state.connected_line(), "");
        assert_eq!(state.rows(), vec![Row::SyncNow, Row::Unpair]);
    }

    #[test]
    fn pairing_a_self_hosted_server_wants_a_url_first() {
        let mut state = SyncState {
            selected: 1,
            ..SyncState::default()
        };
        assert_eq!(state.choose(), None);
        assert_eq!(state.view, SyncView::SelfHostedUrl);
        assert!(state.submit_server_url("  ").is_err());
        assert!(state.submit_server_url("studio.local").is_err());
        let effect = state
            .submit_server_url(" https://studio.local:8443 ")
            .unwrap();
        assert_eq!(
            effect,
            SyncEffect::Pair {
                backend: Backend::SelfHosted,
                url: "https://studio.local:8443".into()
            }
        );
        assert_eq!(state.view, SyncView::Pairing);
        assert!(state.waiting);
    }

    #[test]
    fn the_first_row_starts_pairing_with_nostos() {
        let fake = FakeDaemon::new();
        let mut state = SyncState::default();
        let effect = state.choose().expect("the first service is a choice");
        assert_eq!(
            effect,
            SyncEffect::Pair {
                backend: Backend::Nostos,
                url: String::new()
            }
        );
        assert_eq!(state.view, SyncView::Pairing);
        assert_eq!(state.pairing_live(), "Waiting for approval…");
        state.on_pair_info(fake.pair("nostos", "").unwrap());
        assert_eq!(state.code, "K7Q-4MZ");
        assert!(state.qr_data().contains("app.nostos.page/pair"));
        assert!(state.qr_data().contains("K7Q-4MZ"));
        assert_eq!(
            state.pairing_sentence(),
            "On your phone, scan the code or open app.nostos.page/pair and enter the code."
        );
    }

    #[test]
    fn approval_says_paired_and_remembers_the_service() {
        let mut state = SyncState::default();
        state.begin_pairing(Backend::Nostos);
        state.on_pair_info(PairingInfo {
            backend: "nostos".into(),
            code: "K7Q-4MZ".into(),
            approval_url: "https://app.nostos.page/pair".into(),
            expires_at: 0,
        });
        state.on_pair_status("approved", "K7Q-4MZ", "Paired with Studio.".into());
        assert!(state.paired);
        assert_eq!(state.pairing_live(), "Paired.");
        assert_eq!(state.paired_services, vec![Backend::Nostos]);
        assert_eq!(state.notice, "Paired with Studio.");
    }

    #[test]
    fn google_drive_says_it_is_one_way() {
        let fake = FakeDaemon::new();
        let info = fake.pair("google-drive", "").unwrap();
        let mut state = SyncState::default();
        state.on_pair_info(info);
        assert_eq!(state.backend, Some(Backend::GoogleDrive));
        assert!(state.pairing_note().contains("do not come back"));
        assert_eq!(state.qr_data(), "https://www.google.com/device");
        assert_eq!(
            state.pairing_sentence(),
            "On your phone, open google.com/device and enter the code."
        );
    }

    #[test]
    fn a_failed_pairing_returns_to_the_overview_with_one_line() {
        let mut state = SyncState::default();
        state.begin_pairing(Backend::Nostos);
        state.on_pair_status("error", "", "the code expired".into());
        assert_eq!(state.view, SyncView::Overview);
        assert_eq!(state.error.as_deref(), Some("the code expired"));
        assert!(!state.paired);
    }

    #[test]
    fn sync_info_drives_the_progress_and_last_sync_lines() {
        let mut state = SyncState::default();
        state.on_sync(SyncInfo {
            state: "syncing".into(),
            message: "Syncing…".into(),
            last_sync: 1_760_000_000,
        });
        assert_eq!(state.info.state, "syncing");
        assert!(state.info.last_sync > 0);
    }

    #[test]
    fn back_leaves_the_pairing_flow_but_not_the_overview() {
        let mut state = SyncState::default();
        assert!(!state.back());
        state.view = SyncView::Pairing;
        assert!(state.back());
        assert_eq!(state.view, SyncView::Overview);
    }

    #[test]
    fn the_pairing_memory_survives_a_reboot() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_memory(dir.path()), None);
        save_memory(dir.path(), &[Backend::Nostos, Backend::GoogleDrive]);
        assert_eq!(
            load_memory(dir.path()),
            Some(vec![Backend::Nostos, Backend::GoogleDrive])
        );
        save_memory(dir.path(), &[]);
        assert_eq!(load_memory(dir.path()), Some(Vec::new()));
    }
}
