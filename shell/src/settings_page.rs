//! The Settings screen: one calm list, arrows to change, Enter to open.
//!
//! Every value from `spec/03-ui.md` is a row. The pure [`SettingsState`] owns
//! selection and value changes, so the tests do not need a display; the GTK
//! side only draws the rows and saves the result.

use std::cell::RefCell;
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

/// Every row, with the two Automatic slots right after Theme.
const ROWS_AUTOMATIC: [SettingRow; 19] = [
    SettingRow::Theme,
    SettingRow::DayTheme,
    SettingRow::NightTheme,
    SettingRow::Font,
    SettingRow::Size,
    SettingRow::DailyGoal,
    SettingRow::GoalProgress,
    SettingRow::FocusMode,
    SettingRow::Typewriter,
    SettingRow::Spellcheck,
    SettingRow::Clock,
    SettingRow::Wifi,
    SettingRow::SyncBackends,
    SettingRow::SyncAtShutdown,
    SettingRow::Keyboard,
    SettingRow::Language,
    SettingRow::TimeZone,
    SettingRow::Update,
    SettingRow::About,
];

/// The same list without the day and night pickers: they only exist while
/// Automatic is chosen (spec/03-ui.md).
const ROWS_FIXED: [SettingRow; 17] = [
    SettingRow::Theme,
    SettingRow::Font,
    SettingRow::Size,
    SettingRow::DailyGoal,
    SettingRow::GoalProgress,
    SettingRow::FocusMode,
    SettingRow::Typewriter,
    SettingRow::Spellcheck,
    SettingRow::Clock,
    SettingRow::Wifi,
    SettingRow::SyncBackends,
    SettingRow::SyncAtShutdown,
    SettingRow::Keyboard,
    SettingRow::Language,
    SettingRow::TimeZone,
    SettingRow::Update,
    SettingRow::About,
];

/// The rows this person's settings put on screen.
pub fn rows(settings: &Settings) -> &'static [SettingRow] {
    if settings.theme.is_automatic() {
        &ROWS_AUTOMATIC
    } else {
        &ROWS_FIXED
    }
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

    pub fn label(&self) -> String {
        if self.applying {
            return "Installing…".to_owned();
        }
        match self.wire.as_str() {
            "" | "none" => "Up to date".to_owned(),
            "applying" => "Installing…".to_owned(),
            other if other.starts_with("downloading ") => "Downloading update…".to_owned(),
            other if other.starts_with("available ") => "Update available · Sync".to_owned(),
            other if other.starts_with("error ") => "Update check failed".to_owned(),
            other if other.starts_with("ready ") => {
                format!("Update ready ({}) · Apply", &other[6..])
            }
            other => match other.strip_prefix("ready:") {
                Some(version) if !version.is_empty() => {
                    format!("Update ready ({version}) · Apply")
                }
                _ => "Update ready · Apply".to_owned(),
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct SettingsState {
    pub selected: usize,
    pub about: bool,
    pub update: UpdateState,
}

impl SettingsState {
    pub fn row(&self, settings: &Settings) -> SettingRow {
        let rows = rows(settings);
        rows[self.selected.min(rows.len() - 1)]
    }

    pub fn move_by(&mut self, settings: &Settings, delta: i32) {
        self.about = false;
        let len = rows(settings).len() as i32;
        self.selected = ((self.selected as i32 + delta).rem_euclid(len)) as usize;
    }

    pub fn select(&mut self, settings: &Settings, index: usize) {
        self.about = false;
        self.selected = index.min(rows(settings).len() - 1);
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
            // Navigation and actions do not change a value.
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
            SettingRow::About => {
                self.about = !self.about;
                SettingsEffect::None
            }
            _ => SettingsEffect::None,
        }
    }

    pub fn value(&self, row: SettingRow, settings: &Settings) -> String {
        match row {
            SettingRow::Theme => settings.theme.label().to_owned(),
            SettingRow::DayTheme => settings.day_theme.label().to_owned(),
            SettingRow::NightTheme => settings.night_theme.label().to_owned(),
            SettingRow::Font => settings.font.label().to_owned(),
            SettingRow::Size => format!("{} px", settings.font_size),
            SettingRow::DailyGoal => format!("{} words", settings.daily_goal),
            SettingRow::GoalProgress => settings.goal_display.label().to_owned(),
            SettingRow::FocusMode => on_off(settings.focus_mode),
            SettingRow::Typewriter => on_off(settings.typewriter_mode),
            SettingRow::Spellcheck => on_off(settings.spellcheck),
            SettingRow::Clock => on_off(settings.clock),
            SettingRow::Wifi => "Change".to_owned(),
            SettingRow::SyncBackends => "Change".to_owned(),
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
        if self.about {
            return "Carrel is free software, GPL-3.0-or-later. No account is needed to write."
                .to_owned();
        }
        match self.row(settings) {
            SettingRow::Theme if settings.theme.is_automatic() => format!(
                "{} in the day (07:00–19:00), {} at night.",
                settings.day_theme.label(),
                settings.night_theme.label()
            ),
            // A fixed theme is its own explanation.
            SettingRow::Theme => String::new(),
            SettingRow::DayTheme => {
                "The daytime theme while Automatic follows the clock (07:00–19:00).".to_owned()
            }
            SettingRow::NightTheme => {
                "The night-time theme while Automatic follows the clock (19:00–07:00).".to_owned()
            }
            SettingRow::Font => "The same three fonts everywhere, for calm reading.".to_owned(),
            SettingRow::Size => "Ctrl+= and Ctrl+- change this while writing.".to_owned(),
            SettingRow::DailyGoal => "Shown on the bottom line: words today / goal.".to_owned(),
            SettingRow::GoalProgress => {
                "Hairline draws today's progress along the bottom edge, in the accent.".to_owned()
            }
            SettingRow::FocusMode => {
                "Everything but the paragraph with the cursor fades to 35 %. F2 toggles it."
                    .to_owned()
            }
            SettingRow::Typewriter => {
                "Backspace and Delete do nothing, so the draft only moves forward.".to_owned()
            }
            SettingRow::Spellcheck => "Off by default; no squiggles in the first draft.".to_owned(),
            SettingRow::Clock => "Show the time on the bottom line.".to_owned(),
            SettingRow::Wifi => "The radio is on only while the Wi-Fi screen is open.".to_owned(),
            SettingRow::SyncBackends => "Nostos, a self-hosted server or Google Drive.".to_owned(),
            SettingRow::SyncAtShutdown => {
                "Sync once more before powering off, if a server is paired.".to_owned()
            }
            SettingRow::Keyboard => settings.keyboard.clone(),
            SettingRow::Language => settings.language.clone(),
            SettingRow::TimeZone => settings.time_zone.clone(),
            SettingRow::Update if self.update.ready() => {
                "Press Enter to install the update.".to_owned()
            }
            SettingRow::Update => "Updates arrive only during a sync session.".to_owned(),
            SettingRow::About => "Carrel 0.1.0, a writing device.".to_owned(),
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

const HINT: &str = "Up/Down move · Left/Right change · Enter open · Esc editor";

pub struct SettingsPage {
    pub widget: gtk::Box,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    hint: gtk::Label,
    error: gtk::Label,
    state: RefCell<SettingsState>,
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
        let list = ui::list();
        let scroller = ui::scroller(&list);

        let hint = ui::hint(HINT);

        // A failure (an update that would not apply) is the only line outside
        // the list; every other hint belongs to its selected row.
        let error = gtk::Label::new(None);
        error.set_xalign(0.0);
        error.set_wrap(true);
        error.set_margin_bottom(6);
        error.add_css_class("row-note");
        error.add_css_class("error-text");
        error.set_visible(false);

        let column = ui::column(820);
        column.append(&ui::title("Settings"));
        column.append(&error);
        column.append(&scroller);
        column.append(&hint);

        let widget = ui::page();
        widget.append(&column);

        // Screenshot hook (dev/screenshot.sh): start with a later row selected,
        // as if the writer had pressed Down that many times.
        let mut initial = SettingsState::default();
        if let Some(index) = std::env::var("CARREL_SETTINGS_SELECT")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
        {
            initial.select(&settings.borrow(), index);
        }

        let page = Rc::new(Self {
            widget,
            list,
            scroller,
            hint,
            error,
            state: RefCell::new(initial),
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
                {
                    let settings = page.settings.borrow();
                    page.state
                        .borrow_mut()
                        .select(&settings, row.index().max(0) as usize);
                }
                page.activate();
            }
        });

        let weak = Rc::downgrade(self);
        self.widget.connect_map(move |_| {
            if let Some(page) = weak.upgrade() {
                page.load_update();
                page.refresh();
                page.widget.grab_focus();
            }
        });
    }

    fn load_update(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.client.spawn(
            |daemon| daemon.update_status(),
            move |result| {
                if let Some(page) = weak.upgrade() {
                    page.state.borrow_mut().update =
                        UpdateState::from_wire(result.unwrap_or_default());
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
            SettingsEffect::None => {
                // Enter on a value row behaves like the right arrow.
                let row = {
                    let settings = self.settings.borrow();
                    self.state.borrow().row(&settings)
                };
                if row != SettingRow::About {
                    let changed = {
                        let mut settings = self.settings.borrow_mut();
                        self.state.borrow_mut().change(&mut settings, 1)
                    };
                    if changed {
                        if let Err(error) = self.settings.borrow().save(&self.root) {
                            eprintln!("carrel: could not save settings: {error}");
                        }
                        (self.on_change)();
                    }
                }
                self.refresh();
            }
            SettingsEffect::OpenWifi => self.nav.show("wifi"),
            SettingsEffect::OpenSync => self.nav.show("sync"),
            SettingsEffect::ApplyUpdate => self.apply_update(),
        }
    }

    fn apply_update(self: &Rc<Self>) {
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
        self.state.borrow_mut().about = false;
        self.error.set_text(message);
        self.error.set_visible(true);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(5), move || {
            if let Some(page) = weak.upgrade() {
                page.error.set_visible(false);
            }
        });
    }

    fn refresh(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let settings = self.settings.borrow();
        let state = self.state.borrow();
        for (index, row) in rows(&settings).iter().enumerate() {
            // The hint belongs to the selected row only, and sits under it
            // aligned with the row text.
            let note = if index == state.selected {
                state.hint_for(&settings)
            } else {
                String::new()
            };
            let item = match row {
                SettingRow::Theme | SettingRow::DayTheme | SettingRow::NightTheme => {
                    theme_value_row(*row, &settings, &note)
                }
                SettingRow::About => {
                    ui::marked_value_row(setting_name(*row), &state.value(*row, &settings), &note)
                }
                _ => ui::value_row(setting_name(*row), &state.value(*row, &settings), &note),
            };
            // The day and night pickers belong to Automatic: indent them so
            // they read as part of the Theme row.
            if matches!(*row, SettingRow::DayTheme | SettingRow::NightTheme) {
                if let Some(child) = item.child() {
                    child.set_margin_start(16);
                }
            }
            self.list.append(&item);
        }
        ui::select(&self.list, state.selected);
        if let Some(row) = self.list.row_at_index(state.selected as i32) {
            ui::scroll_to_row(&self.scroller, &self.list, &row);
        }
        self.hint.set_text(HINT);
    }
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
        SettingRow::SyncBackends => "Sync backends",
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
        let index = rows(settings)
            .iter()
            .position(|item| *item == row)
            .expect("row is on screen");
        state.select(settings, index);
    }

    #[test]
    fn every_setting_from_the_spec_has_one_row() {
        let automatic: Vec<&str> = rows(&Settings::default())
            .iter()
            .map(|row| setting_name(*row))
            .collect();
        assert_eq!(
            automatic,
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
                "Sync backends",
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
        let names: Vec<&str> = rows(&fixed).iter().map(|row| setting_name(*row)).collect();
        assert_eq!(names.len(), 17);
        assert!(!names.contains(&"Day theme"));
        assert!(!names.contains(&"Night theme"));
    }

    #[test]
    fn values_read_like_the_spec() {
        let settings = Settings::default();
        let state = SettingsState::default();
        assert_eq!(state.value(SettingRow::Theme, &settings), "Automatic");
        assert_eq!(state.value(SettingRow::DayTheme, &settings), "Light");
        assert_eq!(state.value(SettingRow::NightTheme, &settings), "Dark");
        assert_eq!(state.value(SettingRow::Font, &settings), "Serif");
        assert_eq!(state.value(SettingRow::Size, &settings), "19 px");
        assert_eq!(state.value(SettingRow::DailyGoal, &settings), "500 words");
        assert_eq!(state.value(SettingRow::GoalProgress, &settings), "Numbers");
        assert_eq!(state.value(SettingRow::FocusMode, &settings), "Off");
        assert_eq!(state.value(SettingRow::Typewriter, &settings), "Off");
        assert_eq!(state.value(SettingRow::Keyboard, &settings), "US");
        assert_eq!(state.value(SettingRow::Update, &settings), "Up to date");
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

        // A fixed theme needs no explanation.
        settings.theme = ThemeChoice::Mist;
        assert_eq!(state.hint_for(&settings), "");

        // The About sentence is never stale on another row.
        let mut state = SettingsState::default();
        state.select(&settings, 0);
        state.about = true;
        state.move_by(&settings, 1);
        assert!(!state.about);
    }

    #[test]
    fn the_day_and_night_rows_appear_only_while_automatic_is_chosen() {
        let mut settings = Settings::default();
        let mut state = SettingsState::default();
        assert_eq!(rows(&settings).len(), 19);
        state.move_by(&settings, 1);
        assert_eq!(state.row(&settings), SettingRow::DayTheme);
        state.move_by(&settings, 1);
        assert_eq!(state.row(&settings), SettingRow::NightTheme);

        // Two down from Automatic is Font: choosing a fixed theme takes the
        // slot rows away, and the selection lands on the row now there.
        select(&mut state, &settings, SettingRow::Theme);
        state.change(&mut settings, 1);
        assert_eq!(settings.theme, ThemeChoice::Light);
        assert_eq!(rows(&settings).len(), 17);
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
    fn the_update_row_reads_the_daemons_status_string() {
        use crate::daemon::{DaemonClient, FakeDaemon};
        let fake = FakeDaemon::new();
        let settings = Settings::default();
        let mut state = SettingsState {
            update: UpdateState::from_wire(fake.update_status().unwrap()),
            ..SettingsState::default()
        };
        assert!(state.update.ready());
        select(&mut state, &settings, SettingRow::Update);
        assert_eq!(
            state.value(SettingRow::Update, &settings),
            "Update ready (0.2.0) · Apply"
        );
        fake.set_update("none");
        state.update = UpdateState::from_wire(fake.update_status().unwrap());
        assert!(!state.update.ready());
    }

    #[test]
    fn navigation_rows_open_screens_and_update_applies_only_when_ready() {
        let settings = Settings::default();
        let mut state = SettingsState::default();
        select(&mut state, &settings, SettingRow::Wifi);
        assert_eq!(state.activate(&settings), SettingsEffect::OpenWifi);
        select(&mut state, &settings, SettingRow::SyncBackends);
        assert_eq!(state.activate(&settings), SettingsEffect::OpenSync);
        select(&mut state, &settings, SettingRow::Update);
        state.update = UpdateState::from_wire("none");
        assert_eq!(state.activate(&settings), SettingsEffect::None);
        state.update = UpdateState::from_wire("ready:1.0");
        assert!(state.update.ready());
        assert_eq!(state.update.label(), "Update ready (1.0) · Apply");
        assert_eq!(state.activate(&settings), SettingsEffect::ApplyUpdate);
    }
}
