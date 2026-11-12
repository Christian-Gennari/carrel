//! First run: Language → Keyboard → Time zone → Connect sync? → Editor.
//!
//! Each answer is saved as soon as it is chosen, so a power cut in the middle
//! of setup does not lose the earlier steps.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::daemon::Backend;
use crate::locale;
use crate::nav::Navigator;
use crate::settings::Settings;
use crate::sync::SyncPage;
use crate::ui;

const HINT: &str = "Up/Down move · Enter choose · Esc back";
const SYNC_CHOICES: [&str; 4] = ["Not now", "Nostos", "Self-hosted server", "Google Drive"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstStep {
    Language,
    Keyboard,
    TimeZone,
    Sync,
}

impl FirstStep {
    pub fn title(self) -> &'static str {
        match self {
            Self::Language => "Language",
            Self::Keyboard => "Keyboard",
            Self::TimeZone => "Time zone",
            Self::Sync => "Connect sync?",
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Language => Self::Language,
            Self::Keyboard => Self::Language,
            Self::TimeZone => Self::Keyboard,
            Self::Sync => Self::TimeZone,
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
        self.step.unwrap_or(FirstStep::Language)
    }

    pub fn options(&self, settings: &Settings) -> Vec<String> {
        match self.step() {
            FirstStep::Language => locale::LANGUAGES
                .iter()
                .map(|item| (*item).to_owned())
                .collect(),
            FirstStep::Keyboard => locale::KEYBOARDS
                .iter()
                .map(|(code, label)| format!("{label} ({code})"))
                .collect(),
            FirstStep::TimeZone => locale::TIME_ZONES
                .iter()
                .map(|item| (*item).to_owned())
                .collect(),
            FirstStep::Sync => {
                let mut rows = Vec::new();
                for choice in SYNC_CHOICES {
                    rows.push(choice.to_owned());
                }
                rows.push(format!(
                    "Language: {} · Keyboard: {} · Time zone: {}",
                    settings.language,
                    locale::keyboard_label(&settings.keyboard),
                    settings.time_zone
                ));
                rows
            }
        }
    }

    /// Keep the cursor on the current value when a step opens.
    pub fn on_step(&mut self, settings: &Settings) {
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
            FirstStep::Sync => 0,
        };
    }

    /// The number of rows the cursor may land on. The sync step's last row is
    /// a summary of the earlier answers, not a fifth choice.
    pub fn choice_count(&self, settings: &Settings) -> usize {
        let count = self.options(settings).len();
        if self.step() == FirstStep::Sync {
            count.saturating_sub(1)
        } else {
            count
        }
    }

    pub fn move_by(&mut self, delta: i32, settings: &Settings) {
        let len = self.choice_count(settings) as i32;
        if len == 0 {
            return;
        }
        self.selected = ((self.selected as i32 + delta).rem_euclid(len)) as usize;
    }

    pub fn choose(&mut self, settings: &mut Settings) -> FirstEffect {
        let step = self.step();
        match step {
            FirstStep::Language => {
                let options = locale::LANGUAGES;
                settings.language = options[self.selected.min(options.len() - 1)].to_owned();
                self.step = Some(FirstStep::Keyboard);
                self.on_step(settings);
                FirstEffect::Redraw
            }
            FirstStep::Keyboard => {
                let options = locale::KEYBOARDS;
                settings.keyboard = options[self.selected.min(options.len() - 1)].0.to_owned();
                self.step = Some(FirstStep::TimeZone);
                self.on_step(settings);
                FirstEffect::Redraw
            }
            FirstStep::TimeZone => {
                let options = locale::TIME_ZONES;
                settings.time_zone = options[self.selected.min(options.len() - 1)].to_owned();
                self.step = Some(FirstStep::Sync);
                self.on_step(settings);
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

    pub fn back(&mut self, settings: &Settings) {
        self.step = Some(self.step().previous());
        self.on_step(settings);
    }
}

pub struct FirstRunPage {
    pub widget: gtk::Box,
    step_title: gtk::Label,
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
        let step_title = gtk::Label::new(None);
        step_title.set_xalign(0.0);
        step_title.add_css_class("step-title");

        let list = ui::list();
        let scroller = ui::scroller(&list);

        let intro = ui::hint("A quiet place to write. Four short answers, then the page.");

        let column = ui::column(640);
        column.append(&ui::title("Carrel"));
        column.append(&intro);
        column.append(&step_title);
        column.append(&scroller);
        column.append(&ui::hint(HINT));

        let widget = ui::page();
        widget.append(&column);

        let page = Rc::new(Self {
            widget,
            step_title,
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
                    let settings = page.settings.borrow();
                    page.state.borrow_mut().on_step(&settings);
                }
                page.refresh();
                page.widget.grab_focus();
            }
        });
    }

    pub fn start(self: &Rc<Self>) {
        let settings = self.settings.borrow();
        let mut state = self.state.borrow_mut();
        state.step = Some(FirstStep::Language);
        state.on_step(&settings);
        drop(state);
        drop(settings);
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
                if self.state.borrow().step() == FirstStep::Language {
                    return glib::Propagation::Stop;
                }
                let settings = self.settings.borrow();
                self.state.borrow_mut().back(&settings);
                drop(settings);
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Up => {
                let settings = self.settings.borrow();
                self.state.borrow_mut().move_by(-1, &settings);
                drop(settings);
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Down => {
                let settings = self.settings.borrow();
                self.state.borrow_mut().move_by(1, &settings);
                drop(settings);
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::Home => {
                self.state.borrow_mut().selected = 0;
                self.refresh();
                glib::Propagation::Stop
            }
            gdk::Key::End => {
                let settings = self.settings.borrow();
                let last = self
                    .state
                    .borrow()
                    .choice_count(&settings)
                    .saturating_sub(1);
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
            let max = self
                .state
                .borrow()
                .choice_count(&settings)
                .saturating_sub(1);
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
        self.step_title.set_text(state.step().title());
        let options = state.options(&settings);
        let sync_step = state.step() == FirstStep::Sync;
        let count = options.len();
        for (index, option) in options.iter().enumerate() {
            let row = gtk::ListBoxRow::new();
            row.add_css_class("file-row");
            let label = gtk::Label::new(Some(option));
            label.set_xalign(0.0);
            label.add_css_class("file-name");
            if sync_step && index == count - 1 {
                label.add_css_class("row-note");
            }
            row.set_child(Some(&label));
            if sync_step && index == count - 1 {
                row.set_selectable(false);
                row.set_activatable(false);
            }
            self.list.append(&row);
        }
        // The summary line at the end of the sync step is not a choice: the
        // cursor stops at the four real options.
        let max = if sync_step {
            count.saturating_sub(2)
        } else {
            count.saturating_sub(1)
        };
        ui::select(&self.list, state.selected.min(max));
        if let Some(row) = self.list.row_at_index(state.selected.min(max) as i32) {
            ui::scroll_to_row(&self.scroller, &self.list, &row);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_order_is_language_keyboard_time_zone_then_sync() {
        let mut settings = Settings::default();
        let mut state = FirstRunState::default();
        state.on_step(&settings);
        assert_eq!(state.step(), FirstStep::Language);

        assert_eq!(state.choose(&mut settings), FirstEffect::Redraw);
        assert_eq!(state.step(), FirstStep::Keyboard);
        assert_eq!(settings.language, "English");

        state.selected = 1; // UK
        assert_eq!(state.choose(&mut settings), FirstEffect::Redraw);
        assert_eq!(state.step(), FirstStep::TimeZone);
        assert_eq!(settings.keyboard, "gb");

        state.selected = 1; // UTC
        assert_eq!(state.choose(&mut settings), FirstEffect::Redraw);
        assert_eq!(state.step(), FirstStep::Sync);
        assert_eq!(settings.time_zone, "UTC");
    }

    #[test]
    fn choosing_a_backend_asks_to_pair_and_not_now_finishes() {
        let mut settings = Settings::default();
        let mut state = FirstRunState {
            step: Some(FirstStep::Sync),
            selected: 0,
        };
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
    fn back_steps_through_the_flow_but_stops_at_language() {
        let settings = Settings::default();
        let mut state = FirstRunState {
            step: Some(FirstStep::TimeZone),
            ..FirstRunState::default()
        };
        state.back(&settings);
        assert_eq!(state.step(), FirstStep::Keyboard);
        state.back(&settings);
        state.back(&settings);
        assert_eq!(state.step(), FirstStep::Language);
    }

    #[test]
    fn the_sync_step_shows_the_answers_above_it() {
        let settings = Settings {
            language: "Deutsch".into(),
            keyboard: "de".into(),
            time_zone: "Europe/Berlin".into(),
            ..Settings::default()
        };
        let state = FirstRunState {
            step: Some(FirstStep::Sync),
            ..FirstRunState::default()
        };
        let options = state.options(&settings);
        assert_eq!(options.len(), 5);
        assert_eq!(options[0], "Not now");
        assert!(options[4].contains("Deutsch"));
        assert!(options[4].contains("German"));
        assert!(options[4].contains("Europe/Berlin"));
    }
}
