//! Консольный и файловый лог для отладки (в т.ч. проблемных видео на Windows).

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

/// Инициализация: консоль (Windows) + файл `ail_debug.log` рядом с exe.
pub fn init() {
    if !logging_enabled() {
        return;
    }

    let _ = START.set(Instant::now());

    #[cfg(windows)]
    init_windows_console();

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

#[cfg(windows)]
fn init_windows_console() {
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    extern "system" {
        fn AllocConsole() -> i32;
        fn AttachConsole(pid: u32) -> i32;
        fn SetStdHandle(n: u32, h: *mut std::ffi::c_void) -> i32;
    }

    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5u32;
    const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4u32;

    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            AllocConsole();
        }

        if let Ok(conout) = OpenOptions::new().write(true).open("CONOUT$") {
            let handle = conout.as_raw_handle();
            SetStdHandle(STD_OUTPUT_HANDLE, handle);
            SetStdHandle(STD_ERROR_HANDLE, handle);
        }
    }
}
