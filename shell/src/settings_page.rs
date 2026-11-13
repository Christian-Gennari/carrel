//! The Settings screen: one calm list in four groups, arrows to change, Enter
//! to open a screen.
//!
//! Every value from `spec/03-ui.md` is a row. The pure [`SettingsState`] owns
//! selection and value changes, so the tests do not need a display; the GTK
//! side only draws the rows and saves the result.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::daemon::Client;
use crate::locale;
use crate::nav::Navigator;
use crate::settings::{
    FontChoice, Settings, Theme, DAILY_GOAL_STEP, MAX_DAILY_GOAL, MIN_DAILY_GOAL,
};
use crate::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingRow {
    Theme,
    DayTheme,
    NightTheme,
    Font,
    Size,
    DailyGoal,
    GoalProgress,
    FocusMode,
    Typewriter,
    Spellcheck,
    Clock,
    Wifi,
    SyncBackends,
    SyncAtShutdown,
    Keyboard,
    Language,
    TimeZone,
    Update,
    About,
}

/// One line of the screen: a small group label, a value row, or the sample
/// line under Look. Only the value rows take the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsItem {
    Group(&'static str),
    Row(SettingRow),
    Sample,
}

/// The four group labels, in order.
pub const GROUPS: [&str; 4] = ["Look", "Writing", "Connections", "Device"];

/// Every line of the screen, in order. With an update ready the update row
/// sits at the very top instead of the bottom of Device (issue #28).
pub fn items(settings: &Settings, update_ready: bool) -> Vec<SettingsItem> {
    use SettingRow::*;
    let mut items = Vec::new();
    if update_ready {
        items.push(SettingsItem::Row(Update));
    }
    items.push(SettingsItem::Group(GROUPS[0]));
    items.push(SettingsItem::Row(Theme));
    if settings.theme.is_automatic() {
        items.push(SettingsItem::Row(DayTheme));
        items.push(SettingsItem::Row(NightTheme));
    }
    items.push(SettingsItem::Row(Font));
    items.push(SettingsItem::Row(Size));
    items.push(SettingsItem::Sample);

    items.push(SettingsItem::Group(GROUPS[1]));
    items.extend(
        [
            DailyGoal,
            GoalProgress,
            FocusMode,
            Typewriter,
            Spellcheck,
            Clock,
        ]
        .map(SettingsItem::Row),
    );

    items.push(SettingsItem::Group(GROUPS[2]));
    items.extend([Wifi, SyncBackends, SyncAtShutdown].map(SettingsItem::Row));

    items.push(SettingsItem::Group(GROUPS[3]));
    items.extend([Keyboard, Language, TimeZone].map(SettingsItem::Row));
    if !update_ready {
        items.push(SettingsItem::Row(Update));
    }
    items.push(SettingsItem::Row(About));
    items
}

/// The value rows the cursor walks, in screen order.
pub fn rows(settings: &Settings, update_ready: bool) -> Vec<SettingRow> {
    items(settings, update_ready)
        .into_iter()
        .filter_map(|item| match item {
            SettingsItem::Row(row) => Some(row),
            _ => None,
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsEffect {
    None,
    OpenWifi,
    OpenSync,
    ApplyUpdate,
}

/// The `UpdateStatus()` string, interpreted for the screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateState {
    pub wire: String,
    pub applying: bool,
}

impl UpdateState {
    pub fn from_wire(wire: impl Into<String>) -> Self {
        Self {
            wire: wire.into(),
            applying: false,
        }
    }

    fn ready_version(&self) -> Option<&str> {
        self.wire
            .strip_prefix("ready:")
            .or_else(|| self.wire.strip_prefix("ready "))
    }

    pub fn ready(&self) -> bool {
        !self.applying && self.ready_version().is_some()
    }

    /// Whether the row belongs at the top of Settings: ready to install, or
    /// installing right now.
    pub fn has_top_row(&self) -> bool {
        self.applying || self.ready_version().is_some()
    }

    /// The one clear sentence at the top of Settings.
    pub fn top_label(&self) -> String {
        if self.applying {
            return "Installing update… Carrel will restart.".to_owned();
        }
        match self.ready_version() {
            Some(version) if !version.is_empty() => {
                format!("Update ready ({version}) — Enter to restart and update")
            }
            _ => "Update ready — Enter to restart and update".to_owned(),
        }
    }

    pub fn label(&self) -> String {
        if self.applying {
            return "Installing…".to_owned();
        }
        match self.wire.as_str() {
            "" | "none" => "Up to date".to_owned(),
            "applying" => "Installing…".to_owned(),
            other if other.starts_with("downloading ") => "Downloading update…".to_owned(),
            other if other.starts_with("available ") => "Waiting for a sync".to_owned(),
            other if other.starts_with("error ") => "Could not check for updates".to_owned(),
            _ => "Update ready".to_owned(),
        }
    }
}

#[derive(Debug, Default)]
pub struct SettingsState {
    pub selected: usize,
    pub update: UpdateState,
    /// False until the daemon has answered once: the first answer may put the
    /// update row at the top without moving the writer's cursor.
    update_known: bool,
}

impl SettingsState {
    pub fn row(&self, settings: &Settings) -> SettingRow {
        let rows = rows(settings, self.update.has_top_row());
        rows[self.selected.min(rows.len() - 1)]
    }

    pub fn move_by(&mut self, settings: &Settings, delta: i32) {
        let len = rows(settings, self.update.has_top_row()).len() as i32;
        self.selected = ((self.selected as i32 + delta).rem_euclid(len)) as usize;
    }

    pub fn select(&mut self, settings: &Settings, index: usize) {
        let len = rows(settings, self.update.has_top_row()).len();
        self.selected = index.min(len.saturating_sub(1));
    }

    /// Take the daemon's update status. When the top row appears or leaves
    /// later, the cursor stays on the same setting.
    pub fn set_update(&mut self, update: UpdateState) {
        let first_answer = !self.update_known;
        self.update_known = true;
        let was_top = self.update.has_top_row();
        let is_top = update.has_top_row();
        self.update = update;
        if !first_answer && was_top != is_top {
            if is_top {
                self.selected += 1;
            } else {
                self.selected = self.selected.saturating_sub(1);
            }
        }
    }

    /// Change the selected value. Returns true when something was saved.
    pub fn change(&mut self, settings: &mut Settings, delta: i32) -> bool {
        let row = self.row(settings);
        match row {
            SettingRow::Theme => {
                let next = settings.theme.cycled(delta);
                let changed = next != settings.theme;
                settings.theme = next;
                changed
            }
            SettingRow::DayTheme => {
                let next = cycle_theme(&Theme::DAY, settings.day_theme, delta);
                let changed = next != settings.day_theme;
                settings.day_theme = next;
                changed
            }
            SettingRow::NightTheme => {
                let next = cycle_theme(&Theme::NIGHT, settings.night_theme, delta);
                let changed = next != settings.night_theme;
                settings.night_theme = next;
                changed
            }
            SettingRow::Font => {
                let next = match (settings.font, delta) {
                    (_, 0) => settings.font,
                    (FontChoice::Serif, d) if d > 0 => FontChoice::Sans,
                    (FontChoice::Sans, d) if d > 0 => FontChoice::Mono,
                    (FontChoice::Mono, d) if d > 0 => FontChoice::Serif,
                    (FontChoice::Serif, _) => FontChoice::Mono,
                    (FontChoice::Sans, _) => FontChoice::Serif,
                    (FontChoice::Mono, _) => FontChoice::Sans,
                };
                settings.font = next;
                true
            }
            SettingRow::Size => {
                if delta > 0 {
                    settings.larger_font()
                } else if delta < 0 {
                    settings.smaller_font()
                } else {
                    false
                }
            }
            SettingRow::DailyGoal => {
                let previous = settings.daily_goal;
                if delta > 0 {
                    settings.daily_goal =
                        (settings.daily_goal + DAILY_GOAL_STEP).min(MAX_DAILY_GOAL);
                } else if delta < 0 {
                    settings.daily_goal = settings
                        .daily_goal
                        .saturating_sub(DAILY_GOAL_STEP)
                        .max(MIN_DAILY_GOAL);
                }
                settings.daily_goal != previous
            }
            SettingRow::GoalProgress => {
                let previous = settings.goal_display;
                if delta != 0 {
                    settings.goal_display = settings.goal_display.other();
                }
                settings.goal_display != previous
            }
            SettingRow::FocusMode => {
                settings.focus_mode = !settings.focus_mode;
                true
            }
            SettingRow::Typewriter => {
                settings.typewriter_mode = !settings.typewriter_mode;
                true
            }
            SettingRow::Spellcheck => {
                settings.spellcheck = !settings.spellcheck;
                true
            }
            SettingRow::Clock => {
                settings.clock = !settings.clock;
                true
            }
            SettingRow::SyncAtShutdown => {
                settings.sync_at_shutdown = !settings.sync_at_shutdown;
                true
            }
            SettingRow::Keyboard => {
                let previous = settings.keyboard.clone();
                settings.keyboard =
                    locale::keyboard_cycle(&settings.keyboard, if delta < 0 { -1 } else { 1 });
                settings.keyboard != previous
            }
            SettingRow::Language => {
                let previous = settings.language.clone();
                settings.language = locale::cycle(
                    locale::LANGUAGES,
                    &settings.language,
                    if delta < 0 { -1 } else { 1 },
                );
                settings.language != previous
            }
            SettingRow::TimeZone => {
                let previous = settings.time_zone.clone();
                settings.time_zone = locale::cycle(
                    locale::TIME_ZONES,
                    &settings.time_zone,
                    if delta < 0 { -1 } else { 1 },
                );
                settings.time_zone != previous
            }
            // Enter opens the two sub-screens and applies an update. Nothing
            // else changes on Enter: Left/Right is how a value moves.
            SettingRow::Wifi
            | SettingRow::SyncBackends
            | SettingRow::Update
            | SettingRow::About => false,
        }
    }

    pub fn activate(&mut self, settings: &Settings) -> SettingsEffect {
        match self.row(settings) {
            SettingRow::Wifi => SettingsEffect::OpenWifi,
            SettingRow::SyncBackends => SettingsEffect::OpenSync,
            SettingRow::Update if self.update.ready() => SettingsEffect::ApplyUpdate,
            _ => SettingsEffect::None,
        }
    }

    pub fn value(&self, row: SettingRow, settings: &Settings) -> String {
        match row {
            SettingRow::Theme => settings.theme.label().to_owned(),
            SettingRow::DayTheme => settings.day_theme.label().to_owned(),
            SettingRow::NightTheme => settings.night_theme.label().to_owned(),
            SettingRow::Font => settings.font.label().to_owned(),
            SettingRow::Size => settings.font_size.to_string(),
            SettingRow::DailyGoal => format!("{} words", settings.daily_goal),
            SettingRow::GoalProgress => settings.goal_display.label().to_owned(),
            SettingRow::FocusMode => on_off(settings.focus_mode),
            SettingRow::Typewriter => on_off(settings.typewriter_mode),
            SettingRow::Spellcheck => on_off(settings.spellcheck),
            SettingRow::Clock => on_off(settings.clock),
            SettingRow::Wifi => "Open".to_owned(),
            SettingRow::SyncBackends => "Open".to_owned(),
            SettingRow::SyncAtShutdown => on_off(settings.sync_at_shutdown),
            SettingRow::Keyboard => locale::keyboard_label(&settings.keyboard).to_owned(),
            SettingRow::Language => settings.language.clone(),
            SettingRow::TimeZone => settings.time_zone.clone(),
            SettingRow::Update => self.update.label(),
            SettingRow::About => format!("Carrel {}", env!("CARGO_PKG_VERSION")),
        }
    }

    /// The line under the selected row; empty for a row that needs no help.
    pub fn hint_for(&self, settings: &Settings) -> String {
        match self.row(settings) {
            SettingRow::Theme if settings.theme.is_automatic() => format!(
                "{} in the day (07:00–19:00), {} at night.",
                settings.day_theme.label(),
                settings.night_theme.label()
            ),
            SettingRow::Theme => "This paper and ink stay until you change them.".to_owned(),
            SettingRow::DayTheme => "The theme from 07:00 while Automatic is on.".to_owned(),
            SettingRow::NightTheme => "The theme from 19:00 while Automatic is on.".to_owned(),
            SettingRow::Font => {
                "Serif, sans or mono. The page and this screen change at once.".to_owned()
            }
            SettingRow::Size => "Ctrl+= and Ctrl+- change this while writing.".to_owned(),
            SettingRow::DailyGoal => {
                "The words you aim for today, shown at the bottom of the page.".to_owned()
            }
            SettingRow::GoalProgress => {
                "Numbers on the bottom line, or a thin line along the bottom edge.".to_owned()
            }
            SettingRow::FocusMode => {
                "Only the paragraph with the cursor stays dark. F2 switches it.".to_owned()
            }
            SettingRow::Typewriter => {
                "Backspace and Delete do nothing. Keep moving forward.".to_owned()
            }
            SettingRow::Spellcheck => {
                "Marks words to check later; no squiggles in the first draft.".to_owned()
            }
            SettingRow::Clock => "The time at the end of the bottom line.".to_owned(),
            SettingRow::Wifi => {
                "Choose a network. The radio is on only while you are here.".to_owned()
            }
            SettingRow::SyncBackends => "Nostos, your own server or Google Drive.".to_owned(),
            SettingRow::SyncAtShutdown => {
                "One last sync before Carrel powers off, when a server is set up.".to_owned()
            }
            SettingRow::Keyboard => {
                "The keys you type on, including an external keyboard.".to_owned()
            }
            SettingRow::Language => "The language of the screens.".to_owned(),
            SettingRow::TimeZone => {
                "Used for the Automatic day and night and for the clock.".to_owned()
            }
            SettingRow::Update if self.update.applying => {
                "Carrel is installing the update and will restart.".to_owned()
            }
            SettingRow::Update if self.update.ready() => {
                "Carrel restarts and opens the new version.".to_owned()
            }
            SettingRow::Update => "Carrel looks for updates when it syncs.".to_owned(),
            SettingRow::About => "Free software. No account is needed to write.".to_owned(),
        }
    }
}

fn on_off(value: bool) -> String {
    if value {
        "On".to_owned()
    } else {
        "Off".to_owned()
    }
}

/// Move `delta` places through a day or night slot, wrapping.
fn cycle_theme(list: &[Theme], current: Theme, delta: i32) -> Theme {
    let len = list.len() as i32;
    let index = list.iter().position(|item| *item == current).unwrap_or(0) as i32;
    list[((index + delta).rem_euclid(len)) as usize]
}

/// The keys that work on the selected row: a value moves with Left/Right, a
/// screen opens with Enter, and Esc always goes back.
fn hints_for(state: &SettingsState, settings: &Settings) -> Vec<(&'static str, &'static str)> {
    match state.row(settings) {
        SettingRow::Wifi | SettingRow::SyncBackends => {
            vec![("↑ ↓", "Move"), ("Enter", "Open"), ("Esc", "Back")]
        }
        SettingRow::Update if state.update.ready() => {
            vec![("↑ ↓", "Move"), ("Enter", "Update"), ("Esc", "Back")]
        }
        _ => vec![("↑ ↓", "Move"), ("← →", "Change"), ("Esc", "Back")],
    }
}

/// The line under the Look group: this month's answer in the writer's own
/// font and size, so a change is seen without leaving Settings.
const SAMPLE: &str = "The quiet morning begins with a single sentence.";

pub struct SettingsPage {
    pub widget: gtk::Overlay,
    screen: Rc<ui::Screen>,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    state: RefCell<SettingsState>,
    /// List row index → the value row it selects. Group labels and the sample
    /// have no entry.
    index_map: RefCell<Vec<Option<usize>>>,
    status_generation: Cell<u64>,
    /// The screenshot hook's row, applied once the update status is known.
    initial_select: Cell<Option<usize>>,
    settings: Rc<RefCell<Settings>>,
    root: PathBuf,
    client: Client,
    nav: Rc<Navigator>,
    on_change: Rc<dyn Fn()>,
}

impl SettingsPage {
    pub fn new(
        client: Client,
        nav: Rc<Navigator>,
        settings: Rc<RefCell<Settings>>,
        root: PathBuf,
        on_change: Rc<dyn Fn()>,
    ) -> Rc<Self> {
        let screen = ui::Screen::new("Settings");
        screen.set_hints(&[("↑ ↓", "Move"), ("← →", "Change"), ("Esc", "Back")]);
        let list = ui::list();
        let scroller = ui::scroller(&list);
        screen.content.append(&ui::fade_top(&scroller));

        // Screenshot hook (dev/screenshot.sh): start with a later row selected,
        // as if the writer had pressed Down that many times. It lands after
        // the update status is loaded, so the index means a row on screen.
        let initial_select = std::env::var("CARREL_SETTINGS_SELECT")
            .ok()
            .and_then(|value| value.parse::<usize>().ok());

        let page = Rc::new(Self {
            widget: screen.root.clone(),
            screen,
            list,
            scroller,
            state: RefCell::new(SettingsState::default()),
            index_map: RefCell::new(Vec::new()),
            status_generation: Cell::new(0),
            initial_select: Cell::new(initial_select),
            settings,
            root,
            client,
            nav,
            on_change,
        });
        page.install();
        page
    }

    fn install(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(page) = weak.upgrade() {
                let index = page
                    .index_map
                    .borrow()
                    .get(row.index().max(0) as usize)
                    .copied()
                    .flatten();
                if let Some(index) = index {
                    let settings = page.settings.borrow();
                    page.state.borrow_mut().select(&settings, index);
                    drop(settings);
                    page.activate();
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.widget.connect_map(move |_| {
            if let Some(page) = weak.upgrade() {
                page.load_update();
                page.refresh();
                page.screen.focus_page();
            }
        });
    }

    fn load_update(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.client.spawn(
            |daemon| daemon.update_status(),
            move |result| {
                if let Some(page) = weak.upgrade() {
                    page.state
                        .borrow_mut()
                        .set_update(UpdateState::from_wire(result.unwrap_or_default()));
                    if let Some(index) = page.initial_select.take() {
                        let settings = page.settings.borrow();
                        page.state.borrow_mut().select(&settings, index);
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
        match keyval {
            gdk::Key::Escape => {
                self.nav.show_editor();
                glib::Propagation::Stop
            }
            gdk::Key::Up => {
                {
                    let settings = self.settings.borrow();
                    self.state.borrow_mut().move_by(&settings, -1);
                }
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Down => {
                {
                    let settings = self.settings.borrow();
                    self.state.borrow_mut().move_by(&settings, 1);
                }
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Home => {
                {
                    let settings = self.settings.borrow();
                    self.state.borrow_mut().select(&settings, 0);
                }
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::End => {
                {
                    let settings = self.settings.borrow();
                    self.state.borrow_mut().select(&settings, usize::MAX);
                }
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Left | gdk::Key::Right => {
                let delta = if keyval == gdk::Key::Left { -1 } else { 1 };
                self.change(delta);
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                self.activate();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    /// Show the Settings screen with `row` selected. The command palette uses
    /// it for "Theme…" and "Install update".
    pub fn open_row(self: &Rc<Self>, row: SettingRow) {
        {
            let settings = self.settings.borrow();
            if let Some(index) = rows(&settings).iter().position(|item| *item == row) {
                self.state.borrow_mut().select(&settings, index);
            }
        }
        self.nav.show("settings");
        self.refresh();
        self.screen.focus_page();
    }

    fn change(self: &Rc<Self>, delta: i32) {
        let changed = {
            let mut settings = self.settings.borrow_mut();
            self.state.borrow_mut().change(&mut settings, delta)
        };
        if changed {
            if let Err(error) = self.settings.borrow().save(&self.root) {
                eprintln!("carrel: could not save settings: {error}");
            }
            (self.on_change)();
        }
        self.refresh();
    }

    fn activate(self: &Rc<Self>) {
        let effect = {
            let settings = self.settings.borrow();
            self.state.borrow_mut().activate(&settings)
        };
        match effect {
            SettingsEffect::None => self.refresh(),
            SettingsEffect::OpenWifi => self.nav.show("wifi"),
            SettingsEffect::OpenSync => self.nav.show("sync"),
            SettingsEffect::ApplyUpdate => self.apply_update(),
        }
    }

    pub(crate) fn apply_update(self: &Rc<Self>) {
        self.state.borrow_mut().update.applying = true;
        self.refresh();
        let weak = Rc::downgrade(self);
        self.client.spawn(
            |daemon| daemon.apply_update(),
            move |result| {
                if let Some(page) = weak.upgrade() {
                    let failure = {
                        let mut state = page.state.borrow_mut();
                        match result {
                            Ok(()) => {
                                state.update.wire = "applying".to_owned();
                                state.update.applying = true;
                                None
                            }
                            Err(error) => {
                                state.update.applying = false;
                                Some(error.message)
                            }
                        }
                    };
                    if let Some(message) = failure {
                        page.flash_error(&message);
                    }
                    page.refresh();
                }
            },
        );
    }

    fn flash_error(self: &Rc<Self>, message: &str) {
        let generation = self.status_generation.get().wrapping_add(1);
        self.status_generation.set(generation);
        self.screen.set_status_error(message);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(5), move || {
            if let Some(page) = weak.upgrade() {
                if page.status_generation.get() == generation {
                    page.screen.set_status("");
                }
            }
        });
    }

    fn refresh(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let mut index_map = Vec::new();
        let mut select_index = 0;
        {
            let settings = self.settings.borrow();
            let mut state = self.state.borrow_mut();
            // A row that disappeared (the day and night slots after leaving
            // Automatic) must not leave the cursor past the last one.
            let count = rows(&settings, state.update.has_top_row()).len();
            state.selected = state.selected.min(count.saturating_sub(1));
            let top_update = state.update.has_top_row();
            let mut row_index = 0;
            for (list_index, item) in items(&settings, top_update).iter().enumerate() {
                match item {
                    SettingsItem::Group(name) => {
                        self.list.append(&group_row(name));
                        index_map.push(None);
                    }
                    SettingsItem::Sample => {
                        self.list.append(&sample_row());
                        index_map.push(None);
                    }
                    SettingsItem::Row(row) => {
                        let selected = row_index == state.selected;
                        if selected {
                            select_index = list_index;
                        }
                        // The hint belongs to the selected row only, and sits
                        // under it aligned with the row text.
                        let note = if selected {
                            state.hint_for(&settings)
                        } else {
                            String::new()
                        };
                        let widget = match row {
                            SettingRow::Theme | SettingRow::DayTheme | SettingRow::NightTheme => {
                                theme_value_row(*row, &settings, &note)
                            }
                            // An update ready is one clear sentence at the top,
                            // not a value on the right.
                            SettingRow::Update if top_update => {
                                action_row(&state.update.top_label(), &note)
                            }
                            SettingRow::About => ui::marked_value_row(
                                setting_name(*row),
                                &state.value(*row, &settings),
                                &note,
                            ),
                            _ => ui::value_row(
                                setting_name(*row),
                                &state.value(*row, &settings),
                                &note,
                            ),
                        };
                        // The day and night pickers belong to Automatic:
                        // indent them so they read as part of the Theme row.
                        if matches!(row, SettingRow::DayTheme | SettingRow::NightTheme) {
                            if let Some(child) = widget.child() {
                                child.set_margin_start(16);
                            }
                        }
                        self.list.append(&widget);
                        index_map.push(Some(row_index));
                        row_index += 1;
                    }
                }
            }
        }
        {
            let settings = self.settings.borrow();
            let state = self.state.borrow();
            self.screen.set_hints(&hints_for(&state, &settings));
        }
        *self.index_map.borrow_mut() = index_map;
        ui::select(&self.list, select_index);
        if let Some(row) = self.list.row_at_index(select_index as i32) {
            ui::scroll_to_row(&self.scroller, &self.list, &row);
        }
    }
}

/// The ready-update row: one sentence, in the same row shape as a setting,
/// with the selected row's hint under it.
fn action_row(text: &str, note: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("ui-row");
    row.add_css_class("setting-row");
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.add_css_class("setting-label");
    if note.is_empty() {
        row.set_child(Some(&label));
        return row;
    }
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.append(&label);
    let note = gtk::Label::new(Some(note));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.add_css_class("row-note");
    column.append(&note);
    row.set_child(Some(&column));
    row
}

/// A small, quiet group label. It takes no cursor: Up and Down step over it.
fn group_row(name: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    let label = gtk::Label::new(Some(name));
    label.set_xalign(0.0);
    label.add_css_class("section-title");
    label.set_margin_start(14);
    label.set_margin_top(12);
    label.set_margin_bottom(2);
    row.set_child(Some(&label));
    row
}

/// The sample line under Look: body text at the current font and size, in the
/// page's own colour.
fn sample_row() -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    let label = gtk::Label::new(Some(SAMPLE));
    label.set_xalign(0.0);
    label.add_css_class("setting-label");
    label.set_margin_start(14);
    label.set_margin_top(6);
    label.set_margin_bottom(4);
    row.set_child(Some(&label));
    row
}

/// A theme row: its name, a swatch of its paper, ink and accent, then the
/// theme's name again as the value (spec/03-ui.md).
fn theme_value_row(row: SettingRow, settings: &Settings, note: &str) -> gtk::ListBoxRow {
    let swatch = match row {
        SettingRow::Theme => match settings.theme.theme() {
            Some(theme) => ui::theme_swatch(theme),
            None => ui::automatic_swatch(settings.day_theme, settings.night_theme),
        },
        SettingRow::DayTheme => ui::theme_swatch(settings.day_theme),
        SettingRow::NightTheme => ui::theme_swatch(settings.night_theme),
        _ => ui::theme_swatch(Theme::Light),
    };
    let value = match row {
        SettingRow::DayTheme => settings.day_theme.label(),
        SettingRow::NightTheme => settings.night_theme.label(),
        _ => settings.theme.label(),
    };
    ui::swatch_row(setting_name(row), &swatch, value, note)
}

fn setting_name(row: SettingRow) -> &'static str {
    match row {
        SettingRow::Theme => "Theme",
        SettingRow::DayTheme => "Day theme",
        SettingRow::NightTheme => "Night theme",
        SettingRow::Font => "Font",
        SettingRow::Size => "Size",
        SettingRow::DailyGoal => "Daily goal",
        SettingRow::GoalProgress => "Goal progress",
        SettingRow::FocusMode => "Focus mode",
        SettingRow::Typewriter => "Typewriter mode",
        SettingRow::Spellcheck => "Spellcheck",
        SettingRow::Clock => "Clock",
        SettingRow::Wifi => "Wi-Fi networks",
        SettingRow::SyncBackends => "Sync",
        SettingRow::SyncAtShutdown => "Sync at shutdown",
        SettingRow::Keyboard => "Keyboard layout",
        SettingRow::Language => "Language",
        SettingRow::TimeZone => "Time zone",
        SettingRow::Update => "Update",
        SettingRow::About => "About",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{GoalDisplay, Theme, ThemeChoice};

    fn select(state: &mut SettingsState, settings: &Settings, row: SettingRow) {
        let index = rows(settings, state.update.has_top_row())
            .iter()
            .position(|item| *item == row)
            .expect("row is on screen");
        state.select(settings, index);
    }

    #[test]
    fn every_setting_from_the_spec_has_one_row_in_its_group() {
        let settings = Settings::default();
        let names: Vec<&str> = rows(&settings, false)
            .iter()
            .map(|row| setting_name(*row))
            .collect();
        assert_eq!(
            names,
            vec![
                "Theme",
                "Day theme",
                "Night theme",
                "Font",
                "Size",
                "Daily goal",
                "Goal progress",
                "Focus mode",
                "Typewriter mode",
                "Spellcheck",
                "Clock",
                "Wi-Fi networks",
                "Sync",
                "Sync at shutdown",
                "Keyboard layout",
                "Language",
                "Time zone",
                "Update",
                "About",
            ]
        );

        // A fixed theme has no day or night slot to show.
        let fixed = Settings {
            theme: ThemeChoice::Sepia,
            ..Settings::default()
        };
        let names: Vec<&str> = rows(&fixed, false)
            .iter()
            .map(|row| setting_name(*row))
            .collect();
        assert_eq!(names.len(), 17);
        assert!(!names.contains(&"Day theme"));
        assert!(!names.contains(&"Night theme"));
    }

    #[test]
    fn the_groups_come_in_the_order_the_screen_shows_them() {
        let settings = Settings::default();
        let labels: Vec<&str> = items(&settings, false)
            .into_iter()
            .filter_map(|item| match item {
                SettingsItem::Group(name) => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(labels, GROUPS);

        // Look ends with the sample, so a font or size change is seen at once.
        let items = items(&settings, false);
        let sample = items
            .iter()
            .position(|item| *item == SettingsItem::Sample)
            .expect("the sample line is on the screen");
        assert_eq!(items[sample - 1], SettingsItem::Row(SettingRow::Size));
        assert_eq!(items[sample + 1], SettingsItem::Group("Writing"));
    }

    #[test]
    fn moving_skips_group_labels_and_the_sample() {
        let settings = Settings::default();
        let mut state = SettingsState::default();
        let count = rows(&settings, false).len();
        let mut seen = Vec::new();
        for _ in 0..count {
            seen.push(state.row(&settings));
            state.move_by(&settings, 1);
        }
        assert_eq!(seen.len(), count);
        assert_eq!(seen[0], SettingRow::Theme);
        assert_eq!(seen[count - 1], SettingRow::About);
        assert_eq!(state.row(&settings), SettingRow::Theme);
        // The cursor only ever lands on value rows, never a label or sample.
        for index in 0..count {
            state.select(&settings, index);
            let selected = state.row(&settings);
            assert!(rows(&settings, false).contains(&selected));
        }
    }

    #[test]
    fn values_read_like_the_spec() {
        let settings = Settings::default();
        let state = SettingsState::default();
        assert_eq!(state.value(SettingRow::Theme, &settings), "Automatic");
        assert_eq!(state.value(SettingRow::DayTheme, &settings), "Light");
        assert_eq!(state.value(SettingRow::NightTheme, &settings), "Dark");
        assert_eq!(state.value(SettingRow::Font, &settings), "Serif");
        assert_eq!(state.value(SettingRow::Size, &settings), "19");
        assert_eq!(state.value(SettingRow::DailyGoal, &settings), "500 words");
        assert_eq!(state.value(SettingRow::GoalProgress, &settings), "Numbers");
        assert_eq!(state.value(SettingRow::FocusMode, &settings), "Off");
        assert_eq!(state.value(SettingRow::Typewriter, &settings), "Off");
        assert_eq!(state.value(SettingRow::Keyboard, &settings), "US");
        assert_eq!(state.value(SettingRow::Wifi, &settings), "Open");
        assert_eq!(state.value(SettingRow::SyncBackends, &settings), "Open");
        assert_eq!(state.value(SettingRow::Update, &settings), "Up to date");
    }

    #[test]
    fn every_row_has_one_plain_hint_and_the_typewriter_one_is_the_issues() {
        let settings = Settings::default();
        let mut state = SettingsState::default();
        for index in 0..rows(&settings, false).len() {
            state.select(&settings, index);
            let hint = state.hint_for(&settings);
            assert!(!hint.is_empty(), "{:?} has no hint", state.row(&settings));
            assert!(
                !hint.contains("GPL") && !hint.contains("px"),
                "the hint should use plain words: {hint:?}"
            );
        }
        select(&mut state, &settings, SettingRow::Typewriter);
        assert_eq!(
            state.hint_for(&settings),
            "Backspace and Delete do nothing. Keep moving forward."
        );
    }

    #[test]
    fn the_theme_hint_names_the_chosen_day_and_night_themes() {
        let mut settings = Settings::default();
        let state = SettingsState::default();
        assert_eq!(
            state.hint_for(&settings),
            "Light in the day (07:00–19:00), Dark at night."
        );
        settings.day_theme = Theme::Sepia;
        settings.night_theme = Theme::Study;
        assert_eq!(
            state.hint_for(&settings),
            "Sepia in the day (07:00–19:00), Writer's Study at night."
        );

        // A fixed theme is its own explanation.
        settings.theme = ThemeChoice::Mist;
        assert_eq!(
            state.hint_for(&settings),
            "This paper and ink stay until you change them."
        );
    }

    #[test]
    fn the_day_and_night_rows_appear_only_while_automatic_is_chosen() {
        let mut settings = Settings::default();
        let mut state = SettingsState::default();
        assert_eq!(rows(&settings, false).len(), 19);
        state.move_by(&settings, 1);
        assert_eq!(state.row(&settings), SettingRow::DayTheme);
        state.move_by(&settings, 1);
        assert_eq!(state.row(&settings), SettingRow::NightTheme);

        // Two down from Automatic is Font: choosing a fixed theme takes the
        // slot rows away, and the selection lands on the row now there.
        select(&mut state, &settings, SettingRow::Theme);
        state.change(&mut settings, 1);
        assert_eq!(settings.theme, ThemeChoice::Light);
        assert_eq!(rows(&settings, false).len(), 17);
        state.move_by(&settings, 1);
        assert_eq!(state.row(&settings), SettingRow::Font);
    }

    #[test]
    fn theme_row_walks_automatic_then_the_six_themes() {
        let mut settings = Settings::default();
        let mut state = SettingsState::default();
        let mut labels = Vec::new();
        for _ in 0..7 {
            labels.push(state.value(SettingRow::Theme, &settings));
            state.change(&mut settings, 1);
        }
        assert_eq!(
            labels,
            vec![
                "Automatic",
                "Light",
                "Sepia",
                "Mist",
                "Writer's Study",
                "Dark",
                "High contrast",
            ]
        );
        assert_eq!(settings.theme, ThemeChoice::Automatic);
    }

    #[test]
    fn day_and_night_rows_only_cycle_themes_of_their_mode() {
        let mut settings = Settings::default();
        let mut state = SettingsState::default();
        select(&mut state, &settings, SettingRow::DayTheme);
        state.change(&mut settings, 1);
        assert_eq!(settings.day_theme, Theme::Sepia);
        state.change(&mut settings, 1);
        assert_eq!(settings.day_theme, Theme::HighContrast);
        state.change(&mut settings, 1);
        assert_eq!(settings.day_theme, Theme::Light);

        select(&mut state, &settings, SettingRow::NightTheme);
        state.change(&mut settings, 1);
        assert_eq!(settings.night_theme, Theme::Mist);
        state.change(&mut settings, 1);
        assert_eq!(settings.night_theme, Theme::Study);
        state.change(&mut settings, 1);
        assert_eq!(settings.night_theme, Theme::Dark);
        assert!(settings.night_theme.is_dark());
    }

    #[test]
    fn goal_progress_and_focus_mode_change_and_persist_through_the_page() {
        let mut settings = Settings::default();
        let mut state = SettingsState::default();
        select(&mut state, &settings, SettingRow::GoalProgress);
        assert!(state.change(&mut settings, 1));
        assert_eq!(settings.goal_display, GoalDisplay::Hairline);
        state.change(&mut settings, 1);
        assert_eq!(settings.goal_display, GoalDisplay::Numbers);

        select(&mut state, &settings, SettingRow::FocusMode);
        assert!(state.change(&mut settings, 0));
        assert!(settings.focus_mode);
        state.change(&mut settings, 0);
        assert!(!settings.focus_mode);
    }

    #[test]
    fn changing_values_is_immediate_and_clamped() {
        let mut settings = Settings::default();
        let mut state = SettingsState::default();
        select(&mut state, &settings, SettingRow::DailyGoal);
        for _ in 0..30 {
            state.change(&mut settings, 1);
        }
        assert_eq!(settings.daily_goal, MAX_DAILY_GOAL);
        for _ in 0..30 {
            state.change(&mut settings, -1);
        }
        assert_eq!(settings.daily_goal, MIN_DAILY_GOAL);

        state.select(&settings, 0);
        state.change(&mut settings, 1);
        assert_eq!(settings.theme, ThemeChoice::Light);
        state.change(&mut settings, 1);
        assert_eq!(settings.theme, ThemeChoice::Sepia);
    }

    #[test]
    fn keyboard_language_and_time_zone_cycle() {
        let mut settings = Settings::default();
        let mut state = SettingsState::default();
        select(&mut state, &settings, SettingRow::Keyboard);
        state.change(&mut settings, 1);
        assert_eq!(settings.keyboard, "gb");
        state.change(&mut settings, 1);
        assert_eq!(settings.keyboard, "de");

        select(&mut state, &settings, SettingRow::Language);
        state.change(&mut settings, 1);
        assert_eq!(settings.language, "Deutsch");

        select(&mut state, &settings, SettingRow::TimeZone);
        state.change(&mut settings, 1);
        assert_eq!(settings.time_zone, "UTC");
    }

    #[test]
    fn the_update_row_moves_to_the_top_only_when_one_is_ready() {
        let settings = Settings::default();
        let mut state = SettingsState::default();

        // Up to date: the row is the last of Device.
        state.set_update(UpdateState::from_wire("none"));
        let list = rows(&settings, state.update.has_top_row());
        assert_eq!(list.first(), Some(&SettingRow::Theme));
        assert_eq!(list.last(), Some(&SettingRow::About));
        assert_eq!(state.row(&settings), SettingRow::Theme);
        assert_eq!(state.value(SettingRow::Update, &settings), "Up to date");

        // Ready: one clear row at the top, and no second Update at the bottom.
        state.set_update(UpdateState::from_wire("ready:0.2.0"));
        let list = rows(&settings, state.update.has_top_row());
        assert_eq!(list.first(), Some(&SettingRow::Update));
        assert_eq!(
            list.iter()
                .filter(|row| **row == SettingRow::Update)
                .count(),
            1
        );
        assert!(state.update.has_top_row());
        // A ready update that arrives later leaves the cursor where it was;
        // on a fresh screen the first answer lands on the update row instead.
        assert_eq!(state.row(&settings), SettingRow::Theme);
        state.select(&settings, 0);
        assert_eq!(state.row(&settings), SettingRow::Update);
        assert_eq!(
            state.update.top_label(),
            "Update ready (0.2.0) — Enter to restart and update"
        );
        assert_eq!(state.activate(&settings), SettingsEffect::ApplyUpdate);
    }

    #[test]
    fn the_update_row_stays_at_the_bottom_while_nothing_is_ready() {
        let settings = Settings::default();
        let mut state = SettingsState::default();
        state.set_update(UpdateState::from_wire("none"));
        select(&mut state, &settings, SettingRow::Update);
        assert_eq!(state.activate(&settings), SettingsEffect::None);
        assert_eq!(state.value(SettingRow::Update, &settings), "Up to date");
        state.set_update(UpdateState::from_wire("error network"));
        assert_eq!(
            state.value(SettingRow::Update, &settings),
            "Could not check for updates"
        );
    }

    #[test]
    fn navigation_rows_open_screens_and_values_do_not_change_on_enter() {
        let settings = Settings::default();
        let mut state = SettingsState::default();
        select(&mut state, &settings, SettingRow::Wifi);
        assert_eq!(state.activate(&settings), SettingsEffect::OpenWifi);
        select(&mut state, &settings, SettingRow::SyncBackends);
        assert_eq!(state.activate(&settings), SettingsEffect::OpenSync);

        // Enter on a value row is not a change; Left/Right is.
        select(&mut state, &settings, SettingRow::Font);
        assert_eq!(state.activate(&settings), SettingsEffect::None);
        assert_eq!(settings.font, FontChoice::Serif);
        select(&mut state, &settings, SettingRow::Theme);
        assert_eq!(state.activate(&settings), SettingsEffect::None);
        assert_eq!(settings.theme, ThemeChoice::Automatic);
    }

    #[test]
    fn the_hint_bar_names_enter_only_where_it_opens_something() {
        let settings = Settings::default();
        let mut state = SettingsState::default();
        state.set_update(UpdateState::from_wire("none"));
        for index in 0..rows(&settings, false).len() {
            state.select(&settings, index);
            let row = state.row(&settings);
            let opens = matches!(row, SettingRow::Wifi | SettingRow::SyncBackends);
            let hints = hints_for(&state, &settings);
            assert_eq!(
                hints.iter().any(|(key, _)| *key == "Enter"),
                opens,
                "{row:?}: the hint bar should name Enter only where it opens a screen"
            );
        }

        state.set_update(UpdateState::from_wire("ready:0.2.0"));
        state.select(&settings, 0);
        assert!(hints_for(&state, &settings)
            .iter()
            .any(|(key, word)| *key == "Enter" && *word == "Update"));
    }

    #[test]
    fn a_late_update_keeps_the_cursor_on_the_same_setting() {
        let settings = Settings::default();
        let mut state = SettingsState::default();
        state.set_update(UpdateState::from_wire("none"));
        select(&mut state, &settings, SettingRow::Font);
        assert_eq!(state.row(&settings), SettingRow::Font);

        state.set_update(UpdateState::from_wire("ready:0.2.0"));
        assert_eq!(state.row(&settings), SettingRow::Font);
        state.set_update(UpdateState::from_wire("none"));
        assert_eq!(state.row(&settings), SettingRow::Font);
    }
}
