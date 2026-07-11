//! Безопасная сериализация вызовов FFmpeg (важно для Windows).

#[cfg(feature = "ffmpeg")]
mod inner {
    use std::sync::Mutex;

    use crate::{log_error, log_info};

    static FFMPEG: Mutex<bool> = Mutex::new(false);

    pub fn with_ffmpeg<F, R>(f: F) -> Result<R, String>
    where
        F: FnOnce() -> Result<R, String>,
    {
        log_info!("FFmpeg: waiting for lock");
        let mut guard = FFMPEG
            .lock()
            .map_err(|_| "FFmpeg lock poisoned".to_string())?;

        if !*guard {
            log_info!("FFmpeg: initializing");
            ffmpeg_next::init().map_err(|e| {
                let msg = format!("FFmpeg init failed: {e}");
                log_error!("{msg}");
                msg
            })?;
            *guard = true;
            log_info!("FFmpeg: initialized OK");
        }

        log_info!("FFmpeg: running operation");
        let result = f();
        match &result {
            Ok(_) => log_info!("FFmpeg: operation OK"),
            Err(e) => log_error!("FFmpeg: operation failed: {e}"),
        }
        result
    }

    pub fn ensure_initialized() -> Result<(), String> {
        with_ffmpeg(|| Ok(()))
    }
}

#[cfg(feature = "ffmpeg")]
pub use inner::{ensure_initialized, with_ffmpeg};

#[cfg(not(feature = "ffmpeg"))]
pub fn ensure_initialized() -> Result<(), String> {
    Ok(())
}
