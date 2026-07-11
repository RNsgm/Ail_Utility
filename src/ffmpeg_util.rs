//! Безопасная сериализация вызовов FFmpeg (важно для Windows).

#[cfg(feature = "ffmpeg")]
mod inner {
    use std::sync::Mutex;

    static FFMPEG: Mutex<bool> = Mutex::new(false);

    pub fn with_ffmpeg<F, R>(f: F) -> Result<R, String>
    where
        F: FnOnce() -> Result<R, String>,
    {
        let mut guard = FFMPEG
            .lock()
            .map_err(|_| "FFmpeg lock poisoned".to_string())?;

        if !*guard {
            ffmpeg_next::init().map_err(|e| format!("FFmpeg init failed: {e}"))?;
            *guard = true;
        }

        f()
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
