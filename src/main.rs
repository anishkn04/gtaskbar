use std::cell::Cell;

mod api;
mod auth;
mod config;
mod logfile;
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

    // First, so even startup failures land somewhere readable: a picker
    // launch has no terminal to show stderr on.
    logfile::init();

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

/// The running application, or `None` before it has been registered.
///
/// `adw::Application::default()` looks like it means this and does not.
/// `gio::Application` has *both* an inherent `default()`, which is the real
/// `g_application_get_default()`, and a `Default` impl that constructs a
/// brand-new, unregistered application. Rust does not inherit inherent methods
/// by subtype, so writing `adw::Application::default()` resolves to the trait
/// impl: it returns an object with no application id and no windows, and every
/// `if let Some(window) = app.windows().first()` on it quietly does nothing.
/// That is not a compile error and not a panic, it is a button that does
/// nothing at all.
///
/// Always go through the inherent method on the parent type and downcast.
pub fn running_app() -> Option<adw::Application> {
    gio::Application::default().and_then(|app| app.downcast::<adw::Application>().ok())
}

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

    // Deletes the selected task of the visible pane. Stands down while text
    // is being edited, so Delete keeps deleting characters where it should.
    let delete_selected = gio::ActionEntry::builder("delete-selected")
        .activate(|app: &adw::Application, _, _| {
            let Some(pane) = ui::tasklist_view::visible_pane() else {
                return;
            };
            if pane.editing_text() {
                return;
            }
            let Some(task_id) = pane.selected_task_id() else {
                return;
            };
            match crate::sync::queue::delete_task(&task_id) {
                Ok(()) => {
                    ui::window::set_status(app, Some("Deleted — syncing…"));
                    ui::window::rebuild(app);
                    crate::sync::scheduler::request_sync(app);
                }
                Err(reason) => {
                    log::warn!("{reason}");
                    ui::window::set_status(app, Some(&reason));
                }
            }
        })
        .build();

    let show_help = gio::ActionEntry::builder("show-help")
        .activate(|app: &adw::Application, _, _| show_help(app))
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
        delete_selected,
        show_help,
    ]);

    // Keyboard shortcuts, documented in the help overlay below. Delete is
    // deliberately scoped to the app action, which ignores it while text is
    // being edited, rather than to a window action that could not tell.
    app.set_accels_for_action("app.add-task", &["<Control>n"]);
    app.set_accels_for_action("win.search", &["<Control>f"]);
    app.set_accels_for_action("app.sync-now", &["<Control>r"]);
    app.set_accels_for_action("app.delete-selected", &["Delete"]);
    app.set_accels_for_action("app.show-help", &["F1"]);
}

/// The keyboard shortcuts, kept next to the actions they describe so the two
/// cannot drift apart.
fn show_help(app: &adw::Application) {
    let window = gtk::ShortcutsWindow::builder()
        .title("Keyboard Shortcuts")
        .modal(true)
        .build();

    let tasks = gtk::ShortcutsSection::builder()
        .title("Tasks")
        .visible(true)
        .build();
    let task_group = gtk::ShortcutsGroup::builder().visible(true).build();
    for (title, accel) in [
        ("Focus the quick-add field", "<ctrl>n"),
        ("Delete the selected task", "Delete"),
        ("Cancel what is typed", "Escape"),
    ] {
        task_group.add_shortcut(&shortcut_row(title, accel));
    }
    tasks.add_group(&task_group);

    let syncing = gtk::ShortcutsSection::builder()
        .title("Finding and syncing")
        .visible(true)
        .build();
    let sync_group = gtk::ShortcutsGroup::builder().visible(true).build();
    for (title, accel) in [
        ("Focus search", "<ctrl>f"),
        ("Sync now", "<ctrl>r"),
        ("Keyboard shortcuts", "F1"),
    ] {
        sync_group.add_shortcut(&shortcut_row(title, accel));
    }
    syncing.add_group(&sync_group);

    window.add_section(&tasks);
    window.add_section(&syncing);
    window.set_transient_for(app.active_window().as_ref());
    window.present();
}

fn shortcut_row(title: &str, accelerator: &str) -> gtk::ShortcutsShortcut {
    gtk::ShortcutsShortcut::builder()
        .title(title)
        .accelerator(accelerator)
        .build()
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

    fn install_script() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh");
        std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("could not read {}: {err}", path.display()))
    }

    /// Both entries used `Exec=gtaskbar`, a bare name that the spec resolves
    /// through `$PATH`. `~/.local/bin` is not on a graphical session's `PATH`
    /// the way it is in an interactive shell, and Qt launchers do not fall back
    /// to it the way GIO does, so the entry exec'd nothing and the launcher
    /// reported nothing. The same resolution applies to XDG autostart, so the
    /// app also never started at login. An absolute path is the only form every
    /// launcher agrees on, and the install directory is only known at install
    /// time, so it is a placeholder until then.
    #[test]
    fn both_entries_exec_a_placeholder_that_installs_to_an_absolute_path() {
        for name in ["gtaskbar.desktop", "gtaskbar-autostart.desktop"] {
            let exec = exec_line(name);
            assert_eq!(
                exec.split_whitespace().next(),
                Some("@bindir@/gtaskbar"),
                "{name} must exec @bindir@/gtaskbar, the only form that does not depend on PATH",
            );
        }
    }

    /// Guards against the placeholder being dropped or renamed on one side only,
    /// which would leave an entry that either cannot start or starts the wrong
    /// binary. Both entries have to go through the same substitution.
    #[test]
    fn install_script_substitutes_the_placeholder_for_both_entries() {
        let script = install_script();
        assert!(
            script.contains("s|@bindir@|$BIN_DIR|g"),
            "install.sh no longer substitutes @bindir@, so the entries would never become runnable",
        );
        assert_eq!(
            script.matches("install_desktop_entry \"").count(),
            2,
            "both the launcher entry and the autostart entry must be installed through the \
             substituting helper, or one of them keeps the raw placeholder",
        );
    }

    /// A leftover or misspelled placeholder, or an `Exec` that is still not an
    /// absolute path, reproduces the silent no-start failure, so install.sh
    /// checks for both rather than shipping an entry that cannot launch.
    #[test]
    fn install_script_refuses_to_install_an_unusable_exec() {
        let script = install_script();
        assert!(
            script.contains("grep -q '@bindir@'"),
            "install.sh must abort if the placeholder survives substitution",
        );
        assert!(
            script.contains("!= /* || ! -x"),
            "install.sh must abort if the substituted Exec is not an executable absolute path",
        );
    }

    /// The release tarball told people to install the desktop file by hand,
    /// which cannot substitute the placeholder, and is why the bare name was
    /// there in the first place. The tarball now carries install.sh, and
    /// install.sh accepts a binary sitting next to it rather than only in
    /// target/release, which is the tarball's layout.
    #[test]
    fn the_release_tarball_ships_the_installer_rather_than_a_manual_recipe() {
        let workflow = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/release.yml"),
        )
        .expect("could not read the release workflow");

        assert!(
            workflow.contains("cp install.sh \"$staging/\""),
            "the tarball must ship install.sh, the only install path that substitutes Exec",
        );
        assert!(
            !workflow.contains("install -Dm644 gtaskbar.desktop"),
            "the release notes still tell people to install the desktop file by hand, which \
             leaves the @bindir@ placeholder unsubstituted",
        );

        let script = install_script();
        assert!(
            script.contains("\"$SCRIPT_DIR/$BIN_NAME\""),
            "install.sh must accept the tarball's flat layout, or the shipped installer fails \
             for exactly the people it was packaged for",
        );
    }
}

#[cfg(test)]
mod application_tests {
    /// `adw::Application::default()` does not return the running application
    /// (see `crate::running_app`), and every use of it silently did nothing:
    /// the sidebar's connect button was dead and a successful sign-in never
    /// refreshed the window. This pins the ban so it cannot creep back in.
    /// Doc comments explaining the trap are exempt; code is not.
    #[test]
    fn no_code_reaches_for_the_wrong_default_application() {
        // Built in halves so the test does not match its own source.
        let needle = concat!("adw::Application::", "default()");
        let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src];
        let mut offenders = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let contents = std::fs::read_to_string(&path).expect("read source");
                let used_in_code = contents
                    .lines()
                    .any(|line| !line.trim_start().starts_with("//") && line.contains(needle));
                if used_in_code {
                    offenders.push(path);
                }
            }
        }
        assert!(
            offenders.is_empty(),
            concat!(
                "these files call adw::Application::",
                "default(), which builds a fresh unregistered application instead of returning \
                 the running one; use crate::running_app(): {0:?}"
            ),
            offenders
        );
    }

    /// The helper itself must consult the real default and hand back the
    /// application type the rest of the code holds. There is no GTK main loop
    /// in a unit test, so this only pins the lookup chain, not a live window.
    #[test]
    fn running_app_finds_nothing_before_startup() {
        // No application is registered in the test harness, so the helper must
        // report absence rather than fabricate an empty object, which was the
        // original failure mode.
        assert!(super::running_app().is_none());
    }
}
