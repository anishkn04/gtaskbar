use std::cell::Cell;

mod api;
mod auth;
mod config;
mod model;
mod notify;
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

    // Autostart launches `gtaskbar --hidden`. GApplication parses argv itself
    // and aborts with "Unknown option" and exit code 1 for a flag it does not
    // know, so every flag a desktop entry passes has to be declared here.
    register_main_options(&app);

    app.connect_startup(|_| load_css());

    let start_hidden = std::env::args().any(|arg| arg == "--hidden");
    if start_hidden {
        // Register the background machinery and the tray, then leave without a
        // window. A later launch from the app picker hands over to this process
        // and reaches the same `activate` handler below, which is what builds
        // the window, so nothing is lost by not building one now.
        app.connect_startup(ui::window::start_hidden);
    }

    // GApplication emits `activate` on the local instance immediately after
    // startup, whether or not the user asked for anything, which would defeat
    // --hidden by opening the window anyway. The first activation is therefore
    // swallowed on a hidden start. Every later one comes from another process
    // being handed over to this single instance, and those must open the
    // window, so only the first is dropped.
    let swallow_first_activate = Cell::new(start_hidden);
    app.connect_activate(move |app| {
        if swallow_first_activate.replace(false) {
            log::info!("ignoring the startup activation; started with --hidden");
            return;
        }
        ui::window::activate(app);
    });

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

/// The command-line options the app accepts, as (name, help) pairs.
///
/// GLib's option parser runs before any app code and exits 1 on an unknown
/// flag, so this list is what keeps a desktop entry working: a launcher shows no
/// error at all when the exec fails this early, which is what made the
/// autostart entry look broken.
///
/// The names have no leading dashes because GLib adds them. Passing "--hidden"
/// registers "----hidden", which is then rejected under the very name the
/// desktop entry uses.
///
/// There are no short forms, so `-h` keeps meaning `--help`.
const MAIN_OPTIONS: &[(&str, &str)] = &[("hidden", "Start in the tray without opening a window")];

fn register_main_options(app: &adw::Application) {
    for (name, description) in MAIN_OPTIONS {
        app.add_main_option(
            name,
            glib::Char::from(0),
            glib::OptionFlags::NONE,
            glib::OptionArg::None,
            description,
            None,
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
            // Drops the refresh token from the keyring as well as the in-memory
            // access token, so the next authorisation starts from scratch.
            auth::flow::sign_out();

            // Cached tasks belong to the account that was just disconnected, so
            // leaving them would show one account's data after switching to
            // another.
            match store::Store::open(&store::Store::cache_path()) {
                Ok(cache) => match cache.clear_all() {
                    Ok(()) => log::info!("disconnected and cleared the local cache"),
                    Err(err) => log::error!("disconnected, but could not clear the cache: {err}"),
                },
                Err(err) => log::error!("disconnected, but the cache could not be opened: {err}"),
            }

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

#[cfg(test)]
mod desktop_entry_tests {
    use super::MAIN_OPTIONS;

    fn entry(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("could not read {}: {err}", path.display()))
    }

    fn key<'a>(contents: &'a str, wanted: &str) -> Option<&'a str> {
        contents.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == wanted).then_some(value.trim())
        })
    }

    fn exec_line(name: &str) -> String {
        key(&entry(name), "Exec")
            .unwrap_or_else(|| panic!("{name} has no Exec line"))
            .to_string()
    }

    /// The autostart entry shipped a `--hidden` flag that the app did not
    /// declare. GOption exits 1 before any app code runs, and a launcher that
    /// fails this early reports nothing at all, so the app simply never started
    /// and looked like a broken launcher rather than a bad flag.
    #[test]
    fn every_flag_a_desktop_entry_passes_is_declared() {
        for name in ["gtaskbar.desktop", "gtaskbar-autostart.desktop"] {
            for token in exec_line(name).split_whitespace() {
                let Some(flag) = token.strip_prefix("--") else {
                    continue;
                };
                assert!(
                    MAIN_OPTIONS.iter().any(|(option, _)| *option == flag),
                    "{name} passes --{flag}, which the app does not declare, so GOption \
                     exits 1 and nothing starts",
                );
            }
        }
    }

    /// The entries carried `%U`, but the app has no MimeType and does not set
    /// HANDLES_OPEN, so GIO treats the substituted URL as a file to open and
    /// aborts with "This application can not open files".
    #[test]
    fn no_desktop_entry_substitutes_a_file_or_url() {
        for name in ["gtaskbar.desktop", "gtaskbar-autostart.desktop"] {
            let exec = exec_line(name);
            for field in ["%f", "%F", "%u", "%U", "%i", "%c", "%k"] {
                assert!(
                    !exec.contains(field),
                    "{name} substitutes {field} in Exec, but the app takes no arguments",
                );
            }
        }
    }

    /// An autostart entry is also a launchable application entry, so unless it
    /// is hidden it turns up in the launcher next to the real one. That put two
    /// identical "GTaskbar" entries in the app picker, and the broken autostart
    /// one was as easy to click as the working one.
    #[test]
    fn the_autostart_entry_is_hidden_from_the_launcher() {
        assert_eq!(
            key(&entry("gtaskbar-autostart.desktop"), "NoDisplay"),
            Some("true"),
            "the autostart entry is listed in the app picker as a duplicate",
        );
    }

    /// The launcher entry is the one users click, so it must stay visible.
    #[test]
    fn the_launcher_entry_is_visible() {
        let contents = entry("gtaskbar.desktop");
        assert_ne!(
            key(&contents, "NoDisplay"),
            Some("true"),
            "the launcher entry is hidden, so the app cannot be started from the picker",
        );
        assert_eq!(key(&contents, "Type"), Some("Application"));
    }

    #[test]
    fn both_entries_run_the_installed_binary() {
        for name in ["gtaskbar.desktop", "gtaskbar-autostart.desktop"] {
            let exec = exec_line(name);
            assert_eq!(
                exec.split_whitespace().next(),
                Some("gtaskbar"),
                "{name} should exec the bare binary name, which is what install.sh puts on PATH",
            );
        }
    }
}
