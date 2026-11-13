//! The Wi-Fi screen: scan, choose, type the password, connect, forget.
//!
//! The daemon turns the radio on for this screen's two-minute window and
//! blocks it again afterwards (rule 2). Captive portals cannot be shown, so
//! the daemon reports one and this screen prints the one sentence from the
//! spec.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::daemon::{Client, DaemonError, DaemonErrorKind, WifiNetwork};
use crate::nav::Navigator;
use crate::ui;

const HINTS: [(&str, &str); 5] = [
    ("Enter", "Connect"),
    ("F", "Forget"),
    ("R", "Scan"),
    ("↑ ↓", "Move"),
    ("Esc", "Back"),
];
const PASSWORD_HINTS: [(&str, &str); 2] = [("Enter", "Connect"), ("Esc", "Cancel")];
const RADIO_NOTE: &str = "The radio is on only while this screen is open.";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WifiMode {
    #[default]
    List,
    Password {
        ssid: String,
        secured: bool,
    },
    OtherSsid,
    OtherPassword {
        ssid: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WifiEffect {
    Scan,
    Connect { ssid: String, password: String },
    Forget(String),
}

/// The pure Wi-Fi screen state.
#[derive(Debug, Default)]
pub struct WifiState {
    pub networks: Vec<WifiNetwork>,
    pub selected: usize,
    pub mode: WifiMode,
    pub password: String,
    pub message: String,
    pub error: bool,
    pub loading: bool,
}

impl WifiState {
    pub fn row_count(&self) -> usize {
        self.networks.len() + 1
    }

    pub fn on_other_row(&self) -> bool {
        self.selected >= self.networks.len()
    }

    pub fn selected_network(&self) -> Option<&WifiNetwork> {
        self.networks.get(self.selected)
    }

    pub fn move_by(&mut self, delta: i32) {
        let len = self.row_count() as i32;
        self.selected = ((self.selected as i32 + delta).rem_euclid(len)) as usize;
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.row_count().saturating_sub(1));
    }

    /// Enter in the list: open the password bar or connect straight away.
    pub fn choose(&mut self) -> Option<WifiEffect> {
        self.error = false;
        self.message.clear();
        if self.on_other_row() {
            self.mode = WifiMode::OtherSsid;
            self.password.clear();
            return None;
        }
        let network = self.selected_network()?.clone();
        if network.secured {
            self.mode = WifiMode::Password {
                ssid: network.ssid,
                secured: true,
            };
            self.password.clear();
            None
        } else {
            Some(WifiEffect::Connect {
                ssid: network.ssid,
                password: String::new(),
            })
        }
    }

    pub fn submit_password(&mut self) -> Option<WifiEffect> {
        match self.mode.clone() {
            WifiMode::Password { ssid, secured } => {
                if secured && self.password.is_empty() {
                    self.error = true;
                    self.message = "Type the password for this network.".to_owned();
                    return None;
                }
                let password = self.password.clone();
                self.mode = WifiMode::List;
                Some(WifiEffect::Connect { ssid, password })
            }
            WifiMode::OtherPassword { ssid } => {
                let password = self.password.clone();
                self.mode = WifiMode::List;
                Some(WifiEffect::Connect { ssid, password })
            }
            _ => None,
        }
    }

    pub fn submit_other_ssid(&mut self) -> bool {
        let ssid = self.password.trim().to_owned();
        if ssid.is_empty() {
            self.error = true;
            self.message = "Type the network name.".to_owned();
            return false;
        }
        self.password.clear();
        self.mode = WifiMode::OtherPassword { ssid };
        true
    }

    pub fn forget(&mut self) -> Option<WifiEffect> {
        if self.mode != WifiMode::List || self.on_other_row() {
            return None;
        }
        self.selected_network()
            .cloned()
            .map(|network| WifiEffect::Forget(network.ssid))
    }

    pub fn on_networks(&mut self, networks: Vec<WifiNetwork>) {
        self.networks = networks;
        self.loading = false;
        self.selected = self.selected.min(self.row_count().saturating_sub(1));
        self.error = false;
    }

    pub fn on_connected(&mut self, ssid: &str) {
        self.mode = WifiMode::List;
        self.password.clear();
        self.error = false;
        self.message =
            format!("Connected to {ssid}. The radio turns off when you leave this screen.");
        for network in &mut self.networks {
            network.connected = network.ssid == ssid;
        }
    }

    pub fn on_error(&mut self, error: &DaemonError) {
        self.mode = WifiMode::List;
        self.error = true;
        self.message = match error.kind {
            DaemonErrorKind::CaptivePortal => {
                "This network needs a login page. Use a phone hotspot.".to_owned()
            }
            _ if error.message.to_ascii_lowercase().contains("password")
                || error.message.to_ascii_lowercase().contains("secret") =>
            {
                "Could not connect. Check the password.".to_owned()
            }
            _ => error.message.clone(),
        };
    }

    /// How many of the three calm bars are lit.
    pub fn signal_level(strength: u8) -> usize {
        if strength >= 70 {
            3
        } else if strength >= 40 {
            2
        } else {
            1
        }
    }

    pub fn access_label(network: &WifiNetwork) -> &'static str {
        if network.secured {
            "locked"
        } else {
            "open"
        }
    }
}

pub struct WifiPage {
    pub widget: gtk::Overlay,
    screen: Rc<ui::Screen>,
    list: gtk::ListBox,
    list_scroller: gtk::ScrolledWindow,
    entry_bar: gtk::Box,
    entry_label: gtk::Label,
    entry: gtk::Entry,
    state: RefCell<WifiState>,
    client: Client,
    nav: Rc<Navigator>,
}

impl WifiPage {
    pub fn new(client: Client, nav: Rc<Navigator>) -> Rc<Self> {
        let screen = ui::Screen::new("Wi-Fi");
        screen.set_hints(&HINTS);
        screen.set_status(RADIO_NOTE);

        let list = ui::list();
        let list_scroller = ui::scroller(&list);
        // Keep the password field close to the list it belongs to: the list
        // takes its natural height (up to a page's worth) instead of pushing
        // the field to the bottom of the screen.
        list_scroller.set_vexpand(false);
        list_scroller.set_propagate_natural_height(true);
        list_scroller.set_max_content_height(420);
        let list_fade = ui::fade_top(&list_scroller);
        list_fade.set_vexpand(false);
        screen.content.append(&list_fade);

        let entry_label = gtk::Label::new(Some("Password"));
        entry_label.add_css_class("section-title");
        let entry = gtk::Entry::new();
        entry.set_hexpand(true);
        entry.set_visibility(false);
        entry.add_css_class("calm-entry");
        let entry_bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        entry_bar.append(&entry_label);
        entry_bar.append(&entry);
        entry_bar.set_visible(false);
        screen.content.append(&entry_bar);

        let page = Rc::new(Self {
            widget: screen.root.clone(),
            screen,
            list,
            list_scroller,
            entry_bar,
            entry_label,
            entry,
            state: RefCell::new(WifiState {
                loading: true,
                ..WifiState::default()
            }),
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
                page.state.borrow_mut().select(row.index().max(0) as usize);
                page.choose();
            }
        });

        let weak = Rc::downgrade(self);
        self.entry.connect_activate(move |_| {
            if let Some(page) = weak.upgrade() {
                page.submit_entry();
            }
        });

        let weak = Rc::downgrade(self);
        self.widget.connect_map(move |_| {
            if let Some(page) = weak.upgrade() {
                page.scan();
                page.screen.focus_page();
            }
        });
    }

    fn scan(self: &Rc<Self>) {
        {
            let mut state = self.state.borrow_mut();
            state.loading = true;
            state.error = false;
            state.message = "Scanning…".to_owned();
        }
        self.refresh();
        let weak = Rc::downgrade(self);
        self.client.spawn(
            |daemon| daemon.wifi_scan(),
            move |result| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                match result {
                    Ok(networks) => {
                        let mut state = page.state.borrow_mut();
                        state.on_networks(networks);
                        if state.networks.is_empty() {
                            state.message =
                                "No networks in range. Press R to scan again.".to_owned();
                        } else {
                            state.message.clear();
                        }
                        drop(state);
                    }
                    Err(error) => page.state.borrow_mut().on_error(&error),
                }
                page.refresh();
            },
        );
    }

    fn choose(self: &Rc<Self>) {
        let effect = self.state.borrow_mut().choose();
        match effect {
            Some(effect) => self.run_effect(effect),
            None => {
                let mode = self.state.borrow().mode.clone();
                match mode {
                    WifiMode::OtherSsid => {
                        self.entry.set_visibility(true);
                        self.entry_label.set_text("Network name");
                        self.entry.set_text("");
                        self.entry_bar.set_visible(true);
                        self.entry.grab_focus();
                    }
                    WifiMode::Password { ssid, .. } => {
                        self.entry.set_visibility(false);
                        self.entry_label.set_text(&format!("Password for {ssid}"));
                        self.entry.set_text("");
                        self.entry_bar.set_visible(true);
                        self.entry.grab_focus();
                    }
                    _ => {}
                }
                self.refresh();
            }
        }
    }

    fn submit_entry(self: &Rc<Self>) {
        let mode = self.state.borrow().mode.clone();
        match mode {
            WifiMode::OtherSsid => {
                let name = self.entry.text().to_string();
                let mut state = self.state.borrow_mut();
                state.password = name;
                if !state.submit_other_ssid() {
                    drop(state);
                    self.refresh();
                    return;
                }
                state.password.clear();
                drop(state);
                self.entry.set_visibility(false);
                self.entry_label.set_text(&format!(
                    "Password for {}",
                    other_ssid(&self.state.borrow().mode)
                ));
                self.entry.set_text("");
                self.entry.grab_focus();
            }
            WifiMode::Password { .. } | WifiMode::OtherPassword { .. } => {
                let password = self.entry.text().to_string();
                let effect = {
                    let mut state = self.state.borrow_mut();
                    state.password = password;
                    state.submit_password()
                };
                if let Some(effect) = effect {
                    self.hide_entry();
                    self.run_effect(effect);
                } else {
                    self.refresh();
                }
            }
            WifiMode::List => {}
        }
    }

    fn run_effect(self: &Rc<Self>, effect: WifiEffect) {
        match effect {
            WifiEffect::Scan => self.scan(),
            WifiEffect::Connect { ssid, password } => {
                {
                    let mut state = self.state.borrow_mut();
                    state.loading = true;
                    state.error = false;
                    state.message = format!("Connecting to {ssid}…");
                }
                self.refresh();
                let weak = Rc::downgrade(self);
                let request_ssid = ssid.clone();
                self.client.spawn(
                    move |daemon| daemon.wifi_connect(&request_ssid, &password),
                    move |result| {
                        let Some(page) = weak.upgrade() else {
                            return;
                        };
                        match result {
                            Ok(()) => {
                                page.state.borrow_mut().on_connected(&ssid);
                                page.refresh();
                            }
                            Err(error) => {
                                page.state.borrow_mut().on_error(&error);
                                page.refresh();
                            }
                        }
                    },
                );
            }
            WifiEffect::Forget(ssid) => self.confirm_forget(ssid),
        }
    }

    fn confirm_forget(self: &Rc<Self>, ssid: String) {
        let weak = Rc::downgrade(self);
        self.screen.ask(
            &format!("Forget “{ssid}”?"),
            "The saved password is removed from this device.",
            "Forget",
            move || {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                page.forget(ssid.clone());
            },
        );
    }

    fn forget(self: &Rc<Self>, ssid: String) {
        let weak = Rc::downgrade(self);
        self.client.spawn(
            {
                let ssid = ssid.clone();
                move |daemon| daemon.wifi_forget(&ssid)
            },
            move |result| {
                if let Some(page) = weak.upgrade() {
                    match result {
                        Ok(()) => {
                            let mut state = page.state.borrow_mut();
                            state.networks.retain(|network| network.ssid != ssid);
                            state.message = format!("Forgot {ssid}.");
                            state.error = false;
                            drop(state);
                            page.refresh();
                        }
                        Err(error) => {
                            page.state.borrow_mut().on_error(&error);
                            page.refresh();
                        }
                    }
                }
            },
        );
    }

    fn hide_entry(&self) {
        self.entry_bar.set_visible(false);
        self.entry.set_text("");
    }

    /// The forget card is open; the command palette waits for it.
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
        if state.contains(gdk::ModifierType::CONTROL_MASK) {
            return glib::Propagation::Proceed;
        }
        if self.state.borrow().mode != WifiMode::List {
            if keyval == gdk::Key::Escape {
                self.state.borrow_mut().mode = WifiMode::List;
                self.hide_entry();
                self.refresh();
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        match keyval {
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
            _ if keyval.to_lower() == gdk::Key::f => {
                let effect = self.state.borrow_mut().forget();
                if let Some(effect) = effect {
                    self.run_effect(effect);
                }
                glib::Propagation::Stop
            }
            _ if keyval.to_lower() == gdk::Key::r => {
                self.scan();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn refresh(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        {
            let state = self.state.borrow();
            for network in &state.networks {
                let row = gtk::ListBoxRow::new();
                row.add_css_class("ui-row");
                row.add_css_class("file-row");
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                let name = gtk::Label::new(Some(&network.ssid));
                name.set_xalign(0.0);
                name.set_hexpand(true);
                name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                name.add_css_class("file-name");
                line.append(&name);
                if network.connected {
                    let connected = gtk::Label::new(Some("connected"));
                    connected.add_css_class("row-note");
                    line.append(&connected);
                }
                let bars = gtk::Box::new(gtk::Orientation::Horizontal, 2);
                bars.set_width_request(40);
                bars.set_halign(gtk::Align::End);
                let level = WifiState::signal_level(network.strength);
                for (index, glyph) in ["▂", "▄", "▆"].iter().enumerate() {
                    let bar = gtk::Label::new(Some(glyph));
                    bar.add_css_class(if index < level {
                        "wifi-signal"
                    } else {
                        "wifi-signal-off"
                    });
                    bars.append(&bar);
                }
                line.append(&bars);
                let access = gtk::Label::new(Some(WifiState::access_label(network)));
                access.add_css_class("row-note");
                access.set_width_request(56);
                access.set_xalign(0.0);
                line.append(&access);
                row.set_child(Some(&line));
                self.list.append(&row);
            }
            let other = gtk::ListBoxRow::new();
            other.add_css_class("ui-row");
            other.add_css_class("file-row");
            let label = gtk::Label::new(Some("Other network…"));
            label.set_xalign(0.0);
            label.add_css_class("file-name");
            other.set_child(Some(&label));
            self.list.append(&other);
            ui::select(&self.list, state.selected);
        }
        if let Some(row) = self.list.row_at_index(self.state.borrow().selected as i32) {
            ui::scroll_to_row(&self.list_scroller, &self.list, &row);
        }

        let state = self.state.borrow();
        if state.message.is_empty() {
            self.screen.set_status(RADIO_NOTE);
        } else if state.error {
            self.screen.set_status_error(&state.message);
        } else {
            self.screen.set_status(&state.message);
        }
        if state.mode == WifiMode::List {
            self.screen.set_hints(&HINTS);
        } else {
            self.screen.set_hints(&PASSWORD_HINTS);
        }
    }
}

fn other_ssid(mode: &WifiMode) -> String {
    match mode {
        WifiMode::OtherPassword { ssid } => ssid.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{DaemonClient, FakeDaemon};

    #[test]
    fn the_scan_list_comes_from_the_daemon() {
        let fake = FakeDaemon::new();
        let mut state = WifiState::default();
        state.on_networks(fake.wifi_scan().unwrap());
        assert!(state.networks.len() >= 4);
        assert_eq!(state.row_count(), state.networks.len() + 1);
        assert_eq!(state.networks[0].ssid, "Kitchen");
        assert!(state.networks[0].connected);
    }

    #[test]
    fn a_locked_network_asks_for_a_password_and_an_open_one_does_not() {
        let fake = FakeDaemon::new();
        let mut state = WifiState::default();
        state.on_networks(fake.wifi_scan().unwrap());

        state.select(1); // Studio 5G, locked
        assert_eq!(state.choose(), None);
        assert_eq!(
            state.mode,
            WifiMode::Password {
                ssid: "Studio 5G".into(),
                secured: true
            }
        );
        assert!(state.submit_password().is_none());
        assert!(state.error);
        state.password = "secret".into();
        assert_eq!(
            state.submit_password(),
            Some(WifiEffect::Connect {
                ssid: "Studio 5G".into(),
                password: "secret".into()
            })
        );

        state.select(2); // Hotel Guest, open
        assert_eq!(
            state.choose(),
            Some(WifiEffect::Connect {
                ssid: "Hotel Guest".into(),
                password: String::new()
            })
        );
    }

    #[test]
    fn a_portal_shows_the_sentence_from_the_spec() {
        let fake = FakeDaemon::new();
        let error = fake.wifi_connect("Hotel Guest", "").unwrap_err();
        let mut state = WifiState::default();
        state.on_error(&error);
        assert!(state.error);
        assert_eq!(
            state.message,
            "This network needs a login page. Use a phone hotspot."
        );
    }

    #[test]
    fn forgetting_asks_for_the_selected_network() {
        let fake = FakeDaemon::new();
        let mut state = WifiState::default();
        state.on_networks(fake.wifi_scan().unwrap());
        state.select(1);
        assert_eq!(state.forget(), Some(WifiEffect::Forget("Studio 5G".into())));
        state.select(state.networks.len());
        assert_eq!(state.forget(), None);
    }

    #[test]
    fn hidden_networks_take_a_name_then_a_password() {
        let mut state = WifiState::default();
        state.on_networks(vec![WifiNetwork {
            ssid: "Kitchen".into(),
            strength: 80,
            secured: true,
            connected: false,
        }]);
        state.select(1); // Other network…
        assert_eq!(state.choose(), None);
        assert_eq!(state.mode, WifiMode::OtherSsid);
        state.password = "  ".into();
        assert!(!state.submit_other_ssid());
        state.password = "Writing shed".into();
        assert!(state.submit_other_ssid());
        assert_eq!(
            state.mode,
            WifiMode::OtherPassword {
                ssid: "Writing shed".into()
            }
        );
        state.password = "quiet".into();
        assert_eq!(
            state.submit_password(),
            Some(WifiEffect::Connect {
                ssid: "Writing shed".into(),
                password: "quiet".into()
            })
        );
    }

    #[test]
    fn signal_and_lock_labels_need_no_icon_font() {
        assert_eq!(WifiState::signal_level(92), 3);
        assert_eq!(WifiState::signal_level(50), 2);
        assert_eq!(WifiState::signal_level(10), 1);
        let locked = WifiNetwork {
            ssid: "x".into(),
            strength: 50,
            secured: true,
            connected: false,
        };
        assert_eq!(WifiState::access_label(&locked), "locked");
        let open = WifiNetwork {
            secured: false,
            ..locked
        };
        assert_eq!(WifiState::access_label(&open), "open");
    }
}
