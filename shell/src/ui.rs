//! Small shared pieces for the keyboard-first screens: the one page frame,
//! list rows, the key hint bar, the confirmation dialog, time labels.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::settings::Theme;
use crate::theme::{self, Palette};

/// The max width of every non-editor screen. About the editor column at the
/// default body size, so moving between the page and a screen does not change
/// where the text lives.
pub const COLUMN_WIDTH: i32 = 720;

/// The page itself: fills the window, carries the theme background.
pub fn page() -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.add_css_class("carrel-page");
    page.set_focusable(true);
    page
}

/// The centered reading column every screen uses.
pub fn column(width: i32) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.set_halign(gtk::Align::Center);
    column.set_valign(gtk::Align::Fill);
    column.set_width_request(width);
    column.set_vexpand(true);
    column.set_margin_top(56);
    column.set_margin_bottom(24);
    column
}

pub fn title(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("screen-title");
    label
}

/// One page frame for every non-editor screen: the same centered column, the
/// title in the same place, an optional one-line status under it, the content,
/// and the key hint bar at the bottom. Confirmations are a centered card over
/// a dimmed page, never a separate strip or native window.
pub struct Screen {
    pub root: gtk::Overlay,
    pub content: gtk::Box,
    page: gtk::Box,
    status: gtk::Label,
    hint_slot: gtk::Box,
    hint_key: RefCell<String>,
    scrim: gtk::CenterBox,
    dialog_title: gtk::Label,
    dialog_text: gtk::Label,
    cancel: gtk::Button,
    accept: gtk::Button,
    accept_focused: Cell<bool>,
    action: RefCell<Option<Rc<dyn Fn()>>>,
}

impl Screen {
    pub fn new(title_text: &str) -> Rc<Self> {
        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_hexpand(true);
        status.set_ellipsize(pango::EllipsizeMode::End);
        status.set_single_line_mode(true);
        status.add_css_class("screen-status");
        status.set_visible(false);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
        content.set_vexpand(true);
        content.set_margin_top(22);

        let hint_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        hint_slot.add_css_class("hint-slot");
        hint_slot.set_margin_top(18);

        let column = column(COLUMN_WIDTH);
        column.append(&title(title_text));
        column.append(&status);
        column.append(&content);
        column.append(&hint_slot);

        let page = page();
        page.append(&column);

        let dialog_title = gtk::Label::new(None);
        dialog_title.set_xalign(0.0);
        dialog_title.set_wrap(true);
        dialog_title.add_css_class("dialog-title");
        let dialog_text = gtk::Label::new(None);
        dialog_text.set_xalign(0.0);
        dialog_text.set_wrap(true);
        dialog_text.set_max_width_chars(46);
        dialog_text.add_css_class("dialog-text");

        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("dialog-button");
        cancel.set_size_request(104, -1);
        let accept = gtk::Button::with_label("Delete");
        accept.add_css_class("dialog-button");
        accept.add_css_class("dialog-accept");
        accept.set_size_request(104, -1);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        actions.set_halign(gtk::Align::End);
        actions.set_margin_top(8);
        actions.append(&cancel);
        actions.append(&accept);

        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.add_css_class("dialog-card");
        card.set_halign(gtk::Align::Center);
        card.set_valign(gtk::Align::Center);
        card.set_width_request(420);
        card.append(&dialog_title);
        card.append(&dialog_text);
        card.append(&actions);

        let scrim = gtk::CenterBox::new();
        scrim.add_css_class("dialog-scrim");
        scrim.set_halign(gtk::Align::Fill);
        scrim.set_valign(gtk::Align::Fill);
        scrim.set_hexpand(true);
        scrim.set_vexpand(true);
        scrim.set_can_target(true);
        scrim.set_visible(false);
        scrim.set_center_widget(Some(&card));

        let root = gtk::Overlay::new();
        root.set_child(Some(&page));
        root.add_overlay(&scrim);
        root.set_focusable(true);

        let screen = Rc::new(Self {
            root,
            content,
            page,
            status,
            hint_slot,
            hint_key: RefCell::new(String::new()),
            scrim,
            dialog_title,
            dialog_text,
            cancel,
            accept,
            accept_focused: Cell::new(false),
            action: RefCell::new(None),
        });
        screen.install_dialog();
        screen.set_hints(&[]);
        screen
    }

    fn install_dialog(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.cancel.connect_clicked(move |_| {
            if let Some(screen) = weak.upgrade() {
                screen.close_dialog();
            }
        });
        let weak = Rc::downgrade(self);
        self.accept.connect_clicked(move |_| {
            if let Some(screen) = weak.upgrade() {
                screen.accept_dialog();
            }
        });
    }

    pub fn focus_page(&self) {
        self.page.grab_focus();
    }

    /// The one-line status under the title. An empty string hides it.
    pub fn set_status(&self, text: &str) {
        self.set_status_kind(text, false);
    }

    pub fn set_status_error(&self, text: &str) {
        self.set_status_kind(text, true);
    }

    fn set_status_kind(&self, text: &str, error: bool) {
        if error {
            self.status.add_css_class("error-text");
        } else {
            self.status.remove_css_class("error-text");
        }
        if text.is_empty() {
            self.status.set_visible(false);
            self.status.set_text("");
        } else {
            if self.status.text() != text {
                self.status.set_text(text);
            }
            self.status.set_visible(true);
        }
    }

    /// Replace the bottom hint bar. At most five hints; callers put the most
    /// useful first. A key cap is a small rounded box with the key on it.
    pub fn set_hints(&self, hints: &[(&str, &str)]) {
        let key = hints
            .iter()
            .take(5)
            .map(|(keys, word)| format!("{keys}\u{1f}{word}"))
            .collect::<Vec<_>>()
            .join("\u{1e}");
        if *self.hint_key.borrow() == key {
            return;
        }
        *self.hint_key.borrow_mut() = key;
        while let Some(child) = self.hint_slot.first_child() {
            self.hint_slot.remove(&child);
        }
        self.hint_slot.append(&hint_bar(hints));
    }

    /// Show a confirmation card over a dimmed page. `accept` names what the
    /// destructive button does ("Delete", "Restore"); the safe Cancel button
    /// has focus, so Enter alone never destroys anything.
    pub fn ask(
        self: &Rc<Self>,
        title: &str,
        sentence: &str,
        accept: &str,
        action: impl Fn() + 'static,
    ) {
        self.dialog_title.set_text(title);
        self.dialog_text.set_text(sentence);
        self.accept.set_label(accept);
        *self.action.borrow_mut() = Some(Rc::new(action));
        self.accept_focused.set(false);
        self.scrim.set_visible(true);
        // Cancel is the safe button: Enter alone never destroys anything.
        self.cancel.grab_focus();
    }

    pub fn is_dialog_open(&self) -> bool {
        self.scrim.is_visible()
    }

    fn close_dialog(self: &Rc<Self>) {
        self.scrim.set_visible(false);
        *self.action.borrow_mut() = None;
        self.focus_page();
    }

    fn accept_dialog(self: &Rc<Self>) {
        let action = self.action.borrow_mut().take();
        self.close_dialog();
        if let Some(action) = action {
            action();
        }
    }

    fn toggle_dialog_focus(&self) {
        let accept = !self.accept_focused.get();
        self.accept_focused.set(accept);
        if accept {
            self.accept.grab_focus();
        } else {
            self.cancel.grab_focus();
        }
    }

    /// Route a key to the open confirmation. Esc cancels, Enter activates the
    /// focused button, Left/Right/Tab move between the two.
    pub fn dialog_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        _state: gdk::ModifierType,
    ) -> glib::Propagation {
        if !self.is_dialog_open() {
            return glib::Propagation::Proceed;
        }
        match keyval {
            gdk::Key::Escape => self.close_dialog(),
            gdk::Key::Return | gdk::Key::KP_Enter => {
                if self.accept_focused.get() {
                    self.accept_dialog();
                } else {
                    self.close_dialog();
                }
            }
            gdk::Key::Left
            | gdk::Key::Right
            | gdk::Key::Tab
            | gdk::Key::ISO_Left_Tab
            | gdk::Key::KP_Tab => self.toggle_dialog_focus(),
            _ => {}
        }
        glib::Propagation::Stop
    }
}

/// The Carrel mark ("c|": a round c and a text cursor), drawn with Cairo from
/// the geometry in `brand/README.md`. It is one flat colour, the text colour
/// of the theme (dark ink on Paper, soft white on Night), so it matches
/// `brand/carrel-mark-monochrome-*.svg`. It is static: no animation.
pub fn mark(height: i32) -> gtk::DrawingArea {
    let width = (f64::from(height) * MARK_WIDTH / MARK_HEIGHT).round() as i32;
    let area = gtk::DrawingArea::new();
    area.set_content_width(width);
    area.set_content_height(height);
    area.set_halign(gtk::Align::Start);
    area.set_valign(gtk::Align::Center);
    area.set_can_focus(false);
    area.set_focusable(false);
    area.set_accessible_role(gtk::AccessibleRole::Img);
    area.update_property(&[gtk::accessible::Property::Label("Carrel mark")]);
    area.set_draw_func(|area, context, width, height| {
        let color = area.color();
        context.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        draw_mark(context, f64::from(width), f64::from(height));
    });
    area
}

/// Size of the glyph without the tile, in the 1024 canvas of the SVG files.
const MARK_WIDTH: f64 = 596.0;
const MARK_HEIGHT: f64 = 480.0;

/// Fill the mark so it fits in `width` x `height`, centred.
fn draw_mark(context: &gtk::cairo::Context, width: f64, height: f64) {
    let scale = (width / MARK_WIDTH).min(height / MARK_HEIGHT);
    context.translate(
        (width - MARK_WIDTH * scale) / 2.0,
        (height - MARK_HEIGHT * scale) / 2.0,
    );
    context.scale(scale, scale);
    // The glyph spans x 215..811 and y 276..756 on the canvas.
    context.translate(-215.0, -276.0);

    // The "c": outer circle (455, 516) r 240, counter (486, 484) r 142, with
    // flat cuts at y 456 and y 576 on the right.
    let outer = (455.0_f64, 516.0_f64, 240.0_f64);
    let inner = (486.0_f64, 484.0_f64, 142.0_f64);
    let angle = |circle: (f64, f64, f64), x: f64, y: f64| (y - circle.1).atan2(x - circle.0);
    context.new_path();
    context.move_to(687.38, 456.0);
    context.arc_negative(
        outer.0,
        outer.1,
        outer.2,
        angle(outer, 687.38, 456.0),
        angle(outer, 687.38, 576.0),
    );
    context.line_to(594.17, 576.0);
    context.arc(
        inner.0,
        inner.1,
        inner.2,
        angle(inner, 594.17, 576.0),
        angle(inner, 625.21, 456.0),
    );
    context.close_path();
    let _ = context.fill();

    // The cursor.
    context.rectangle(745.0, 276.0, 66.0, 480.0);
    let _ = context.fill();
}

/// A one-line hint with real key caps, e.g. `[Enter] Open  [Esc] Back`. Every
/// screen and the Quotes palette footer use this same bar.
pub fn hint_bar(hints: &[(&str, &str)]) -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    bar.add_css_class("hint-bar");
    bar.set_halign(gtk::Align::Start);
    bar.set_valign(gtk::Align::End);
    for (keys, word) in hints.iter().take(5) {
        let item = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        item.add_css_class("hint-item");
        item.set_valign(gtk::Align::Center);
        let cap = gtk::Label::new(Some(keys));
        cap.add_css_class("key-cap");
        cap.set_valign(gtk::Align::Center);
        let word = gtk::Label::new(Some(word));
        word.add_css_class("hint-word");
        word.set_valign(gtk::Align::Center);
        item.append(&cap);
        item.append(&word);
        bar.append(&item);
    }
    bar
}

pub fn line() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_xalign(0.0);
    label.add_css_class("status-line");
    label
}

pub fn list() -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(false);
    // Keys are handled by the page so the calm look has no focus ring.
    list.set_can_focus(false);
    list.add_css_class("file-list");
    list
}

pub fn scroller(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(child)
        .build()
}

/// A row with a label on the left and a value on the right. A non-empty
/// `note` sits under them, aligned with the label: Settings shows the hint on
/// the selected row only (spec/03-ui.md).
pub fn value_row(name: &str, value: &str, note: &str) -> gtk::ListBoxRow {
    setting_row(name, value, note, None, false)
}

/// A value row whose note is led by the Carrel mark: the About row.
pub fn marked_value_row(name: &str, value: &str, note: &str) -> gtk::ListBoxRow {
    setting_row(name, value, note, None, true)
}

/// A value row with a theme swatch between the name and the value.
pub fn swatch_row(
    name: &str,
    swatch: &gtk::DrawingArea,
    value: &str,
    note: &str,
) -> gtk::ListBoxRow {
    setting_row(name, value, note, Some(swatch), false)
}

fn setting_row(
    name: &str,
    value: &str,
    note: &str,
    swatch: Option<&gtk::DrawingArea>,
    mark: bool,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("ui-row");
    row.add_css_class("setting-row");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let label = gtk::Label::new(Some(name));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(pango::EllipsizeMode::End);
    label.add_css_class("setting-label");
    let value = gtk::Label::new(Some(value));
    value.set_xalign(1.0);
    value.set_ellipsize(pango::EllipsizeMode::End);
    value.add_css_class("setting-value");
    line.append(&label);
    if let Some(swatch) = swatch {
        line.append(swatch);
    }
    line.append(&value);

    // The About mark belongs with its sentence, never on its own.
    if note.is_empty() {
        row.set_child(Some(&line));
        return row;
    }

    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.append(&line);
    let note_line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    if mark {
        note_line.append(&self::mark(22));
    }
    let note = gtk::Label::new(Some(note));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.add_css_class("row-note");
    note_line.append(&note);
    column.append(&note_line);
    row.set_child(Some(&column));
    row
}

/// A theme in miniature: its paper, a 1 px edge in its border colour, a 2 px
/// rule of ink and its accent dot. Settings shows one beside each theme's
/// name (spec/03-ui.md).
pub fn theme_swatch(theme: Theme) -> gtk::DrawingArea {
    swatch(theme, None)
}

/// Automatic in miniature: one rectangle split in two halves, the day theme on
/// the left and the night theme on the right.
pub fn automatic_swatch(day: Theme, night: Theme) -> gtk::DrawingArea {
    swatch(day, Some(night))
}

/// Big enough to read as a small page, small enough to sit in a row.
const SWATCH_WIDTH: i32 = 28;
const SWATCH_HEIGHT: i32 = 18;
const SWATCH_RADIUS: f64 = 4.0;

fn swatch(theme: Theme, night: Option<Theme>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(SWATCH_WIDTH);
    area.set_content_height(SWATCH_HEIGHT);
    area.set_halign(gtk::Align::End);
    area.set_valign(gtk::Align::Center);
    area.set_can_focus(false);
    area.set_focusable(false);
    area.set_accessible_role(gtk::AccessibleRole::Img);
    area.update_property(&[gtk::accessible::Property::Label("Theme swatch")]);
    area.set_draw_func(move |_, context, width, height| {
        let width = f64::from(width);
        let height = f64::from(height);
        match night {
            Some(night) => draw_automatic_swatch(
                context,
                width,
                height,
                Palette::of(theme),
                Palette::of(night),
            ),
            None => draw_swatch(context, width, height, Palette::of(theme)),
        }
    });
    area
}

/// One page: paper, the theme's own hairline edge, an ink rule and the dot.
fn draw_swatch(context: &gtk::cairo::Context, width: f64, height: f64, palette: Palette) {
    rounded_rect(
        context,
        0.5,
        0.5,
        (width - 1.0).max(1.0),
        (height - 1.0).max(1.0),
        SWATCH_RADIUS,
    );
    set_source(context, palette.background);
    let _ = context.fill_preserve();
    set_source(context, palette.border);
    context.set_line_width(1.0);
    let _ = context.stroke();
    draw_marks(context, 0.0, width, height, palette);
}

/// Day on the left, night on the right, one edge around both.
fn draw_automatic_swatch(
    context: &gtk::cairo::Context,
    width: f64,
    height: f64,
    day: Palette,
    night: Palette,
) {
    let half = width / 2.0;
    rounded_rect(
        context,
        0.5,
        0.5,
        (width - 1.0).max(1.0),
        (height - 1.0).max(1.0),
        SWATCH_RADIUS,
    );
    let _ = context.save();
    context.clip();
    set_source(context, day.background);
    let _ = context.paint();
    set_source(context, night.background);
    context.rectangle(half, 0.0, width - half, height);
    let _ = context.fill();
    let _ = context.restore();
    // The divide, then the day theme's edge around the whole rectangle.
    set_source(context, day.border);
    context.set_line_width(1.0);
    context.move_to(half, 1.0);
    context.line_to(half, height - 1.0);
    let _ = context.stroke();
    rounded_rect(
        context,
        0.5,
        0.5,
        (width - 1.0).max(1.0),
        (height - 1.0).max(1.0),
        SWATCH_RADIUS,
    );
    let _ = context.stroke();

    draw_marks(context, 0.0, half, height, day);
    draw_marks(context, half, width - half, height, night);
}

/// A 2 px rule of ink and the accent dot, centred in `width` from `x`.
fn draw_marks(context: &gtk::cairo::Context, x: f64, width: f64, height: f64, palette: Palette) {
    let pad = (width * 0.22).max(2.0);
    let rule = (width - 2.0 * pad).max(1.0);
    set_source(context, palette.text);
    context.rectangle(x + pad, height * 0.30, rule, 2.0);
    let _ = context.fill();
    set_source(context, palette.accent);
    context.arc(
        x + width * 0.50,
        height * 0.66,
        2.2,
        0.0,
        std::f64::consts::TAU,
    );
    let _ = context.fill();
}

fn set_source(context: &gtk::cairo::Context, hex: &str) {
    let (red, green, blue) = theme::rgb(hex);
    context.set_source_rgb(red, green, blue);
}

fn rounded_rect(
    context: &gtk::cairo::Context,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
) {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    use std::f64::consts::{FRAC_PI_2, PI};
    context.new_path();
    context.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
    context.arc(
        x + width - radius,
        y + height - radius,
        radius,
        0.0,
        FRAC_PI_2,
    );
    context.arc(x + radius, y + height - radius, radius, FRAC_PI_2, PI);
    context.arc(x + radius, y + radius, radius, PI, PI + FRAC_PI_2);
    context.close_path();
}

pub fn plain_row(name: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("ui-row");
    row.add_css_class("setting-row");
    let label = gtk::Label::new(Some(name));
    label.set_xalign(0.0);
    label.add_css_class("setting-label");
    row.set_child(Some(&label));
    row
}

pub fn select(list: &gtk::ListBox, index: usize) {
    let count = list.observe_children().n_items();
    if count == 0 {
        return;
    }
    let index = (index as u32).min(count - 1);
    if let Some(row) = list.row_at_index(index as i32) {
        list.select_row(Some(&row));
    }
}

pub fn selected_index(list: &gtk::ListBox) -> usize {
    list.selected_row()
        .map(|row| row.index().max(0) as usize)
        .unwrap_or(0)
}

/// Keep the selected row visible.
///
/// Screens rebuild their list on every key press, so the new rows have no
/// size yet when this is called and the bounds are empty. The scroll is done
/// again once the layout is done, which is what makes the last rows of a long
/// list (Settings at 800 px: Update, About) reachable with the keyboard.
pub fn scroll_to_row(scroller: &gtk::ScrolledWindow, list: &gtk::ListBox, row: &gtk::ListBoxRow) {
    scroll_now(scroller, list, row);
    let scroller = scroller.clone();
    let list = list.clone();
    let row = row.clone();
    glib::idle_add_local_once(move || scroll_now(&scroller, &list, &row));
}

fn scroll_now(scroller: &gtk::ScrolledWindow, list: &gtk::ListBox, row: &gtk::ListBoxRow) {
    let Some(bounds) = row.compute_bounds(list) else {
        return;
    };
    if bounds.height() <= 0.0 {
        return;
    }
    let adjustment = scroller.vadjustment();
    let top = f64::from(bounds.y());
    let bottom = top + f64::from(bounds.height());
    let value = adjustment.value();
    let page = adjustment.page_size();
    if top < value {
        adjustment.set_value(top);
    } else if bottom > value + page {
        adjustment.set_value(bottom - page);
    }
}

/// Local time for history and sync. Unix 0 means "never".
pub fn format_moment(unix: i64) -> String {
    if unix <= 0 {
        return "Never".to_owned();
    }
    let Ok(date) = glib::DateTime::from_unix_local(unix) else {
        return "Unknown time".to_owned();
    };
    let today = glib::DateTime::now_local()
        .ok()
        .and_then(|now| now.format("%Y-%m-%d").ok())
        .map(|date| date.to_string());
    let yesterday = glib::DateTime::now_local()
        .ok()
        .and_then(|now| now.add_days(-1).ok())
        .and_then(|day| day.format("%Y-%m-%d").ok())
        .map(|date| date.to_string());
    let day = date
        .format("%Y-%m-%d")
        .map(|day| day.to_string())
        .unwrap_or_default();
    let time = date
        .format("%H:%M")
        .map(|time| time.to_string())
        .unwrap_or_default();
    if today.as_deref() == Some(day.as_str()) {
        format!("Today, {time}")
    } else if yesterday.as_deref() == Some(day.as_str()) {
        format!("Yesterday, {time}")
    } else {
        let text = date
            .format("%d %b %Y, %H:%M")
            .map(|text| text.to_string())
            .unwrap_or_default();
        text.strip_prefix('0').unwrap_or(&text).to_owned()
    }
}

/// `12 Nov 2026`, for the dated "Everything" export folder.
pub fn local_date() -> String {
    glib::DateTime::now_local()
        .ok()
        .and_then(|date| date.format("%Y-%m-%d").ok())
        .map(|date| date.to_string())
        .unwrap_or_default()
}
