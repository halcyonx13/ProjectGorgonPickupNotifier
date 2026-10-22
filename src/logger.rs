//! A tiny append-only log file for troubleshooting (there is no console window).

use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// Open the log file. Keeps the file small by starting over when it passes 1 MB.
pub fn init(tag: &str) {
    let path = crate::paths::log_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        *LOG.lock().unwrap() = Some(f);
    }
    write_line(&format!("---- {} {} ({tag}) ----", crate::paths::APP_NAME, env!("CARGO_PKG_VERSION")));
    std::panic::set_hook(Box::new(|info| {
        write_line(&format!("PANIC: {info}"));
    }));
}

pub fn write_line(msg: &str) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let line = format!("{h:02}:{m:02}:{s:02}Z {msg}\n");
    if let Ok(mut guard) = LOG.lock()
        && let Some(f) = guard.as_mut()
    {
        let _ = f.write_all(line.as_bytes());
        return;
    }
    eprint!("{line}");
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::logger::write_line(&format!($($arg)*)) };
}
