//! First run: two Enters and the page.
//!
//! The image already knows the writer's language, keyboard and time zone, so
//! the welcome screen shows those three answers filled in and one action,
//! "Start writing". Changing an answer opens its list and comes back; the only
//! other step is the optional "Connect sync?".
//!
//! Every answer is saved as soon as it is chosen, so a power cut in the middle
//! of setup does not lose the earlier steps.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::daemon::Backend;
use crate::first_run_defaults;
use crate::locale;
use crate::nav::Navigator;
use crate::settings::{settings_path, Settings};
use crate::sync::SyncPage;
use crate::ui;

const HINTS: [(&str, &str); 3] = [("Enter", "Choose"), ("↑ ↓", "Move"), ("Esc", "Back")];
const WELCOME_HINTS: [(&str, &str); 2] = [("Enter", "Choose"), ("↑ ↓", "Move")];
const START_HINTS: [(&str, &str); 2] = [("Enter", "Start"), ("↑ ↓", "Choose")];
const SYNC_CHOICES: [&str; 4] = ["Not now", "Nostos", "Self-hosted server", "Google Drive"];

/// The last row of the welcome screen is the action, not a value.
const START_WRITING: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstStep {
    Welcome,
    Language,
    Keyboard,
    TimeZone,
    Sync,
}

impl FirstStep {
    pub fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome to Carrel",
            Self::Language => "Language",
            Self::Keyboard => "Keyboard",
            Self::TimeZone => "Time zone",
            Self::Sync => "Connect sync?",
        }
    }

    /// The welcome row this list belongs to, so closing the list comes back to
    /// the row the writer came from.
    fn welcome_row(self) -> usize {
        match self {
            Self::Language => 0,
            Self::Keyboard => 1,
            Self::TimeZone => 2,
            _ => START_WRITING,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstEffect {
    Redraw,
    Finish,
    Pair(Backend),
}

#[derive(Debug, Default)]
pub struct FirstRunState {
    pub step: Option<FirstStep>,
    pub selected: usize,
}

impl FirstRunState {
    pub fn step(&self) -> FirstStep {
        self.step.unwrap_or(FirstStep::Welcome)
    }

    /// The welcome screen starts on its one action, so a writer who is happy
    /// with the answers presses Enter and moves on.
    pub fn start(&mut self) {
        self.step = Some(FirstStep::Welcome);
        self.selected = START_WRITING;
    }

    /// The three prefilled answers and the action, in screen order.
    pub fn welcome_options(&self, settings: &Settings) -> [(&'static str, String); 4] {
        [
            ("Language", settings.language.clone()),
            (
                "Keyboard",
                locale::keyboard_label(&settings.keyboard).to_owned(),
            ),
            ("Time zone", settings.time_zone.clone()),
            ("Start writing", String::new()),
        ]
    }

    /// The rows of an open list. The welcome screen and the sync step have
    /// their own fixed rows.
    pub fn options(&self) -> Vec<String> {
        match self.step() {
            FirstStep::Welcome => Vec::new(),
            FirstStep::Language => locale::LANGUAGES
                .iter()
                .map(|item| (*item).to_owned())
                .collect(),
            FirstStep::Keyboard => locale::KEYBOARDS
                .iter()
                .map(|(_, label)| (*label).to_owned())
                .collect(),
            FirstStep::TimeZone => locale::TIME_ZONES
                .iter()
                .map(|item| (*item).to_owned())
                .collect(),
            FirstStep::Sync => SYNC_CHOICES.iter().map(|item| (*item).to_owned()).collect(),
        }
    }

    /// How many rows the cursor may land on.
    pub fn choice_count(&self) -> usize {
        match self.step() {
            FirstStep::Welcome => START_WRITING + 1,
            FirstStep::Sync => SYNC_CHOICES.len(),
            _ => self.options().len(),
        }
    }

    /// Keep the cursor on the current value when a list opens.
    fn on_list(&mut self, settings: &Settings) {
        self.selected = match self.step() {
            FirstStep::Language => locale::LANGUAGES
                .iter()
                .position(|item| *item == settings.language)
                .unwrap_or(0),
            FirstStep::Keyboard => locale::KEYBOARDS
                .iter()
                .position(|(code, _)| *code == settings.keyboard)
                .unwrap_or(0),
            FirstStep::TimeZone => locale::TIME_ZONES
                .iter()
                .position(|item| *item == settings.time_zone)
                .unwrap_or(0),
            _ => 0,
        };
    }

    fn back_to_welcome(&mut self, row: usize) {
        self.step = Some(FirstStep::Welcome);
        self.selected = row;
    }

    pub fn move_by(&mut self, delta: i32) {
        let len = self.choice_count() as i32;
        if len == 0 {
            return;
        }
        self.selected = ((self.selected as i32 + delta).rem_euclid(len)) as usize;
    }

    /// Enter: open the selected list, start writing, or answer the sync step.
    pub fn choose(&mut self, settings: &mut Settings) -> FirstEffect {
        let step = self.step();
        match step {
            FirstStep::Welcome => {
                if self.selected >= START_WRITING {
                    self.step = Some(FirstStep::Sync);
                    self.selected = 0;
                    return FirstEffect::Redraw;
                }
                self.step = Some(match self.selected {
                    0 => FirstStep::Language,
                    1 => FirstStep::Keyboard,
                    _ => FirstStep::TimeZone,
                });
                self.on_list(settings);
                FirstEffect::Redraw
            }
            FirstStep::Language => {
                let options = locale::LANGUAGES;
                settings.language = options[self.selected.min(options.len() - 1)].to_owned();
                self.back_to_welcome(FirstStep::Language.welcome_row());
                FirstEffect::Redraw
            }
            FirstStep::Keyboard => {
                let options = locale::KEYBOARDS;
                settings.keyboard = options[self.selected.min(options.len() - 1)].0.to_owned();
                self.back_to_welcome(FirstStep::Keyboard.welcome_row());
                FirstEffect::Redraw
            }
            FirstStep::TimeZone => {
                let options = locale::TIME_ZONES;
                settings.time_zone = options[self.selected.min(options.len() - 1)].to_owned();
                self.back_to_welcome(FirstStep::TimeZone.welcome_row());
                FirstEffect::Redraw
            }
            FirstStep::Sync => match self.selected {
                0 => FirstEffect::Finish,
                1 => FirstEffect::Pair(Backend::Nostos),
                2 => FirstEffect::Pair(Backend::SelfHosted),
                _ => FirstEffect::Pair(Backend::GoogleDrive),
            },
        }
    }

    /// Esc: a list goes back to the welcome screen, and so does the sync step.
    pub fn back(&mut self) {
        match self.step() {
            FirstStep::Welcome => {}
            FirstStep::Sync => self.back_to_welcome(START_WRITING),
            step => self.back_to_welcome(step.welcome_row()),
        }
    }
}

pub struct FirstRunPage {
    pub widget: gtk::Overlay,
    screen: Rc<ui::Screen>,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    state: RefCell<FirstRunState>,
    settings: Rc<RefCell<Settings>>,
    root: PathBuf,
    nav: Rc<Navigator>,
    sync: Rc<SyncPage>,
}

impl FirstRunPage {
    pub fn new(
        settings: Rc<RefCell<Settings>>,
        root: PathBuf,
        nav: Rc<Navigator>,
        sync: Rc<SyncPage>,
    ) -> Rc<Self> {
        // A brand-new device has no settings file yet, so the image's own
        // language, keyboard and zone are the first answers. Anything already
        // saved belongs to the writer and is left alone.
        if !settings.borrow().first_run_complete && !settings_path(&root).exists() {
            let defaults = first_run_defaults::detect();
            let mut settings = settings.borrow_mut();
            settings.language = defaults.language;
            settings.keyboard = defaults.keyboard;
            settings.time_zone = defaults.time_zone;
        }

        let screen = ui::Screen::new(FirstStep::Welcome.title());
        screen.set_hints(&WELCOME_HINTS);

        let list = ui::list();
        let scroller = ui::scroller(&list);
        screen.content.append(&scroller);

        let page = Rc::new(Self {
            widget: screen.root.clone(),
            screen,
            list,
            scroller,
            state: RefCell::new(FirstRunState::default()),
            settings,
            root,
            nav,
            sync,
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
        self.widget.connect_map(move |_| {
            if let Some(page) = weak.upgrade() {
                if page.state.borrow().step.is_none() {
                    page.state.borrow_mut().start();
                }
                page.refresh();
                page.screen.focus_page();
            }
        });
    }

    pub fn start(self: &Rc<Self>) {
        self.state.borrow_mut().start();
        self.refresh();
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
                if self.state.borrow().step() != FirstStep::Welcome {
                    self.state.borrow_mut().back();
                    self.refresh();
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
            gdk::Key::Home => {
                self.state.borrow_mut().selected = 0;
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::End => {
                let last = self.state.borrow().choice_count().saturating_sub(1);
                self.state.borrow_mut().selected = last;
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

    fn choose(self: &Rc<Self>) {
        let effect = {
            let mut settings = self.settings.borrow_mut();
            let max = self.state.borrow().choice_count().saturating_sub(1);
            let mut state = self.state.borrow_mut();
            state.selected = state.selected.min(max);
            state.choose(&mut settings)
        };
        if let Err(error) = self.settings.borrow().save(&self.root) {
            eprintln!("carrel: could not save first-run settings: {error}");
        }
        match effect {
            FirstEffect::Redraw => self.refresh(),
            FirstEffect::Finish => self.finish(),
            FirstEffect::Pair(backend) => {
                self.finish_to_sync(backend);
            }
        }
    }

    fn finish(self: &Rc<Self>) {
        self.settings.borrow_mut().first_run_complete = true;
        if let Err(error) = self.settings.borrow().save(&self.root) {
            eprintln!("carrel: could not save first-run settings: {error}");
        }
        self.nav.show_editor();
    }

    fn finish_to_sync(self: &Rc<Self>, backend: Backend) {
        self.settings.borrow_mut().first_run_complete = true;
        if let Err(error) = self.settings.borrow().save(&self.root) {
            eprintln!("carrel: could not save first-run settings: {error}");
        }
        self.sync.open_pairing(backend);
    }

    fn refresh(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let settings = self.settings.borrow();
        let state = self.state.borrow();
        let step = state.step();
        self.screen.set_title(step.title());
        if step == FirstStep::Welcome {
            if state.selected == START_WRITING {
                self.screen.set_hints(&START_HINTS);
            } else {
                self.screen.set_hints(&WELCOME_HINTS);
            }
            for (index, (name, value)) in state.welcome_options(&settings).iter().enumerate() {
                let row = if index == START_WRITING {
                    ui::plain_row(name)
                } else {
                    ui::value_row(name, value, "")
                };
                self.list.append(&row);
            }
        } else {
            self.screen.set_hints(&HINTS);
            for option in state.options() {
                let row = gtk::ListBoxRow::new();
                row.add_css_class("ui-row");
                row.add_css_class("file-row");
                let label = gtk::Label::new(Some(&option));
                label.set_xalign(0.0);
                label.add_css_class("file-name");
                row.set_child(Some(&label));
                self.list.append(&row);
            }
        }
        let selected = state.selected.min(state.choice_count().saturating_sub(1));
        drop(state);
        drop(settings);
        ui::select(&self.list, selected);
        if let Some(row) = self.list.row_at_index(selected as i32) {
            ui::scroll_to_row(&self.scroller, &self.list, &row);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_enters_reach_the_editor_with_the_answers_already_filled_in() {
        let mut settings = Settings::default();
        let mut state = FirstRunState::default();
        state.start();
        assert_eq!(state.step(), FirstStep::Welcome);
        assert_eq!(state.selected, START_WRITING);

        // Enter on "Start writing", then Enter on the default "Not now".
        assert_eq!(state.choose(&mut settings), FirstEffect::Redraw);
        assert_eq!(state.step(), FirstStep::Sync);
        assert_eq!(state.selected, 0);
        assert_eq!(state.choose(&mut settings), FirstEffect::Finish);
        assert_eq!(settings.language, "English");
        assert_eq!(settings.keyboard, "us");
        assert_eq!(settings.time_zone, "Local time");
    }

    #[test]
    fn the_welcome_screen_shows_the_three_answers_and_one_action() {
        let settings = Settings {
            language: "Deutsch".into(),
            keyboard: "de".into(),
            time_zone: "Europe/Berlin".into(),
            ..Settings::default()
        };
        let state = FirstRunState::default();
        let rows = state.welcome_options(&settings);
        assert_eq!(rows[0], ("Language", "Deutsch".to_owned()));
        assert_eq!(rows[1], ("Keyboard", "German".to_owned()));
        assert_eq!(rows[2], ("Time zone", "Europe/Berlin".to_owned()));
        assert_eq!(rows[3], ("Start writing", String::new()));
        assert_eq!(state.choice_count(), 4);
    }

    #[test]
    fn changing_an_answer_opens_its_list_and_comes_back_to_its_row() {
        let mut settings = Settings::default();
        let mut state = FirstRunState::default();
        state.start();

        // Language: the cursor starts on the current answer and choosing
        // returns to the welcome screen, on the Language row.
        state.selected = 0;
        assert_eq!(state.choose(&mut settings), FirstEffect::Redraw);
        assert_eq!(state.step(), FirstStep::Language);
        assert_eq!(state.selected, 0);
        state.selected = 1; // Deutsch
        assert_eq!(state.choose(&mut settings), FirstEffect::Redraw);
        assert_eq!(settings.language, "Deutsch");
        assert_eq!(state.step(), FirstStep::Welcome);
        assert_eq!(state.selected, 0);

        // Keyboard, with the cursor already on US.
        state.selected = 1;
        assert_eq!(state.choose(&mut settings), FirstEffect::Redraw);
        assert_eq!(state.step(), FirstStep::Keyboard);
        assert_eq!(state.selected, 0);
        state.selected = 1; // UK
        assert_eq!(state.choose(&mut settings), FirstEffect::Redraw);
        assert_eq!(settings.keyboard, "gb");
        assert_eq!(state.step(), FirstStep::Welcome);
        assert_eq!(state.selected, 1);

        // Time zone: the cursor starts on Local time.
        state.selected = 2;
        assert_eq!(state.choose(&mut settings), FirstEffect::Redraw);
        assert_eq!(state.step(), FirstStep::TimeZone);
        assert_eq!(state.selected, 0);
        state.selected = 1; // UTC
        assert_eq!(state.choose(&mut settings), FirstEffect::Redraw);
        assert_eq!(settings.time_zone, "UTC");
        assert_eq!(state.step(), FirstStep::Welcome);
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn esc_closes_a_list_or_the_sync_step_and_stays_on_the_welcome_screen() {
        let mut state = FirstRunState::default();
        state.start();
        state.selected = 1;
        state.choose(&mut Settings::default());
        assert_eq!(state.step(), FirstStep::Keyboard);
        state.back();
        assert_eq!(state.step(), FirstStep::Welcome);
        assert_eq!(state.selected, 1);

        state.selected = START_WRITING;
        state.choose(&mut Settings::default());
        assert_eq!(state.step(), FirstStep::Sync);
        state.back();
        assert_eq!(state.step(), FirstStep::Welcome);
        assert_eq!(state.selected, START_WRITING);

        state.back();
        assert_eq!(state.step(), FirstStep::Welcome);
    }

    #[test]
    fn the_sync_step_offers_not_now_first() {
        let mut settings = Settings::default();
        let mut state = FirstRunState {
            step: Some(FirstStep::Sync),
            selected: 0,
        };
        assert_eq!(state.options().len(), 4);
        assert_eq!(state.options()[0], "Not now");
        assert_eq!(state.choice_count(), 4);
        assert_eq!(state.choose(&mut settings), FirstEffect::Finish);
        state.selected = 1;
        assert_eq!(
            state.choose(&mut settings),
            FirstEffect::Pair(Backend::Nostos)
        );
        state.selected = 2;
        assert_eq!(
            state.choose(&mut settings),
            FirstEffect::Pair(Backend::SelfHosted)
        );
        state.selected = 3;
        assert_eq!(
            state.choose(&mut settings),
            FirstEffect::Pair(Backend::GoogleDrive)
        );
    }

    #[test]
    fn the_arrow_keys_skip_nothing_on_the_short_welcome_screen() {
        let mut state = FirstRunState::default();
        state.start();
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(state.selected);
            state.move_by(1);
        }
        assert_eq!(seen, vec![3, 0, 1, 2]);
        state.move_by(-1);
        assert_eq!(state.selected, 2);
    }
}
