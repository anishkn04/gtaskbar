//! A log file next to the cache, so failures are diagnosable after the fact.
//!
//! The app is launched from a desktop entry, where stderr goes nowhere. Every
//! silent failure so far — the unknown `--hidden` flag, the keyring panics, the
//! quick-add handler dropping writes — was invisible for exactly this reason.
//! This keeps env_logger's stderr output untouched and additionally appends
//! every record to `gtaskbar.log` in the data directory, with one rotated
//! generation so a chatty session cannot grow it without bound.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

const LOG_FILE: &str = "gtaskbar.log";
const ROTATED_LOG_FILE: &str = "gtaskbar.log.old";
const MAX_BYTES: u64 = 1024 * 1024;

/// Installed as the global logger at startup, before anything else can log.
pub fn init() {
    let dir = crate::config::data_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }

    let path = dir.join(LOG_FILE);
    if path.metadata().map(|m| m.len()).unwrap_or(0) > MAX_BYTES {
        let _ = std::fs::rename(&path, dir.join(ROTATED_LOG_FILE));
    }

    let file = OpenOptions::new().create(true).append(true).open(&path);

    let stderr = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("gtaskbar=info"),
    )
    .build();

    // The documented default is `info` for this crate; the file gets whatever
    // passes the same filter, so the two outputs never disagree about what was
    // worth recording.
    let max_level = stderr.filter();
    let tee = TeeLogger {
        stderr,
        file: file.map(Mutex::new).ok(),
    };

    if log::set_logger(Box::leak(Box::new(tee))).is_ok() {
        log::set_max_level(max_level);
    }
}

struct TeeLogger {
    stderr: env_logger::Logger,
    file: Option<Mutex<std::fs::File>>,
}

impl log::Log for TeeLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.stderr.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        self.stderr.log(record);
        if let Some(file) = &self.file {
            if let Ok(mut file) = file.lock() {
                let _ = writeln!(
                    file,
                    "[{} {} {}] {}",
                    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
                    record.level(),
                    record.target(),
                    record.args()
                );
            }
        }
    }

    fn flush(&self) {
        if let Some(file) = &self.file {
            if let Ok(mut file) = file.lock() {
                let _ = file.flush();
            }
        }
    }
}
