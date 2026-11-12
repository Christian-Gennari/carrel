//! The Sync screen: one progress line, the last sync time, the result, and
//! the pairing flows for the three backends.
//!
//! The daemon owns the network and the radio; this screen only starts calls
//! and draws the `SyncStatus` / `PairStatus` signals it receives.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::daemon::{Backend, Client, PairingInfo, SyncInfo};
use crate::nav::Navigator;
use crate::ui;

const OVERVIEW_HINT: &str = "Up/Down move · Enter choose · Esc editor";
const URL_HINT: &str = "Enter start pairing · Esc back";
const PAIRING_HINT: &str = "Waiting for approval · Backspace back · Esc editor";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncAction {
    SyncNow,
    Pair(Backend),
    Unpair,
}

pub const OVERVIEW_ROWS: [SyncAction; 5] = [
    SyncAction::SyncNow,
    SyncAction::Pair(Backend::Nostos),
    SyncAction::Pair(Backend::SelfHosted),
    SyncAction::Pair(Backend::GoogleDrive),
    SyncAction::Unpair,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncEffect {
    Sync,
    Pair { backend: Backend, url: String },
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
    pub selected: usize,
    pub backend: Option<Backend>,
    pub code: String,
    pub approval_url: String,
    pub waiting: bool,
    pub notice: String,
    pub error: Option<String>,
}

impl SyncState {
    pub fn action(&self) -> SyncAction {
        OVERVIEW_ROWS[self.selected.min(OVERVIEW_ROWS.len() - 1)]
    }

    pub fn move_by(&mut self, delta: i32) {
        let len = OVERVIEW_ROWS.len() as i32;
        self.selected = ((self.selected as i32 + delta).rem_euclid(len)) as usize;
    }

    /// Enter on the overview. Pairing a self-hosted server needs the URL
    /// first, so it only switches the view.
    pub fn choose(&mut self) -> Option<SyncEffect> {
        match self.action() {
            SyncAction::SyncNow => Some(SyncEffect::Sync),
            SyncAction::Pair(Backend::SelfHosted) => {
                self.view = SyncView::SelfHostedUrl;
                self.error = None;
                None
            }
            SyncAction::Pair(backend) => {
                self.begin_pairing(backend);
                Some(SyncEffect::Pair {
                    backend,
                    url: String::new(),
                })
            }
            SyncAction::Unpair => Some(SyncEffect::Unpair),
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
        self.error = None;
        self.notice.clear();
    }

    pub fn on_pair_info(&mut self, info: PairingInfo) {
        self.view = SyncView::Pairing;
        self.backend = Backend::ALL
            .iter()
            .copied()
            .find(|backend| backend.wire() == info.backend);
        self.code = info.code;
        self.approval_url = info.approval_url;
        self.waiting = true;
        self.error = None;
    }

    pub fn on_pair_status(&mut self, state: &str, _code: &str, message: String) {
        if state == "approved" {
            self.view = SyncView::Overview;
            self.waiting = false;
            self.notice = message;
            self.error = None;
        } else {
            self.view = SyncView::Overview;
            self.waiting = false;
            self.error = Some(message);
        }
    }

    pub fn on_sync(&mut self, info: SyncInfo) {
        self.info = info;
    }

    /// Backspace in the pairing flow goes back one step, not to the editor.
    pub fn back(&mut self) -> bool {
        match self.view {
            SyncView::Overview => false,
            SyncView::SelfHostedUrl | SyncView::Pairing => {
                self.view = SyncView::Overview;
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

    pub fn pairing_note(&self) -> &'static str {
        match self.backend {
            Some(Backend::GoogleDrive) => {
                "One-way mirror: edits made in the Drive folder do not come back."
            }
            Some(Backend::SelfHosted) => {
                "Your files stay on your own server. Nothing is kept in the cloud."
            }
            _ => "Your files go to your Nostos Studio drafts.",
        }
    }
}

pub struct SyncPage {
    pub widget: gtk::Box,
    status: gtk::Label,
    last_sync: gtk::Label,
    stack: gtk::Stack,
    list: gtk::ListBox,
    url_entry: gtk::Entry,
    url_error: gtk::Label,
    code: gtk::Label,
    approval: gtk::Label,
    pairing_note: gtk::Label,
    pairing_status: gtk::Label,
    qr: gtk::Picture,
    notice: gtk::Label,
    hint: gtk::Label,
    state: RefCell<SyncState>,
    client: Client,
    nav: Rc<Navigator>,
}

impl SyncPage {
    pub fn new(client: Client, nav: Rc<Navigator>) -> Rc<Self> {
        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.add_css_class("sync-progress");
        let last_sync = gtk::Label::new(Some("Last sync: never"));
        last_sync.set_xalign(0.0);
        last_sync.add_css_class("row-note");

        let list = ui::list();
        for action in OVERVIEW_ROWS {
            list.append(&ui::plain_row(action_label(action)));
        }
        let overview = ui::scroller(&list);

        let url_title = gtk::Label::new(Some("Address of your Carrel server"));
        url_title.set_xalign(0.0);
        url_title.add_css_class("section-title");
        let url_entry = gtk::Entry::new();
        url_entry.set_placeholder_text(Some("https://studio.local:8443"));
        url_entry.add_css_class("calm-entry");
        let url_error = gtk::Label::new(None);
        url_error.set_xalign(0.0);
        url_error.add_css_class("error-text");
        let url_hint = ui::hint(
            "The server prints its address when it starts. A LAN name or a Tailscale name both work.",
        );
        let url_box = gtk::Box::new(gtk::Orientation::Vertical, 14);
        url_box.set_margin_top(20);
        url_box.append(&url_title);
        url_box.append(&url_entry);
        url_box.append(&url_error);
        url_box.append(&url_hint);

        let code = gtk::Label::new(None);
        code.set_xalign(0.0);
        code.add_css_class("pairing-code");
        let approval = gtk::Label::new(None);
        approval.set_xalign(0.0);
        approval.set_wrap(true);
        approval.add_css_class("pairing-url");
        let qr = gtk::Picture::new();
        qr.set_size_request(216, 216);
        qr.set_can_shrink(true);
        qr.set_content_fit(gtk::ContentFit::Contain);
        let qr_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        qr_frame.add_css_class("qr");
        qr_frame.set_size_request(236, 236);
        qr_frame.set_halign(gtk::Align::Start);
        qr_frame.set_valign(gtk::Align::Start);
        qr_frame.append(&qr);
        let pairing_note = gtk::Label::new(None);
        pairing_note.set_xalign(0.0);
        pairing_note.set_wrap(true);
        pairing_note.add_css_class("row-note");
        let pairing_status = gtk::Label::new(Some("Waiting for approval…"));
        pairing_status.set_xalign(0.0);
        pairing_status.add_css_class("status-line");
        let pairing_left = gtk::Box::new(gtk::Orientation::Vertical, 12);
        pairing_left.set_hexpand(true);
        pairing_left.append(&code);
        pairing_left.append(&approval);
        pairing_left.append(&pairing_note);
        pairing_left.append(&pairing_status);
        let pairing_box = gtk::Box::new(gtk::Orientation::Horizontal, 36);
        pairing_box.set_margin_top(8);
        pairing_box.append(&pairing_left);
        pairing_box.append(&qr_frame);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.set_margin_top(10);
        stack.add_named(&overview, Some("overview"));
        stack.add_named(&url_box, Some("url"));
        stack.add_named(&pairing_box, Some("pairing"));
        stack.set_visible_child_name("overview");

        let notice = gtk::Label::new(None);
        notice.set_xalign(0.0);
        notice.set_wrap(true);
        notice.add_css_class("notice-line");
        notice.set_visible(false);

        let hint = ui::hint(OVERVIEW_HINT);

        let column = ui::column(820);
        column.append(&ui::title("Sync"));
        column.append(&status);
        column.append(&last_sync);
        column.append(&stack);
        column.append(&notice);
        column.append(&hint);

        let widget = ui::page();
        widget.append(&column);

        let page = Rc::new(Self {
            widget,
            status,
            last_sync,
            stack,
            list,
            url_entry,
            url_error,
            code,
            approval,
            pairing_note,
            pairing_status,
            qr,
            notice,
            hint,
            state: RefCell::new(SyncState::default()),
            client,
            nav,
        });
        page.install();
        page
    }

    fn install(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(page) = weak.upgrade() {
                page.state.borrow_mut().selected = row.index().max(0) as usize;
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
            if let Some(page) = weak.upgrade() {
                page.load_sync_info();
                page.refresh();
                page.widget.grab_focus();
            }
        });
    }

    fn load_sync_info(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.client.spawn(
            |daemon| daemon.sync_info(),
            move |result| {
                if let Some(page) = weak.upgrade() {
                    match result {
                        Ok(info) => page.state.borrow_mut().on_sync(info),
                        Err(error) => page.state.borrow_mut().error = Some(error.message),
                    }
                    page.refresh();
                }
            },
        );
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

    /// `Ctrl+Shift+S`: show the screen and start a sync now.
    pub fn sync_now(self: &Rc<Self>) {
        self.nav.show("sync");
        {
            let mut state = self.state.borrow_mut();
            state.view = SyncView::Overview;
            state.notice.clear();
            state.error = None;
        }
        self.refresh();
        self.run_effect(SyncEffect::Sync);
    }

    pub fn on_sync(self: &Rc<Self>, info: SyncInfo) {
        self.state.borrow_mut().on_sync(info);
        self.refresh();
    }

    pub fn on_pair_status(self: &Rc<Self>, state: &str, code: &str, message: String) {
        self.state.borrow_mut().on_pair_status(state, code, message);
        self.refresh();
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
            SyncEffect::Pair { backend, url } => self.start_pair(backend, url),
            SyncEffect::Unpair => self.unpair(),
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
                        state.error = Some(error.message);
                        drop(state);
                        page.refresh();
                    }
                }
            },
        );
    }

    fn unpair(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.client.spawn(
            |daemon| daemon.unpair(),
            move |result| {
                if let Some(page) = weak.upgrade() {
                    match result {
                        Ok(()) => {
                            page.state.borrow_mut().notice =
                                "Unpaired. The files on this device are untouched.".to_owned();
                            page.state.borrow_mut().error = None;
                        }
                        Err(error) => page.state.borrow_mut().error = Some(error.message),
                    }
                    page.refresh();
                }
            },
        );
    }

    pub(crate) fn handle_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        if state.contains(gdk::ModifierType::CONTROL_MASK) {
            return glib::Propagation::Proceed;
        }
        let view = self.state.borrow().view;

        if view == SyncView::SelfHostedUrl {
            match keyval {
                gdk::Key::Escape => {
                    self.state.borrow_mut().view = SyncView::Overview;
                    self.refresh();
                    return glib::Propagation::Stop;
                }
                _ => return glib::Propagation::Proceed,
            }
        }

        match keyval {
            gdk::Key::Escape => {
                self.nav.show_editor();
                glib::Propagation::Stop
            }
            gdk::Key::BackSpace => {
                if self.state.borrow_mut().back() {
                    self.refresh();
                }
                glib::Propagation::Stop
            }
            gdk::Key::Up if view == SyncView::Overview => {
                self.state.borrow_mut().move_by(-1);
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Down if view == SyncView::Overview => {
                self.state.borrow_mut().move_by(1);
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter if view == SyncView::Overview => {
                self.choose();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn refresh(self: &Rc<Self>) {
        let state = self.state.borrow();
        let (view, message, error_state) = (
            state.view,
            state.info.message.clone(),
            state.info.state == "error",
        );
        self.status.set_text(if message.is_empty() {
            "Not synced yet."
        } else {
            &message
        });
        if error_state {
            self.status.add_css_class("error-text");
        } else {
            self.status.remove_css_class("error-text");
        }
        self.last_sync.set_text(&format!(
            "Last sync: {}",
            ui::format_moment(state.info.last_sync)
        ));

        match view {
            SyncView::Overview => {
                self.stack.set_visible_child_name("overview");
                ui::select(&self.list, state.selected);
                self.hint.set_text(OVERVIEW_HINT);
            }
            SyncView::SelfHostedUrl => {
                self.stack.set_visible_child_name("url");
                self.hint.set_text(URL_HINT);
            }
            SyncView::Pairing => {
                self.stack.set_visible_child_name("pairing");
                self.hint.set_text(PAIRING_HINT);
                self.render_pairing(&state);
            }
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

        if view == SyncView::SelfHostedUrl {
            self.url_entry.grab_focus();
        }
        drop(state);
    }

    fn render_pairing(&self, state: &SyncState) {
        let code = if state.code.is_empty() {
            "· · ·".to_owned()
        } else {
            state.code.clone()
        };
        self.code.set_text(&code);
        if state.approval_url.is_empty() {
            self.approval.set_text("Starting pairing…");
        } else {
            self.approval.set_text(&format!(
                "{}\n{}",
                state
                    .backend
                    .map(Backend::pair_hint)
                    .unwrap_or("Approve the code on your phone."),
                state.approval_url
            ));
        }
        self.pairing_note.set_text(state.pairing_note());
        if state.waiting {
            self.pairing_status.set_text("Waiting for approval…");
            self.pairing_status.remove_css_class("error-text");
            self.pairing_status.add_css_class("notice-ok");
        } else {
            self.pairing_status.set_text("Not paired.");
        }
        let data = state.qr_data();
        if data.is_empty() {
            self.qr.set_paintable(None::<&gtk::gdk::Paintable>);
        } else {
            self.qr.set_paintable(Some(&qr_texture(&data)));
        }
    }
}

fn action_label(action: SyncAction) -> &'static str {
    match action {
        SyncAction::SyncNow => "Sync now",
        SyncAction::Pair(Backend::Nostos) => "Pair with Nostos",
        SyncAction::Pair(Backend::SelfHosted) => "Pair with a self-hosted server",
        SyncAction::Pair(Backend::GoogleDrive) => "Pair with Google Drive",
        SyncAction::Unpair => "Unpair",
    }
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
    use std::sync::Arc;

    #[test]
    fn the_overview_offers_every_backend_and_sync() {
        let rows: Vec<&str> = OVERVIEW_ROWS.iter().map(|row| action_label(*row)).collect();
        assert_eq!(
            rows,
            vec![
                "Sync now",
                "Pair with Nostos",
                "Pair with a self-hosted server",
                "Pair with Google Drive",
                "Unpair",
            ]
        );
    }

    #[test]
    fn pairing_a_self_hosted_server_wants_a_url_first() {
        let mut state = SyncState {
            selected: 2,
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
    fn a_pairing_code_and_qr_come_from_the_daemon_info() {
        let fake = FakeDaemon::new();
        let info = fake.pair("nostos", "").unwrap();
        let mut state = SyncState::default();
        state.on_pair_info(info.clone());
        assert_eq!(state.code, "K7Q-4MZ");
        assert!(state.qr_data().contains("app.nostos.page/pair"));
        assert!(state.qr_data().contains("K7Q-4MZ"));
        assert_eq!(state.view, SyncView::Pairing);

        state.on_pair_status("approved", "K7Q-4MZ", "Paired with Studio.".into());
        assert_eq!(state.view, SyncView::Overview);
        assert_eq!(state.notice, "Paired with Studio.");
        assert!(!state.waiting);
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
    }

    #[test]
    fn a_failed_pairing_returns_to_the_overview_with_one_line() {
        let mut state = SyncState::default();
        state.begin_pairing(Backend::Nostos);
        state.on_pair_status("error", "", "the code expired".into());
        assert_eq!(state.view, SyncView::Overview);
        assert_eq!(state.error.as_deref(), Some("the code expired"));
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
        let fake = Arc::new(FakeDaemon::new());
        fake.set_sync(SyncInfo {
            state: "error".into(),
            message: "network is off".into(),
            last_sync: 0,
        });
        assert_eq!(fake.sync_info().unwrap().message, "network is off");
    }

    #[test]
    fn back_leaves_the_pairing_flow_but_not_the_overview() {
        let mut state = SyncState::default();
        assert!(!state.back());
        state.view = SyncView::Pairing;
        assert!(state.back());
        assert_eq!(state.view, SyncView::Overview);
    }
}
