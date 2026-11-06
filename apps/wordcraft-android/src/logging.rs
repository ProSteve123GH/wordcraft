//! A tiny file logger for Android.
//!
//! A sideloaded APK has no adb, so logcat is not a useful place to look when the app does not
//! start: the log goes next to the app's private data instead. Deliberately dependency-free (the
//! `log` facade only) and panic-free: a logger that cannot open its file must not take the editor
//! down with it.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// `<internal data>/wordcraft/logs/android.log`, the file [`init`] writes.
pub fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("wordcraft").join("logs").join("android.log")
}

/// Starts logging to `<data_dir>/wordcraft/logs/android.log` and returns its path.
///
/// `None` when there is no data directory or the file cannot be opened; the app then runs without
/// file logging rather than failing to start.
pub fn init(data_dir: Option<&Path>) -> Option<PathBuf> {
    let data_dir = data_dir?;
    let path = log_path(data_dir);
    let Some(dir) = path.parent() else {
        eprintln!("wordcraft: log path has no parent: {}", path.display());
        return None;
    };
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("wordcraft: cannot create {}: {e}", dir.display());
        return None;
    }
    let file = match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("wordcraft: cannot open {}: {e}", path.display());
            return None;
        }
    };
    let logger = Logger { file: Mutex::new(Some(file)) };
    if LOGGER.set(logger).is_err() {
        eprintln!("wordcraft: logger was already installed");
        return None;
    }
    let Some(logger) = LOGGER.get() else { return None };
    if let Err(e) = log::set_logger(logger) {
        eprintln!("wordcraft: cannot install logger: {e}");
        return None;
    }
    log::set_max_level(log::LevelFilter::Info);
    Some(path)
}

/// Writes buffered lines to the log file, if one is open.
pub fn flush() {
    if let Some(logger) = LOGGER.get() {
        logger.flush_file();
    }
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

struct Logger {
    file: Mutex<Option<std::fs::File>>,
}

impl Logger {
    fn flush_file(&self) {
        if let Ok(mut file) = self.file.lock()
            && let Some(file) = file.as_mut()
        {
            let _ = file.flush();
        }
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let Some(file) = &mut *self.file.lock().unwrap_or_else(std::sync::PoisonError::into_inner) else {
            return;
        };
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        // A failed write is not worth reporting through the logger that just failed to write.
        let _ = writeln!(file, "[{secs}] {} {}: {}", record.level(), record.target(), record.args());
        let _ = file.flush();
    }

    fn flush(&self) {
        self.flush_file();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_lives_in_the_private_data_folder() {
        let path = log_path(Path::new("/data/user/0/ai.storyteller.wordcraft/files"));
        assert_eq!(path, Path::new("/data/user/0/ai.storyteller.wordcraft/files/wordcraft/logs/android.log"));
    }

    #[test]
    fn no_data_directory_means_no_log_file_and_no_panic() {
        assert!(init(None).is_none());
    }
}
