//! The Quotes palette (Ctrl+]): search the cached Nostos library, filter by
//! book, author or topic, and insert a block quote with its citation.
//!
//! The pure state below is tested without GTK. The page only turns that state
//! into a search entry, three filter chips and a calm list of results.
//!
//! Enter inserts `> quote` and the citation link as its own paragraph;
//! Shift+Enter inserts the citation link alone. Long passages arrive as a
//! snippet from the daemon, so what the writer sees is what goes into the file.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::daemon::{Client, LibraryFilterOptions, LibraryHit};
use crate::editor::EditorPage;
use crate::nav::Navigator;
use crate::ui;

/// The spec's sentences for a library that is absent or has never synced.
pub const UNAVAILABLE_MESSAGE: &str = "Quotes need a Nostos account.";
pub const EMPTY_MESSAGE: &str = "Sync once to download your library.";
/// Shown when a search or a filter matches nothing.
pub const NO_MATCHES_MESSAGE: &str = "No quotes match.";

pub const HINT: &str = "Type to search · Alt+B book · Alt+A author · Alt+T topic · ↑↓ move · \
     Enter quote · Shift+Enter citation · Esc editor";

/// How long after the last keystroke the palette asks the daemon.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(160);

/// What the palette shows below the filter row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuotesStatus {
    #[default]
    Loading,
    Ready,
    /// The library synced but has no items yet.
    Empty,
    /// The paired server has no Nostos library.
    Unavailable,
}

/// The pure Quotes palette state.
#[derive(Debug, Default)]
pub struct QuotesState {
    pub query: String,
    pub options: LibraryFilterOptions,
    /// 0 means "All"; otherwise an index into `options`.
    pub book: usize,
    pub author: usize,
    pub topic: usize,
    pub hits: Vec<LibraryHit>,
    pub selected: usize,
    pub status: QuotesStatus,
    pub message: String,
    pub error: Option<String>,
}

impl QuotesState {
    /// The filter map sent over D-Bus. Empty values are omitted.
    pub fn filters(&self) -> HashMap<String, String> {
        let mut filters = HashMap::new();
        if let Some(book) = self
            .book
            .checked_sub(1)
            .and_then(|index| self.options.books.get(index))
        {
            filters.insert("book".to_string(), book.id.clone());
        }
        if let Some(author) = self
            .author
            .checked_sub(1)
            .and_then(|index| self.options.authors.get(index))
        {
            filters.insert("author".to_string(), author.clone());
        }
        if let Some(topic) = self
            .topic
            .checked_sub(1)
            .and_then(|index| self.options.topics.get(index))
        {
            filters.insert("topic".to_string(), topic.clone());
        }
        filters
    }

    pub fn has_filters(&self) -> bool {
        self.book > 0 || self.author > 0 || self.topic > 0
    }

    pub fn set_options(&mut self, options: LibraryFilterOptions) {
        self.options = options;
        self.book = self.book.min(self.options.books.len());
        self.author = self.author.min(self.options.authors.len());
        self.topic = self.topic.min(self.options.topics.len());
    }

    /// Cycle a filter chip: All → first value → … → All. Returns true when a
    /// value exists to cycle through.
    pub fn cycle_book(&mut self) -> bool {
        if self.options.books.is_empty() {
            return false;
        }
        self.book = (self.book + 1) % (self.options.books.len() + 1);
        self.selected = 0;
        true
    }

    pub fn cycle_author(&mut self) -> bool {
        if self.options.authors.is_empty() {
            return false;
        }
        self.author = (self.author + 1) % (self.options.authors.len() + 1);
        self.selected = 0;
        true
    }

    pub fn cycle_topic(&mut self) -> bool {
        if self.options.topics.is_empty() {
            return false;
        }
        self.topic = (self.topic + 1) % (self.options.topics.len() + 1);
        self.selected = 0;
        true
    }

    pub fn book_label(&self) -> String {
        match self
            .book
            .checked_sub(1)
            .and_then(|index| self.options.books.get(index))
        {
            Some(book) => format!("Book: {}", book.title),
            None => "Book: All".to_string(),
        }
    }

    pub fn author_label(&self) -> String {
        match self
            .author
            .checked_sub(1)
            .and_then(|index| self.options.authors.get(index))
        {
            Some(author) => format!("Author: {author}"),
            None => "Author: All".to_string(),
        }
    }

    pub fn topic_label(&self) -> String {
        match self
            .topic
            .checked_sub(1)
            .and_then(|index| self.options.topics.get(index))
        {
            Some(topic) => format!("Topic: {topic}"),
            None => "Topic: All".to_string(),
        }
    }

    /// Replace the result list. An empty result is either an empty library or
    /// a search that matched nothing; the two get different sentences.
    pub fn set_hits(&mut self, hits: Vec<LibraryHit>) {
        self.hits = hits;
        self.selected = 0;
        self.error = None;
        if !self.hits.is_empty() {
            self.status = QuotesStatus::Ready;
            self.message.clear();
        } else if self.query.trim().is_empty() && !self.has_filters() {
            self.status = QuotesStatus::Empty;
            self.message = EMPTY_MESSAGE.to_string();
        } else {
            self.status = QuotesStatus::Ready;
            self.message = NO_MATCHES_MESSAGE.to_string();
        }
    }

    /// A daemon error. The spec's sentence gets its own calm state; anything
    /// else (for example `stylosd` not running) is shown as an error line.
    pub fn set_error(&mut self, message: String) {
        self.hits.clear();
        self.selected = 0;
        if message.contains("Nostos account") {
            self.status = QuotesStatus::Unavailable;
            self.message = UNAVAILABLE_MESSAGE.to_string();
            self.error = None;
        } else {
            self.status = QuotesStatus::Ready;
            self.message.clear();
            self.error = Some(message);
        }
    }

    pub fn move_by(&mut self, delta: i32) {
        if self.hits.is_empty() {
            return;
        }
        let len = self.hits.len() as i32;
        self.selected = ((self.selected as i32 + delta).rem_euclid(len)) as usize;
    }

    pub fn current(&self) -> Option<&LibraryHit> {
        self.hits.get(self.selected)
    }

    /// The text Enter/Shift+Enter inserts, or `None` when nothing is selected.
    pub fn insert_text(&self, with_quote: bool) -> Option<String> {
        self.current().map(|hit| format_insert(hit, with_quote))
    }
}

/// The citation link for a hit. The daemon sends ready-to-insert Markdown;
/// when a server omits it, a `nostos:cite/` link keeps the quote traceable.
pub fn citation_for(hit: &LibraryHit) -> String {
    let markdown = hit.citation_markdown.trim();
    if !markdown.is_empty() {
        return markdown.to_string();
    }
    let label = if hit.book_title.trim().is_empty() {
        "Quote".to_string()
    } else {
        hit.book_title.trim().to_string()
    };
    format!("[{label}](nostos:cite/{})", hit.id)
}

/// The quote body, including every line of a multi-line passage.
pub fn quote_for(hit: &LibraryHit) -> String {
    let text = hit.text.trim();
    if text.is_empty() {
        return String::new();
    }
    text.lines()
        .map(|line| {
            let line = line.trim_end();
            if line.is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format one palette insert.
///
/// Enter: the block quote, a blank line (so the citation is not a lazy
/// continuation of the quote), then the citation, then a final newline.
/// Shift+Enter: only the citation and a final newline.
pub fn format_insert(hit: &LibraryHit, with_quote: bool) -> String {
    let citation = citation_for(hit);
    if !with_quote {
        return format!("{citation}\n");
    }
    let quote = quote_for(hit);
    if quote.is_empty() {
        return format!("{citation}\n");
    }
    format!("{quote}\n\n{citation}\n")
}

pub struct QuotesPage {
    pub widget: gtk::Box,
    entry: gtk::Entry,
    book_button: gtk::Button,
    author_button: gtk::Button,
    topic_button: gtk::Button,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    message: gtk::Label,
    hint: gtk::Label,
    state: RefCell<QuotesState>,
    client: Client,
    nav: Rc<Navigator>,
    editor: Rc<EditorPage>,
    debounce_generation: Cell<u64>,
    request_generation: Cell<u64>,
}

impl QuotesPage {
    pub fn new(client: Client, nav: Rc<Navigator>, editor: Rc<EditorPage>) -> Rc<Self> {
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("Search quotes…"));
        entry.set_hexpand(true);
        entry.add_css_class("quote-search");

        let book_button = gtk::Button::with_label("Book: All");
        let author_button = gtk::Button::with_label("Author: All");
        let topic_button = gtk::Button::with_label("Topic: All");
        for button in [&book_button, &author_button, &topic_button] {
            button.add_css_class("filter-chip");
            button.set_can_focus(false);
            button.set_focus_on_click(false);
        }

        let filters = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        filters.append(&book_button);
        filters.append(&author_button);
        filters.append(&topic_button);

        let list = ui::list();
        list.add_css_class("quote-list");
        let scroller = ui::scroller(&list);

        let message = gtk::Label::new(None);
        message.set_xalign(0.0);
        message.set_wrap(true);
        message.add_css_class("quote-message");
        message.set_visible(false);

        let hint = ui::hint(HINT);

        let search_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
        search_box.append(&entry);
        search_box.append(&filters);

        let column = ui::column(820);
        column.append(&ui::title("Quotes"));
        column.append(&search_box);
        // The empty/unavailable sentence sits directly under the search box,
        // where the writer is looking, not at the bottom of an empty list.
        column.append(&message);
        column.append(&scroller);
        column.append(&hint);

        let widget = ui::page();
        widget.append(&column);

        let page = Rc::new(Self {
            widget,
            entry,
            book_button,
            author_button,
            topic_button,
            list,
            scroller,
            message,
            hint,
            state: RefCell::new(QuotesState::default()),
            client,
            nav,
            editor,
            debounce_generation: Cell::new(0),
            request_generation: Cell::new(0),
        });
        page.install();
        // The screenshot script starts the palette with a search already typed.
        if let Ok(query) = std::env::var("STYLOS_QUOTE_QUERY") {
            if !query.trim().is_empty() {
                page.entry.set_text(&query);
                page.state.borrow_mut().query = query;
            }
        }
        page
    }

    fn install(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.entry.connect_changed(move |entry| {
            if let Some(page) = weak.upgrade() {
                let query = entry.text().to_string();
                page.state.borrow_mut().query = query;
                page.schedule_search();
            }
        });

        for (button, kind) in [
            (&self.book_button, FilterKind::Book),
            (&self.author_button, FilterKind::Author),
            (&self.topic_button, FilterKind::Topic),
        ] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.cycle_filter(kind);
                }
            });
        }

        let weak = Rc::downgrade(self);
        self.widget.connect_map(move |_| {
            if let Some(page) = weak.upgrade() {
                page.on_open();
            }
        });
    }

    /// Ctrl+] from any screen, or `STYLOS_START_SCREEN=quotes`.
    pub fn open(self: &Rc<Self>) {
        self.nav.show("quotes");
        self.on_open();
    }

    fn on_open(self: &Rc<Self>) {
        self.entry.grab_focus();
        self.load_options();
        self.search_now();
    }

    fn load_options(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.client.spawn(
            |daemon| daemon.library_filters(),
            move |result| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                match result {
                    Ok(options) => {
                        page.state.borrow_mut().set_options(options);
                        page.refresh_chips();
                    }
                    Err(error) => page.fail(error.message),
                }
            },
        );
    }

    fn schedule_search(self: &Rc<Self>) {
        let generation = self.debounce_generation.get().wrapping_add(1);
        self.debounce_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(SEARCH_DEBOUNCE, move || {
            if let Some(page) = weak.upgrade() {
                if page.debounce_generation.get() == generation {
                    page.search_now();
                }
            }
        });
    }

    fn search_now(self: &Rc<Self>) {
        // An immediate filter change cancels a pending debounced query.
        self.debounce_generation
            .set(self.debounce_generation.get().wrapping_add(1));
        let generation = self.request_generation.get().wrapping_add(1);
        self.request_generation.set(generation);
        let (query, filters) = {
            let state = self.state.borrow();
            (state.query.clone(), state.filters())
        };
        let weak = Rc::downgrade(self);
        self.client.spawn(
            move |daemon| daemon.search_library(&query, filters),
            move |result| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                if page.request_generation.get() != generation {
                    return;
                }
                match result {
                    Ok(hits) => {
                        page.state.borrow_mut().set_hits(hits);
                        page.refresh_results();
                        page.refresh_message();
                    }
                    Err(error) => page.fail(error.message),
                }
            },
        );
    }

    fn fail(self: &Rc<Self>, message: String) {
        self.state.borrow_mut().set_error(message);
        self.refresh_results();
        self.refresh_message();
    }

    fn cycle_filter(self: &Rc<Self>, kind: FilterKind) {
        let changed = {
            let mut state = self.state.borrow_mut();
            match kind {
                FilterKind::Book => state.cycle_book(),
                FilterKind::Author => state.cycle_author(),
                FilterKind::Topic => state.cycle_topic(),
            }
        };
        self.refresh_chips();
        if changed {
            self.search_now();
        }
    }

    fn refresh_chips(&self) {
        let state = self.state.borrow();
        self.book_button.set_label(&state.book_label());
        self.author_button.set_label(&state.author_label());
        self.topic_button.set_label(&state.topic_label());
        set_chip_active(&self.book_button, state.book > 0);
        set_chip_active(&self.author_button, state.author > 0);
        set_chip_active(&self.topic_button, state.topic > 0);
    }

    fn refresh_results(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let state = self.state.borrow();
        for hit in &state.hits {
            let row = result_row(hit);
            self.list.append(&row);
        }
        let selected = state.selected;
        drop(state);
        ui::select(&self.list, selected);
        if let Some(row) = self.list.row_at_index(selected as i32) {
            ui::scroll_to_row(&self.scroller, &self.list, &row);
        }
    }

    fn refresh_message(&self) {
        let state = self.state.borrow();
        let text = match (&state.error, state.status) {
            (Some(error), _) => Some(error.clone()),
            (None, QuotesStatus::Unavailable) => Some(UNAVAILABLE_MESSAGE.to_string()),
            (None, QuotesStatus::Empty) => Some(EMPTY_MESSAGE.to_string()),
            (None, QuotesStatus::Ready) if !state.message.is_empty() => Some(state.message.clone()),
            _ => None,
        };
        match text {
            Some(text) => {
                self.message.set_text(&text);
                self.message.set_visible(true);
                if state.error.is_some() {
                    self.message.add_css_class("error-text");
                    self.hint.add_css_class("error-text");
                } else {
                    self.message.remove_css_class("error-text");
                    self.hint.remove_css_class("error-text");
                }
            }
            None => self.message.set_visible(false),
        }
    }

    pub(crate) fn handle_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        if state.contains(gdk::ModifierType::CONTROL_MASK) {
            // Ctrl+] is handled by the window so it can toggle the palette.
            return glib::Propagation::Proceed;
        }
        if state.contains(gdk::ModifierType::ALT_MASK) {
            let handled = match keyval.to_lower() {
                gdk::Key::b => {
                    self.cycle_filter(FilterKind::Book);
                    true
                }
                gdk::Key::a => {
                    self.cycle_filter(FilterKind::Author);
                    true
                }
                gdk::Key::t => {
                    self.cycle_filter(FilterKind::Topic);
                    true
                }
                _ => false,
            };
            return if handled {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            };
        }

        match keyval {
            gdk::Key::Escape => {
                self.nav.show_editor();
                glib::Propagation::Stop
            }
            gdk::Key::Up => {
                self.state.borrow_mut().move_by(-1);
                self.refresh_results();
                glib::Propagation::Stop
            }
            gdk::Key::Down => {
                self.state.borrow_mut().move_by(1);
                self.refresh_results();
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                let with_quote = !state.contains(gdk::ModifierType::SHIFT_MASK);
                self.insert(with_quote);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn insert(self: &Rc<Self>, with_quote: bool) {
        let text = self.state.borrow().insert_text(with_quote);
        let Some(text) = text else {
            return;
        };
        match self.editor.insert_block(&text) {
            Ok(()) => self.nav.show_editor(),
            Err(message) => self.fail(message),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum FilterKind {
    Book,
    Author,
    Topic,
}

fn set_chip_active(button: &gtk::Button, active: bool) {
    if active {
        button.add_css_class("filter-chip-active");
    } else {
        button.remove_css_class("filter-chip-active");
    }
}

fn result_row(hit: &LibraryHit) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("quote-row");

    let text = gtk::Label::new(Some(&hit.text));
    text.set_xalign(0.0);
    text.set_wrap(true);
    text.set_wrap_mode(pango::WrapMode::WordChar);
    text.set_max_width_chars(64);
    text.add_css_class("quote-text");

    let mut meta_parts = Vec::new();
    if !hit.kind_label().is_empty() {
        meta_parts.push(hit.kind_label().to_string());
    }
    if !hit.book_title.trim().is_empty() {
        meta_parts.push(hit.book_title.trim().to_string());
    }
    if !hit.author.trim().is_empty() {
        meta_parts.push(hit.author.trim().to_string());
    }
    if !hit.location.trim().is_empty() {
        meta_parts.push(hit.location.trim().to_string());
    }
    let meta = gtk::Label::new(Some(&meta_parts.join(" · ")));
    meta.set_xalign(0.0);
    meta.set_ellipsize(pango::EllipsizeMode::End);
    meta.add_css_class("quote-meta");

    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.append(&text);
    column.append(&meta);
    row.set_child(Some(&column));
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::LibraryBookOption;
    use std::time::Instant;

    fn hit(id: &str, text: &str) -> LibraryHit {
        LibraryHit {
            id: id.into(),
            kind: "highlight".into(),
            text: text.into(),
            book_title: "The Quiet Hours".into(),
            author: "Mara Ellison".into(),
            location: "p. 41".into(),
            citation_markdown: format!("[The Quiet Hours, p. 41](nostos:cite/{id})"),
        }
    }

    fn options() -> LibraryFilterOptions {
        LibraryFilterOptions {
            books: vec![
                LibraryBookOption {
                    id: "b1".into(),
                    title: "The Quiet Hours".into(),
                    author: "Mara Ellison".into(),
                },
                LibraryBookOption {
                    id: "b2".into(),
                    title: "On Attention".into(),
                    author: "Iris Vale".into(),
                },
            ],
            authors: vec!["Iris Vale".into(), "Mara Ellison".into()],
            topics: vec!["attention".into(), "writing".into()],
        }
    }

    #[test]
    fn enter_formats_a_block_quote_and_its_citation() {
        let item = hit("q1", "A sentence is a small room.");
        assert_eq!(
            format_insert(&item, true),
            "> A sentence is a small room.\n\n\
             [The Quiet Hours, p. 41](nostos:cite/q1)\n"
        );
    }

    #[test]
    fn shift_enter_formats_only_the_citation_link() {
        let item = hit("q1", "A sentence is a small room.");
        assert_eq!(
            format_insert(&item, false),
            "[The Quiet Hours, p. 41](nostos:cite/q1)\n"
        );
    }

    #[test]
    fn every_line_of_a_multi_line_quote_is_a_block_quote_line() {
        let item = hit("q2", "First line.\nSecond line.\n\nFourth line.");
        assert_eq!(
            format_insert(&item, true),
            "> First line.\n> Second line.\n>\n> Fourth line.\n\n\
             [The Quiet Hours, p. 41](nostos:cite/q2)\n"
        );
    }

    #[test]
    fn a_missing_citation_falls_back_to_a_nostos_cite_link() {
        let mut item = hit("q3", "Text.");
        item.citation_markdown.clear();
        item.book_title.clear();
        assert_eq!(citation_for(&item), "[Quote](nostos:cite/q3)");
        item.book_title = "On Attention".into();
        assert_eq!(citation_for(&item), "[On Attention](nostos:cite/q3)");
    }

    #[test]
    fn filter_chips_cycle_and_become_dbus_filters() {
        let mut state = QuotesState::default();
        state.set_options(options());
        assert!(state.filters().is_empty());
        assert_eq!(state.book_label(), "Book: All");

        assert!(state.cycle_book());
        assert_eq!(state.book_label(), "Book: The Quiet Hours");
        assert_eq!(state.filters().get("book").map(String::as_str), Some("b1"));
        assert!(state.cycle_book());
        assert_eq!(state.filters().get("book").map(String::as_str), Some("b2"));
        assert!(state.cycle_book());
        assert!(state.filters().is_empty(), "All comes back around");

        assert!(state.cycle_author());
        assert_eq!(
            state.filters().get("author").map(String::as_str),
            Some("Iris Vale")
        );
        assert!(state.cycle_topic());
        assert_eq!(
            state.filters().get("topic").map(String::as_str),
            Some("attention")
        );
        assert!(state.has_filters());
    }

    #[test]
    fn the_palette_moves_through_results_and_wraps() {
        let mut state = QuotesState::default();
        state.set_hits(vec![hit("q1", "One"), hit("q2", "Two"), hit("q3", "Three")]);
        assert_eq!(state.current().unwrap().id, "q1");
        state.move_by(1);
        assert_eq!(state.current().unwrap().id, "q2");
        state.move_by(1);
        state.move_by(1);
        assert_eq!(state.current().unwrap().id, "q1", "Down wraps");
        state.move_by(-1);
        assert_eq!(state.current().unwrap().id, "q3", "Up wraps");
        assert_eq!(state.insert_text(true).unwrap().lines().count(), 3);
    }

    #[test]
    fn empty_and_unavailable_libraries_get_the_spec_sentences() {
        let mut state = QuotesState::default();
        state.set_hits(Vec::new());
        assert_eq!(state.status, QuotesStatus::Empty);
        assert_eq!(state.message, EMPTY_MESSAGE);

        let mut state = QuotesState::default();
        state.set_error(UNAVAILABLE_MESSAGE.to_string());
        assert_eq!(state.status, QuotesStatus::Unavailable);
        assert_eq!(state.message, UNAVAILABLE_MESSAGE);
        assert!(state.error.is_none());

        let mut state = QuotesState::default();
        state.set_error("stylosd is not running.".into());
        assert_eq!(state.status, QuotesStatus::Ready);
        assert_eq!(state.error.as_deref(), Some("stylosd is not running."));

        let mut state = QuotesState {
            query: "zzz".into(),
            ..QuotesState::default()
        };
        state.set_hits(Vec::new());
        assert_eq!(state.status, QuotesStatus::Ready);
        assert_eq!(state.message, NO_MATCHES_MESSAGE);
    }

    #[test]
    fn five_quotes_take_far_less_than_copy_paste_time() {
        let hits: Vec<LibraryHit> = (0..5)
            .map(|index| {
                hit(
                    &format!("q{index}"),
                    &format!("The sentence number {index} is a small room."),
                )
            })
            .collect();
        let started = Instant::now();
        let mut inserted = String::new();
        for item in &hits {
            inserted.push_str(&format_insert(item, true));
        }
        let elapsed = started.elapsed();
        assert_eq!(inserted.matches("nostos:cite/").count(), 5);
        assert!(
            elapsed < Duration::from_secs(1),
            "five inserts took {elapsed:?}"
        );
    }
}
