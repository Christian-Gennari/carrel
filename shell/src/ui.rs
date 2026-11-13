//! Small shared pieces for the keyboard-first screens: page columns, list
//! rows, time labels and the one way to show a key hint.

use gtk::prelude::*;
use gtk::{glib, pango};

use crate::settings::Theme;
use crate::theme::{self, Palette};

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
    column.set_margin_top(52);
    column.set_margin_bottom(18);
    column
}

pub fn title(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("screen-title");
    label
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

pub fn hint(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("hint");
    label
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

/// A row with a label on the left and a value on the right.
pub fn value_row(name: &str, value: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("setting-row");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let label = gtk::Label::new(Some(name));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.add_css_class("setting-label");
    let value = gtk::Label::new(Some(value));
    value.set_xalign(1.0);
    value.set_ellipsize(pango::EllipsizeMode::End);
    value.add_css_class("setting-value");
    line.append(&label);
    line.append(&value);
    row.set_child(Some(&line));
    row
}

/// A value row with a small drawing (a theme swatch) before the value.
pub fn swatch_row(name: &str, swatch: &gtk::DrawingArea, value: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("setting-row");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let label = gtk::Label::new(Some(name));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.add_css_class("setting-label");
    let value_label = gtk::Label::new(Some(value));
    value_label.set_xalign(1.0);
    value_label.set_ellipsize(pango::EllipsizeMode::End);
    value_label.add_css_class("setting-value");
    line.append(&label);
    line.append(swatch);
    line.append(&value_label);
    row.set_child(Some(&line));
    row
}

/// A theme in miniature: its paper, two rules of its ink and its accent dot.
/// Settings shows one beside each theme's name (spec/03-ui.md).
pub fn theme_swatch(theme: Theme) -> gtk::DrawingArea {
    swatch(24, theme, None)
}

/// Automatic in miniature: the day theme beside the night theme.
pub fn automatic_swatch(day: Theme, night: Theme) -> gtk::DrawingArea {
    swatch(42, day, Some(night))
}

fn swatch(width: i32, theme: Theme, night: Option<Theme>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(width);
    area.set_content_height(24);
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
            Some(night) => {
                let gap = 2.0;
                let half = ((width - gap) / 2.0).max(1.0);
                draw_swatch(context, 0.0, half, height, Palette::of(theme));
                draw_swatch(context, half + gap, half, height, Palette::of(night));
            }
            None => draw_swatch(context, 0.0, width, height, Palette::of(theme)),
        }
    });
    area
}

/// One small page: paper fill, a hairline edge, ink rules and the accent dot.
fn draw_swatch(context: &gtk::cairo::Context, x: f64, width: f64, height: f64, palette: Palette) {
    let radius = (height * 0.18).min(4.0);
    rounded_rect(
        context,
        x + 0.5,
        0.5,
        (width - 1.0).max(1.0),
        (height - 1.0).max(1.0),
        radius,
    );
    set_source(context, palette.background);
    let _ = context.fill_preserve();
    set_source(
        context,
        &theme::blend(palette.text, palette.background, 0.30),
    );
    context.set_line_width(1.0);
    let _ = context.stroke();

    // One rule of ink and one accent dot: the theme's paper, ink and accent.
    let pad = (width * 0.20).max(2.0);
    let rule = (width - 2.0 * pad).max(1.0);
    let thick = (height * 0.13).max(1.5);
    set_source(context, palette.text);
    context.rectangle(x + pad, height * 0.32, rule, thick);
    let _ = context.fill();
    set_source(context, palette.accent);
    context.arc(
        x + width * 0.50,
        height * 0.66,
        (height * 0.12).max(1.5),
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
