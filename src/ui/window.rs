use std::cell::RefCell;

use adw::prelude::*;
use gtk::gio;

use super::icons;
use crate::config::Config;

/// The single main window. Created once and reused for the lifetime of the
/// process, so the sidebar state and scroll position survive hide/show cycles
/// triggered from the tray.
pub fn activate(app: &adw::Application) {
    if let Some(window) = app
        .windows()
        .first()
        .and_then(|w| w.downcast_ref::<adw::ApplicationWindow>())
    {
        window.present();
        return;
    }

    let config = Config::load();

    // The cache must be open before the window is built: the sidebar reads the
    // cached task lists while constructing its widgets.
    crate::sync::scheduler::open_store();

    let root = build_ui(&config);
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&root));
    TOASTS.with(|slot| *slot.borrow_mut() = Some(toast_overlay.clone()));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("GTaskbar")
        .default_width(980)
        .default_height(680)
        .content(&toast_overlay)
        .build();

    // Closing the window hides it to the tray rather than quitting, so the
    // background sync and notification timers keep running. The "Quit" action
    // is the way to actually exit.
    window.connect_close_request(|window| {
        if allow_close_requested() {
            glib::Propagation::Proceed
        } else {
            window.set_visible(false);
            glib::Propagation::Stop
        }
    });

    window.present();

    crate::register_actions(app);
    crate::sync::scheduler::init(app, &window.clone().upcast(), &config);
}

/// Rebuilds the split view, so the sidebar reflects a change such as
/// connecting or disconnecting an account.
pub fn rebuild(app: &adw::Application) {
    let windows = app.windows();
    let Some(window) = windows
        .first()
        .and_then(|w| w.downcast_ref::<adw::ApplicationWindow>())
    else {
        return;
    };

    let config = Config::load();
    let root = build_ui(&config);

    // The window's content is always the ToastOverlay that `activate` created;
    // swapping its child preserves any toast that is already showing.
    let child = window.child();
    let is_overlay = matches!(
        child
            .as_ref()
            .and_then(|c| c.downcast_ref::<adw::ToastOverlay>()),
        Some(_)
    );

    if is_overlay {
        if let Some(overlay) = child
            .as_ref()
            .and_then(|c| c.downcast_ref::<adw::ToastOverlay>())
        {
            overlay.set_child(Some(&root));
            return;
        }
    }
    window.set_content(Some(&root));
}

thread_local! {
    static ALLOW_CLOSE: RefCell<bool> = const { RefCell::new(false) };
    static TOASTS: RefCell<Option<adw::ToastOverlay>> = const { RefCell::new(None) };
}

/// Shows a transient message at the bottom of the main window.
///
/// Safe to call from any thread: the work is marshalled onto the GLib main
/// context, because the sync worker reports back from a Tokio thread.
pub fn set_status(app: &adw::Application, message: Option<&str>) {
    let message = message.map(str::to_string);
    let app = app.clone();

    let show = move || {
        let Some(message) = message else { return };
        let Some(overlay) = TOASTS.with(|slot| slot.borrow().clone()) else {
            return;
        };
        if !app.windows().iter().any(|w| w.is_visible()) {
            return;
        }
        let toast = adw::Toast::new(&message);
        toast.set_timeout(4);
        overlay.add_toast(toast);
    };

    let main = glib::MainContext::default();
    if main.is_owner() {
        show();
    } else {
        main.spawn_local(async move { show() });
    }
}

fn allow_close_requested() -> bool {
    ALLOW_CLOSE.with(|flag| *flag.borrow())
}

pub fn present(app: &adw::Application) {
    if let Some(window) = app.windows().first() {
        window.present();
    }
}

/// Focus (and reveal) the quick-add entry, creating the main window if the app
/// was launched with `--hidden` and has no window yet.
pub fn focus_quick_add(app: &adw::Application) {
    activate(app);
}

/// Stop intercepting `close-request`, so the window really closes. Called from
/// the quit action just before `Application::quit`.
pub fn allow_close() {
    ALLOW_CLOSE.with(|flag| *flag.borrow_mut() = true);
}

fn build_ui(config: &Config) -> adw::NavigationSplitView {
    let root = adw::NavigationSplitView::builder()
        .min_sidebar_width(200.0)
        .max_sidebar_width(360.0)
        .sidebar_width_fraction(0.28)
        .build();

    // Both panes of a NavigationSplitView must be NavigationPages; each one
    // wraps an AdwViewStack so we can swap the inner page without rebuilding
    // the split view.
    let content_stack = adw::ViewStack::new();
    let placeholder = adw::StatusPage::builder()
        .title("GTaskbar")
        .description("Google Tasks client")
        .icon_name(icons::APP)
        .build();
    content_stack.add_titled(&placeholder, Some("placeholder"), "Overview");

    let content_page = adw::NavigationPage::builder()
        .title("Tasks")
        .child(&content_stack)
        .build();
    content_page.add_css_class("gtaskbar-content");

    root.set_content(Some(&content_page));
    root.set_sidebar(Some(&build_sidebar(config)));
    root
}

fn build_sidebar(config: &Config) -> adw::NavigationPage {
    let header = adw::HeaderBar::new();

    // A primary menu button in the header, matching GNOME convention.
    // `primary` is set through the GObject property API rather than the typed
    // setter, because the typed one is behind the crate's `v4_4` feature and
    // enabling it would require a newer GTK than some distributions ship.
    let menu_button = gtk::MenuButton::builder()
        .icon_name(icons::MENU)
        .tooltip_text("Main menu")
        .menu_model(&main_menu())
        .build();
    menu_button.set_property("primary", true);
    header.pack_end(&menu_button);

    let stack = adw::ViewStack::new();

    // Lists come from the local cache, so the sidebar renders instantly and
    // works with no network. A cache miss (first run, or not connected yet)
    // falls back to an explanatory empty state.
    let lists = crate::sync::scheduler::with_store(|store| match store.task_lists() {
        Ok(lists) => lists,
        Err(err) => {
            log::warn!("could not read task lists from the cache: {err}");
            Vec::new()
        }
    })
    .unwrap_or_default();

    if lists.is_empty() {
        let status = adw::StatusPage::builder()
            .icon_name(if crate::auth::session::is_connected() {
                icons::LIST
            } else {
                icons::ACCOUNT
            })
            .title(if crate::auth::session::is_connected() {
                "No task lists"
            } else {
                "Not connected"
            })
            .description(if crate::auth::session::is_connected() {
                format!(
                    "Your account is connected, but no task lists have arrived yet.\nSort: {} · Group: {}",
                    config.sort_mode.label(),
                    config.group_mode.label()
                )
            } else {
                "Connect your Google account to load your task lists.".to_string()
            })
            .build();

        // Only offer the connect action when there is something to connect.
        if !crate::auth::session::is_connected() {
            let connect = gtk::Button::with_label("Connect Google account");
            connect.add_css_class("pill");
            connect.add_css_class("suggested-action");
            connect.set_halign(gtk::Align::Center);
            connect.connect_clicked(|_| {
                crate::ui::connect::present();
            });
            status.set_child(Some(&connect));
        }

        stack.add_titled(&status, Some("status"), "Overview");
    } else {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["navigation-sidebar"])
            .build();
        list.add_css_class("gtaskbar-sidebar");

        for task_list in &lists {
            let incomplete = crate::sync::scheduler::with_store(|store| {
                store
                    .tasks_in_list(task_list.key())
                    .map(|tasks| tasks.iter().filter(|task| !task.is_completed()).count())
                    .unwrap_or(0)
            })
            .unwrap_or(0);

            let row = adw::ActionRow::builder()
                .title(&task_list.title)
                .activatable(true)
                .build();
            row.add_prefix(&icons::image(icons::LIST));
            if incomplete > 0 {
                let count = gtk::Button::builder()
                    .label(incomplete.to_string())
                    .css_classes(["flat", "circular", "suggested-action"])
                    .valign(gtk::Align::Center)
                    .build();
                count.set_sensitive(false);
                row.add_suffix(&count);
            }
            list.append(&row);
        }

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        stack.add_titled(&scroller, Some("lists"), "Task lists");
    }

    // A NavigationPage's header is part of its child, so stack the header above
    // the view stack inside a simple vertical box.
    let layout = gtk::Box::new(gtk::Orientation::Vertical, 0);
    layout.append(&header);
    layout.append(&stack);

    adw::NavigationPage::builder()
        .title("Tasks")
        .child(&layout)
        .build()
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();

    let primary = gio::Menu::new();
    primary.append(Some("Add Task"), Some("app.add-task"));
    primary.append(Some("Sync Now"), Some("app.sync-now"));
    if crate::auth::session::is_connected() {
        primary.append(Some("Disconnect Account"), Some("app.disconnect-account"));
    } else {
        primary.append(Some("Connect Account"), Some("app.connect-account"));
    }
    menu.append_section(None, &primary);

    let secondary = gio::Menu::new();
    secondary.append(Some("Preferences"), Some("app.preferences"));
    secondary.append(Some("About GTaskbar"), Some("app.about"));
    secondary.append(Some("Quit"), Some("app.quit"));
    menu.append_section(None, &secondary);

    menu
}
