/// Запускает фоновую задачу; паника не роняет процесс (Windows fast-fail).
pub fn spawn_worker<F>(name: &'static str, f: F)
where
    F: FnOnce() + Send + 'static,
{
    std::thread::spawn(move || {
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            eprintln!("Background task '{name}' panicked: {payload:?}");
        }
    });
}
