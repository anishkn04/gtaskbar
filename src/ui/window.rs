use std::cell::RefCell;

use adw::prelude::*;
use gtk::gio;

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

    let root = build_ui(&config);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("GTaskbar")
        .default_width(980)
        .default_height(680)
        .content(&root)
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

thread_local! {
    static ALLOW_CLOSE: RefCell<bool> = const { RefCell::new(false) };
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
        .icon_name("org.gnome.Todo")
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
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main menu")
        .menu_model(&main_menu())
        .primary(true)
        .build();
    header.pack_end(&menu_button);

    let stack = adw::ViewStack::new();

    // Sidebar contents are wired up in the UI step; for now show the current
    // configuration so the shell is verifiable.
    let status = adw::StatusPage::builder()
        .icon_name("view-list-symbolic")
        .title("No lists yet")
        .description(format!(
            "Connect your Google account to load your task lists.\nSort: {} · Group: {}",
            config.sort_mode.label(),
            config.group_mode.label()
        ))        .build();
    stack.add_titled(&status, Some("status"), "Overview");

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
    menu.append_section(None, &primary);

    let secondary = gio::Menu::new();
    secondary.append(Some("Preferences"), Some("app.preferences"));
    secondary.append(Some("About GTaskbar"), Some("app.about"));
    secondary.append(Some("Quit"), Some("app.quit"));
    menu.append_section(None, &secondary);

    menu
}
