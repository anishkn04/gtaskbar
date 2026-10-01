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

    crate::ui::sidebar::register_actions(&window);
    crate::notify::register_actions(app);
    crate::notify::init(app);
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
    let content = window.child();
    if let Some(overlay) = content
        .as_ref()
        .and_then(|c| c.downcast_ref::<adw::ToastOverlay>())
    {
        overlay.set_child(Some(&root));
        return;
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
    crate::ui::tasklist_view::focus_quick_add();
}

/// Focus the search field of the visible pane.
pub fn focus_search(app: &adw::Application) {
    activate(app);
    crate::ui::tasklist_view::focus_search();
}

/// Stop intercepting `close-request`, so the window really closes. Called from
/// the quit action just before `Application::quit`.
pub fn allow_close() {
    ALLOW_CLOSE.with(|flag| *flag.borrow_mut() = true);
}
/// What the UI needs from the cache, read once per rebuild.
///
/// Smart views span every list, so they read `all_tasks`; a list view reads
/// only its own rows via `tasks_in_list`. The store already scopes by list, so
/// the UI never has to infer membership by hand.
struct Snapshot {
    lists: Vec<(String, String)>,
    smart_tasks: Vec<crate::store::models::Task>,
}

fn snapshot() -> Snapshot {
    let lists = crate::sync::scheduler::with_store(|store| match store.task_lists() {
        Ok(lists) => lists,
        Err(err) => {
            log::warn!("could not read task lists from the cache: {err}");
            Vec::new()
        }
    })
    .unwrap_or_default();

    let smart_tasks = crate::sync::scheduler::with_store(|store| match store.all_tasks() {
        Ok(tasks) => tasks,
        Err(err) => {
            log::warn!("could not read tasks from the cache: {err}");
            Vec::new()
        }
    })
    .unwrap_or_default();

    Snapshot {
        lists: lists
            .into_iter()
            // `key` is the model's own identifier accessor, used here so the
            // sidebar and the content stack agree on what identifies a list.
            .map(|list| (list.key().to_string(), list.title))
            .collect(),
        smart_tasks,
    }
}

fn tasks_for(view: &crate::model::view::View, snap: &Snapshot) -> Vec<crate::store::models::Task> {
    match view {
        crate::model::view::View::List(id) => {
            crate::sync::scheduler::with_store(|store| match store.tasks_in_list(id) {
                Ok(tasks) => tasks,
                Err(err) => {
                    log::warn!("could not read tasks for list {id}: {err}");
                    Vec::new()
                }
            })
            .unwrap_or_default()
        }
        // Every other view spans the whole account.
        _ => snap.smart_tasks.clone(),
    }
}

fn build_ui(_config: &Config) -> adw::NavigationSplitView {
    let snap = snapshot();

    let root = adw::NavigationSplitView::builder()
        .min_sidebar_width(200.0)
        .max_sidebar_width(360.0)
        .sidebar_width_fraction(0.28)
        .build();

    let content_stack = adw::ViewStack::new();

    // Smart views first, then each account's own lists.
    let mut views: Vec<crate::model::view::View> = vec![
        crate::model::view::View::Today,
        crate::model::view::View::Upcoming,
        crate::model::view::View::Overdue,
        crate::model::view::View::All,
        crate::model::view::View::Completed,
    ];
    views.extend(
        snap.lists
            .iter()
            .map(|(id, _)| crate::model::view::View::List(id.clone())),
    );

    let mut panes = Vec::with_capacity(views.len());

    for view in &views {
        let title = match view {
            crate::model::view::View::List(id) => snap
                .lists
                .iter()
                .find(|(candidate, _)| candidate == id)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| "Tasks".to_string()),
            other => other.title(),
        };

        let pane = crate::ui::tasklist_view::TaskListView::new(view.clone(), &title);
        pane.render(&tasks_for(view, &snap));
        content_stack.add_titled(&pane.root, Some(&view.key()), &title);
        panes.push(pane);
    }

    crate::ui::tasklist_view::register_panes(panes);

    if views.is_empty() {
        let placeholder = adw::StatusPage::builder()
            .title("GTaskbar")
            .description("Connect your Google account to get started.")
            .icon_name(icons::APP)
            .build();
        content_stack.add_titled(&placeholder, Some("welcome"), "Welcome");
    }

    let content_page = adw::NavigationPage::builder()
        .title("Tasks")
        .child(&content_stack)
        .build();
    content_page.add_css_class("gtaskbar-content");

    // The sidebar names each row after its view key, so switching panes is a
    // lookup rather than a walk of the widget tree.
    let sidebar_list = crate::ui::sidebar::build(&snap.smart_tasks, &snap.lists);

    if !snap.lists.is_empty() {
        sidebar_list.connect_row_selected(glib::clone!(
            #[weak]
            content_stack,
            move |_, row| {
                let Some(row) = row else { return };
                let name = row.widget_name().to_string();
                if !name.is_empty() {
                    content_stack.set_visible_child_name(&name);
                }
            }
        ));
    }

    root.set_content(Some(&content_page));
    root.set_sidebar(Some(&build_sidebar_page(&sidebar_list, &snap)));
    root
}

fn build_sidebar_page(sidebar_list: &gtk::ListBox, snap: &Snapshot) -> adw::NavigationPage {
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

    let layout = gtk::Box::new(gtk::Orientation::Vertical, 0);
    layout.append(&header);

    if snap.lists.is_empty() {
        let connected = crate::auth::session::is_connected();
        let status = adw::StatusPage::builder()
            .icon_name(if connected {
                icons::LIST
            } else {
                icons::ACCOUNT
            })
            .title(if connected {
                "No task lists"
            } else {
                "Not connected"
            })
            .description(if connected {
                "Your account is connected, but no task lists have arrived yet."
            } else {
                "Connect your Google account to load your task lists."
            })
            .build();

        if !connected {
            let connect = gtk::Button::with_label("Connect Google account");
            connect.add_css_class("pill");
            connect.add_css_class("suggested-action");
            connect.set_halign(gtk::Align::Center);
            connect.connect_clicked(|_| crate::ui::connect::present());
            status.set_child(Some(&connect));
        }

        layout.append(&status);
    } else {
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(sidebar_list)
            .build();
        layout.append(&scroller);
    }

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
