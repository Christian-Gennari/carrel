//! Small shared pieces for the keyboard-first screens: page columns, list
//! rows, time labels and the one way to show a key hint.

use gtk::prelude::*;
use gtk::{glib, pango};

/// The page itself: fills the window, carries the theme background.
pub fn page() -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.add_css_class("stylos-page");
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

/// Keep the selected row visible without waiting for a key repeat.
pub fn scroll_to_row(scroller: &gtk::ScrolledWindow, list: &gtk::ListBox, row: &gtk::ListBoxRow) {
    let Some(bounds) = row.compute_bounds(list) else {
        return;
    };
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
