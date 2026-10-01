use std::cell::RefCell;
use std::time::Duration;

use gtk::glib;

use crate::config::Config;

/// Minimum interval between two sync attempts, regardless of what the
/// scheduler timer would otherwise allow. Guards against hammering the API
/// (courtesy limit is 50,000 queries/day) when the user holds down "Sync Now".
const MIN_SYNC_INTERVAL: Duration = Duration::from_secs(15);

thread_local! {
    static LAST_SYNC: RefCell<Option<i64>> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
}

pub fn init(app: &adw::Application, window: &gtk::Window, config: &Config) {
    WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));

    // Make sure the data directory exists before anything tries to write to it.
    if let Err(err) = std::fs::create_dir_all(crate::config::data_dir()) {
        log::warn!(
            "could not create data directory {}: {err}",
            crate::config::data_dir().display()
        );
    }

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
