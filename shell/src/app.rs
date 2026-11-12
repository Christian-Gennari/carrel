//! Application wiring: one window, every screen, and the daemon boundary.
//!
//! The shell never touches the network. All daemon work goes through [`Client`]
//! on worker threads; the GTK main loop only receives callbacks and signals.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::battery::{Battery, SharedBattery};
use crate::daemon::{Backend, Client, DaemonClient, DaemonEvent, FakeDaemon, ZbusDaemon};
use crate::editor::EditorPage;
use crate::export::ExportPage;
use crate::files::FilesPage;
use crate::first_run::FirstRunPage;
use crate::history::HistoryPage;
use crate::nav::Navigator;
use crate::quotes::QuotesPage;
use crate::settings::{Settings, Theme};
use crate::settings_page::SettingsPage;
use crate::store::{self, Store};
use crate::sync::SyncPage;
use crate::theme;
use crate::wifi::WifiPage;

pub fn run(application: &gtk::Application) {
    let root = store::root_from_env();
    let store = match Store::open(&root) {
        Ok(store) => store,
        Err(error) => {
            present_root_error(application, &root, &error);
            return;
        }
    };

    let settings = Rc::new(RefCell::new(Settings::load(&root)));
    let provider = gtk::CssProvider::new();
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    theme::apply(&provider, &settings.borrow());
    if let Some(gtk_settings) = gtk::Settings::default() {
        gtk_settings.set_gtk_application_prefer_dark_theme(settings.borrow().theme == Theme::Night);
        // spec/03-ui.md: no caret animation, no keystroke sounds.
        gtk_settings.set_gtk_cursor_blink(false);
        gtk_settings.set_gtk_enable_event_sounds(false);
        gtk_settings.set_gtk_enable_input_feedback_sounds(false);
    }

    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("Carrel")
        .default_width(1280)
        .default_height(800)
        .decorated(false)
        .build();

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(150)
        .build();

    let daemon: Arc<dyn DaemonClient> = if std::env::var("CARREL_FAKE_DAEMON").as_deref() == Ok("1")
    {
        Arc::new(FakeDaemon::demo())
    } else {
        Arc::new(ZbusDaemon::new())
    };
    let client = Client::new(daemon);

    let battery = Rc::new(SharedBattery::new());
    let battery_trait: Rc<dyn Battery> = Rc::clone(&battery) as Rc<dyn Battery>;

    let editor = EditorPage::new(
        store.clone(),
        Rc::clone(&settings),
        stack.clone(),
        provider.clone(),
        battery_trait,
    );
    let nav = Navigator::new(stack.clone());
    nav.attach(&editor);

    let files = FilesPage::new(store.clone(), Rc::clone(&editor), stack.clone());
    let history = HistoryPage::new(client.clone(), Rc::clone(&nav), Rc::clone(&editor));
    let quotes = QuotesPage::new(client.clone(), Rc::clone(&nav), Rc::clone(&editor));
    let sync = SyncPage::new(client.clone(), Rc::clone(&nav));
    let wifi = WifiPage::new(client.clone(), Rc::clone(&nav));

    let restyle: Rc<dyn Fn()> = {
        let settings = Rc::clone(&settings);
        let provider = provider.clone();
        let editor = Rc::clone(&editor);
        Rc::new(move || {
            {
                let settings = settings.borrow();
                theme::apply(&provider, &settings);
                if let Some(gtk_settings) = gtk::Settings::default() {
                    gtk_settings
                        .set_gtk_application_prefer_dark_theme(settings.theme == Theme::Night);
                }
            }
            editor.reapply_style();
        })
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

    stack.add_named(&editor.widget, Some("editor"));
    stack.add_named(&files.widget, Some("files"));
    stack.add_named(&history.widget, Some("history"));
    stack.add_named(&quotes.widget, Some("quotes"));
    stack.add_named(&sync.widget, Some("sync"));
    stack.add_named(&settings_page.widget, Some("settings"));
    stack.add_named(&wifi.widget, Some("wifi"));
    stack.add_named(&export.widget, Some("export"));
    stack.add_named(&first_run.widget, Some("first_run"));
    stack.set_visible_child_name("editor");
    window.set_child(Some(&stack));

    // Battery changes go to the bottom line and to the 10 % / 5 % notes;
    // sync and pairing signals go to the screen that shows them.
    {
        let battery = Rc::clone(&battery);
        let editor = Rc::clone(&editor);
        let sync = Rc::clone(&sync);
        client.subscribe(move |event| match event {
            DaemonEvent::Battery(status) => {
                battery.set(status);
                editor.on_battery();
            }
            DaemonEvent::Sync(info) => sync.on_sync(info),
            DaemonEvent::Pair {
                state,
                code,
                message,
            } => sync.on_pair_status(&state, &code, message),
        });
    }

    // The first reading, without waiting for the daemon's 30-second tick.
    {
        let battery = Rc::clone(&battery);
        let editor = Rc::clone(&editor);
        client.spawn(
            |daemon| daemon.battery(),
            move |result| {
                if let Ok(status) = result {
                    battery.set(status);
                    editor.on_battery();
                }
            },
        );
    }

    // Key events go to the focused widget, and the focused widget can change
    // (or disappear) when a screen does. A window-level capture controller
    // dispatches to the visible page, so keys never depend on focus luck.
    let key = gtk::EventControllerKey::new();
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let nav = Rc::clone(&nav);
        let editor = Rc::clone(&editor);
        let files = Rc::clone(&files);
        let history = Rc::clone(&history);
        let quotes = Rc::clone(&quotes);
        let sync = Rc::clone(&sync);
        let settings_page = Rc::clone(&settings_page);
        let wifi = Rc::clone(&wifi);
        let export = Rc::clone(&export);
        let first_run = Rc::clone(&first_run);
        let stack = stack.clone();
        key.connect_key_pressed(move |_, keyval, _, state| {
            let visible = stack
                .visible_child_name()
                .map(|name| name.to_string())
                .unwrap_or_default();
            let control = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            let key = keyval.to_lower();

            // The global keys from spec/03-ui.md. First run is a closed flow:
            // only its own keys apply until the editor is reached.
            if control && visible != "first_run" {
                if key == gdk::Key::h {
                    nav.show("history");
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::s && shift {
                    sync.sync_now();
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::e {
                    nav.show("export");
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::comma {
                    nav.show("settings");
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::bracketright {
                    // One key opens and closes the palette from any screen.
                    if visible == "quotes" {
                        nav.show_editor();
                    } else {
                        quotes.open();
                    }
                    return glib::Propagation::Stop;
                }
            }

            match visible.as_str() {
                "files" => files.handle_key(keyval, state),
                "history" => history.handle_key(keyval, state),
                "quotes" => quotes.handle_key(keyval, state),
                "sync" => sync.handle_key(keyval, state),
                "settings" => settings_page.handle_key(keyval, state),
                "wifi" => wifi.handle_key(keyval, state),
                "export" => export.handle_key(keyval, state),
                "first_run" => first_run.handle_key(keyval, state),
                _ => editor.handle_key(keyval, state),
            }
        });
    }
    window.add_controller(key);

    if let Err(error) = editor.open_last_or_new() {
        eprintln!("carrel: cannot open a document: {error}");
    }
    editor.reapply_style();
    editor.focus_editor();

    // The screenshot script and the UI tests start on a named screen. In
    // production the variable is unset and the editor is the first screen.
    let start = std::env::var("CARREL_START_SCREEN").unwrap_or_default();
    let first_run_needed = !settings.borrow().first_run_complete;
    if start == "first-run" || (start.is_empty() && first_run_needed) {
        first_run.start();
        nav.show("first_run");
    } else {
        match start.as_str() {
            "files" => nav.show("files"),
            "history" => nav.show("history"),
            "quotes" => nav.show("quotes"),
            "sync" => nav.show("sync"),
            "settings" => nav.show("settings"),
            "wifi" => nav.show("wifi"),
            "export" => nav.show("export"),
            "export-drive" => export.start_at_drive(),
            "pair-nostos" => sync.open_pairing(Backend::Nostos),
            "pair-selfhosted" => sync.open_pairing(Backend::SelfHosted),
            "pair-selfhosted-code" => {
                sync.open_pairing_url(Backend::SelfHosted, "https://studio.local:8443")
            }
            "pair-gdrive" => sync.open_pairing(Backend::GoogleDrive),
            _ => {}
        }
    }

    let closing_editor = Rc::clone(&editor);
    window.connect_close_request(move |_| {
        closing_editor.maybe_name_untitled(true);
        closing_editor.flush();
        glib::Propagation::Proceed
    });

    window.present();
}

fn present_root_error(application: &gtk::Application, root: &Path, error: &std::io::Error) {
    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("Carrel")
        .default_width(900)
        .default_height(600)
        .decorated(false)
        .build();
    let label = gtk::Label::new(Some(&format!(
        "Carrel could not open the writing folder.\n\n{}\n\n{}\n\nCreate the folder or set CARREL_WRITING_DIR.",
        root.display(),
        error
    )));
    label.set_wrap(true);
    label.set_justify(gtk::Justification::Center);
    label.set_max_width_chars(48);
    label.add_css_class("carrel-page");
    window.set_child(Some(&label));
    window.present();
}
