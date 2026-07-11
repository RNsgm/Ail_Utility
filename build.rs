fn main() {
    // Линковка системных библиотек Windows для unrar
    if std::env::var("TARGET")
        .unwrap_or_default()
        .contains("windows")
    {
        println!("cargo:rustc-link-lib=advapi32");
        println!("cargo:rustc-link-lib=crypt32");

        // Настройка FFmpeg: указываем путь к библиотекам
        let ffmpeg_dir = if let Ok(dir) = std::env::var("FFMPEG_DIR") {
            dir
        } else {
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
            format!("{}/ffmpeg", manifest_dir)
        };

        let ffmpeg_lib = format!("{}/lib", ffmpeg_dir);
        let ffmpeg_bin = format!("{}/bin", ffmpeg_dir);

        if std::path::Path::new(&ffmpeg_lib).exists() {
            println!("cargo:rustc-link-search=native={}", ffmpeg_lib);
        }

        // Копируем DLL из ffmpeg/bin в папку с exe после сборки
        if std::path::Path::new(&ffmpeg_bin).exists() {
            let out_dir = std::env::var("OUT_DIR").unwrap_or_default();
            let profile = if out_dir.contains("debug") {
                "debug"
            } else {
                "release"
            };
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
            let target_dir = format!("{}/target/{}", manifest_dir, profile);

            if let Ok(entries) = std::fs::read_dir(&ffmpeg_bin) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "dll") {
                        let dest = format!(
                            "{}/{}",
                            target_dir,
                            path.file_name().unwrap().to_string_lossy()
                        );
                        if let Ok(content) = std::fs::read(&path) {
                            let _ = std::fs::write(&dest, content);
                        }
                    }
                }
            }
        }
    }
}
