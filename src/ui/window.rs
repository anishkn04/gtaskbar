use std::cell::RefCell;

use adw::prelude::*;
use gtk::gio;
use gtk::gio::prelude::ApplicationExtManual;

use super::icons;
use crate::config::Config;

/// Starts the app with no window, for `gtaskbar --hidden`.
///
/// The autostart entry passes this: it wants the tray icon and the background
/// sync from login onwards, but a window appearing unprompted at every login is
/// the reason people disable autostart entries. Everything except the window is
/// the same as a normal start, so the app is fully functional and the tray's
/// "Open GTaskbar" builds the window on demand.
///
/// `activate` is still connected in this mode, so a later launch from the app
/// picker hands over to this process and opens the window.
pub fn start_hidden(app: &adw::Application) {
    let config = Config::load();
    crate::sync::scheduler::open_store();
    crate::notify::register_actions(app);
    crate::notify::init(app);
    crate::register_actions(app);
    crate::sync::scheduler::init(app, None, &config);

    // GApplication ends `run` as soon as the last window goes away, and here
    // there has never been one, so without a hold the process would exit
    // immediately after startup and autostart would do nothing at all. The hold
    // is dropped again once a window exists, because that keeps the app alive by
    // itself from then on.
    HIDE_HOLD.with(|slot| *slot.borrow_mut() = Some(app.hold()));
    log::info!("started hidden; the tray icon is the only way in");
}

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
            return glib::Propagation::Proceed;
        }

        // Only hide when there is genuinely somewhere to unhide from. Without a
        // tray this would leave the app running with no way to reach it.
        if Config::load().close_to_tray && crate::sync::scheduler::has_tray() {
            window.set_visible(false);
            return glib::Propagation::Stop;
        }

        glib::Propagation::Proceed
    });

    crate::ui::sidebar::register_actions(&window);
    register_window_actions(&window);
    crate::notify::register_actions(app);
    crate::notify::init(app);
    window.present();

    // The window now keeps the app running on its own, so the startup hold from
    // a `--hidden` start is no longer needed. Dropping it here also means the
    // app exits normally on quit rather than being pinned open by the hold.
    HIDE_HOLD.with(|slot| slot.borrow_mut().take());

    crate::register_actions(app);
    crate::sync::scheduler::init(app, Some(&window.clone().upcast()), &config);
}

/// Actions scoped to the window rather than the application.
///
/// `close` in particular has to be window-scoped: it must go through the
/// window's `close-request`, which decides between hiding to the tray and
/// actually quitting. Binding it to `app.quit` would skip that decision and
/// exit even when the user only meant to dismiss the window.
fn register_window_actions(window: &adw::ApplicationWindow) {
    use gtk::gio;

    let close = gio::ActionEntry::builder("close")
        .activate(|window: &adw::ApplicationWindow, _, _| window.close())
        .build();

    let search = gio::ActionEntry::builder("search")
        .activate(|_window: &adw::ApplicationWindow, _, _| {
            crate::ui::tasklist_view::focus_search();
        })
        .build();

    window.add_action_entries([close, search]);
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
    /// Keeps a `--hidden` start alive while it has no window.
    static HIDE_HOLD: RefCell<Option<gio::ApplicationHoldGuard>> = const { RefCell::new(None) };
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

/// Selects a task list in the sidebar, switching the content pane to it.
///
/// Used by the notification Open action. Selecting through the list box
/// reuses the same selection handler as a click, so the content stack
/// follows without any parallel bookkeeping.
pub fn reveal_list(app: &adw::Application, list_id: &str) {
    let key = crate::model::view::View::List(list_id.to_string()).key();
    let windows = app.windows();
    let Some(window) = windows
        .first()
        .and_then(|w| w.downcast_ref::<adw::ApplicationWindow>())
    else {
        return;
    };
    let Some(content) = window.content() else {
        return;
    };
    fn collect(widget: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
        out.push(widget.clone());
        let mut child = widget.first_child();
        while let Some(next) = child {
            collect(&next, out);
            child = next.next_sibling();
        }
    }
    let mut all = Vec::new();
    collect(&content.upcast(), &mut all);
    let Some(list) = all.iter().find_map(|widget| {
        widget
            .downcast_ref::<gtk::ListBox>()
            .filter(|list| list.has_css_class("gtaskbar-sidebar"))
    }) else {
        return;
    };
    let mut row = list.first_child();
    while let Some(candidate) = row {
        if let Some(list_row) = candidate.downcast_ref::<gtk::ListBoxRow>() {
            if list_row.widget_name() == key {
                list.select_row(Some(list_row));
                return;
            }
        }
        row = candidate.next_sibling();
    }
}

/// Focus (and reveal) the quick-add entry, creating the main window if the app
/// was launched with `--hidden` and has no window yet.
pub fn focus_quick_add(app: &adw::Application) {
    activate(app);
    crate::ui::tasklist_view::focus_quick_add();
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
    /// One entry per account list: id, title, and its own outstanding count.
    ///
    /// The count is computed per list here rather than in the sidebar, because
    /// counting the whole task slice once per row shows every list's badge as
    /// the account total. That is how an empty list ended up badged with
    /// another list's tasks.
    lists: Vec<(String, String, usize)>,
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

    let lists: Vec<(String, String, usize)> = lists
        .into_iter()
        // `key` is the model's own identifier accessor, used here so the
        // sidebar and the content stack agree on what identifies a list.
        .map(|list| {
            let id = list.key().to_string();
            let open = crate::sync::scheduler::with_store(|store| {
                store
                    .tasks_in_list(&id)
                    .map(|tasks| tasks.iter().filter(|task| !task.is_completed()).count())
                    .unwrap_or(0)
            })
            .unwrap_or(0);
            (id, list.title, open)
        })
        .collect();

    Snapshot { lists, smart_tasks }
}

fn tasks_for(view: &crate::model::view::View) -> Vec<(String, crate::store::models::Task)> {
    match view {
        crate::model::view::View::List(id) => {
            crate::sync::scheduler::with_store(|store| match store.tasks_in_list(id) {
                Ok(tasks) => tasks.into_iter().map(|task| (id.clone(), task)).collect(),
                Err(err) => {
                    log::warn!("could not read tasks for list {id}: {err}");
                    Vec::new()
                }
            })
            .unwrap_or_default()
        }
        // Every other view spans the whole account.
        _ => crate::sync::scheduler::with_store(|store| match store.all_tasks_with_lists() {
            Ok(pairs) => pairs,
            Err(err) => {
                log::warn!("could not read tasks from the cache: {err}");
                Vec::new()
            }
        })
        .unwrap_or_default(),
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
            .map(|(id, _, _)| crate::model::view::View::List(id.clone())),
    );

    let mut panes = Vec::with_capacity(views.len());

    for view in &views {
        let title = match view {
            crate::model::view::View::List(id) => snap
                .lists
                .iter()
                .find(|(candidate, _, _)| candidate == id)
                .map(|(_, name, _)| name.clone())
                .unwrap_or_else(|| "Tasks".to_string()),
            other => other.title(),
        };

        let pane = crate::ui::tasklist_view::TaskListView::new(view.clone(), &title);
        pane.render(&tasks_for(view), &snap.lists);
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

/// The state banner for the content panes, if any condition needs one.
///
/// Priority is deliberate: an account problem outranks connectivity, because
/// reconnecting is actionable while offline is usually transient, and a stale
/// sync error yields to both. Nothing here replaces the toasts, which report
/// individual writes; this is the persistent surface for states.
pub fn banner() -> Option<adw::Banner> {
    let (title, action) = banner_for(
        crate::auth::session::access_token().is_some(),
        crate::auth::credentials::refresh_token().is_some()
            || crate::auth::credentials::ClientCredentials::load().is_some(),
        crate::sync::scheduler::needs_reauth(),
        crate::sync::scheduler::online(),
        crate::sync::scheduler::last_sync_error(),
    )?;
    let banner = adw::Banner::new(&title);
    match action {
        None => {}
        Some(BannerAction::Connect { reconnect }) => {
            banner.set_button_label(Some(if reconnect { "Reconnect" } else { "Connect" }));
            banner.connect_button_clicked(|_| crate::ui::connect::present());
        }
        Some(BannerAction::Retry) => {
            banner.set_button_label(Some("Retry"));
            banner.connect_button_clicked(|_| {
                if let Some(app) = crate::running_app() {
                    crate::sync::scheduler::request_sync(&app);
                }
            });
        }
    }
    Some(banner)
}

/// What the banner shows, if anything.
///
/// Pure, so the priority order is testable without standing up windows:
/// account problems outrank connectivity, because reconnecting is actionable
/// while offline is usually transient, and a stale sync error yields to both.
/// `recoverable` means credentials exist to restore with; without them the
/// empty state (rather than a restore attempt) is the way back.
fn banner_for(
    connected: bool,
    recoverable: bool,
    needs_reauth: bool,
    online: bool,
    last_error: Option<String>,
) -> Option<(String, Option<BannerAction>)> {
    if needs_reauth {
        return Some((
            "Session expired — reconnect your Google account".to_string(),
            Some(BannerAction::Connect { reconnect: true }),
        ));
    }
    if !connected && !recoverable {
        return Some((
            "Not connected — add your Google account to load task lists".to_string(),
            Some(BannerAction::Connect { reconnect: false }),
        ));
    }
    if !online {
        return Some((
            "Offline — changes will sync when the network returns".to_string(),
            None,
        ));
    }
    if let Some(error) = last_error {
        return Some((error, Some(BannerAction::Retry)));
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BannerAction {
    Connect { reconnect: bool },
    Retry,
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
    secondary.append(Some("Keyboard Shortcuts"), Some("app.show-help"));
    secondary.append(Some("Preferences"), Some("app.preferences"));
    secondary.append(Some("About GTaskbar"), Some("app.about"));
    secondary.append(Some("Quit"), Some("app.quit"));
    menu.append_section(None, &secondary);

    menu
}

#[cfg(test)]
mod tests {
    use super::banner_for;
    use super::BannerAction;

    #[test]
    fn a_dead_session_outranks_everything() {
        let banner = banner_for(false, false, true, false, Some("old".into())).expect("banner");
        assert!(banner.0.contains("expired"));
        assert_eq!(banner.1, Some(BannerAction::Connect { reconnect: true }));
    }

    #[test]
    fn nothing_to_restore_with_means_connect() {
        let banner = banner_for(false, false, false, true, None).expect("banner");
        assert!(banner.0.contains("Not connected"));
        assert_eq!(banner.1, Some(BannerAction::Connect { reconnect: false }));
    }

    #[test]
    fn a_restore_in_flight_shows_no_banner() {
        // Token absent but credentials present: the restore has not finished,
        // and a banner now would flash on every launch.
        assert!(banner_for(false, true, false, true, None).is_none());
    }

    #[test]
    fn offline_beats_a_stale_sync_error() {
        let banner = banner_for(true, true, false, false, Some("boom".into())).expect("banner");
        assert!(banner.0.contains("Offline"));
        assert_eq!(banner.1, None);
    }

    #[test]
    fn a_failed_sync_offers_retry() {
        let banner = banner_for(true, true, false, true, Some("boom".into())).expect("banner");
        assert_eq!(banner.0, "boom");
        assert_eq!(banner.1, Some(BannerAction::Retry));
    }

    #[test]
    fn a_healthy_session_shows_nothing() {
        assert!(banner_for(true, true, false, true, None).is_none());
    }
}
