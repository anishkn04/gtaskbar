mod api;
mod auth;
mod config;
mod store;
mod sync;
mod ui;

use adw::prelude::*;
use gtk::gio;

fn main() -> glib::ExitCode {
    // Registers the compiled GResource bundle (icons, stylesheet, menus) with
    // the default resource lookup path.
    gio::resources_register_include!("gtaskbar.gresource")
        .expect("failed to register GResource bundle");

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("gtaskbar=info"))
        .init();

    let app = adw::Application::builder()
        .application_id("dev.anishkn04.gtaskbar")
        .resource_base_path("/dev/anishkn04/gtaskbar")
        .build();

    app.connect_startup(|_| load_css());
    app.connect_activate(ui::window::activate);

    app.run()
}

/// Layer the app stylesheet beneath the system/user GTK theme, then on top of
/// libadwaita's own defaults. Using `Application` + `CssProvider` with a low
/// priority keeps user theme CSS winning over ours.
fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_resource("/dev/anishkn04/gtaskbar/ui/style.css");

    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Shared Gio actions registered on the application object.
pub fn register_actions(app: &adw::Application) {
    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| {
            // Stop intercepting close-request so the window really goes away,
            // then tear the application down.
            ui::window::allow_close();
            app.quit();
        })
        .build();

    let about = gio::ActionEntry::builder("about")
        .activate(|app: &adw::Application, _, _| {
            let dialog = about_dialog();
            dialog.present(app.active_window().as_ref());
        })
        .build();

    let sync_now = gio::ActionEntry::builder("sync-now")
        .activate(|app: &adw::Application, _, _| {
            crate::sync::scheduler::request_sync(app);
        })
        .build();

    let add_task = gio::ActionEntry::builder("add-task")
        .activate(|app: &adw::Application, _, _| {
            ui::window::focus_quick_add(app);
        })
        .build();

    let show_window = gio::ActionEntry::builder("show-window")
        .activate(|app: &adw::Application, _, _| ui::window::present(app))
        .build();

    let connect = gio::ActionEntry::builder("connect-account")
        .activate(|app: &adw::Application, _, _| {
            if let Some(window) = app
                .windows()
                .first()
                .and_then(|w| w.downcast_ref::<adw::ApplicationWindow>())
            {
                ui::connect::present_for(window.upcast_ref());
            }
        })
        .build();

    let disconnect = gio::ActionEntry::builder("disconnect-account")
        .activate(|app: &adw::Application, _, _| {
            auth::session::clear();
            log::info!("disconnected; cached tasks left in place until the next clear");
            ui::window::rebuild(app);
        })
        .build();

    let preferences = gio::ActionEntry::builder("preferences")
        .activate(|app: &adw::Application, _, _| ui::settings::present(app))
        .build();

    app.add_action_entries([
        quit,
        about,
        sync_now,
        add_task,
        show_window,
        connect,
        disconnect,
        preferences,
    ]);
}

fn about_dialog() -> adw::AboutDialog {
    adw::AboutDialog::builder()
        .application_name("GTaskbar")
        .application_icon("dev.anishkn04.gtaskbar")
        .developer_name("Anish Kumar Neupane")
        .version(env!("CARGO_PKG_VERSION"))
        .license_type(gtk::License::MitX11)
        .comments("A native, tray-first Google Tasks client for Linux")
        .website("https://github.com/anishkn04/gtaskbar")
        .issue_url("https://github.com/anishkn04/gtaskbar/issues")
        .developers(vec!["Anish Kumar Neupane"])
        .build()
}
