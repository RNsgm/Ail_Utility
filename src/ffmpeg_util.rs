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

        f().map_err(|e| {
            log_error!("FFmpeg: operation failed: {e}");
            e
        })
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
