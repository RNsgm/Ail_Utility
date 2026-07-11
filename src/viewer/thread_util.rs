/// Запускает фоновую задачу; паника не роняет процесс (Windows fast-fail).
pub fn spawn_worker<F>(name: &'static str, f: F)
where
    F: FnOnce() + Send + 'static,
{
    crate::log_info!("spawn_worker: start '{name}'");
    std::thread::spawn(move || {
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            crate::log_error!("Background task '{name}' panicked: {payload:?}");
        } else {
            crate::log_info!("spawn_worker: done '{name}'");
        }
    });
}
