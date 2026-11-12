//! The editor screen: GtkSourceView, inline Markdown styling, typewriter
//! scrolling, the bottom line and the durability wiring.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use sourceview::prelude::*;

use crate::battery::{Battery, BatteryHint, BATTERY_NOTE_SECONDS};
use crate::markdown::{self, Kind};
use crate::settings::Settings;
use crate::store::{naming, stats, DailyStats, Document, Store};
use crate::theme;

pub struct EditorPage {
    pub widget: gtk::Box,
    view: sourceview::View,
    buffer: sourceview::Buffer,
    scroller: gtk::ScrolledWindow,
    status: gtk::Label,
    note: gtk::Label,
    note_generation: Cell<u64>,
    battery_hint: RefCell<BatteryHint>,
    store: Store,
    settings: Rc<RefCell<Settings>>,
    css: gtk::CssProvider,
    stack: gtk::Stack,
    doc: RefCell<Option<Document>>,
    current: RefCell<Option<PathBuf>>,
    untitled: Cell<bool>,
    loading: Cell<bool>,
    last_edit_at: Cell<Instant>,
    last_words: Cell<u64>,
    today_words: Cell<u64>,
    today_date: RefCell<String>,
    stats_saved_at: Cell<Instant>,
    tags: Tags,
    restyle_generation: Cell<u64>,
    status_generation: Cell<u64>,
    battery: Rc<dyn Battery>,
    save_error: Cell<bool>,
}

impl EditorPage {
    pub fn new(
        store: Store,
        settings: Rc<RefCell<Settings>>,
        stack: gtk::Stack,
        css: gtk::CssProvider,
        battery: Rc<dyn Battery>,
    ) -> Rc<Self> {
        let buffer = sourceview::Buffer::new(None);
        buffer.set_highlight_syntax(false);
        buffer.set_highlight_matching_brackets(false);
        buffer.set_style_scheme(None);

        let view = sourceview::View::new();
        view.set_buffer(Some(&buffer));
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_justification(gtk::Justification::Left);
        view.set_left_margin(32);
        view.set_right_margin(32);
        // Room above and below so the current line can sit in the middle even
        // at the start and the end of a document.
        view.set_top_margin(240);
        view.set_bottom_margin(240);
        view.set_pixels_below_lines(6);
        view.set_highlight_current_line(false);
        view.set_show_line_numbers(false);
        view.set_auto_indent(false);
        view.set_tab_width(4);
        view.set_monospace(false);
        view.add_css_class("stylos-editor");

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .halign(gtk::Align::Center)
            .hexpand(false)
            .vexpand(true)
            .child(&view)
            .build();
        scroller.set_width_request(860);

        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.add_css_class("bottom-line");
        status.set_visible(settings.borrow().show_bottom_line);

        let note = gtk::Label::new(None);
        note.set_xalign(0.5);
        note.set_halign(gtk::Align::Center);
        note.set_wrap(true);
        note.set_margin_bottom(10);
        note.add_css_class("battery-note");
        note.set_visible(false);

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("stylos-page");
        widget.append(&scroller);
        widget.append(&note);
        widget.append(&status);

        let palette = theme::palette(&settings.borrow());
        let tags = Tags::new(&buffer, &palette);

        let today = local_date();
        let stats = store.load_stats(&today);
        let page = Rc::new(Self {
            widget,
            view,
            buffer,
            scroller,
            status,
            note,
            note_generation: Cell::new(0),
            battery_hint: RefCell::new(BatteryHint::default()),
            store,
            settings,
            css,
            stack,
            doc: RefCell::new(None),
            current: RefCell::new(None),
            untitled: Cell::new(false),
            loading: Cell::new(false),
            last_edit_at: Cell::new(Instant::now()),
            last_words: Cell::new(0),
            today_words: Cell::new(stats.words),
            today_date: RefCell::new(today),
            stats_saved_at: Cell::new(Instant::now()),
            tags,
            restyle_generation: Cell::new(0),
            status_generation: Cell::new(0),
            battery,
            save_error: Cell::new(false),
        });
        page.install();
        page
    }

    fn install(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.buffer.connect_insert_text(move |_, location, text| {
            let Some(page) = weak.upgrade() else {
                return;
            };
            if page.loading.get() {
                return;
            }
            let offset = location.offset().max(0) as usize;
            if let Some(doc) = page.doc.borrow().as_ref() {
                if let Err(error) = doc.apply(offset, 0, text) {
                    eprintln!("stylos: could not journal edit: {error}");
                    page.save_error.set(true);
                }
            }
            page.note_edit();
        });

        let weak = Rc::downgrade(self);
        self.buffer.connect_delete_range(move |_, start, end| {
            let Some(page) = weak.upgrade() else {
                return;
            };
            if page.loading.get() {
                return;
            }
            let offset = start.offset().max(0) as usize;
            let delete_len = (end.offset() - start.offset()).max(0) as usize;
            if let Some(doc) = page.doc.borrow().as_ref() {
                if let Err(error) = doc.apply(offset, delete_len, "") {
                    eprintln!("stylos: could not journal delete: {error}");
                    page.save_error.set(true);
                }
            }
            page.note_edit();
        });

        // Typewriter scrolling: keep the insertion point near the middle.
        let buffer = self.buffer.clone();
        let view = self.view.clone();
        let weak = Rc::downgrade(self);
        self.buffer.connect_mark_set(move |_, _, mark| {
            let Some(page) = weak.upgrade() else {
                return;
            };
            let is_insert = mark
                .name()
                .map(|name| name.as_str() == "insert")
                .unwrap_or(false);
            if is_insert && !page.loading.get() {
                scroll_to_cursor(&view, &buffer);
            }
        });

        // Recompute the centered column after the widget has a real font, and
        // give the text enough room above and below to centre the first and
        // last line (typewriter scrolling).
        let weak = Rc::downgrade(self);
        self.view.connect_map(move |_| {
            for delay in [40u64, 450] {
                let weak = weak.clone();
                glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                    if let Some(page) = weak.upgrade() {
                        page.update_padding();
                        page.update_column_width();
                        page.scroll_to_cursor();
                    }
                });
            }
        });

        // Battery and clock refresh, even when the cursor never moves.
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(20, move || {
            if let Some(page) = weak.upgrade() {
                page.refresh_status();
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });

        // A new file is named from its first line once the writer pauses.
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(1, move || {
            if let Some(page) = weak.upgrade() {
                page.maybe_name_untitled(false);
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    }

    pub(crate) fn handle_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        let control = state.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = state.contains(gdk::ModifierType::ALT_MASK);
        let key = keyval.to_lower();

        if control && !alt {
            if key == gdk::Key::o {
                self.stack.set_visible_child_name("files");
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::n {
                if let Err(error) = self.create_new_file() {
                    eprintln!("stylos: could not create file: {error}");
                }
                return glib::Propagation::Stop;
            }
            if matches!(keyval, gdk::Key::equal | gdk::Key::plus) {
                self.change_font(true);
                return glib::Propagation::Stop;
            }
            if keyval == gdk::Key::minus {
                self.change_font(false);
                return glib::Propagation::Stop;
            }
        }

        if keyval == gdk::Key::F1 {
            self.toggle_bottom_line();
            return glib::Propagation::Stop;
        }

        if self.settings.borrow().typewriter_mode
            && matches!(keyval, gdk::Key::BackSpace | gdk::Key::Delete)
        {
            return glib::Propagation::Stop;
        }

        glib::Propagation::Proceed
    }

    fn note_edit(self: &Rc<Self>) {
        self.last_edit_at.set(Instant::now());
        self.schedule_restyle();
        self.schedule_status();
    }

    fn schedule_restyle(self: &Rc<Self>) {
        let generation = self.restyle_generation.get().wrapping_add(1);
        self.restyle_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(30), move || {
            if let Some(page) = weak.upgrade() {
                if page.restyle_generation.get() == generation {
                    page.restyle_now();
                }
            }
        });
    }

    fn schedule_status(self: &Rc<Self>) {
        let generation = self.status_generation.get().wrapping_add(1);
        self.status_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(140), move || {
            if let Some(page) = weak.upgrade() {
                if page.status_generation.get() == generation {
                    page.update_status(true);
                }
            }
        });
    }

    fn buffer_text(&self) -> String {
        self.buffer
            .text(&self.buffer.start_iter(), &self.buffer.end_iter(), false)
            .to_string()
    }

    fn restyle_now(&self) {
        let text = self.buffer_text();
        let spans = markdown::spans(&text);
        self.tags.paint(&self.buffer, &spans);
    }

    fn scroll_to_cursor(&self) {
        scroll_to_cursor(&self.view, &self.buffer);
    }

    fn update_column_width(&self) {
        // ~70 average characters of body text (spec/03-ui.md). Digits are
        // wider than average letters, so use Pango's approximate char width.
        let metrics = self.view.pango_context().metrics(None, None);
        let char_width = metrics.approximate_char_width() / gtk::pango::SCALE;
        let width = (char_width * 70 + 96).clamp(520, 1100);
        if self.scroller.width_request() != width {
            self.scroller.set_width_request(width);
        }
    }

    /// Half the viewport above and below, so the first and last line can sit
    /// in the middle as typewriter scrolling promises.
    fn update_padding(&self) {
        let height = self.view.height();
        if height < 100 {
            return;
        }
        let padding = (height / 2).max(80);
        if self.view.top_margin() != padding {
            self.view.set_top_margin(padding);
            self.view.set_bottom_margin(padding);
        }
    }

    pub fn focus_editor(&self) {
        self.view.grab_focus();
    }

    /// A one-line note above the bottom line. Used for the low-battery
    /// warnings and for "Quotes need a Nostos account" until WP9.
    pub fn flash_note(self: &Rc<Self>, text: &str, seconds: u64) {
        let generation = self.note_generation.get().wrapping_add(1);
        self.note_generation.set(generation);
        self.note.set_text(text);
        self.note.set_visible(true);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(seconds), move || {
            if let Some(page) = weak.upgrade() {
                if page.note_generation.get() == generation {
                    page.note.set_visible(false);
                }
            }
        });
    }

    /// Ctrl+]: the quotes palette is WP9.
    pub fn quotes_note(self: &Rc<Self>) {
        self.flash_note("Quotes need a Nostos account.", 4);
    }

    /// A new battery reading. The bottom line always updates; when the writer
    /// hid it, the 10 % and 5 % notes still appear for five seconds.
    pub fn on_battery(self: &Rc<Self>) {
        self.refresh_status();
        if self.settings.borrow().show_bottom_line {
            return;
        }
        let status = self.battery.percent();
        let charging = self.battery.charging();
        if !self.battery.present() {
            return;
        }
        let due = self.battery_hint.borrow_mut().observe(status, charging);
        if let Some(percent) = due {
            self.flash_note(&format!("{percent}% battery"), BATTERY_NOTE_SECONDS);
        }
    }

    /// Flush and report the error instead of only printing it, for History's
    /// replace-the-file flow.
    pub fn flush_now(&self) -> Result<(), String> {
        if let Some(doc) = self.doc.borrow().as_ref() {
            doc.flush().map_err(|error| error.to_string())?;
        }
        self.save_stats();
        self.save_error.set(false);
        Ok(())
    }

    /// Replace the whole document with `text` as a normal journaled edit.
    /// History uses this to apply an old version; the change is saved before
    /// returning so the next commit sees it.
    pub fn replace_text(self: &Rc<Self>, text: &str) -> Result<(), String> {
        if self.doc.borrow().is_none() {
            return Err("No file is open.".to_owned());
        }
        self.loading.set(false);
        self.buffer.set_text(text);
        let words = stats::count_words(text);
        self.last_words.set(words);
        self.update_status(false);
        if let Some(doc) = self.doc.borrow().as_ref() {
            doc.flush().map_err(|error| error.to_string())?;
        }
        let end = self.buffer.end_iter();
        self.buffer.place_cursor(&end);
        self.restyle_now();
        self.update_status(false);
        self.scroll_to_cursor();
        Ok(())
    }

    pub fn current_rel(&self) -> Option<PathBuf> {
        self.current.borrow().clone()
    }

    pub fn has_document(&self) -> bool {
        self.doc.borrow().is_some()
    }

    pub fn open_rel(&self, rel: &Path) -> Result<(), String> {
        self.close_document();
        let doc = self
            .store
            .open_document(rel)
            .map_err(|error| format!("Cannot open {}: {error}", rel.display()))?;
        let text = doc.text();
        let words = stats::count_words(&text);

        *self.doc.borrow_mut() = Some(doc);
        *self.current.borrow_mut() = Some(rel.to_path_buf());
        self.untitled.set(false);
        self.save_error.set(false);

        self.loading.set(true);
        self.buffer.set_text(&text);
        self.loading.set(false);
        self.buffer.set_modified(false);

        self.last_words.set(words);
        if let Err(error) = self.store.set_last_file(Some(rel)) {
            eprintln!("stylos: could not remember last file: {error}");
        }

        let end = self.buffer.end_iter();
        self.buffer.place_cursor(&end);
        self.restyle_now();
        self.update_status(false);
        self.scroll_to_cursor();
        Ok(())
    }

    /// Open the remembered file, else the newest one, else a new untitled file.
    pub fn open_last_or_new(self: &Rc<Self>) -> Result<(), String> {
        if let Some(rel) = self.store.last_file() {
            if self.open_rel(&rel).is_ok() {
                return Ok(());
            }
        }
        if let Ok(Some(rel)) = self.store.most_recent_file() {
            if self.open_rel(&rel).is_ok() {
                return Ok(());
            }
        }
        self.create_new_file()
    }

    pub fn create_new_file(self: &Rc<Self>) -> Result<(), String> {
        self.create_new_file_in(Path::new(""))
    }

    /// Create a new file in `dir` (relative to the writing root). The Files
    /// screen uses this so Ctrl+N inside a folder stays in that folder.
    pub fn create_new_file_in(self: &Rc<Self>, dir: &Path) -> Result<(), String> {
        if self.untitled.get() {
            let same_dir = self
                .current_rel()
                .and_then(|current| current.parent().map(Path::to_path_buf))
                .is_none_or(|parent| parent == dir);
            if same_dir && self.buffer_text().trim().is_empty() {
                // Ctrl+N on a fresh, empty Untitled should not pile up blank
                // files; the writer is already holding a new file.
                return Ok(());
            }
            self.maybe_name_untitled(true);
        }
        let rel = self
            .store
            .unique_rel_path(dir, "Untitled", "md")
            .map_err(|error| format!("Cannot choose a name: {error}"))?;
        self.store
            .create_file(&rel)
            .map_err(|error| format!("Cannot create {}: {error}", rel.display()))?;
        self.open_rel(&rel)?;
        self.untitled.set(true);
        self.last_edit_at.set(Instant::now());
        Ok(())
    }

    /// Name a new file from its first line once the writer pauses. Forced on
    /// exit so a one-line note still gets a real name.
    pub fn maybe_name_untitled(self: &Rc<Self>, force: bool) {
        if !self.untitled.get() {
            return;
        }
        if !force && self.last_edit_at.get().elapsed() < Duration::from_millis(1500) {
            return;
        }
        let Some(current) = self.current_rel() else {
            return;
        };
        let stem = current
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_default();
        if !stem.starts_with("Untitled") {
            self.untitled.set(false);
            return;
        }
        let text = self.buffer_text();
        let Some(title) = naming::title_from_first_line(&text) else {
            return;
        };
        if title == stem {
            return;
        }
        let dir = current
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf();
        let target = match self.store.unique_rel_path(&dir, &title, "md") {
            Ok(target) => target,
            Err(error) => {
                eprintln!("stylos: cannot name file: {error}");
                return;
            }
        };

        self.close_document();
        if let Err(error) = self.store.rename(&current, &target) {
            eprintln!("stylos: cannot rename {}: {error}", current.display());
            let _ = self.open_rel(&current);
            return;
        }
        if let Err(error) = self.open_rel(&target) {
            eprintln!("stylos: cannot open {}: {error}", target.display());
        }
    }

    fn close_document(&self) {
        if let Some(doc) = self.doc.borrow_mut().take() {
            if let Err(error) = doc.flush() {
                eprintln!(
                    "stylos: could not flush {}: {error}",
                    doc.rel_path().display()
                );
            }
        }
        *self.current.borrow_mut() = None;
    }

    /// Close the open document if `path` is it, or is a folder containing it.
    /// Returns the old path so the caller can reopen it after a move.
    pub fn close_if_affected(&self, path: &Path) -> Option<PathBuf> {
        let current = self.current_rel()?;
        if current == path || current.starts_with(path) {
            self.close_document();
            Some(current)
        } else {
            None
        }
    }

    pub fn flush(&self) {
        if let Some(doc) = self.doc.borrow().as_ref() {
            if let Err(error) = doc.flush() {
                eprintln!("stylos: flush failed: {error}");
                self.save_error.set(true);
            }
        }
        self.save_stats();
    }

    fn save_settings(&self) {
        if let Err(error) = self.settings.borrow().save(self.store.root()) {
            eprintln!("stylos: could not save settings: {error}");
        }
    }

    fn create_stats(&self) -> DailyStats {
        DailyStats {
            date: self.today_date.borrow().clone(),
            words: self.today_words.get(),
        }
    }

    pub fn save_stats(&self) {
        let stats = self.create_stats();
        if let Err(error) = self.store.save_stats(&stats) {
            eprintln!("stylos: could not save word count: {error}");
        }
        self.stats_saved_at.set(Instant::now());
    }

    fn rollover_if_needed(&self) {
        let today = local_date();
        if today.is_empty() || today == *self.today_date.borrow() {
            return;
        }
        self.today_date.replace(today);
        self.today_words.set(0);
        self.save_stats();
    }

    fn update_status(&self, count_delta: bool) {
        self.rollover_if_needed();
        let text = self.buffer_text();
        let words = stats::count_words(&text);
        if count_delta {
            let previous = self.last_words.get();
            let delta = words as i64 - previous as i64;
            self.today_words
                .set(stats::add_delta(self.today_words.get(), delta));
        }
        self.last_words.set(words);
        self.refresh_status();
        if count_delta && self.stats_saved_at.get().elapsed() >= Duration::from_secs(5) {
            self.save_stats();
        }
    }

    fn refresh_status(&self) {
        let settings = self.settings.borrow();
        let mut parts = vec![
            format!("{} words", stats::format_number(self.last_words.get())),
            format!(
                "{} / {} today",
                stats::format_number(self.today_words.get()),
                stats::format_number(u64::from(settings.daily_goal))
            ),
            self.battery.label(),
        ];
        if settings.clock {
            if let Some(time) = local_time() {
                parts.push(time);
            }
        }
        let mut line = parts.join(" · ");
        if self.save_error.get() {
            line.push_str(" · not saved");
        }
        self.status.set_text(&line);
    }

    fn change_font(self: &Rc<Self>, larger: bool) {
        let changed = {
            let mut settings = self.settings.borrow_mut();
            if larger {
                settings.larger_font()
            } else {
                settings.smaller_font()
            }
        };
        if !changed {
            return;
        }
        self.save_settings();
        self.reapply_style();
    }

    fn toggle_bottom_line(self: &Rc<Self>) {
        let visible = {
            let mut settings = self.settings.borrow_mut();
            settings.show_bottom_line = !settings.show_bottom_line;
            settings.show_bottom_line
        };
        self.status.set_visible(visible);
        if visible {
            self.note.set_visible(false);
        } else {
            // Hiding the line at 10 % or 5 % should show the note at once,
            // without waiting for the next 30-second battery poll.
            self.on_battery();
        }
        self.save_settings();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(50), move || {
            if let Some(page) = weak.upgrade() {
                page.update_padding();
                page.scroll_to_cursor();
            }
        });
    }

    /// Called by the app after a theme or font change.
    pub fn reapply_style(self: &Rc<Self>) {
        theme::apply(&self.css, &self.settings.borrow());
        self.tags
            .update_colors(&theme::palette(&self.settings.borrow()));
        self.status
            .set_visible(self.settings.borrow().show_bottom_line);
        self.restyle_now();
        self.refresh_status();
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(page) = weak.upgrade() {
                page.update_column_width();
            }
        });
    }
}

fn scroll_to_cursor(view: &sourceview::View, buffer: &sourceview::Buffer) {
    let view = view.clone();
    let buffer = buffer.clone();
    glib::idle_add_local_once(move || {
        let mark = buffer.get_insert();
        view.scroll_to_mark(&mark, 0.0, true, 0.0, 0.55);
    });
}

fn local_date() -> String {
    glib::DateTime::now_local()
        .ok()
        .and_then(|date| date.format("%Y-%m-%d").ok())
        .map(|date| date.to_string())
        .unwrap_or_default()
}

fn local_time() -> Option<String> {
    glib::DateTime::now_local()
        .ok()
        .and_then(|date| date.format("%H:%M").ok())
        .map(|time| time.to_string())
}

struct Tags {
    bold: gtk::TextTag,
    italic: gtk::TextTag,
    dim: gtk::TextTag,
    quote: gtk::TextTag,
    cite: gtk::TextTag,
    headings: Vec<gtk::TextTag>,
}

impl Tags {
    fn new(buffer: &sourceview::Buffer, palette: &theme::Palette) -> Self {
        let table = buffer.tag_table();
        let make = |name: &str| {
            let tag = gtk::TextTag::new(Some(name));
            let _ = table.add(&tag);
            tag
        };

        let bold = make("stylos-bold");
        bold.set_weight(700);

        let italic = make("stylos-italic");
        italic.set_style(pango::Style::Italic);

        let dim = make("stylos-dim");
        dim.set_foreground(Some(palette.dim));

        let quote = make("stylos-quote");
        // A tag's left margin replaces the view's (32 px), so add the quote
        // indent to it; wrapped lines then line up with the first line.
        quote.set_left_margin(32 + 28);
        quote.set_pixels_below_lines(6);

        let cite = make("stylos-cite");
        cite.set_scale(0.72);
        cite.set_rise(7);
        cite.set_foreground(Some(palette.accent));

        let scales = [1.65, 1.45, 1.28, 1.18, 1.10, 1.05];
        let mut headings = Vec::new();
        for (index, scale) in scales.iter().enumerate() {
            let tag = make(&format!("stylos-heading-{}", index + 1));
            tag.set_scale(*scale);
            tag.set_weight(700);
            tag.set_pixels_above_lines(if index < 2 { 18 } else { 12 });
            tag.set_pixels_below_lines(6);
            headings.push(tag);
        }

        // Priorities must be assigned after every tag is in the table and
        // must be smaller than the number of tags. Markers (dim) win over the
        // style scheme's foreground; headings lose to nothing else.
        quote.set_priority(0);
        bold.set_priority(2);
        italic.set_priority(3);
        for tag in &headings {
            tag.set_priority(4);
        }
        cite.set_priority(7);
        dim.set_priority(9);

        Self {
            bold,
            italic,
            dim,
            quote,
            cite,
            headings,
        }
    }

    fn update_colors(&self, palette: &theme::Palette) {
        self.dim.set_foreground(Some(palette.dim));
        self.cite.set_foreground(Some(palette.accent));
    }

    fn paint(&self, buffer: &sourceview::Buffer, spans: &[markdown::Span]) {
        let start = buffer.start_iter();
        let end = buffer.end_iter();
        buffer.remove_all_tags(&start, &end);
        for span in spans {
            let tag = match span.kind {
                Kind::Bold => &self.bold,
                Kind::Italic => &self.italic,
                Kind::Marker => &self.dim,
                Kind::Quote => &self.quote,
                Kind::Cite => &self.cite,
                Kind::Heading(level) => &self.headings[usize::from(level.clamp(1, 6) - 1)],
            };
            let from = buffer.iter_at_offset(span.start as i32);
            let to = buffer.iter_at_offset(span.end as i32);
            buffer.apply_tag(tag, &from, &to);
        }
    }
}
