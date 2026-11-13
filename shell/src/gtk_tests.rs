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
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::battery::SharedBattery;
use crate::daemon::{Client, FakeDaemon};
use crate::editor::{place_block, EditorPage};
use crate::export::ExportPage;
use crate::files::FilesPage;
use crate::first_run::FirstRunPage;
use crate::history::HistoryPage;
use crate::keys::Keys;
use crate::nav::Navigator;
use crate::palette::{CommandPalette, KeySheet};
use crate::quotes::QuotePalette;
use crate::settings::Settings;
use crate::settings_page::SettingsPage;
use crate::store::Store;
use crate::sync::SyncPage;
use crate::wifi::WifiPage;

#[test]
fn gtk_scenarios() {
    if gtk::init().is_err() {
        eprintln!("no display: GTK scenarios skipped");
        return;
    }
    sourceview::init();
    replacing_a_query_is_one_undoable_step();
    the_palette_follows_the_text_and_the_keys();
    the_command_palette_finds_every_action();
    the_key_sheet_is_behind_ctrl_slash_only();
    the_start_hint_waits_for_the_first_character();
    the_confirmation_keeps_the_safe_button_first();
    rename_happens_in_the_place_of_the_row();
    keystroke_baseline();
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

/// Ctrl+K opens one palette from the editor and from every screen, filters by
/// fuzzy words, runs an action and opens any file by name.
fn the_command_palette_finds_every_action() {
    let none = gdk::ModifierType::empty();
    let shell = TestShell::new(
        &[
            ("Alpha.md", "# Alpha\n"),
            ("Novel/Chapter one.md", "# Chapter one\n"),
            ("Zebra.md", "# Zebra\n"),
        ],
        "Alpha.md",
    );

    // The editor: the curated first rows, then "exp" lands on Export.
    shell.press("Ctrl+K", gdk::Key::k, gdk::ModifierType::CONTROL_MASK);
    settle();
    assert!(shell.palette.is_open());
    assert_eq!(shell.palette.labels()[0], "New file");
    shell.type_text("exp");
    assert_eq!(shell.palette.selected_label().as_deref(), Some("Export…"));
    shell.press("Esc", gdk::Key::Escape, none);
    assert!(!shell.palette.is_open());
    assert!(contents(&shell.editor.buffer()).contains("# Alpha"));

    // Every screen: the palette floats over Files and still opens a document.
    shell.press("Ctrl+O", gdk::Key::o, gdk::ModifierType::CONTROL_MASK);
    settle();
    shell.press("Ctrl+K", gdk::Key::k, gdk::ModifierType::CONTROL_MASK);
    settle();
    assert!(shell.palette.is_open());
    assert_eq!(shell.visible(), "files");
    shell.type_text("zebra");
    let label = shell.palette.selected_label().expect("a row");
    assert_eq!(label, "Open: Zebra");
    shell.press("Enter", gdk::Key::Return, none);
    settle();
    assert!(!shell.palette.is_open(), "running closes the palette");
    assert_eq!(shell.visible(), "editor");
    assert_eq!(
        shell.editor.current_rel().as_deref(),
        Some(Path::new("Zebra.md"))
    );

    // A theme by name, and the palette closes at once.
    shell.press("Ctrl+K", gdk::Key::k, gdk::ModifierType::CONTROL_MASK);
    shell.type_text("sepia");
    assert_eq!(
        shell.palette.selected_label().as_deref(),
        Some("Theme: Sepia")
    );
    shell.press("Enter", gdk::Key::Return, none);
    settle();
    assert!(!shell.palette.is_open());
    assert_eq!(shell.settings.borrow().theme.label(), "Sepia");

    // Focus mode is a toggle: the row says what Enter will do.
    shell.press("Ctrl+K", gdk::Key::k, gdk::ModifierType::CONTROL_MASK);
    shell.type_text("focus");
    assert_eq!(
        shell.palette.selected_label().as_deref(),
        Some("Turn focus mode on")
    );
    shell.press("Enter", gdk::Key::Return, none);
    settle();
    assert!(shell.settings.borrow().focus_mode);

    // First run is a closed flow: Ctrl+K does nothing there.
    shell.stack.set_visible_child_name("first_run");
    shell.press("Ctrl+K", gdk::Key::k, gdk::ModifierType::CONTROL_MASK);
    assert!(!shell.palette.is_open());
    shell.stack.set_visible_child_name("editor");

    // "Keyboard shortcuts" in the palette opens the same card as Ctrl+/.
    shell.press("Ctrl+K", gdk::Key::k, gdk::ModifierType::CONTROL_MASK);
    shell.type_text("keyboard");
    assert_eq!(
        shell.palette.selected_label().as_deref(),
        Some("Keyboard shortcuts")
    );
    shell.press("Enter", gdk::Key::Return, none);
    assert!(!shell.palette.is_open());
    assert!(shell.key_sheet.is_open());
    shell.press("Esc", gdk::Key::Escape, none);
    assert!(!shell.key_sheet.is_open());
}

/// Ctrl+/ opens the key sheet; `?` stays an ordinary character.
fn the_key_sheet_is_behind_ctrl_slash_only() {
    let none = gdk::ModifierType::empty();
    let shell = TestShell::new(&[("Alpha.md", "# Alpha\n")], "Alpha.md");

    shell.type_text("?");
    assert!(contents(&shell.editor.buffer()).ends_with('?'));
    assert!(!shell.key_sheet.is_open(), "? types a character");

    shell.press("Ctrl+/", gdk::Key::slash, gdk::ModifierType::CONTROL_MASK);
    settle();
    assert!(shell.key_sheet.is_open());
    assert!(!shell.palette.is_open());
    // The card owns the keyboard: a letter does not reach the text.
    let before = contents(&shell.editor.buffer());
    shell.press("x", gdk::Key::x, none);
    assert_eq!(contents(&shell.editor.buffer()), before);
    shell.press("Ctrl+K", gdk::Key::k, gdk::ModifierType::CONTROL_MASK);
    assert!(!shell.palette.is_open(), "the sheet stays on top of Ctrl+K");
    shell.press("Esc", gdk::Key::Escape, none);
    assert!(!shell.key_sheet.is_open());
}

/// The start hint appears in an empty document and leaves with the first
/// character; it never comes back in that document.
fn the_start_hint_waits_for_the_first_character() {
    let shell = TestShell::new(&[("Alpha.md", "# Alpha\n")], "Alpha.md");
    assert!(
        !shell.editor.start_hint_visible(),
        "a document with text shows no hint"
    );

    shell.press("Ctrl+N", gdk::Key::n, gdk::ModifierType::CONTROL_MASK);
    settle();
    assert!(shell.editor.start_hint_visible(), "the new file is empty");

    shell.type_text("H");
    assert!(
        !shell.editor.start_hint_visible(),
        "the first character clears it"
    );
    shell.editor.buffer().set_text("");
    settle();
    assert!(
        !shell.editor.start_hint_visible(),
        "it never returns in the same document"
    );
}

/// The one confirmation card starts on Cancel: Esc and Enter alone cancel,
/// and only Right then Enter reaches the destructive button.
fn the_confirmation_keeps_the_safe_button_first() {
    let shell = TestShell::new(
        &[("Alpha.md", "# Alpha\n"), ("Zebra.md", "# Zebra\n")],
        "Alpha.md",
    );
    shell.press("Ctrl+O", gdk::Key::o, gdk::ModifierType::CONTROL_MASK);
    settle();
    let alpha = shell.root.join("Alpha.md");
    assert!(alpha.is_file());

    shell.press("Delete", gdk::Key::Delete, gdk::ModifierType::empty());
    wait_for_dialog_button("Delete");
    shell.press("Esc", gdk::Key::Escape, gdk::ModifierType::empty());
    assert!(!has_dialog_button("Delete"), "Esc closed the card");
    assert!(alpha.is_file(), "Esc changed nothing");

    shell.press("Delete", gdk::Key::Delete, gdk::ModifierType::empty());
    wait_for_dialog_button("Delete");
    shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
    assert!(
        !has_dialog_button("Delete"),
        "Enter on the safe button closed the card"
    );
    assert!(alpha.is_file(), "the safe default changed nothing");

    shell.press("Delete", gdk::Key::Delete, gdk::ModifierType::empty());
    wait_for_dialog_button("Delete");
    shell.press("Right", gdk::Key::Right, gdk::ModifierType::empty());
    shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
    assert!(!alpha.exists(), "Right then Enter deleted the file");
}

/// F2 turns the selected row itself into the entry, with the name shown
/// without its extension, and Enter saves it back with the extension.
fn rename_happens_in_the_place_of_the_row() {
    let shell = TestShell::new(&[("Alpha.md", "# Alpha\n")], "Alpha.md");
    shell.press("Ctrl+O", gdk::Key::o, gdk::ModifierType::CONTROL_MASK);
    settle();
    shell.press("F2", gdk::Key::F2, gdk::ModifierType::empty());
    settle();
    let entries = gtk::Window::list_toplevels()
        .iter()
        .find_map(|window| visible_entry(window.upcast_ref()))
        .expect("the selected row is an entry");
    assert_eq!(entries.text(), "Alpha", "the .md extension is hidden");
    entries.set_text("Beta");
    shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
    settle();
    assert!(shell.root.join("Beta.md").is_file(), "renamed in place");
    assert!(!shell.root.join("Alpha.md").exists());
}

fn visible_entry(widget: &gtk::Widget) -> Option<gtk::Entry> {
    if !widget.is_visible() {
        return None;
    }
    if let Some(entry) = widget.downcast_ref::<gtk::Entry>() {
        return Some(entry.clone());
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        if let Some(found) = visible_entry(&widget) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

// ---------------------------------------------------------------------------
// Keystroke baseline.
//
// Each writer task runs against the real pages and the same window dispatcher
// the application installs (`keys::Keys`). The count is the number of key
// presses the writer makes: a chord is one press, every typed character is
// one. A confirmation card counts the two keys it takes to confirm it
// (`Right`, `Enter`); both reach the card's real buttons through the same
// dispatcher.
//
// The table is printed and written to `docs/ux-baseline.md`. The counts are
// asserted, so a later UI round that needs more keys fails here and must
// change the baseline deliberately.
// ---------------------------------------------------------------------------

const BASELINE: [(&str, usize); 11] = [
    ("Open an existing file by name", 4),
    ("Create a new file", 12),
    ("Insert a quote with @", 7),
    ("Start a sync", 1),
    ("Export the current file to USB as .docx", 6),
    ("Restore an earlier version", 5),
    ("Change the theme", 2),
    ("Open a file with the command palette", 7),
    ("Toggle focus mode with the command palette", 7),
    ("Set the theme to Sepia with the command palette", 7),
    ("Show the keyboard shortcuts", 1),
];

struct TestShell {
    root: PathBuf,
    stack: gtk::Stack,
    window: gtk::Window,
    editor: Rc<EditorPage>,
    quotes: Rc<QuotePalette>,
    palette: Rc<CommandPalette>,
    key_sheet: Rc<KeySheet>,
    keys: Rc<Keys>,
    settings: Rc<RefCell<Settings>>,
    daemon: Arc<FakeDaemon>,
    sequence: RefCell<Vec<String>>,
    // Dropped last: the writing folder outlives the pages that use it.
    _temp: tempfile::TempDir,
}

impl TestShell {
    /// A window with every screen, wired like `app::run`, on a fresh writing
    /// folder. `open` is the document the editor starts on.
    fn new(files: &[(&str, &str)], open: &str) -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().to_path_buf();
        for (relative, text) in files {
            let path = root.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("folder");
            }
            std::fs::write(path, text).expect("fixture file");
        }
        let store = Store::open(&root).expect("store");
        let settings = Rc::new(RefCell::new(Settings {
            first_run_complete: true,
            ..Settings::default()
        }));
        let provider = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        crate::theme::apply(&provider, &settings.borrow());

        let window = gtk::Window::builder()
            .title("Carrel test")
            .default_width(1280)
            .default_height(800)
            .decorated(false)
            .build();
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(150)
            .build();
        let daemon = Arc::new(FakeDaemon::new());
        let client = Client::new(daemon.clone());
        let battery: Rc<dyn crate::battery::Battery> = Rc::new(SharedBattery::new());

        let editor = EditorPage::new(
            store.clone(),
            Rc::clone(&settings),
            stack.clone(),
            provider,
            battery,
        );
        let nav = Navigator::new(stack.clone());
        nav.attach(&editor);
        let files = FilesPage::new(store.clone(), Rc::clone(&editor), stack.clone());
        let history = HistoryPage::new(client.clone(), Rc::clone(&nav), Rc::clone(&editor));
        let quotes = QuotePalette::new(client.clone(), Rc::clone(&editor));
        let sync = SyncPage::new(client.clone(), Rc::clone(&nav));
        let wifi = WifiPage::new(client.clone(), Rc::clone(&nav));
        let restyle: Rc<dyn Fn()> = {
            let editor = Rc::clone(&editor);
            Rc::new(move || editor.reapply_style())
        };
        let settings_page = SettingsPage::new(
            client.clone(),
            Rc::clone(&nav),
            Rc::clone(&settings),
            root.clone(),
            Rc::clone(&restyle),
        );
        let export = ExportPage::new(
            client.clone(),
            Rc::clone(&nav),
            Rc::clone(&editor),
            store.clone(),
        );
        let first_run = FirstRunPage::new(
            Rc::clone(&settings),
            root.clone(),
            Rc::clone(&nav),
            Rc::clone(&sync),
        );
        let dim = gtk::Box::new(gtk::Orientation::Vertical, 0);
        dim.add_css_class("overlay-dim");
        dim.set_hexpand(true);
        dim.set_vexpand(true);
        dim.set_can_target(true);
        dim.set_visible(false);
        let key_sheet = KeySheet::new(dim.clone());
        let palette = CommandPalette::new(
            client.clone(),
            store.clone(),
            Rc::clone(&settings),
            restyle,
            Rc::clone(&editor),
            Rc::clone(&nav),
            Rc::clone(&quotes),
            Rc::clone(&sync),
            Rc::clone(&settings_page),
            Rc::clone(&key_sheet),
            dim.clone(),
        );

        stack.add_named(&editor.widget, Some("editor"));
        stack.add_named(&files.widget, Some("files"));
        stack.add_named(&history.widget, Some("history"));
        stack.add_named(&sync.widget, Some("sync"));
        stack.add_named(&settings_page.widget, Some("settings"));
        stack.add_named(&wifi.widget, Some("wifi"));
        stack.add_named(&export.widget, Some("export"));
        stack.add_named(&first_run.widget, Some("first_run"));
        stack.set_visible_child_name("editor");
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&stack));
        overlay.add_overlay(&dim);
        overlay.add_overlay(&palette.widget);
        overlay.add_overlay(&key_sheet.widget);
        window.set_child(Some(&overlay));
        editor.open_rel(Path::new(open)).expect("open fixture");
        window.present();
        settle();

        let keys = Keys::new(
            stack.clone(),
            nav,
            Rc::clone(&editor),
            files,
            history,
            Rc::clone(&quotes),
            sync,
            settings_page,
            wifi,
            export,
            first_run,
            Rc::clone(&palette),
            Rc::clone(&key_sheet),
        );

        Self {
            root,
            stack,
            window,
            editor,
            quotes,
            palette,
            key_sheet,
            keys,
            settings,
            daemon,
            sequence: RefCell::new(Vec::new()),
            _temp: temp,
        }
    }

    /// One key press, recorded and dispatched through the window's dispatcher.
    fn press(&self, label: &str, key: gdk::Key, modifiers: gdk::ModifierType) {
        self.sequence.borrow_mut().push(label.to_owned());
        self.keys.handle(key, modifiers);
    }

    /// Type text as GTK delivers it: the key goes through the dispatcher
    /// first, then the character reaches the focused text view. While the
    /// command palette is open it takes the letters itself, as it does in the
    /// application.
    fn type_text(&self, text: &str) {
        let palette_open = self.palette.is_open();
        for character in text.chars() {
            let name = character.to_string();
            let (label, key) = match character {
                '\n' => ("Enter".to_owned(), gdk::Key::Return),
                ' ' => ("Space".to_owned(), gdk::Key::space),
                '@' => ("@".to_owned(), gdk::Key::at),
                other => (
                    other.to_string(),
                    gdk::Key::from_name(&name)
                        .or_else(|| gdk::Key::from_name(name.to_ascii_lowercase()))
                        .unwrap_or(gdk::Key::a),
                ),
            };
            self.press(&label, key, gdk::ModifierType::empty());
            if !palette_open {
                self.editor
                    .buffer()
                    .insert_at_cursor(&character.to_string());
            }
        }
        settle();
    }

    fn take_sequence(&self) -> Vec<String> {
        let mut sequence = self.sequence.borrow_mut();
        std::mem::take(&mut *sequence)
    }

    fn visible(&self) -> String {
        self.stack
            .visible_child_name()
            .map(|name| name.to_string())
            .unwrap_or_default()
    }
}

impl Drop for TestShell {
    fn drop(&mut self) {
        self.window.destroy();
    }
}

fn keystroke_baseline() {
    let mut measured: Vec<(&'static str, Vec<String>)> = Vec::new();

    // Open an existing file by name: the Files screen lists folders first,
    // then names alphabetically. Zebra.md is the third row.
    {
        let shell = TestShell::new(
            &[
                ("Novel/Chapter one.md", "# Chapter one\n"),
                ("Alpha.md", "# Alpha\n"),
                ("Zebra.md", "# Zebra\n"),
            ],
            "Alpha.md",
        );
        shell.press("Ctrl+O", gdk::Key::o, gdk::ModifierType::CONTROL_MASK);
        settle();
        for _ in 0..2 {
            shell.press("Down", gdk::Key::Down, gdk::ModifierType::empty());
        }
        shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
        assert_eq!(
            shell.editor.current_rel().as_deref(),
            Some(Path::new("Zebra.md")),
            "the third Files row opened"
        );
        assert!(contents(&shell.editor.buffer()).contains("# Zebra"));
        measured.push(("Open an existing file by name", shell.take_sequence()));
    }

    // Create a new file and name it with its first line.
    {
        let shell = TestShell::new(&[("Alpha.md", "# Alpha\n")], "Alpha.md");
        shell.press("Ctrl+N", gdk::Key::n, gdk::ModifierType::CONTROL_MASK);
        assert_eq!(
            shell
                .editor
                .current_rel()
                .and_then(|path| path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned()))
                .as_deref(),
            Some("Untitled.md")
        );
        shell.type_text("Chapter two");
        pump(|| shell.editor.current_rel().as_deref() == Some(Path::new("Chapter two.md")));
        assert!(
            shell.root.join("Chapter two.md").is_file(),
            "the new file was named from its first line"
        );
        measured.push(("Create a new file", shell.take_sequence()));
    }

    // Insert a quote with `@`: type the trigger and the query, pick the
    // second match with Down, insert with Enter.
    {
        let shell = TestShell::new(&[("Room.md", "Intro\n\n")], "Room.md");
        shell.type_text("@room");
        assert!(shell.quotes.is_open(), "the @ trigger opened the palette");
        pump(|| shell.quotes.result_count() >= 2);
        shell.press("Down", gdk::Key::Down, gdk::ModifierType::empty());
        shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
        assert!(!shell.quotes.is_open());
        let text = contents(&shell.editor.buffer());
        assert!(!text.contains("@room"), "{text:?}");
        assert!(
            text.contains("> Silence is not the absence of work"),
            "{text:?}"
        );
        assert!(text.contains("(nostos:cite/"), "{text:?}");
        measured.push(("Insert a quote with @", shell.take_sequence()));
    }

    // Start a sync from the editor.
    {
        let shell = TestShell::new(&[("Alpha.md", "# Alpha\n")], "Alpha.md");
        shell.press(
            "Ctrl+Shift+S",
            gdk::Key::s,
            gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK,
        );
        pump(|| shell.daemon.synced() == 1);
        assert_eq!(shell.visible(), "sync", "the Sync screen shows progress");
        measured.push(("Start a sync", shell.take_sequence()));
    }

    // Export the current file to the USB drive as .docx.
    {
        let shell = TestShell::new(
            &[("The quiet morning.md", "# The quiet morning\n")],
            "The quiet morning.md",
        );
        shell.press("Ctrl+E", gdk::Key::e, gdk::ModifierType::CONTROL_MASK);
        settle();
        shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
        for _ in 0..2 {
            shell.press("Down", gdk::Key::Down, gdk::ModifierType::empty());
        }
        shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
        settle();
        shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
        pump(|| !shell.daemon.exported().is_empty());
        let exported = shell.daemon.exported();
        assert_eq!(exported[0].1, "docx");
        assert_eq!(
            exported[0].0,
            vec!["current:The quiet morning.md".to_owned()]
        );
        measured.push((
            "Export the current file to USB as .docx",
            shell.take_sequence(),
        ));
    }

    // Restore an earlier version: open History, go to the second version,
    // Enter, then confirm the dialog with Right and Enter.
    {
        let shell = TestShell::new(
            &[("The quiet morning.md", "# The quiet morning\n")],
            "The quiet morning.md",
        );
        shell.press("Ctrl+H", gdk::Key::h, gdk::ModifierType::CONTROL_MASK);
        settle();
        shell.press("Down", gdk::Key::Down, gdk::ModifierType::empty());
        settle();
        shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
        wait_for_dialog_button("Restore");
        // Cancel, the safe button, has focus; Right reaches the real Restore
        // button and Enter activates it.
        shell.press("Right", gdk::Key::Right, gdk::ModifierType::empty());
        shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
        assert!(
            !has_dialog_button("Restore"),
            "Enter activated the Restore button and closed the card"
        );
        pump(|| contents(&shell.editor.buffer()).contains("The kettle clicks off at six"));
        measured.push(("Restore an earlier version", shell.take_sequence()));
    }

    // Change the theme: Settings opens on its first row, Right changes it.
    {
        let shell = TestShell::new(&[("Alpha.md", "# Alpha\n")], "Alpha.md");
        let before = shell.settings.borrow().theme;
        shell.press("Ctrl+,", gdk::Key::comma, gdk::ModifierType::CONTROL_MASK);
        settle();
        shell.press("Right", gdk::Key::Right, gdk::ModifierType::empty());
        assert_ne!(shell.settings.borrow().theme, before, "the theme changed");
        measured.push(("Change the theme", shell.take_sequence()));
    }

    // Open a document by its name: Ctrl+K, the name, Enter. The palette lists
    // the file under the actions, so no screen and no arrows are needed.
    {
        let shell = TestShell::new(
            &[("Alpha.md", "# Alpha\n"), ("Zebra.md", "# Zebra\n")],
            "Alpha.md",
        );
        shell.press("Ctrl+K", gdk::Key::k, gdk::ModifierType::CONTROL_MASK);
        shell.type_text("zebra");
        assert_eq!(
            shell.palette.selected_label().as_deref(),
            Some("Open: Zebra")
        );
        shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
        settle();
        assert_eq!(
            shell.editor.current_rel().as_deref(),
            Some(Path::new("Zebra.md"))
        );
        measured.push((
            "Open a file with the command palette",
            shell.take_sequence(),
        ));
    }

    // Toggle focus mode: Ctrl+K, "focus", Enter. The toggle applies at once.
    {
        let shell = TestShell::new(&[("Alpha.md", "# Alpha\n")], "Alpha.md");
        shell.press("Ctrl+K", gdk::Key::k, gdk::ModifierType::CONTROL_MASK);
        shell.type_text("focus");
        assert_eq!(
            shell.palette.selected_label().as_deref(),
            Some("Turn focus mode on")
        );
        shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
        settle();
        assert!(shell.settings.borrow().focus_mode);
        assert!(!shell.palette.is_open());
        measured.push((
            "Toggle focus mode with the command palette",
            shell.take_sequence(),
        ));
    }

    // Set the theme by name: Ctrl+K, "sepia", Enter.
    {
        let shell = TestShell::new(&[("Alpha.md", "# Alpha\n")], "Alpha.md");
        shell.press("Ctrl+K", gdk::Key::k, gdk::ModifierType::CONTROL_MASK);
        shell.type_text("sepia");
        assert_eq!(
            shell.palette.selected_label().as_deref(),
            Some("Theme: Sepia")
        );
        shell.press("Enter", gdk::Key::Return, gdk::ModifierType::empty());
        settle();
        assert_eq!(shell.settings.borrow().theme.label(), "Sepia");
        measured.push((
            "Set the theme to Sepia with the command palette",
            shell.take_sequence(),
        ));
    }

    // The key sheet: one chord from the editor.
    {
        let shell = TestShell::new(&[("Alpha.md", "# Alpha\n")], "Alpha.md");
        shell.press("Ctrl+/", gdk::Key::slash, gdk::ModifierType::CONTROL_MASK);
        assert!(shell.key_sheet.is_open());
        measured.push(("Show the keyboard shortcuts", shell.take_sequence()));
    }

    finish_baseline(&measured);
}

/// Print and write the table; fail when a count moved away from the baseline.
fn finish_baseline(measured: &[(&str, Vec<String>)]) {
    assert_eq!(measured.len(), BASELINE.len());
    let mut table = String::new();
    table.push_str("# UX baseline — keystrokes per writer task\n\n");
    table.push_str(
        "Measured by `shell/src/gtk_tests.rs` through the real pages and the \
         window's key dispatcher. A later UI round must not need more key \
         presses for the same task; the test asserts these counts.\n\n",
    );
    table.push_str("A chord counts once. Every typed character counts once. ");
    table.push_str("A confirmation dialog counts the keys that reach its button.\n\n");
    table.push_str("Regenerate with `dev/run.sh bash dev/ux-baseline.sh` (Xvfb, no GPU).\n\n");
    table.push_str("| Task | Keys pressed | Key sequence |\n");
    table.push_str("|---|---:|---|\n");

    println!("\nKeystroke baseline (a chord counts once):\n");
    println!("{:<44} {:>4}  Sequence", "Task", "Keys");
    for (index, (task, sequence)) in measured.iter().enumerate() {
        let expected = BASELINE[index].1;
        assert_eq!(
            sequence.len(),
            expected,
            "{task} needs {} keys, baseline is {expected}; update docs/ux-baseline.md and BASELINE deliberately",
            sequence.len()
        );
        println!("{:<44} {:>4}  {}", task, sequence.len(), sequence.join(" "));
        table.push_str(&format!(
            "| {task} | {} | {} |\n",
            sequence.len(),
            sequence.join(" ")
        ));
    }
    println!();

    let docs = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("docs")
        .join("ux-baseline.md");
    if std::fs::read_to_string(&docs).ok().as_deref() != Some(&table) {
        std::fs::write(&docs, table).expect("write docs/ux-baseline.md");
    }
}

fn widget_has_button(widget: &gtk::Widget, label: &str) -> bool {
    // A closed card hides its scrim; the buttons inside it are not on screen.
    if !widget.is_visible() {
        return false;
    }
    if let Some(button) = widget.downcast_ref::<gtk::Button>() {
        if button.label().as_deref() == Some(label) {
            return true;
        }
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        if widget_has_button(&widget, label) {
            return true;
        }
        child = widget.next_sibling();
    }
    false
}

fn has_dialog_button(label: &str) -> bool {
    gtk::Window::list_toplevels()
        .iter()
        .filter(|window| window.is_visible())
        .any(|window| widget_has_button(window.upcast_ref(), label))
}

fn wait_for_dialog_button(label: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !has_dialog_button(label) {
        assert!(
            Instant::now() < deadline,
            "the dialog with {label:?} did not appear"
        );
        glib::MainContext::default().iteration(false);
        std::thread::sleep(Duration::from_millis(5));
    }
}
