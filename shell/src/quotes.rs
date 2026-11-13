//! The Quotes palette: type `@` at the start of a word (or press Ctrl+') and a
//! small list opens at the caret. What is typed after `@` searches the cached
//! Nostos library; Enter replaces `@query` with the chosen quote and its
//! citation.
//!
//! This is the Nostos Writing Studio's source palette, so both products open
//! and close it the same way. The trigger rules are in `quote_trigger`; the
//! pure palette state and the insert formats are below and are tested without
//! GTK. The popover only turns that state into a list. The caret never leaves
//! the editor: the editor text changes only when a source is inserted, and one
//! undo brings `@query` back.
//!
//! Enter inserts `> quote` and the citation link as its own paragraph;
//! Shift+Enter and Ctrl+Enter insert the citation link alone. Long passages
//! arrive as a snippet from the daemon, so what the writer sees is what goes
//! into the file.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::daemon::{Client, LibraryHit};
use crate::editor::EditorPage;
use crate::quote_trigger::{self, Trigger};

/// The spec's sentences for a library that is absent or has never synced.
pub const UNAVAILABLE_MESSAGE: &str = "Quotes need a Nostos account.";
pub const EMPTY_MESSAGE: &str = "Sync once to download your library.";
/// Shown while the first answer is on its way.
pub const SEARCHING_MESSAGE: &str = "Searching…";
/// The heading above the list for a bare `@`: the library's newest items.
pub const RECENT_HEADING: &str = "Recent";

/// The key hints under the list, in the same key-cap bar as every screen.
const HINTS: [(&str, &str); 3] = [
    ("Enter", "Quote"),
    ("Shift+Enter", "Citation"),
    ("Esc", "Close"),
];

/// How long after the last keystroke the palette asks the daemon.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);
/// The popover's width in pixels.
const POPOVER_WIDTH: i32 = 560;
/// The tallest the result list grows before it scrolls.
const LIST_MAX_HEIGHT: i32 = 320;

/// What the palette shows below the heading.
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
    pub hits: Vec<LibraryHit>,
    pub selected: usize,
    pub status: QuotesStatus,
    pub message: String,
    pub error: Option<String>,
    /// An answer for the current query is on its way.
    pub searching: bool,
}

impl QuotesState {
    /// A new query. The old results stay on screen until the answer comes, so
    /// the list does not flicker while the writer types.
    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.searching = true;
    }

    /// Replace the result list. An empty result is either an empty library or
    /// a search that matched nothing; the two get different sentences.
    pub fn set_hits(&mut self, hits: Vec<LibraryHit>) {
        self.hits = hits;
        self.selected = 0;
        self.error = None;
        self.searching = false;
        if !self.hits.is_empty() {
            self.status = QuotesStatus::Ready;
            self.message.clear();
        } else if self.query.trim().is_empty() {
            self.status = QuotesStatus::Empty;
            self.message = EMPTY_MESSAGE.to_string();
        } else {
            self.status = QuotesStatus::Ready;
            self.message = no_matches_message(&self.query);
        }
    }

    /// A daemon error. The spec's sentence gets its own calm state; anything
    /// else (for example `carreld` not running) is shown as an error line.
    pub fn set_error(&mut self, message: String) {
        self.hits.clear();
        self.selected = 0;
        self.searching = false;
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

    /// The line above the list: what a bare `@` shows, or that an answer is
    /// still coming while older results are on screen.
    pub fn heading(&self) -> &'static str {
        if self.hits.is_empty() {
            ""
        } else if self.searching {
            SEARCHING_MESSAGE
        } else if self.query.trim().is_empty() {
            RECENT_HEADING
        } else {
            ""
        }
    }

    /// The sentence that stands in for an empty list, if any.
    pub fn notice(&self) -> Option<String> {
        if let Some(error) = &self.error {
            return Some(error.clone());
        }
        match self.status {
            QuotesStatus::Loading => Some(SEARCHING_MESSAGE.to_string()),
            QuotesStatus::Unavailable => Some(UNAVAILABLE_MESSAGE.to_string()),
            QuotesStatus::Empty => Some(EMPTY_MESSAGE.to_string()),
            QuotesStatus::Ready if !self.message.is_empty() => Some(self.message.clone()),
            QuotesStatus::Ready => None,
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

/// What a search that matched nothing says.
pub fn no_matches_message(query: &str) -> String {
    format!("No quotes match “{}”.", query.trim())
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

/// An open `@` query in the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Active {
    /// Buffer offset of the `@`.
    start: i32,
    query: String,
}

/// The palette: a popover at the caret that follows the editor's text.
pub struct QuotePalette {
    popover: gtk::Popover,
    heading: gtk::Label,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    message: gtk::Label,
    hint: gtk::Box,
    /// The footer while a state is shown: only Esc works with no results.
    message_hint: gtk::Box,
    view: sourceview::View,
    buffer: sourceview::Buffer,
    editor: Rc<EditorPage>,
    client: Client,
    state: RefCell<QuotesState>,
    active: RefCell<Option<Active>>,
    /// The `@` that Esc closed. It stays closed until that `@` stops being a
    /// trigger, so typing on does not open it again.
    dismissed: Cell<Option<i32>>,
    /// The `@` that Ctrl+' typed, and how many characters it typed. Closing
    /// with nothing after it takes them back.
    typed_by_shortcut: Cell<Option<(i32, usize)>>,
    /// Only a key press may open the palette, never a file loading.
    armed: Cell<bool>,
    /// Our own edits must not reopen or close the palette.
    editing: Cell<bool>,
    /// Enter pressed while the answer was on its way: whether to quote.
    pending: Cell<Option<bool>>,
    debounce_generation: Cell<u64>,
    request_generation: Cell<u64>,
}

impl QuotePalette {
    pub fn new(client: Client, editor: Rc<EditorPage>) -> Rc<Self> {
        let view = editor.view();
        let buffer = editor.buffer();

        let heading = gtk::Label::new(None);
        heading.set_xalign(0.0);
        heading.add_css_class("quote-heading");
        heading.set_visible(false);

        let list = crate::ui::list();
        list.add_css_class("quote-list");
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(LIST_MAX_HEIGHT)
            .child(&list)
            .build();
        scroller.set_can_focus(false);

        let message = gtk::Label::new(None);
        message.set_xalign(0.0);
        message.set_wrap(true);
        message.set_max_width_chars(60);
        message.add_css_class("quote-message");
        message.set_visible(false);

        let hint = crate::ui::hint_bar(&HINTS);
        hint.add_css_class("card-hint-bar");
        let message_hint = crate::ui::hint_bar(&[("Esc", "Close")]);
        message_hint.add_css_class("card-hint-bar");
        message_hint.set_visible(false);

        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.set_width_request(POPOVER_WIDTH);
        column.append(&heading);
        column.append(&message);
        column.append(&scroller);
        column.append(&hint);
        column.append(&message_hint);

        let popover = gtk::Popover::new();
        popover.add_css_class("quote-popover");
        popover.set_child(Some(&column));
        popover.set_parent(&view);
        popover.set_autohide(false);
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_can_focus(false);
        popover.set_focusable(false);
        // Keyboard only: the pointer never reaches the palette, and the caret
        // never leaves the editor.
        popover.set_can_target(false);

        let palette = Rc::new(Self {
            popover,
            heading,
            list,
            scroller,
            message,
            hint,
            message_hint,
            view,
            buffer,
            editor,
            client,
            state: RefCell::new(QuotesState::default()),
            active: RefCell::new(None),
            dismissed: Cell::new(None),
            typed_by_shortcut: Cell::new(None),
            armed: Cell::new(false),
            editing: Cell::new(false),
            pending: Cell::new(None),
            debounce_generation: Cell::new(0),
            request_generation: Cell::new(0),
        });
        palette.install();
        palette
    }

    fn install(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.buffer.connect_changed(move |_| {
            if let Some(palette) = weak.upgrade() {
                palette.refresh();
            }
        });

        let weak = Rc::downgrade(self);
        self.buffer.connect_mark_set(move |_, _, mark| {
            let is_insert = mark
                .name()
                .map(|name| name.as_str() == "insert")
                .unwrap_or(false);
            if !is_insert {
                return;
            }
            if let Some(palette) = weak.upgrade() {
                palette.refresh();
            }
        });

        // Leaving the editor (Ctrl+O, Ctrl+H, …) closes the palette.
        let weak = Rc::downgrade(self);
        self.view.connect_unmap(move |_| {
            if let Some(palette) = weak.upgrade() {
                palette.close();
            }
        });

        // Typewriter scrolling moves the text under the caret; the popover
        // follows it.
        if let Some(adjustment) = self.view.vadjustment() {
            let weak = Rc::downgrade(self);
            adjustment.connect_value_changed(move |_| {
                if let Some(palette) = weak.upgrade() {
                    palette.place();
                }
            });
        }
    }

    pub fn is_open(&self) -> bool {
        self.active.borrow().is_some()
    }

    /// Step aside for the command palette. A `@` typed by Ctrl+' with nothing
    /// after it is taken back, exactly as Esc does.
    pub(crate) fn close_from_outside(self: &Rc<Self>) {
        self.close();
    }

    /// How many results the open palette lists.
    #[cfg(test)]
    pub(crate) fn result_count(&self) -> usize {
        self.state.borrow().hits.len()
    }

    // -- Opening, following and closing ------------------------------------

    /// Look at the text before the caret and open, update or close.
    fn refresh(self: &Rc<Self>) {
        if self.editing.get() {
            return;
        }
        let trigger = self.trigger_at_caret();
        let active = trigger.map(|(start, trigger)| Active {
            start,
            query: trigger.query,
        });
        // Esc closed this one; it stays closed until it is no longer a trigger.
        match &active {
            Some(found) if self.dismissed.get() == Some(found.start) => return,
            Some(_) => {}
            None => self.dismissed.set(None),
        }
        let previous = self.active.borrow().clone();
        match (previous, active) {
            (None, None) => {}
            (Some(_), None) => self.close(),
            (None, Some(found)) => {
                if self.armed.get() {
                    self.open(found);
                }
            }
            (Some(old), Some(found)) => {
                if old.start != found.start {
                    self.close();
                    if self.armed.get() {
                        self.open(found);
                    }
                } else if old.query != found.query {
                    self.follow(found);
                }
            }
        }
    }

    /// The trigger the caret is in, with the buffer offset of its `@`.
    fn trigger_at_caret(&self) -> Option<(i32, Trigger)> {
        if self.buffer.has_selection() {
            return None;
        }
        let cursor = self.buffer.iter_at_mark(&self.buffer.get_insert());
        let mut line_start = cursor;
        line_start.set_line_offset(0);
        let before = self.buffer.text(&line_start, &cursor, false);
        // Cheap checks first: most keys are nowhere near an `@`.
        if !before.contains('@') {
            return None;
        }
        let trigger = quote_trigger::find(&before, false)?;
        let text = self.buffer.text(&self.buffer.start_iter(), &cursor, false);
        let offset = text.chars().count();
        if quote_trigger::in_code(&text, offset) {
            return None;
        }
        Some((line_start.offset() + trigger.start as i32, trigger))
    }

    fn open(self: &Rc<Self>, active: Active) {
        *self.active.borrow_mut() = Some(active.clone());
        {
            let mut state = self.state.borrow_mut();
            *state = QuotesState::default();
            state.set_query(&active.query);
        }
        self.pending.set(None);
        self.render();
        self.place();
        self.popover.popup();
        self.search_now();
    }

    fn follow(self: &Rc<Self>, active: Active) {
        self.state.borrow_mut().set_query(&active.query);
        *self.active.borrow_mut() = Some(active);
        self.render();
        self.place();
        self.schedule_search();
    }

    /// Close without touching the text, except for the `@` Ctrl+' typed when
    /// nothing came after it.
    fn close(self: &Rc<Self>) {
        let active = self.active.borrow_mut().take();
        self.popover.popdown();
        self.pending.set(None);
        // Answers still on their way are for a palette that is gone.
        self.request_generation
            .set(self.request_generation.get().wrapping_add(1));
        self.debounce_generation
            .set(self.debounce_generation.get().wrapping_add(1));
        if let (Some(active), Some((start, length))) = (active, self.typed_by_shortcut.take()) {
            if active.query.is_empty() && active.start + 1 == start + length as i32 {
                self.drop_typed_at(start, length);
            }
        }
    }

    /// Take back the characters Ctrl+' typed, if they are still there.
    fn drop_typed_at(self: &Rc<Self>, start: i32, length: usize) {
        let from = self.buffer.iter_at_offset(start);
        let to = self.buffer.iter_at_offset(start + length as i32);
        let text = self.buffer.text(&from, &to, false);
        if text.as_str() != " @" && text.as_str() != "@" {
            return;
        }
        let mut from = from;
        let mut to = to;
        self.editing.set(true);
        self.buffer.delete(&mut from, &mut to);
        self.editing.set(false);
    }

    /// Esc: close and leave the typed text as it is.
    fn dismiss(self: &Rc<Self>) {
        let start = self.active.borrow().as_ref().map(|active| active.start);
        self.dismissed.set(start);
        self.close();
    }

    /// Ctrl+': open the palette at the caret by typing the `@` for the writer.
    pub(crate) fn open_at_caret(self: &Rc<Self>) {
        if self.is_open() {
            return;
        }
        // Never replace a selection: the query starts after it.
        let mut at = self.buffer.iter_at_mark(&self.buffer.get_insert());
        if let Some((_, end)) = self.buffer.selection_bounds() {
            at = end;
        }
        let mut line_start = at;
        line_start.set_line_offset(0);
        let before = self.buffer.text(&line_start, &at, false);
        let text = self.buffer.text(&self.buffer.start_iter(), &at, false);
        if quote_trigger::in_code(&text, text.chars().count()) {
            return;
        }
        let last = before.chars().last();
        let needs_space = last.is_some_and(|c| !c.is_whitespace() && !quote_trigger::is_prefix(c));
        let typed = if needs_space { " @" } else { "@" };
        let start = at.offset();
        // Moving the caret to the end of a selection must not reopen a query
        // the caret passes on the way.
        self.editing.set(true);
        self.buffer.place_cursor(&at);
        self.editing.set(false);
        self.armed.set(true);
        self.typed_by_shortcut
            .set(Some((start, typed.chars().count())));
        self.buffer.insert_at_cursor(typed);
        self.view.scroll_mark_onscreen(&self.buffer.get_insert());
    }

    /// For screenshots and tests: type `text` at the end of the document as a
    /// new paragraph, as a writer's keys would.
    pub fn type_at_end(self: &Rc<Self>, text: &str) {
        self.armed.set(true);
        let mut end = self.buffer.end_iter();
        self.buffer.place_cursor(&end);
        self.buffer.insert(&mut end, text);
        self.armed.set(false);
    }

    // -- Keys ---------------------------------------------------------------

    /// Keys on the editor screen. Ctrl+' opens the palette; while it is open
    /// Up, Down, Enter and Esc belong to it and everything else is typing.
    pub fn handle_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        let control = state.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = state.contains(gdk::ModifierType::ALT_MASK);

        if control && !alt && keyval == gdk::Key::apostrophe {
            self.open_at_caret();
            return glib::Propagation::Stop;
        }

        // A key press is what lets the palette open from the text it changes;
        // the permission ends when this key has been handled.
        self.armed.set(true);
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(palette) = weak.upgrade() {
                palette.armed.set(false);
            }
        });

        if !self.is_open() || alt {
            return glib::Propagation::Proceed;
        }
        match keyval {
            gdk::Key::Escape => {
                self.dismiss();
                glib::Propagation::Stop
            }
            gdk::Key::Up | gdk::Key::Down => {
                if self.state.borrow().hits.is_empty() {
                    return glib::Propagation::Proceed;
                }
                self.state
                    .borrow_mut()
                    .move_by(if keyval == gdk::Key::Up { -1 } else { 1 });
                self.render_list();
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                // Shift+Enter and Ctrl+Enter insert the citation alone.
                let with_quote = !state
                    .intersects(gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::CONTROL_MASK);
                self.choose(with_quote)
            }
            _ => glib::Propagation::Proceed,
        }
    }

    /// Insert the selected source in place of `@query`. With no answer yet,
    /// the choice waits for it; with an empty answer, Enter is an ordinary
    /// Enter.
    fn choose(self: &Rc<Self>, with_quote: bool) -> glib::Propagation {
        let (searching, empty) = {
            let state = self.state.borrow();
            (state.searching, state.hits.is_empty())
        };
        if searching {
            self.pending.set(Some(with_quote));
            return glib::Propagation::Stop;
        }
        if empty {
            return glib::Propagation::Proceed;
        }
        self.insert(with_quote);
        glib::Propagation::Stop
    }

    fn insert(self: &Rc<Self>, with_quote: bool) {
        let text = self.state.borrow().insert_text(with_quote);
        let start = self.active.borrow().as_ref().map(|active| active.start);
        let (Some(text), Some(start)) = (text, start) else {
            return;
        };
        let end = self.buffer.iter_at_mark(&self.buffer.get_insert()).offset();
        // The palette is done: nothing it does next may close it a second time
        // or take back a typed `@`.
        self.typed_by_shortcut.set(None);
        self.editing.set(true);
        self.close();
        let result = self.editor.replace_with_block(Some((start, end)), &text);
        self.editing.set(false);
        if let Err(message) = result {
            self.editor.flash_note(&message, 4);
        }
    }

    // -- Searching ----------------------------------------------------------

    fn schedule_search(self: &Rc<Self>) {
        let generation = self.debounce_generation.get().wrapping_add(1);
        self.debounce_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(SEARCH_DEBOUNCE, move || {
            if let Some(palette) = weak.upgrade() {
                if palette.debounce_generation.get() == generation {
                    palette.search_now();
                }
            }
        });
    }

    fn search_now(self: &Rc<Self>) {
        self.debounce_generation
            .set(self.debounce_generation.get().wrapping_add(1));
        let generation = self.request_generation.get().wrapping_add(1);
        self.request_generation.set(generation);
        let query = self.state.borrow().query.clone();
        let weak = Rc::downgrade(self);
        self.client.spawn(
            move |daemon| daemon.search_library(query.trim(), HashMap::new()),
            move |result| {
                let Some(palette) = weak.upgrade() else {
                    return;
                };
                if palette.request_generation.get() != generation {
                    return;
                }
                match result {
                    Ok(hits) => palette.state.borrow_mut().set_hits(hits),
                    Err(error) => palette.state.borrow_mut().set_error(error.message),
                }
                palette.render();
                palette.place();
                if let Some(with_quote) = palette.pending.take() {
                    if !palette.state.borrow().hits.is_empty() {
                        palette.insert(with_quote);
                    }
                }
            },
        );
    }

    // -- Drawing ------------------------------------------------------------

    fn render(self: &Rc<Self>) {
        self.render_list();
        let state = self.state.borrow();
        let heading = state.heading();
        self.heading.set_text(heading);
        self.heading.set_visible(!heading.is_empty());
        match (state.hits.is_empty(), state.notice()) {
            (true, Some(text)) => {
                self.message.set_text(&text);
                self.message.set_visible(true);
                if state.error.is_some() {
                    self.message.add_css_class("error-text");
                } else {
                    self.message.remove_css_class("error-text");
                }
            }
            _ => self.message.set_visible(false),
        }
        // A state keeps a footer, so the card never ends bare: Esc closes it.
        self.scroller.set_visible(!state.hits.is_empty());
        self.hint.set_visible(!state.hits.is_empty());
        self.message_hint.set_visible(state.hits.is_empty());
    }

    fn render_list(&self) {
        let state = self.state.borrow();
        // Rebuild the rows only when the results changed; Up and Down only
        // move the selection.
        if self.list.observe_children().n_items() as usize != state.hits.len()
            || state.hits.iter().enumerate().any(|(index, hit)| {
                self.list
                    .row_at_index(index as i32)
                    .map(|row| row.widget_name().as_str() != hit.id)
                    .unwrap_or(true)
            })
        {
            while let Some(child) = self.list.first_child() {
                self.list.remove(&child);
            }
            for hit in &state.hits {
                let row = result_row(hit);
                row.set_widget_name(&hit.id);
                self.list.append(&row);
            }
        }
        let selected = state.selected;
        drop(state);
        crate::ui::select(&self.list, selected);
        if let Some(row) = self.list.row_at_index(selected as i32) {
            crate::ui::scroll_to_row(&self.scroller, &self.list, &row);
        }
    }

    /// Point the popover at the `@`, in the view's own coordinates.
    fn place(&self) {
        let Some(active) = self.active.borrow().clone() else {
            return;
        };
        let iter = self.buffer.iter_at_offset(active.start);
        let rect = self.view.iter_location(&iter);
        let (x, y) =
            self.view
                .buffer_to_window_coords(gtk::TextWindowType::Widget, rect.x(), rect.y());
        self.popover.set_pointing_to(Some(&gdk::Rectangle::new(
            x,
            y,
            rect.width().max(1),
            rect.height(),
        )));
    }
}

fn result_row(hit: &LibraryHit) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("ui-row");
    row.add_css_class("quote-row");

    let text = gtk::Label::new(Some(&hit.text));
    text.set_xalign(0.0);
    text.set_wrap(true);
    text.set_wrap_mode(pango::WrapMode::WordChar);
    text.set_lines(2);
    text.set_ellipsize(pango::EllipsizeMode::End);
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

    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.append(&text);
    column.append(&meta);
    row.set_child(Some(&column));
    row
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(state.notice().as_deref(), Some(EMPTY_MESSAGE));

        let mut state = QuotesState::default();
        state.set_error(UNAVAILABLE_MESSAGE.to_string());
        assert_eq!(state.status, QuotesStatus::Unavailable);
        assert_eq!(state.notice().as_deref(), Some(UNAVAILABLE_MESSAGE));
        assert!(state.error.is_none());

        let mut state = QuotesState::default();
        state.set_error("carreld is not running.".into());
        assert_eq!(state.status, QuotesStatus::Ready);
        assert_eq!(state.error.as_deref(), Some("carreld is not running."));
        assert_eq!(state.notice().as_deref(), Some("carreld is not running."));

        let mut state = QuotesState::default();
        state.set_query("  zzz ");
        state.set_hits(Vec::new());
        assert_eq!(state.status, QuotesStatus::Ready);
        assert_eq!(state.notice().as_deref(), Some("No quotes match “zzz”."));
    }

    #[test]
    fn a_new_state_says_it_is_searching_until_the_answer_comes() {
        let mut state = QuotesState::default();
        assert_eq!(state.notice().as_deref(), Some(SEARCHING_MESSAGE));
        assert_eq!(state.heading(), "");
        state.set_hits(vec![hit("q1", "One")]);
        assert_eq!(state.notice(), None);
        assert_eq!(state.heading(), RECENT_HEADING, "a bare @ lists the newest");
        // Typing keeps the old list and says so, then the answer replaces it.
        state.set_query("one");
        assert_eq!(state.hits.len(), 1);
        assert_eq!(state.heading(), SEARCHING_MESSAGE);
        state.set_hits(vec![hit("q2", "Two")]);
        assert_eq!(state.heading(), "");
        assert_eq!(state.current().unwrap().id, "q2");
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
