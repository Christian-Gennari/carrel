//! Tests that need GTK itself: a real text buffer with undo, and the Quotes
//! palette driven by key presses against the fake daemon.
//!
//! GTK belongs to one thread and the test harness gives every test its own,
//! so every scenario runs from the one test below. Without a display it does
//! nothing; `dev/run.sh` runs it under Xvfb for real:
//!
//! ```text
//! Xvfb :98 & DISPLAY=:98 GDK_BACKEND=x11 cargo test -p carrel-shell --lib gtk_scenarios
//! ```

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::battery::SharedBattery;
use crate::daemon::{Client, FakeDaemon};
use crate::editor::{place_block, EditorPage};
use crate::quotes::QuotePalette;
use crate::settings::Settings;
use crate::store::Store;

#[test]
fn gtk_scenarios() {
    if gtk::init().is_err() {
        eprintln!("no display: GTK scenarios skipped");
        return;
    }
    sourceview::init();
    replacing_a_query_is_one_undoable_step();
    the_palette_follows_the_text_and_the_keys();
}

/// A buffer with `text`, the caret at its end and an empty undo history.
fn buffer_with(text: &str) -> sourceview::Buffer {
    let buffer = sourceview::Buffer::new(None);
    buffer.set_enable_undo(false);
    buffer.set_text(text);
    buffer.set_enable_undo(true);
    buffer.place_cursor(&buffer.end_iter());
    buffer
}

fn type_text(buffer: &sourceview::Buffer, text: &str) {
    for character in text.chars() {
        buffer.insert_at_cursor(&character.to_string());
    }
}

fn contents(buffer: &sourceview::Buffer) -> String {
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), true)
        .to_string()
}

const BLOCK: &str = "> A small room.\n\n[Hours](nostos:cite/q1)\n";

fn replacing_a_query_is_one_undoable_step() {
    // `@query` on its own line.
    let buffer = buffer_with("Intro\n\n");
    type_text(&buffer, "@small ro");
    let typed = contents(&buffer);
    assert_eq!(typed, "Intro\n\n@small ro");
    let to = buffer.end_iter().offset();
    place_block(&buffer, Some((7, to)), BLOCK);
    assert_eq!(contents(&buffer), format!("Intro\n\n{BLOCK}"));
    buffer.undo();
    assert_eq!(contents(&buffer), typed, "one undo restores @query");
    buffer.redo();
    assert_eq!(contents(&buffer), format!("Intro\n\n{BLOCK}"));

    // Mid-line: a new paragraph, without the trailing space.
    let buffer = buffer_with("As Mara wrote ");
    type_text(&buffer, "@quiet");
    let typed = contents(&buffer);
    let to = buffer.end_iter().offset();
    place_block(&buffer, Some((14, to)), BLOCK);
    assert_eq!(contents(&buffer), format!("As Mara wrote\n\n{BLOCK}"));
    buffer.undo();
    assert_eq!(contents(&buffer), typed);

    // Text after the query stays in its own paragraph.
    let buffer = buffer_with("see @qui and on");
    place_block(&buffer, Some((4, 8)), "[Hours](nostos:cite/q1)\n");
    assert_eq!(
        contents(&buffer),
        "see\n\n[Hours](nostos:cite/q1)\n\n and on"
    );

    // A plain insert keeps the old rules.
    let buffer = buffer_with("one line");
    place_block(&buffer, None, BLOCK);
    assert_eq!(contents(&buffer), format!("one line\n\n{BLOCK}"));
}

/// Run the main loop until `done` is true, so the daemon's answer arrives.
fn pump(done: impl Fn() -> bool) {
    let context = glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "the main loop timed out");
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn settle() {
    let context = glib::MainContext::default();
    let until = Instant::now() + Duration::from_millis(400);
    while Instant::now() < until {
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn press(palette: &Rc<QuotePalette>, key: gdk::Key, state: gdk::ModifierType) -> bool {
    let stopped = palette.handle_key(key, state) == glib::Propagation::Stop;
    // A key press ends with the idle that withdraws the palette's permission.
    settle();
    stopped
}

/// Type `text` as a writer does: every key goes to the palette first, then
/// into the buffer.
fn type_keys(palette: &Rc<QuotePalette>, buffer: &sourceview::Buffer, text: &str) {
    for character in text.chars() {
        // Any plain key: the palette only needs to know a key was pressed.
        palette.handle_key(gdk::Key::a, gdk::ModifierType::empty());
        buffer.insert_at_cursor(&character.to_string());
    }
    settle();
}

fn the_palette_follows_the_text_and_the_keys() {
    let root = tempfile::tempdir().expect("temp dir");
    let store = Store::open(root.path()).expect("store");
    let settings = Rc::new(RefCell::new(Settings::default()));
    let editor = EditorPage::new(
        store,
        settings,
        gtk::Stack::new(),
        gtk::CssProvider::new(),
        Rc::new(SharedBattery::new()),
    );
    editor.create_new_file().expect("new file");
    // The popover needs a window to stand on.
    let window = gtk::Window::new();
    window.set_default_size(1280, 800);
    window.set_child(Some(&editor.widget));
    window.present();
    settle();
    let client = Client::new(Arc::new(FakeDaemon::demo()));
    let palette = QuotePalette::new(client, Rc::clone(&editor));
    let buffer = editor.buffer();
    let none = gdk::ModifierType::empty();

    // An email address never opens it.
    type_keys(&palette, &buffer, "write to me@example.com");
    assert!(!palette.is_open(), "mid-word @ does nothing");

    // `@` at the start of a word does, and the typed words are the query.
    type_keys(&palette, &buffer, " and @attention");
    assert!(palette.is_open());
    let typed = contents(&buffer);
    assert!(typed.ends_with(" and @attention"), "{typed:?}");

    // Esc closes it and the text stays; typing on does not reopen it.
    assert!(press(&palette, gdk::Key::Escape, none));
    assert!(!palette.is_open());
    assert_eq!(contents(&buffer), typed, "Esc leaves the text as typed");
    type_keys(&palette, &buffer, "s");
    assert!(!palette.is_open(), "Esc keeps this @ closed");

    // Ctrl+' opens it at the caret with a fresh `@`; Esc takes it back.
    type_keys(&palette, &buffer, " ");
    let before = contents(&buffer);
    assert!(press(
        &palette,
        gdk::Key::apostrophe,
        gdk::ModifierType::CONTROL_MASK
    ));
    assert!(palette.is_open());
    assert_eq!(contents(&buffer), format!("{before}@"));
    assert!(press(&palette, gdk::Key::Escape, none));
    assert!(!palette.is_open());
    assert_eq!(contents(&buffer), before, "the stray @ is gone");

    // Choose with the keyboard: Down to the second result, Enter inserts the
    // quote and its citation in place of `@room`.
    type_keys(&palette, &buffer, "\n\n@room");
    assert!(palette.is_open());
    pump(|| palette.result_count() >= 2);
    let typed = contents(&buffer);
    assert!(press(&palette, gdk::Key::Down, none));
    assert!(press(&palette, gdk::Key::Return, none));
    assert!(!palette.is_open());
    let after = contents(&buffer);
    assert!(!after.contains("@room"), "{after:?}");
    assert!(
        after.contains("> Silence is not the absence of work"),
        "{after:?}"
    );
    assert!(after.contains("(nostos:cite/"), "{after:?}");
    buffer.undo();
    assert_eq!(contents(&buffer), typed, "one undo restores @room");

    // Shift+Enter inserts the citation alone.
    buffer.place_cursor(&buffer.end_iter());
    pump_open(&palette, &buffer);
    assert!(press(
        &palette,
        gdk::Key::Return,
        gdk::ModifierType::SHIFT_MASK
    ));
    let after = contents(&buffer);
    assert!(!after.contains("@room"), "{after:?}");
    assert!(!after.ends_with("\n> "), "{after:?}");
    assert!(after.trim_end().ends_with(")"), "{after:?}");
}

/// Bring the palette back on the `@room` at the end of the buffer by moving
/// the caret there with a key, and wait for its answer.
fn pump_open(palette: &Rc<QuotePalette>, buffer: &sourceview::Buffer) {
    palette.handle_key(gdk::Key::End, gdk::ModifierType::empty());
    buffer.place_cursor(&buffer.end_iter());
    settle();
    assert!(palette.is_open(), "the caret came back to @room");
    pump(|| palette.result_count() >= 1);
}
