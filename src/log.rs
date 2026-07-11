//! Файловый лог для отладки (в т.ч. проблемных видео на Windows).
//! По умолчанию пишет в `ail_debug.log` рядом с exe; консольное окно не открывается.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);
static START: OnceLock<Instant> = OnceLock::new();

/// Включён ли лог (по умолчанию да; отключить: `AIL_LOG=0`).
pub fn logging_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        !matches!(
            std::env::var("AIL_LOG").as_deref(),
            Ok("0") | Ok("false") | Ok("off")
        )
    })
}

/// Инициализация: файл `ail_debug.log` рядом с exe.
pub fn init() {
    if !logging_enabled() {
        return;
    }

    let _ = START.set(Instant::now());

    let log_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("ail_debug.log")))
        .unwrap_or_else(|| PathBuf::from("ail_debug.log"));

    let _ = LOG_PATH.set(log_path.clone());

    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        if let Ok(mut guard) = LOG_FILE.lock() {
            *guard = Some(file);
        }
    }

    write(
        "INFO",
        "log",
        &format!("=== AIL log started, file: {} ===", log_path.display()),
    );
}

pub fn log_path() -> Option<&'static PathBuf> {
    LOG_PATH.get()
}

pub fn write(level: &str, module: &str, message: &str) {
    if !logging_enabled() {
        return;
    }

    let elapsed = START
        .get()
        .map(|s| format!("+{:.3}s", s.elapsed().as_secs_f64()))
        .unwrap_or_else(|| "+0.000s".to_string());
    let line = format!("[{elapsed}] [{level}] [{module}] {message}");

    #[cfg(debug_assertions)]
    eprintln!("{line}");

    if let Ok(mut guard) = LOG_FILE.lock() {
        if let Some(ref mut file) = *guard {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::log::write("INFO", module_path!(), &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::log::write("WARN", module_path!(), &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::log::write("ERROR", module_path!(), &format!($($arg)*))
    };
}
