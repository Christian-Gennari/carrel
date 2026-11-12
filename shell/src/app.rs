//! Application wiring: one window, two screens, no network.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::battery::{Battery, FakeBattery};
use crate::editor::EditorPage;
use crate::files::FilesPage;
use crate::settings::{Settings, Theme};
use crate::store::{self, Store};
use crate::theme;

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
    }

    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("Stylos")
        .default_width(1280)
        .default_height(800)
        .decorated(false)
        .build();

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(150)
        .build();

    let battery: Rc<dyn Battery> = Rc::new(FakeBattery::default());
    let editor = EditorPage::new(
        store.clone(),
        Rc::clone(&settings),
        stack.clone(),
        provider.clone(),
        battery,
    );
    let files = FilesPage::new(store, Rc::clone(&editor), stack.clone());

    stack.add_named(&editor.widget, Some("editor"));
    stack.add_named(&files.widget, Some("files"));
    stack.set_visible_child_name("editor");
    window.set_child(Some(&stack));

    // Key events go to the focused widget, and the focused widget can change
    // (or disappear) when a screen does. A window-level capture controller
    // dispatches to the visible page, so keys never depend on focus luck.
    let key = gtk::EventControllerKey::new();
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    let editor_keys = Rc::clone(&editor);
    let files_keys = Rc::clone(&files);
    let stack_keys = stack.clone();
    key.connect_key_pressed(move |_, keyval, _, state| {
        let on_files = stack_keys
            .visible_child_name()
            .map(|name| name.as_str() == "files")
            .unwrap_or(false);
        if on_files {
            files_keys.handle_key(keyval, state)
        } else {
            editor_keys.handle_key(keyval, state)
        }
    });
    window.add_controller(key);

    if let Err(error) = editor.open_last_or_new() {
        eprintln!("stylos: cannot open a document: {error}");
    }
    editor.reapply_style();
    editor.focus_editor();

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
        .title("Stylos")
        .default_width(900)
        .default_height(600)
        .decorated(false)
        .build();
    let label = gtk::Label::new(Some(&format!(
        "Stylos could not open the writing folder.\n\n{}\n\n{}\n\nCreate the folder or set STYLOS_WRITING_DIR.",
        root.display(),
        error
    )));
    label.set_wrap(true);
    label.set_justify(gtk::Justification::Center);
    label.set_max_width_chars(48);
    label.add_css_class("stylos-page");
    window.set_child(Some(&label));
    window.present();
}
