//! `stylos-shell`: the only application on a Stylos device.

use gtk::prelude::*;

fn main() -> gtk::glib::ExitCode {
    let application = gtk::Application::builder()
        .application_id("org.stylos.Shell")
        // Kiosk session: exactly one instance, even without a session bus.
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application.connect_activate(stylos_shell::app::run);
    application.run()
}
