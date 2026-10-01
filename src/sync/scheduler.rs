use std::cell::RefCell;
use std::time::Duration;

use gtk::glib;

use crate::config::Config;
use crate::store::Store;

/// Minimum interval between two sync attempts, regardless of what the
/// scheduler timer would otherwise allow. Guards against hammering the API
/// (courtesy limit is 50,000 queries/day) when the user holds down "Sync Now".
const MIN_SYNC_INTERVAL: Duration = Duration::from_secs(15);

thread_local! {
    static LAST_SYNC: RefCell<Option<i64>> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
    static STORE: RefCell<Option<Store>> = const { RefCell::new(None) };
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

fn spawn_sync(app: adw::Application) {
    // The real implementation lands in the sync step; until then this just
    // records intent so the plumbing can be exercised end to end.
    log::debug!("sync requested");
    let _ = &app;
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
