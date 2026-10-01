use std::cell::RefCell;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use crate::api::TasksClient;
use crate::config::Config;
use crate::store::Store;

use super::engine;

/// Minimum interval between two sync attempts, regardless of what the
/// scheduler timer would otherwise allow. Guards against hammering the API
/// (courtesy limit is 50,000 queries/day) when the user holds down "Sync Now".
const MIN_SYNC_INTERVAL: Duration = Duration::from_secs(15);

thread_local! {
    static LAST_SYNC: RefCell<Option<i64>> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
    static STORE: RefCell<Option<Store>> = const { RefCell::new(None) };
    static SYNCING: RefCell<bool> = const { RefCell::new(false) };
    static TRAY: RefCell<Option<crate::ui::tray::TrayIcon>> = const { RefCell::new(None) };
    /// Whether a tray icon is actually registered, so closing the window knows
    /// whether hiding somewhere is safe.
    static HAS_TRAY: RefCell<bool> = const { RefCell::new(true) };
}

thread_local! {
    // Handle to the background Tokio runtime the sync worker runs on. A
    // `current_thread` runtime is deliberate: sync is one sequential job at a
    // time, so there is nothing to parallelise and no benefit to paying for a
    // multi-threaded scheduler.
    static RUNTIME: RefCell<Option<tokio::runtime::Handle>> = const { RefCell::new(None) };
}

/// The shared task cache.
///
/// `Store` wraps a `rusqlite::Connection`, which is not `Sync`, so it lives in
/// a thread-local rather than a global. Everything in the GTK main loop touches
/// it on the main thread; the sync worker gets its own connection.
///
/// Runs `f` against the shared cache, or returns `None` if it is not open.
///
/// Convenience wrapper for read paths in the UI, where "no cache yet" should
/// degrade to an empty state rather than surface as an error.
pub fn with_store<T>(f: impl FnOnce(&Store) -> T) -> Option<T> {
    STORE.with(|slot| slot.borrow().as_ref().map(f))
}

/// Opens the cache, if it is not open already.
///
/// This is separate from `init` because the UI reads from the cache while
/// building the window: opening it afterwards would leave the first render
/// reading an empty cache.
pub fn open_store() {
    let already_open = STORE.with(|slot| slot.borrow().is_some());
    if already_open {
        return;
    }

    if let Err(err) = std::fs::create_dir_all(crate::config::data_dir()) {
        log::warn!(
            "could not create data directory {}: {err}",
            crate::config::data_dir().display()
        );
    }

    match Store::open(&Store::cache_path()) {
        Ok(store) => {
            // Surface schema problems and leftover writes at startup rather
            // than on the first sync.
            match store.pending_op_count() {
                Ok(0) => {}
                Ok(n) => log::info!("{n} pending operation(s) waiting to sync"),
                Err(err) => log::warn!("could not read the pending queue: {err}"),
            }
            STORE.with(|slot| *slot.borrow_mut() = Some(store));
        }
        Err(err) => log::error!("could not open the task cache: {err}"),
    }
}

/// Starts the sync worker, the tray and the poll timers.
///
/// `window` is absent when the app was launched with `--hidden`: the tray and
/// the timers have to run from the moment the session starts, otherwise
/// autostart would give an app with nothing to sync and no way to show a window.
pub fn init(app: &adw::Application, window: Option<&gtk::Window>, config: &Config) {
    if let Some(window) = window {
        WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));
    }
    open_store();

    // Start the background runtime the sync worker runs on. It is created and
    // owned here so its lifetime is tied to the main loop.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            log::error!("could not start the sync runtime: {err}");
            return;
        }
    };
    let handle = runtime.handle().clone();
    RUNTIME.with(|slot| *slot.borrow_mut() = Some(handle.clone()));
    // Park the runtime on its own thread so `Handle::spawn` has somewhere to
    // run the task. The tray lives on this runtime too, since it is an async
    // D-Bus service.
    std::thread::Builder::new()
        .name("gtaskbar-sync".into())
        .spawn(move || runtime.block_on(std::future::pending::<()>()))
        .map_err(|err| log::error!("could not start the sync thread: {err}"))
        .ok();

    start_tray(&handle, app);

    let minutes = config.poll_interval_minutes.max(1);
    glib::timeout_add_local_once(Duration::from_secs(2), {
        let app = app.clone();
        move || spawn_sync(app)
    });

    glib::timeout_add_local(Duration::from_secs(minutes as u64 * 60), {
        let app = app.clone();
        move || {
            spawn_sync(app.clone());
            glib::ControlFlow::Continue
        }
    });

    // The tray's callbacks arrive on the runtime thread and must not touch
    // widgets, so its intents are drained here on the main loop.
    let app_for_tray = app.clone();
    glib::timeout_add_local(Duration::from_millis(250), move || {
        // The borrow of TRAY is released before anything else here runs, since
        // `refresh_tray_attention` borrows it again and a nested borrow would
        // panic.
        let dropped = TRAY.with(|slot| match slot.borrow().as_ref() {
            Some(tray) => !tray.is_alive(),
            None => false,
        });

        if dropped {
            log::warn!("the tray icon was dropped by the shell; re-registering");
            TRAY.with(|slot| {
                slot.borrow_mut().take();
            });
            HAS_TRAY.with(|flag| *flag.borrow_mut() = false);
            if let Some(runtime) = RUNTIME.with(|slot| slot.borrow().clone()) {
                start_tray(&runtime, &app_for_tray);
            }
            return glib::ControlFlow::Continue;
        }

        let events = TRAY.with(|slot| {
            let mut collected = Vec::new();
            if let Some(tray) = slot.borrow_mut().as_mut() {
                tray.dispatch(|event| collected.push(event));
            }
            collected
        });

        for event in events {
            handle_tray_event(&app_for_tray, event);
        }

        // Recomputed on the same tick, so completing a task from the tray
        // clears the badge without waiting for a sync.
        refresh_tray_attention();
        glib::ControlFlow::Continue
    });

    log::info!("scheduler initialised with a {minutes} minute poll interval");
}

/// Registers the tray icon, if the shell has somewhere to put it.
///
/// A missing tray is a supported state, not a failure: without one, closing the
/// window has to quit rather than hide, or the app becomes unreachable.
fn start_tray(runtime: &tokio::runtime::Handle, app: &adw::Application) {
    match crate::ui::tray::TrayIcon::spawn(runtime) {
        Some(tray) => {
            tray.set_connected(crate::auth::session::is_connected());
            log::info!("tray icon registered");
            HAS_TRAY.with(|flag| *flag.borrow_mut() = true);
            TRAY.with(|slot| *slot.borrow_mut() = Some(tray));
            refresh_tray_attention();
        }
        None => {
            log::warn!(
                "no status-notifier host is available; closing the window will quit instead of \
                 hiding to the tray"
            );
            HAS_TRAY.with(|flag| *flag.borrow_mut() = false);
            let _ = app;
        }
    }
}

/// Whether closing the window can hide to a tray.
///
/// The window uses this to decide between hiding and quitting: hiding with no
/// tray would leave the app running with no way to reach or close it.
pub fn has_tray() -> bool {
    HAS_TRAY.with(|flag| *flag.borrow())
}

/// Recomputes the tray's attention count from the cache.
///
/// Due today plus overdue, because those are the tasks that are actually
/// actionable now; anything later is not what the badge is for. A task already
/// completed never counts, and an undated task never counts.
pub fn refresh_tray_attention() {
    let today = crate::model::view::today();
    use crate::store::models::TaskStatus;

    let count = with_store(|cache| {
        cache
            .all_tasks()
            .map(|tasks| {
                tasks
                    .iter()
                    .filter(|task| {
                        task.status == TaskStatus::NeedsAction
                            && task.due.is_some_and(|due| due <= today)
                    })
                    .count()
            })
            .unwrap_or(0)
    })
    .unwrap_or(0);

    TRAY.with(|slot| {
        if let Some(tray) = slot.borrow().as_ref() {
            tray.set_attention(count);
        }
    });
}

/// Carries out a tray intent on the main thread.
fn handle_tray_event(app: &adw::Application, event: crate::ui::tray::Event) {
    use crate::ui::tray::Event;

    match event {
        Event::AddTask => crate::ui::tasklist_view::focus_quick_add(),
        Event::SyncNow => request_sync(app),
        Event::ShowWindow => crate::ui::window::present(app),
        Event::Quit => {
            crate::ui::window::allow_close();
            app.quit();
        }
    }
}

/// Public entry point used by the `sync-now` action and the tray menu.
pub fn request_sync(app: &adw::Application) {
    let now_micros = glib::monotonic_time();
    let min_micros = MIN_SYNC_INTERVAL.as_micros() as i64;
    let too_soon = LAST_SYNC.with(|last| match *last.borrow() {
        Some(previous) if now_micros - previous < min_micros => true,
        _ => {
            *last.borrow_mut() = Some(now_micros);
            false
        }
    });

    if too_soon {
        log::debug!("sync requested too soon after the last one; ignoring");
        return;
    }

    spawn_sync(app.clone());
}

/// Kicks off a sync pass on the background runtime.
///
/// `reqwest` needs an async runtime, while the widgets need the GLib main loop.
/// The two are bridged by running the sync on a dedicated Tokio runtime and
/// delivering the result back with `spawn_local`, so no widget is ever touched
/// off the main thread.
fn spawn_sync(app: adw::Application) {
    // Nothing to do until an account is connected. After a restart the access
    // token is gone from memory but a refresh token is in the keyring, so
    // restore it before deciding the account is not connected.
    if crate::auth::session::access_token().is_none() && crate::auth::flow::has_stored_token() {
        restore_session();
    }

    let Some(token) = crate::auth::session::access_token() else {
        log::debug!("skipping sync: no account is connected");
        return;
    };

    let client = match TasksClient::new(token) {
        Ok(client) => client,
        Err(err) => {
            log::error!("could not build the API client: {err}");
            return;
        }
    };

    SYNCING.with(|flag| *flag.borrow_mut() = true);
    TRAY.with(|slot| {
        if let Some(tray) = slot.borrow().as_ref() {
            tray.set_syncing(true);
        }
    });
    update_status(&app, Some("Syncing…"));

    let handle = match RUNTIME.with(|slot| slot.borrow().clone()) {
        Some(handle) => handle,
        None => {
            log::error!("no tokio runtime is available; cannot sync");
            SYNCING.with(|flag| *flag.borrow_mut() = false);
            return;
        }
    };

    // Results come back over a channel rather than by awaiting inside
    // `spawn_local`, which keeps the GLib side free of any `.await`.
    // A plain channel is enough: exactly one message is ever sent.
    let (tx, rx) = std::sync::mpsc::channel();
    let main = glib::MainContext::default();

    main.spawn_local({
        let app = app.clone();
        async move {
            // Poll the channel until the worker reports once, then stop the
            // timer: a sync that never finishes must not leave a timer running
            // for the life of the process.
            glib::timeout_add_local(Duration::from_millis(100), move || {
                match rx.try_recv() {
                    Ok(outcome) => {
                        SYNCING.with(|flag| *flag.borrow_mut() = false);
                        let (message, is_error) = describe(&outcome);
                        if is_error {
                            log::warn!("{message}");
                        } else {
                            log::info!("{message}");
                        }
                        crate::ui::window::set_status(&app, None);
                        refresh_tray_attention();
                        glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    // The sender was dropped without a result; treat as done.
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        SYNCING.with(|flag| *flag.borrow_mut() = false);
                        glib::ControlFlow::Break
                    }
                }
            });
        }
    });

    handle.spawn(async move {
        // The worker opens its own connection: `rusqlite::Connection` is not
        // `Sync`, so the main thread's handle stays on the main thread.
        let cache_path = Store::cache_path();
        let outcome = engine::sync_all(&cache_path, &mut { client }).await;
        let _ = tx.send(outcome);
    });
}

/// Exchanges the stored refresh token for a fresh access token.
///
/// Runs on its own runtime because it is a one-shot at startup and must not
/// block the sync worker, which is about to be scheduled separately.
fn restore_session() {
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();

    if let Err(err) = std::thread::Builder::new()
        .name("gtaskbar-refresh".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = tx.send(Err(format!("could not start a runtime: {err}")));
                    return;
                }
            };

            let outcome = runtime.block_on(async {
                let authorized = crate::auth::flow::refresh().await?;
                // Persisting also stores any rotated refresh token, which must
                // happen before the access token is used for anything.
                authorized.persist()?;
                Ok::<(), anyhow::Error>(())
            });

            let _ = tx.send(outcome.map_err(|err| err.to_string()));
        })
    {
        log::error!("could not start the token refresh thread: {err}");
        return;
    }

    glib::MainContext::default().spawn_local(async move {
        glib::timeout_add_local(Duration::from_millis(100), move || match rx.try_recv() {
            Ok(Ok(())) => {
                log::info!("restored the session from the stored refresh token");
                glib::ControlFlow::Break
            }
            Ok(Err(message)) => {
                log::warn!("could not restore the session: {message}");
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        });
    });
}

/// How a completed pass should be presented to the user.
fn describe(outcome: &anyhow::Result<engine::SyncReport, engine::SyncError>) -> (String, bool) {
    match outcome {
        Ok(report) if report.failed => (
            format!(
                "Sync finished with errors across {} list(s)",
                report.lists_seen
            ),
            true,
        ),
        Ok(report) => {
            let mut summary = format!(
                "Synced {} list(s), {} task(s) updated",
                report.lists_seen, report.tasks_changed
            );
            if report.pending_flushed > 0 {
                summary.push_str(&format!(", {} change(s) pushed", report.pending_flushed));
            }
            (summary, false)
        }
        Err(err) if err.needs_user_action() => {
            crate::auth::session::clear();
            (format!("Reconnect needed: {err}"), true)
        }
        Err(err) => (format!("Sync failed: {err}"), true),
    }
}

/// Shows a transient message in the main window, if one is open.
fn update_status(app: &adw::Application, message: Option<&str>) {
    let main = glib::MainContext::default();
    let owned = message.map(str::to_string);
    let app = app.clone();

    if main.is_owner() {
        crate::ui::window::set_status(&app, owned.as_deref());
    } else {
        main.spawn_local(async move { crate::ui::window::set_status(&app, owned.as_deref()) });
    }
}

#[cfg(test)]
mod tests {
    use super::MIN_SYNC_INTERVAL;
    use std::time::Duration;

    #[test]
    fn min_interval_is_long_enough_to_be_a_real_guard() {
        assert!(MIN_SYNC_INTERVAL >= Duration::from_secs(10));
    }
}
