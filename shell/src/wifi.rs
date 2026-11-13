//! The Wi-Fi screen: scan, choose, type the password, connect, forget.
//!
//! The daemon turns the radio on for this screen's two-minute window and
//! blocks it again afterwards (rule 2). Captive portals cannot be shown, so
//! the daemon reports one and this screen prints the one sentence from the
//! spec.
//!
//! Everything that belongs to one network stays under that network: the
//! password field and any error sit in a row of their own, right below the
//! selected name. When Sync opened this screen to fix a failed sync, a
//! connection hands the writer back to Sync (see [`crate::handoff`]).

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::daemon::{Client, DaemonError, DaemonErrorKind, WifiNetwork};
use crate::handoff::WifiHandoff;
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
    /// The status line under the page title: scanning, connected, forgot.
    pub message: String,
    /// True when the status line is an error (a scan that failed).
    pub error: bool,
    /// The sentence under the selected network: a refused password, a captive
    /// portal, a missing name. Always about that one network.
    pub inline: Option<String>,
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
        self.inline = None;
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.row_count().saturating_sub(1));
        self.inline = None;
    }

    /// Enter in the list: open the password field or connect straight away.
    pub fn choose(&mut self) -> Option<WifiEffect> {
        self.error = false;
        self.message.clear();
        self.inline = None;
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
                    self.error = false;
                    self.inline = Some("Type the password for this network.".to_owned());
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
            self.inline = Some("Type the network name.".to_owned());
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
        self.inline = None;
    }

    pub fn on_connected(&mut self, ssid: &str) {
        self.mode = WifiMode::List;
        self.password.clear();
        self.error = false;
        self.inline = None;
        self.message = format!("Connected to {ssid}.");
        for network in &mut self.networks {
            network.connected = network.ssid == ssid;
        }
    }

    /// A connect that failed. The sentence stays under the network it belongs
    /// to, and a wrong password keeps the field open so it can be retyped.
    pub fn on_connect_error(&mut self, ssid: &str, error: &DaemonError) {
        self.error = false;
        self.message.clear();
        let lower = error.message.to_ascii_lowercase();
        if error.kind == DaemonErrorKind::CaptivePortal {
            self.mode = WifiMode::List;
            self.inline = Some(error.message.clone());
        } else if lower.contains("password") || lower.contains("secret") {
            self.mode = WifiMode::Password {
                ssid: ssid.to_owned(),
                secured: true,
            };
            self.password.clear();
            self.inline = Some("That password did not work. Try again.".to_owned());
        } else {
            self.mode = WifiMode::List;
            self.inline = Some(error.message.clone());
        }
    }

    /// A scan that failed: there is no network row to sit under, so the status
    /// line carries it.
    pub fn on_scan_error(&mut self, error: &DaemonError) {
        self.mode = WifiMode::List;
        self.error = true;
        self.inline = None;
        self.message = match error.kind {
            DaemonErrorKind::CaptivePortal => {
                "This network needs a login page. Use a phone hotspot.".to_owned()
            }
            _ => error.message.clone(),
        };
    }

    /// How many of the three arcs are lit.
    pub fn signal_level(strength: u8) -> usize {
        if strength >= 70 {
            3
        } else if strength >= 40 {
            2
        } else {
            1
        }
    }

    /// The word an open network needs instead of a lock: none. Kept as the
    /// accessible name for the icon.
    pub fn access_label(network: &WifiNetwork) -> &'static str {
        if network.secured {
            "locked"
        } else {
            "open"
        }
    }

    /// The label above the field that belongs to the selected network.
    pub fn field_label(&self) -> String {
        match &self.mode {
            WifiMode::Password { ssid, .. } | WifiMode::OtherPassword { ssid } => {
                format!("Password for {ssid}")
            }
            WifiMode::OtherSsid => "Network name".to_owned(),
            WifiMode::List => String::new(),
        }
    }
}

pub struct WifiPage {
    pub widget: gtk::Overlay,
    screen: Rc<ui::Screen>,
    list: gtk::ListBox,
    list_scroller: gtk::ScrolledWindow,
    entry: gtk::Entry,
    state: RefCell<WifiState>,
    client: Client,
    nav: Rc<Navigator>,
    handoff: WifiHandoff,
}

impl WifiPage {
    pub fn new(client: Client, nav: Rc<Navigator>, handoff: WifiHandoff) -> Rc<Self> {
        let screen = ui::Screen::new("Wi-Fi");
        screen.set_hints(&HINTS);
        screen.set_status(RADIO_NOTE);

        let list = ui::list();
        // The password field lives inside a row of this list, and GTK only
        // lets a widget take focus when every ancestor can: the list stays
        // focusable so the field under a network can be typed into.
        list.set_can_focus(true);
        let list_scroller = ui::scroller(&list);
        // The list takes its natural height (up to a page's worth) so the field
        // under a network never lands at the bottom of the screen.
        list_scroller.set_vexpand(false);
        list_scroller.set_propagate_natural_height(true);
        list_scroller.set_max_content_height(420);
        let list_fade = ui::fade_top(&list_scroller);
        list_fade.set_vexpand(false);
        screen.content.append(&list_fade);

        let entry = gtk::Entry::new();
        entry.set_hexpand(true);
        entry.set_visibility(false);
        entry.add_css_class("calm-entry");

        let page = Rc::new(Self {
            widget: screen.root.clone(),
            screen,
            list,
            list_scroller,
            entry,
            state: RefCell::new(WifiState {
                loading: true,
                ..WifiState::default()
            }),
            client,
            nav,
            handoff,
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

    /// How many networks the last scan found, for the tests.
    pub fn network_count(&self) -> usize {
        self.state.borrow().networks.len()
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
                    Err(error) => page.state.borrow_mut().on_scan_error(&error),
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
                // A fresh field for a fresh network.
                if matches!(self.state.borrow().mode, WifiMode::Password { .. }) {
                    self.entry.set_text("");
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
                self.entry.set_text("");
                self.refresh();
            }
            WifiMode::Password { .. } | WifiMode::OtherPassword { .. } => {
                let password = self.entry.text().to_string();
                let effect = {
                    let mut state = self.state.borrow_mut();
                    state.password = password;
                    state.submit_password()
                };
                if let Some(effect) = effect {
                    self.entry.set_text("");
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
                                // Opened from Sync to fix a failed sync: go
                                // back and let Sync pick the thread up.
                                if page.handoff.connect() {
                                    page.nav.show("sync");
                                }
                            }
                            Err(error) => {
                                page.state.borrow_mut().on_connect_error(&ssid, &error);
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
                            state.inline = None;
                            drop(state);
                            page.refresh();
                        }
                        Err(error) => {
                            page.state.borrow_mut().on_scan_error(&error);
                            page.refresh();
                        }
                    }
                }
            },
        );
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
                {
                    let mut state = self.state.borrow_mut();
                    state.mode = WifiMode::List;
                    state.inline = None;
                }
                self.entry.set_text("");
                self.refresh();
                return glib::Propagation::Stop;
            }
            // Enter belongs to the field even when the dispatcher sees the key
            // first: every screen keeps its keys in one place.
            if matches!(keyval, gdk::Key::Return | gdk::Key::KP_Enter) {
                self.submit_entry();
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        match keyval {
            gdk::Key::Escape => {
                // Opened from Sync: one step back is Sync, not the editor.
                if self.handoff.is_asked() {
                    self.handoff.abandon();
                    self.nav.show("sync");
                } else {
                    self.nav.show_editor();
                }
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
        // The field lives in the row of its network, so lift it out before the
        // rows are rebuilt.
        if let Some(parent) = self.entry.parent() {
            if let Ok(holder) = parent.downcast::<gtk::Box>() {
                holder.remove(&self.entry);
            }
        }
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        let state = self.state.borrow();
        let field = state.field_label();
        let inline = state.inline.clone();
        let mode = state.mode.clone();
        let mut field_placed = false;
        let mut selected_index = 0i32;
        for (index, network) in state.networks.iter().enumerate() {
            let row = network_row(network);
            self.list.append(&row);
            let row_index = self.list.observe_children().n_items() as i32 - 1;
            if index != state.selected {
                continue;
            }
            selected_index = row_index;
            match &mode {
                WifiMode::Password { ssid, .. } if *ssid == network.ssid => {
                    self.place_field(&field, inline.as_deref());
                    field_placed = true;
                }
                // A failed connect left a sentence for this network.
                _ => {
                    if let Some(sentence) = inline.as_deref() {
                        if mode == WifiMode::List {
                            self.list.append(&error_row(sentence));
                        }
                    }
                }
            }
        }
        let other = gtk::ListBoxRow::new();
        other.add_css_class("ui-row");
        other.add_css_class("file-row");
        let label = gtk::Label::new(Some("Other network…"));
        label.set_xalign(0.0);
        label.add_css_class("file-name");
        other.set_child(Some(&label));
        self.list.append(&other);
        let other_index = self.list.observe_children().n_items() as i32 - 1;
        if state.on_other_row() {
            selected_index = other_index;
        }
        if state.on_other_row()
            && matches!(mode, WifiMode::OtherSsid | WifiMode::OtherPassword { .. })
        {
            self.place_field(&field, inline.as_deref());
            field_placed = true;
        }
        if !field_placed {
            if let Some(sentence) = inline.as_deref() {
                // The other row can carry an error of its own (an empty name).
                if state.on_other_row() && mode == WifiMode::List {
                    self.list.append(&error_row(sentence));
                }
            }
        }
        if let Some(row) = self.list.row_at_index(selected_index) {
            self.list.select_row(Some(&row));
            ui::scroll_to_row(&self.list_scroller, &self.list, &row);
        }

        self.entry.set_visibility(mode == WifiMode::OtherSsid);
        if !state.message.is_empty() {
            if state.error {
                self.screen.set_status_error(&state.message);
            } else {
                self.screen.set_status(&state.message);
            }
        } else if self.handoff.is_asked() {
            // Sync sent the writer here to fix a failed sync: say why.
            self.screen
                .set_status("Choose a network. Sync finishes when you are connected.");
        } else {
            self.screen.set_status(RADIO_NOTE);
        }
        if mode == WifiMode::List {
            self.screen.set_hints(&HINTS);
        } else {
            self.screen.set_hints(&PASSWORD_HINTS);
        }
        let focus_field = mode != WifiMode::List;
        drop(state);
        if focus_field {
            // The field was just moved into its new row, so it is mapped on
            // the next layout pass rather than now.
            let entry = self.entry.clone();
            glib::idle_add_local_once(move || {
                entry.grab_focus();
            });
        }
    }

    /// Put the field in a row of its own, right under its network, with the
    /// sentence that belongs to it underneath.
    fn place_field(&self, label: &str, inline: Option<&str>) {
        let row = gtk::ListBoxRow::new();
        row.set_selectable(false);
        row.set_activatable(false);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
        column.set_margin_start(14);
        column.set_margin_end(14);
        column.set_margin_top(2);
        column.set_margin_bottom(10);
        let title = gtk::Label::new(Some(label));
        title.set_xalign(0.0);
        title.add_css_class("section-title");
        column.append(&title);
        column.append(&self.entry);
        if let Some(sentence) = inline {
            let error = gtk::Label::new(Some(sentence));
            error.set_xalign(0.0);
            error.set_wrap(true);
            error.add_css_class("error-text");
            column.append(&error);
        }
        row.set_child(Some(&column));
        self.list.append(&row);
    }
}

/// One network: its name, then the state that has no word of its own as an
/// icon. A Wi-Fi fan for strength, a padlock for a network that needs a
/// password, and one short word for the network in use.
fn network_row(network: &WifiNetwork) -> gtk::ListBoxRow {
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
        let connected = gtk::Label::new(Some("Connected"));
        connected.add_css_class("row-note");
        line.append(&connected);
    }
    if network.secured {
        line.append(&lock_icon());
    }
    line.append(&signal_icon(network.strength));
    row.set_child(Some(&line));
    row
}

/// A sentence under a network, for a connection that did not work.
fn error_row(sentence: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    row.set_can_focus(false);
    let label = gtk::Label::new(Some(sentence));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_margin_start(14);
    label.set_margin_end(14);
    label.set_margin_bottom(10);
    label.add_css_class("error-text");
    row.set_child(Some(&label));
    row
}

/// The Wi-Fi mark: one dot and three arcs, filled from the dot out. Strength
/// reads at a glance and needs no icon font; unlit arcs stay as a quiet
/// outline so the icon never changes size.
fn signal_icon(strength: u8) -> gtk::DrawingArea {
    let level = WifiState::signal_level(strength);
    let area = gtk::DrawingArea::new();
    area.set_content_width(18);
    area.set_content_height(15);
    area.set_valign(gtk::Align::Center);
    area.set_can_focus(false);
    area.set_focusable(false);
    area.add_css_class("wifi-signal");
    area.set_accessible_role(gtk::AccessibleRole::Img);
    area.update_property(&[gtk::accessible::Property::Label(&format!(
        "Wi-Fi signal {level} of 3"
    ))]);
    area.set_draw_func(move |area, context, _width, _height| {
        let color = area.color();
        let (red, green, blue) = (
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
        );
        let (cx, cy) = (9.0, 13.0);
        for (index, radius) in [4.5_f64, 8.0, 11.5].iter().enumerate() {
            let lit = index < level;
            context.set_source_rgba(red, green, blue, if lit { 1.0 } else { 0.28 });
            context.set_line_width(1.8);
            context.new_path();
            context.arc(
                cx,
                cy,
                *radius,
                std::f64::consts::PI * 1.25,
                std::f64::consts::PI * 1.75,
            );
            let _ = context.stroke();
        }
        context.set_source_rgba(red, green, blue, 1.0);
        context.new_path();
        context.arc(cx, cy, 1.7, 0.0, std::f64::consts::TAU);
        let _ = context.fill();
    });
    area
}

/// A small padlock: the one thing that makes a network need a password.
fn lock_icon() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(11);
    area.set_content_height(15);
    area.set_valign(gtk::Align::Center);
    area.set_can_focus(false);
    area.set_focusable(false);
    area.add_css_class("wifi-signal");
    area.set_accessible_role(gtk::AccessibleRole::Img);
    area.update_property(&[gtk::accessible::Property::Label("Locked")]);
    area.set_draw_func(|area, context, _width, _height| {
        let color = area.color();
        context.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        context.set_line_width(1.5);
        context.new_path();
        context.arc(5.5, 6.0, 3.0, std::f64::consts::PI, std::f64::consts::TAU);
        let _ = context.stroke();
        context.new_path();
        context.rectangle(1.0, 6.0, 9.0, 8.0);
        let _ = context.fill();
    });
    area
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
        assert_eq!(state.field_label(), "Password for Studio 5G");
        assert!(state.submit_password().is_none());
        assert_eq!(
            state.inline.as_deref(),
            Some("Type the password for this network.")
        );
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
    fn a_portal_shows_the_sentence_from_the_spec_under_the_network() {
        let fake = FakeDaemon::new();
        let error = fake.wifi_connect("Hotel Guest", "").unwrap_err();
        let mut state = WifiState::default();
        state.on_connect_error("Hotel Guest", &error);
        assert!(!state.error, "the status line is not the error");
        assert_eq!(
            state.inline.as_deref(),
            Some("This network needs a login page. Use a phone hotspot.")
        );
        assert_eq!(state.mode, WifiMode::List);
    }

    #[test]
    fn a_refused_password_reopens_the_field_with_one_line() {
        let mut state = WifiState::default();
        let error = DaemonError::failed("Studio 5G refused the connection (wrong password?)");
        state.on_connect_error("Studio 5G", &error);
        assert_eq!(
            state.mode,
            WifiMode::Password {
                ssid: "Studio 5G".into(),
                secured: true
            }
        );
        assert_eq!(
            state.inline.as_deref(),
            Some("That password did not work. Try again.")
        );
    }

    #[test]
    fn connecting_says_so_and_moves_the_mark() {
        let fake = FakeDaemon::new();
        let mut state = WifiState::default();
        state.on_networks(fake.wifi_scan().unwrap());
        state.on_connected("Studio 5G");
        assert_eq!(state.message, "Connected to Studio 5G.");
        assert!(state.networks[1].connected);
        assert!(!state.networks[0].connected);
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
        assert_eq!(state.field_label(), "Network name");
        state.password = "  ".into();
        assert!(!state.submit_other_ssid());
        assert_eq!(state.inline.as_deref(), Some("Type the network name."));
        state.password = "Writing shed".into();
        assert!(state.submit_other_ssid());
        assert_eq!(
            state.mode,
            WifiMode::OtherPassword {
                ssid: "Writing shed".into()
            }
        );
        assert_eq!(state.field_label(), "Password for Writing shed");
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
    fn moving_clears_the_sentence_of_the_previous_network() {
        let fake = FakeDaemon::new();
        let mut state = WifiState::default();
        state.on_networks(fake.wifi_scan().unwrap());
        let error = fake.wifi_connect("Hotel Guest", "").unwrap_err();
        state.select(2);
        state.on_connect_error("Hotel Guest", &error);
        assert!(state.inline.is_some());
        state.move_by(1);
        assert_eq!(state.inline, None);
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
