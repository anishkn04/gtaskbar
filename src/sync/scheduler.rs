use std::cell::RefCell;
use std::time::Duration;

use gtk::glib;

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

pub fn init(app: &adw::Application, window: &gtk::Window, config: &Config) {
    WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));
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
    RUNTIME.with(|slot| *slot.borrow_mut() = Some(handle));
    // Park the runtime on its own thread so `Handle::spawn` has somewhere to
    // run the task.
    std::thread::Builder::new()
        .name("gtaskbar-sync".into())
        .spawn(move || runtime.block_on(std::future::pending::<()>()))
        .map_err(|err| log::error!("could not start the sync thread: {err}"))
        .ok();

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

    log::info!("scheduler initialised with a {minutes} minute poll interval");
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
    // Nothing to do until an account is connected.
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
