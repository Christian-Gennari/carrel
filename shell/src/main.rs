//! `carrel-shell`: the only application on a Carrel device.

use gtk::prelude::*;

fn main() -> gtk::glib::ExitCode {
    let application = gtk::Application::builder()
        .application_id("page.nostos.Carrel.Shell")
        // Kiosk session: exactly one instance, even without a session bus.
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application.connect_activate(carrel_shell::app::run);
    application.run()
}
