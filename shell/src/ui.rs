//! Small shared pieces for the keyboard-first screens: the one page frame,
//! list rows, the key hint bar, the confirmation dialog, time labels.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::settings::Theme;
use crate::theme::{self, Palette};

/// The action a three-answer card runs with the chosen button's index.
type ChoiceAction = Rc<dyn Fn(usize)>;

/// The max width of every non-editor screen. About the editor column at the
/// default body size, so moving between the page and a screen does not change
/// where the text lives.
pub const COLUMN_WIDTH: i32 = 720;

/// The wide page frame, for a screen with two panes where one of them is text
/// to read (History today). It keeps the 720 px column's left edge, so the
/// title, the status and the hint bar still start where every other screen's
/// do, and grows to the right only: the diff gets a readable measure instead
/// of a narrow strip. 860 px gives the preview about 66 characters a line at
/// the default body size, and the measure does not change with the window.
pub const WIDE_COLUMN_WIDTH: i32 = 860;

/// The confirmation card's width. One width for every card, so a delete, a
/// restore and an unpair all look like the same object.
pub const DIALOG_WIDTH: i32 = 460;

/// The page frame's top margin, in proportion to the window: 64 px at
/// 1366×768 and 90 at 1920×1080. The editor keeps its first line at the same
/// proportion, so the screens follow it instead of hugging the top of a
/// taller page.
pub fn frame_top(height: i32) -> i32 {
    if height <= 0 {
        return 56;
    }
    (height / 12).clamp(56, 96)
}

/// The left margin that puts a column on the centred 720 px frame's edge. A
/// wide screen uses it as its own left margin, which is what keeps its title
/// under every other screen's title.
pub fn frame_margin(width: i32) -> i32 {
    ((width - COLUMN_WIDTH) / 2).max(24)
}

/// The wide frame's width at this window: never more than the measure above,
/// and never past the window's right edge on a small screen.
pub fn frame_wide(width: i32) -> i32 {
    let start = frame_margin(width);
    WIDE_COLUMN_WIDTH.min(width - start - 24).max(320)
}

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
    column: gtk::Box,
    title: gtk::Label,
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
    /// Three answers for a question that is not a plain confirmation (the
    /// Files screen's conflict copies). Hidden for every other card.
    choices: Vec<gtk::Button>,
    choice_focus: Cell<usize>,
    choice_action: RefCell<Option<ChoiceAction>>,
    /// A wide frame (History): same left edge, wider to the right.
    wide: bool,
}

impl Screen {
    /// The 720 px frame every screen uses.
    pub fn new(title_text: &str) -> Rc<Self> {
        Self::build(title_text, false)
    }

    /// The wide frame for a two-pane screen (History): the 720 px frame's
    /// left edge, so the title, the status and the hint bar start where every
    /// other screen's do, and the extra width goes to the pane that holds
    /// text to read.
    pub fn new_wide(title_text: &str) -> Rc<Self> {
        Self::build(title_text, true)
    }

    fn build(title_text: &str, wide: bool) -> Rc<Self> {
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

        let title = title(title_text);
        let column = column(if wide {
            WIDE_COLUMN_WIDTH
        } else {
            COLUMN_WIDTH
        });
        if wide {
            column.set_halign(gtk::Align::Start);
        }
        column.append(&title);
        column.append(&status);
        column.append(&content);
        column.append(&hint_slot);

        let page = page();
        page.append(&column);

        let dialog_title = gtk::Label::new(None);
        dialog_title.set_xalign(0.0);
        dialog_title.set_wrap(true);
        // Both labels are capped under the card's width, so the card itself is
        // always the same size and a long title wraps instead of stretching it.
        dialog_title.set_max_width_chars(32);
        dialog_title.add_css_class("dialog-title");
        let dialog_text = gtk::Label::new(None);
        dialog_text.set_xalign(0.0);
        dialog_text.set_wrap(true);
        dialog_text.set_max_width_chars(44);
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
        let mut choices = Vec::new();
        for _ in 0..3 {
            let button = gtk::Button::new();
            button.add_css_class("dialog-button");
            button.set_size_request(104, -1);
            button.set_visible(false);
            actions.append(&button);
            choices.push(button);
        }

        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.add_css_class("dialog-card");
        card.set_halign(gtk::Align::Center);
        card.set_valign(gtk::Align::Center);
        card.set_size_request(DIALOG_WIDTH, -1);
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
            column,
            title,
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
            choices,
            choice_focus: Cell::new(0),
            choice_action: RefCell::new(None),
            wide,
        });
        screen.install_dialog();
        screen.follow_page();
        screen.set_hints(&[]);
        screen
    }

    /// Keep the frame proportional to the window, the way the editor keeps
    /// its first line: the top margin grows with the height, and a wide frame
    /// keeps its left edge on the 720 px column's while it grows to the right.
    /// The page has its size when it is first mapped, and again each time the
    /// screen is shown, so that is when the frame is measured.
    fn follow_page(self: &Rc<Self>) {
        let page = self.page.clone();
        let mapped = page.clone();
        let weak = Rc::downgrade(self);
        page.connect_map(move |_| {
            let Some(screen) = weak.upgrade() else {
                return;
            };
            screen.resize(&mapped);
            // The first allocation can land just after the map; one idle is
            // enough to catch it.
            let page = mapped.clone();
            let weak = Rc::downgrade(&screen);
            glib::idle_add_local_once(move || {
                if let Some(screen) = weak.upgrade() {
                    screen.resize(&page);
                }
            });
        });
    }

    fn resize(&self, page: &gtk::Box) {
        let top = frame_top(page.height());
        if self.column.margin_top() != top {
            self.column.set_margin_top(top);
        }
        if !self.wide {
            return;
        }
        let width = page.width();
        if width <= 0 {
            return;
        }
        let start = frame_margin(width);
        if self.column.margin_start() != start {
            self.column.set_margin_start(start);
        }
        let wide = frame_wide(width);
        if self.column.width_request() != wide {
            self.column.set_width_request(wide);
        }
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
        for (index, button) in self.choices.iter().enumerate() {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(screen) = weak.upgrade() {
                    screen.accept_choice(index);
                }
            });
        }
    }

    pub fn focus_page(&self) {
        self.page.grab_focus();
    }

    /// Rename the page without rebuilding the frame: Export becomes Import
    /// for the import flow and its result.
    pub fn set_title(&self, text: &str) {
        if self.title.text() != text {
            self.title.set_text(text);
        }
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
        self.reset_choices();
        self.dialog_title.set_text(title);
        self.dialog_text.set_text(sentence);
        self.accept.set_label(accept);
        *self.action.borrow_mut() = Some(Rc::new(action));
        self.accept_focused.set(false);
        self.cancel.set_visible(true);
        self.accept.set_visible(true);
        self.scrim.set_visible(true);
        // Cancel is the safe button: Enter alone never destroys anything.
        self.cancel.grab_focus();
    }

    /// Show the same card with up to three answers instead of Cancel/accept.
    /// Esc still leaves everything as it is. `focus` names the safest answer
    /// (for a conflict copy, "Keep both"), and Left/Right/Tab move between
    /// them. The Files screen uses this for sync conflict copies.
    pub fn ask_choices(
        self: &Rc<Self>,
        title: &str,
        sentence: &str,
        labels: &[&str],
        focus: usize,
        action: impl Fn(usize) + 'static,
    ) {
        let count = labels.len().min(self.choices.len());
        self.dialog_title.set_text(title);
        self.dialog_text.set_text(sentence);
        *self.action.borrow_mut() = None;
        for (index, button) in self.choices.iter().enumerate() {
            if index < count {
                button.set_label(labels[index]);
                button.set_visible(true);
            } else {
                button.set_visible(false);
            }
        }
        *self.choice_action.borrow_mut() = Some(Rc::new(action));
        self.choice_focus.set(focus.min(count.saturating_sub(1)));
        self.cancel.set_visible(false);
        self.accept.set_visible(false);
        self.scrim.set_visible(true);
        self.focus_choice(self.choice_focus.get());
    }

    fn reset_choices(&self) {
        for button in &self.choices {
            button.set_visible(false);
        }
        *self.choice_action.borrow_mut() = None;
        self.choice_focus.set(0);
    }

    pub fn is_dialog_open(&self) -> bool {
        self.scrim.is_visible()
    }

    fn close_dialog(self: &Rc<Self>) {
        self.scrim.set_visible(false);
        *self.action.borrow_mut() = None;
        *self.choice_action.borrow_mut() = None;
        self.focus_page();
    }

    fn accept_dialog(self: &Rc<Self>) {
        let action = self.action.borrow_mut().take();
        self.close_dialog();
        if let Some(action) = action {
            action();
        }
    }

    fn accept_choice(self: &Rc<Self>, index: usize) {
        let action = self.choice_action.borrow_mut().take();
        self.close_dialog();
        if let Some(action) = action {
            action(index);
        }
    }

    fn focus_choice(&self, index: usize) {
        self.choice_focus.set(index);
        if let Some(button) = self.choices.get(index) {
            button.grab_focus();
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

    /// Route a key to the open card. Esc cancels, Enter activates the focused
    /// button, Left/Right/Tab move between the buttons.
    pub fn dialog_key(
        self: &Rc<Self>,
        keyval: gdk::Key,
        _state: gdk::ModifierType,
    ) -> glib::Propagation {
        if !self.is_dialog_open() {
            return glib::Propagation::Proceed;
        }
        let choice_count = self
            .choices
            .iter()
            .filter(|button| button.is_visible())
            .count();
        match keyval {
            gdk::Key::Escape => self.close_dialog(),
            gdk::Key::Return | gdk::Key::KP_Enter => {
                if choice_count > 0 {
                    self.accept_choice(self.choice_focus.get());
                } else if self.accept_focused.get() {
                    self.accept_dialog();
                } else {
                    self.close_dialog();
                }
            }
            gdk::Key::Left
            | gdk::Key::Right
            | gdk::Key::Tab
            | gdk::Key::ISO_Left_Tab
            | gdk::Key::KP_Tab => {
                if choice_count > 0 {
                    let backwards = matches!(keyval, gdk::Key::Left | gdk::Key::ISO_Left_Tab);
                    let current = self.choice_focus.get().min(choice_count - 1);
                    let next = if backwards {
                        (current + choice_count - 1) % choice_count
                    } else {
                        (current + 1) % choice_count
                    };
                    self.focus_choice(next);
                } else {
                    self.toggle_dialog_focus();
                }
            }
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

/// The hint bar that ends a floating card (palette, Quotes, key sheet): the
/// same keys, padded inside the card and with its rule across the full width.
pub fn card_hint_bar(hints: &[(&str, &str)]) -> gtk::Box {
    let bar = hint_bar(hints);
    bar.add_css_class("card-hint-bar");
    bar.set_halign(gtk::Align::Fill);
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
    // A list never widens the page frame: a long line or a long file name is
    // the list's business, not the frame's. Every screen keeps one width.
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_width(false)
        .vexpand(true)
        .child(child)
        .build()
}

/// Put a short fade over the top edge of a list. It appears only while the
/// list is scrolled, so a row at the edge dissolves into the page instead of
/// being cut in half under the title.
pub fn fade_top(scroller: &gtk::ScrolledWindow) -> gtk::Overlay {
    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    overlay.set_child(Some(scroller));

    let fade = gtk::DrawingArea::new();
    fade.set_content_height(24);
    fade.set_vexpand(false);
    fade.set_valign(gtk::Align::Start);
    fade.set_halign(gtk::Align::Fill);
    fade.set_can_target(false);
    fade.set_focusable(false);
    fade.add_css_class("scroll-fade-top");
    fade.set_visible(false);
    overlay.add_overlay(&fade);

    let adjustment = scroller.vadjustment();
    adjustment.connect_value_changed(move |adjustment| {
        fade.set_visible(adjustment.value() > 1.0);
    });
    overlay
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

/// One page: paper, a visible edge, an ink rule and the dot. The edge comes
/// from the ink, not the surface border, or a white swatch on white paper
/// would show only the rule.
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
    set_source(context, &palette.edge());
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
    // The divide, then the edge around the whole rectangle.
    set_source(context, &day.edge());
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

/// The one action on a screen ("Start writing", "Sync now"): the same row as
/// every other, with its name in the action weight, so it reads as the thing
/// Enter will do.
pub fn primary_row(name: &str) -> gtk::ListBoxRow {
    let row = plain_row(name);
    if let Some(label) = row
        .child()
        .and_then(|child| child.downcast::<gtk::Label>().ok())
    {
        label.add_css_class("primary-row");
    }
    row
}

/// A quiet group label inside any list ("Recent", "Connections", "Look"). It
/// takes no cursor: Up and Down step over it. One implementation, so every
/// group title starts on the same edge as the row text under it.
pub fn group_row(text: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    row.set_can_focus(false);
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("section-title");
    label.set_margin_start(14);
    label.set_margin_top(10);
    label.set_margin_bottom(2);
    row.set_child(Some(&label));
    row
}

/// A state that takes the place of the content: empty, running, done or an
/// error that stopped the screen. One headline line and one small detail line,
/// the same shape on every screen (spec/03-ui.md). A detail is a sentence, so
/// it keeps its full stop; the headline does not.
pub struct State {
    pub widget: gtk::Box,
    headline: gtk::Label,
    details: gtk::Box,
}

impl State {
    pub fn new() -> Self {
        let headline = gtk::Label::new(None);
        headline.set_xalign(0.0);
        headline.set_wrap(true);
        headline.add_css_class("state-headline");
        let details = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 6);
        widget.set_margin_top(8);
        widget.append(&headline);
        widget.append(&details);
        widget.set_visible(false);
        Self {
            widget,
            headline,
            details,
        }
    }

    /// Put the state on screen. An empty headline hides the block.
    pub fn set(&self, headline: &str, lines: &[&str], error: bool) {
        if error {
            self.headline.add_css_class("error-text");
        } else {
            self.headline.remove_css_class("error-text");
        }
        if headline.is_empty() {
            self.hide();
            return;
        }
        if self.headline.text() != headline {
            self.headline.set_text(headline);
        }
        while let Some(child) = self.details.first_child() {
            self.details.remove(&child);
        }
        for line in lines.iter().filter(|line| !line.is_empty()) {
            let label = gtk::Label::new(Some(line));
            label.set_xalign(0.0);
            label.set_wrap(true);
            label.add_css_class("state-detail");
            self.details.append(&label);
        }
        self.widget.set_visible(true);
    }

    pub fn hide(&self) {
        self.widget.set_visible(false);
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// A form row: a quiet field name in a fixed column, its value beside it and
/// an optional change mark at the right. Export's What / Format / To rows.
pub fn form_row(name: &str, value: &str, dim: bool, changeable: bool) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("ui-row");
    row.add_css_class("file-row");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let label = gtk::Label::new(Some(name));
    label.set_xalign(0.0);
    label.set_size_request(86, -1);
    label.add_css_class("setting-value");
    let value = gtk::Label::new(Some(value));
    value.set_xalign(0.0);
    value.set_hexpand(true);
    value.set_ellipsize(pango::EllipsizeMode::End);
    value.add_css_class(if dim { "row-note" } else { "file-name" });
    let arrows = gtk::Label::new(Some("‹ ›"));
    arrows.add_css_class("row-note");
    arrows.set_visible(changeable);
    line.append(&label);
    line.append(&value);
    line.append(&arrows);
    row.set_child(Some(&line));
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
    // A small margin means the selected row never sits against the edge.
    const MARGIN: f64 = 14.0;
    if top - MARGIN < value {
        adjustment.set_value((top - MARGIN).max(0.0));
    } else if bottom + MARGIN > value + page {
        adjustment.set_value(bottom + MARGIN - page);
    }
}

/// Local time for history and sync. Unix 0 means "never".
pub fn format_moment(unix: i64) -> String {
    let Some((date, today, yesterday)) = moment(unix) else {
        return never(unix);
    };
    let time = format_of(&date, "%H:%M");
    if today {
        format!("Today, {time}")
    } else if yesterday {
        format!("Yesterday, {time}")
    } else {
        let text = format_of(&date, "%d %b %Y, %H:%M");
        text.strip_prefix('0').unwrap_or(&text).to_owned()
    }
}

/// The compact form the History list uses: `Today, 15:13`, `11 Nov, 16:07`.
/// The year lives in the preview's title, so the row keeps its room for the
/// word change and the diff gets the width.
pub fn format_moment_short(unix: i64) -> String {
    let Some((date, today, yesterday)) = moment(unix) else {
        return never(unix);
    };
    let time = format_of(&date, "%H:%M");
    if today {
        format!("Today, {time}")
    } else if yesterday {
        format!("Yesterday, {time}")
    } else {
        let text = format_of(&date, "%d %b, %H:%M");
        text.strip_prefix('0').unwrap_or(&text).to_owned()
    }
}

fn never(unix: i64) -> String {
    if unix <= 0 {
        "Never".to_owned()
    } else {
        "Unknown time".to_owned()
    }
}

/// The local date, and whether it is today or yesterday. `None` when the
/// moment cannot be read at all.
fn moment(unix: i64) -> Option<(glib::DateTime, bool, bool)> {
    if unix <= 0 {
        return None;
    }
    let date = glib::DateTime::from_unix_local(unix).ok()?;
    let day = format_of(&date, "%Y-%m-%d");
    let today = glib::DateTime::now_local()
        .ok()
        .map(|now| format_of(&now, "%Y-%m-%d"))
        .as_deref()
        == Some(day.as_str());
    let yesterday = glib::DateTime::now_local()
        .ok()
        .and_then(|now| now.add_days(-1).ok())
        .map(|now| format_of(&now, "%Y-%m-%d"))
        .as_deref()
        == Some(day.as_str());
    Some((date, today, yesterday))
}

fn format_of(date: &glib::DateTime, format: &str) -> String {
    date.format(format)
        .map(|text| text.to_string())
        .unwrap_or_default()
}

/// `12 Nov 2026`, for the dated "Everything" export folder.
pub fn local_date() -> String {
    glib::DateTime::now_local()
        .ok()
        .and_then(|date| date.format("%Y-%m-%d").ok())
        .map(|date| date.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- UI round 10 (issue #38): the wide frame. ---
    #[test]
    fn the_wide_frame_keeps_the_720_column_left_edge() {
        // The left edge is the centred 720 px column's, so a wide screen's
        // title sits exactly under every other screen's title.
        for width in [1366, 1920] {
            let start = frame_margin(width);
            assert_eq!(start, (width - COLUMN_WIDTH) / 2, "{width}: left edge");
            assert_eq!(start + COLUMN_WIDTH, (width + COLUMN_WIDTH) / 2);
        }
        // It grows to the right only: the measure is the same at both sizes,
        // and it never passes the window's right edge on a small one.
        assert_eq!(frame_wide(1366), WIDE_COLUMN_WIDTH);
        assert_eq!(frame_wide(1920), WIDE_COLUMN_WIDTH);
        let small = 800;
        assert!(frame_margin(small) + frame_wide(small) <= small - 24);
        assert!(frame_wide(640) >= 320);
    }

    #[test]
    fn the_history_list_shows_a_short_moment() {
        // 2026-11-13 15:12 local is either today, yesterday or an older day,
        // depending on when the test runs; all three forms are short and keep
        // the time.
        let moment = glib::DateTime::new(&glib::TimeZone::local(), 2026, 11, 13, 15, 12, 0.0)
            .expect("a moment");
        let unix = moment.to_unix();
        let short = format_moment_short(unix);
        let full = format_moment(unix);
        assert!(short.contains("15:12"), "{short}");
        assert!(full.contains("15:12"), "{full}");
        assert!(!short.contains("2026"), "{short}");
        assert!(
            short.len() < full.len() || short == full,
            "{short} vs {full}"
        );
        assert_eq!(format_moment_short(0), "Never");
        assert_eq!(format_moment(0), "Never");
    }
    // --- end UI round 10 (issue #38) ---
}
