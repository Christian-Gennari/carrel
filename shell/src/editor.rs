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
use crate::focus::{self, Lines};
use crate::goal;
use crate::markdown::{self, Kind};
use crate::settings::{GoalDisplay, Settings};
use crate::store::{naming, stats, DailyStats, Document, Store};
use crate::theme;

/// Line height of body text: 1.6 (spec/03-ui.md).
const LINE_HEIGHT: f64 = 1.6;
/// Average characters per line in the editor column. The spec asks for
/// 60–75; Pango's approximate width runs a little wide, so 66 measured
/// characters lands around 70 real ones in the serif.
const COLUMN_CHARACTERS: f64 = 66.0;

pub struct EditorPage {
    pub widget: gtk::Box,
    view: sourceview::View,
    buffer: sourceview::Buffer,
    scroller: gtk::ScrolledWindow,
    status: gtk::Label,
    note: gtk::Label,
    hairline: gtk::DrawingArea,
    hairline_fraction: Rc<Cell<f64>>,
    hairline_color: Rc<RefCell<(f64, f64, f64)>>,
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
    /// The paragraph currently drawn at full strength in focus mode.
    focus_previous: Cell<Option<(i32, i32)>>,
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

        // The "Hairline" goal display: 2 px along the bottom window edge,
        // filled from left to right toward the daily goal in the accent.
        let palette = theme::palette(&settings.borrow());
        let hairline_fraction = Rc::new(Cell::new(0.0));
        let hairline_color = Rc::new(RefCell::new(theme::rgb(palette.accent)));
        let hairline = gtk::DrawingArea::new();
        hairline.set_content_height(2);
        hairline.set_vexpand(false);
        hairline.set_can_target(false);
        hairline.set_focusable(false);
        hairline.add_css_class("goal-hairline");
        hairline.set_visible(settings.borrow().goal_display == GoalDisplay::Hairline);
        {
            let fraction = Rc::clone(&hairline_fraction);
            let color = Rc::clone(&hairline_color);
            hairline.set_draw_func(move |_, context, width, height| {
                let value: f64 = fraction.get();
                let fill = value.clamp(0.0, 1.0);
                if fill <= 0.0 || width <= 0 || height <= 0 {
                    return;
                }
                let (red, green, blue) = *color.borrow();
                context.set_source_rgb(red, green, blue);
                context.rectangle(0.0, 0.0, fill * f64::from(width), f64::from(height));
                let _ = context.fill();
            });
        }

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("stylos-page");
        widget.append(&scroller);
        widget.append(&note);
        widget.append(&status);
        widget.append(&hairline);

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
            hairline,
            hairline_fraction,
            hairline_color,
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
            focus_previous: Cell::new(None),
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
        // Focus mode follows the cursor here too, so a click or an arrow key
        // fades the paragraph that was left and restores the one entered.
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
                page.sync_focus();
                scroll_to_cursor(&view, &buffer);
            }
        });

        // Recompute the centered column and the line height after the widget
        // has a real font, and give the text enough room above and below to
        // centre the first and last line (typewriter scrolling).
        let weak = Rc::downgrade(self);
        self.view.connect_map(move |_| {
            for delay in [40u64, 450] {
                let weak = weak.clone();
                glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                    if let Some(page) = weak.upgrade() {
                        page.update_line_height();
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

        if keyval == gdk::Key::F2 {
            self.toggle_focus_mode();
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
        // Markdown tags and focus tags are separate; after a paint the focus
        // state only needs the cheap incremental check.
        self.sync_focus();
    }

    fn scroll_to_cursor(&self) {
        scroll_to_cursor(&self.view, &self.buffer);
    }

    /// The body font as Pango will draw it: the first family from the
    /// settings that this system actually has, at the current pixel size.
    fn body_font(&self) -> pango::FontDescription {
        let settings = self.settings.borrow();
        let context = self.view.pango_context();
        let available = context.list_families();
        let resolved = settings
            .font
            .families()
            .iter()
            .find(|wanted| {
                available
                    .iter()
                    .any(|family| family.name().eq_ignore_ascii_case(wanted))
            })
            .copied()
            .unwrap_or(settings.font.families()[0]);
        let mut description = pango::FontDescription::new();
        description.set_family(resolved);
        description.set_absolute_size(
            f64::from(settings.font_size)
                * f64::from(self.view.scale_factor())
                * f64::from(pango::SCALE),
        );
        description
    }

    /// Pango units to logical pixels, the unit CSS and the view margins use.
    fn device_pixels(&self, value: i32) -> f64 {
        f64::from(value) / f64::from(pango::SCALE) / f64::from(self.view.scale_factor())
    }

    fn update_line_height(&self) {
        // The spec asks for 1.6 line height. GtkTextView has no line-height
        // property, so split the difference between the font's own metrics
        // and the target across pixels-above-lines and pixels-below-lines.
        let metrics = self
            .view
            .pango_context()
            .metrics(Some(&self.body_font()), None);
        let natural = self.device_pixels(metrics.height());
        let target = f64::from(self.settings.borrow().font_size) * LINE_HEIGHT;
        let extra = (target - natural).max(0.0).round() as i32;
        let above = extra / 2;
        let below = extra - above;
        if self.view.pixels_above_lines() != above || self.view.pixels_below_lines() != below {
            self.view.set_pixels_above_lines(above);
            self.view.set_pixels_below_lines(below);
        }
        // Wrapped lines inside one paragraph get the same rhythm instead of
        // the text view's separate inside-wrap spacing.
        if self.view.pixels_inside_wrap() != extra {
            self.view.set_pixels_inside_wrap(extra);
        }
    }

    fn update_column_width(&self) {
        // ~70 average characters of body text (spec/03-ui.md), measured with
        // the actual body font and size, so the column survives Ctrl+= and
        // font changes. The view's 32 px side margins are added on top.
        let metrics = self
            .view
            .pango_context()
            .metrics(Some(&self.body_font()), None);
        let char_width = self.device_pixels(metrics.approximate_char_width());
        let width = (char_width * COLUMN_CHARACTERS + 64.0)
            .round()
            .clamp(520.0, 1100.0) as i32;
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

    fn focus_enabled(&self) -> bool {
        self.settings.borrow().focus_mode
    }

    /// The paragraph range (character offsets) around the cursor.
    fn paragraph_at(&self, iter: &gtk::TextIter) -> Option<(i32, i32)> {
        let lines = BufferLines {
            buffer: &self.buffer,
        };
        focus::focused_paragraph(&lines, iter.line())
    }

    fn dim(&self, start: i32, end: i32) {
        if start >= end {
            return;
        }
        let from = self.buffer.iter_at_offset(start);
        let to = self.buffer.iter_at_offset(end);
        self.buffer.apply_tag(&self.tags.focus, &from, &to);
    }

    fn undim(&self, start: i32, end: i32) {
        if start >= end {
            return;
        }
        let from = self.buffer.iter_at_offset(start);
        let to = self.buffer.iter_at_offset(end);
        self.buffer.remove_tag(&self.tags.focus, &from, &to);
    }

    /// Drop every focus tag and forget the paragraph at full strength.
    fn reset_focus(&self) {
        let start = self.buffer.start_iter();
        let end = self.buffer.end_iter();
        self.buffer.remove_tag(&self.tags.focus, &start, &end);
        self.focus_previous.set(None);
    }

    /// Dim every paragraph except the one with the cursor. One pass over the
    /// document, used when focus mode is switched on and after a load.
    fn reapply_focus(&self) {
        self.reset_focus();
        if !self.focus_enabled() {
            return;
        }
        let lines = BufferLines {
            buffer: &self.buffer,
        };
        let count = lines.count();
        let cursor = self.buffer.iter_at_mark(&self.buffer.get_insert());
        let current = self.paragraph_at(&cursor);
        let mut line = 0;
        while line < count {
            if !lines.is_blank(line) {
                if let Some((start, end)) = focus::paragraph_range(&lines, line) {
                    if Some((start, end)) != current {
                        self.dim(start, end);
                    }
                    line = self.buffer.iter_at_offset(end).line();
                }
            }
            line += 1;
        }
        self.focus_previous.set(current);
    }

    /// Keep focus mode current after a cursor move or an edit. Only the
    /// paragraph that was left and the one entered are touched, so this stays
    /// cheap on a long document.
    fn sync_focus(&self) {
        if !self.focus_enabled() {
            if self.focus_previous.get().is_some() {
                self.reset_focus();
            }
            return;
        }
        if self.focus_previous.get().is_none() {
            self.reapply_focus();
            return;
        }
        let cursor = self.buffer.iter_at_mark(&self.buffer.get_insert());
        let Some(current) = self.paragraph_at(&cursor) else {
            return;
        };
        let previous = self.focus_previous.get();
        if previous == Some(current) {
            return;
        }
        let ops = focus::retag(previous, current);
        if let Some((start, end)) = ops.dim {
            self.dim(start, end);
        }
        // The paragraph under the cursor is always at full strength, which
        // also repairs the merge after Backspace or Delete.
        if let Some((start, end)) = ops.clear {
            self.undim(start, end);
        }
        self.focus_previous.set(Some(current));
    }

    fn toggle_focus_mode(self: &Rc<Self>) {
        {
            let mut settings = self.settings.borrow_mut();
            settings.focus_mode = !settings.focus_mode;
        }
        self.save_settings();
        self.reapply_focus();
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
        self.focus_previous.set(None);
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

    /// Insert a block at the cursor as a normal journaled edit: the Quotes
    /// palette uses it for a block quote plus its citation.
    ///
    /// When the cursor is mid-line the block starts a new paragraph; when text
    /// follows the cursor it starts a new paragraph too, so the citation never
    /// becomes a lazy continuation of the quote.
    pub fn insert_block(self: &Rc<Self>, block: &str) -> Result<(), String> {
        if self.doc.borrow().is_none() {
            return Err("No file is open.".to_owned());
        }
        if block.is_empty() {
            return Ok(());
        }
        let mark = self.buffer.get_insert();
        let cursor = self.buffer.iter_at_mark(&mark);
        let at_line_start = cursor.line_offset() == 0;
        let at_end = cursor.offset() == self.buffer.end_iter().offset();
        let next_is_newline = {
            let mut next = cursor;
            next.forward_char();
            next.char() == '\n'
        };
        let prefix = if at_line_start { "" } else { "\n\n" };
        let suffix = if at_end || next_is_newline { "" } else { "\n" };
        let text = format!("{prefix}{block}{suffix}");
        self.loading.set(false);
        self.buffer.insert_at_cursor(&text);
        let words = stats::count_words(&self.buffer_text());
        self.last_words.set(words);
        self.update_status(false);
        if let Some(doc) = self.doc.borrow().as_ref() {
            doc.flush().map_err(|error| error.to_string())?;
        }
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
        // `set_text` drops every tag; focus mode starts again from the new
        // document rather than trusting the old paragraph offsets.
        self.focus_previous.set(None);

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
        let numbers = settings.goal_display == GoalDisplay::Numbers;
        let mut parts = Vec::new();
        if numbers {
            parts.push(format!(
                "{} words",
                stats::format_number(self.last_words.get())
            ));
            parts.push(format!(
                "{} / {} today",
                stats::format_number(self.today_words.get()),
                stats::format_number(u64::from(settings.daily_goal))
            ));
        }
        parts.push(self.battery.label());
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

        // Goal progress: numbers on the bottom line, or the hairline along
        // the bottom edge. It is full, never over-full, at the goal.
        self.hairline.set_visible(!numbers);
        self.hairline_fraction.set(goal::hairline_fraction(
            self.today_words.get(),
            settings.daily_goal,
        ));
        self.hairline.queue_draw();
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
        let palette = theme::palette(&self.settings.borrow());
        theme::apply(&self.css, &self.settings.borrow());
        self.tags.update_colors(&palette);
        *self.hairline_color.borrow_mut() = theme::rgb(palette.accent);
        self.status
            .set_visible(self.settings.borrow().show_bottom_line);
        self.update_line_height();
        self.update_column_width();
        self.restyle_now();
        // Document-wide, but only on a theme, font or settings change; typing
        // and cursor movement use the incremental `sync_focus`.
        self.reapply_focus();
        self.refresh_status();
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(page) = weak.upgrade() {
                page.update_padding();
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
    /// Focus mode: everything but the cursor's paragraph, at 35 % opacity.
    focus: gtk::TextTag,
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
        dim.set_foreground(Some(&palette.dim()));

        let quote = make("stylos-quote");
        // A tag's left margin replaces the view's (32 px), so add the quote
        // indent to it; wrapped lines then line up with the first line.
        quote.set_left_margin(32 + 28);

        let cite = make("stylos-cite");
        cite.set_scale(0.72);
        cite.set_rise(7);
        // Citation markers use the dim text colour, not the accent.
        cite.set_foreground(Some(&palette.dim()));

        let focus = make("stylos-focus-dim");
        focus.set_foreground_rgba(Some(&palette.focus_text()));

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
        // style scheme's foreground; focus is last so a dimmed paragraph
        // fades evenly, markers and citations included.
        quote.set_priority(0);
        bold.set_priority(2);
        italic.set_priority(3);
        for tag in &headings {
            tag.set_priority(4);
        }
        cite.set_priority(7);
        dim.set_priority(9);
        focus.set_priority(11);

        Self {
            bold,
            italic,
            dim,
            quote,
            cite,
            headings,
            focus,
        }
    }

    fn update_colors(&self, palette: &theme::Palette) {
        self.dim.set_foreground(Some(&palette.dim()));
        self.cite.set_foreground(Some(&palette.dim()));
        self.focus.set_foreground_rgba(Some(&palette.focus_text()));
    }

    /// Every tag this module owns. Focus tags are not in this list: they
    /// survive a restyle and are updated by the editor's focus logic.
    fn own(&self) -> Vec<&gtk::TextTag> {
        let mut tags = vec![&self.bold, &self.italic, &self.dim, &self.quote, &self.cite];
        tags.extend(self.headings.iter());
        tags
    }

    fn paint(&self, buffer: &sourceview::Buffer, spans: &[markdown::Span]) {
        let start = buffer.start_iter();
        let end = buffer.end_iter();
        for tag in self.own() {
            buffer.remove_tag(tag, &start, &end);
        }
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

/// The document's logical lines for [`focus::paragraph_range`], read lazily
/// from the buffer so a cursor move only touches its own paragraph.
struct BufferLines<'a> {
    buffer: &'a sourceview::Buffer,
}

impl Lines for BufferLines<'_> {
    fn count(&self) -> i32 {
        self.buffer.line_count()
    }

    fn is_blank(&self, line: i32) -> bool {
        let from = self.buffer.iter_at_offset(self.start(line));
        let to = self.buffer.iter_at_offset(self.end(line));
        self.buffer.text(&from, &to, false).trim().is_empty()
    }

    fn start(&self, line: i32) -> i32 {
        self.buffer
            .iter_at_line(line)
            .map(|iter| iter.offset())
            .unwrap_or(0)
    }

    fn end(&self, line: i32) -> i32 {
        // The line break itself belongs to the line, so the next line starts
        // one character later. (`forward_to_line_end` skips empty lines.)
        if line + 1 < self.count() {
            self.start(line + 1) - 1
        } else {
            self.buffer.end_iter().offset()
        }
    }
}
