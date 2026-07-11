pub mod format;
pub mod jobs;
pub mod reader;
pub mod video_convert;

pub use format::{
    read_index_entries, AilBuilder, AilEntry, AilHeader, AilThumbnail, CompressionType, EntryType,
};
pub use jobs::{collect_encoding_jobs, EncodingJob};
pub use reader::{
    batch_encode, encode_archive_to_ail, encode_from_directory, encode_media_to_ail,
    list_media_in_archive, BatchEncodeResult, EncodeOptions, EncodeStatus,
};
pub use video_convert::{
    collect_convertible_files, convert_to_mp4, convert_to_mp4_if_needed, is_tts_file,
    mp4_output_path, needs_mp4_conversion,
};

use std::path::Path;
/// Типы поддерживаемых архивов
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveType {
    Zip,
    Rar,
    SevenZip,
}

impl ArchiveType {
    /// Определяет тип архива по расширению файла
    pub fn from_path<P: AsRef<Path>>(path: P) -> Option<Self> {
        let path = path.as_ref();
        let ext = path.extension()?.to_str()?.to_lowercase();
        match ext.as_str() {
            "zip" => Some(ArchiveType::Zip),
            "rar" => Some(ArchiveType::Rar),
            "7z" => Some(ArchiveType::SevenZip),
            _ => None,
        }
    }
}

/// Ошибки кодировщика
#[derive(Debug)]
pub enum EncoderError {
    Io(std::io::Error),
    Zip(zip::result::ZipError),
    Rar(String),
    SevenZip(String),
    InvalidArchiveType,
    NoImagesFound,
    Crypto(String),
    VideoConvert(String),
}

impl std::fmt::Display for EncoderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncoderError::Io(e) => write!(f, "IO error: {}", e),
            EncoderError::Zip(e) => write!(f, "ZIP error: {}", e),
            EncoderError::Rar(e) => write!(f, "RAR error: {}", e),
            EncoderError::SevenZip(e) => write!(f, "7Z error: {}", e),
            EncoderError::InvalidArchiveType => write!(f, "Invalid or unsupported archive type"),
            EncoderError::NoImagesFound => write!(f, "No image files found in archive"),
            EncoderError::Crypto(e) => write!(f, "Crypto error: {}", e),
            EncoderError::VideoConvert(e) => write!(f, "Video convert error: {}", e),
        }
    }
}

impl std::error::Error for EncoderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EncoderError::Io(e) => Some(e),
            EncoderError::Zip(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for EncoderError {
    fn from(e: std::io::Error) -> Self {
        EncoderError::Io(e)
    }
}

impl From<zip::result::ZipError> for EncoderError {
    fn from(e: zip::result::ZipError) -> Self {
        EncoderError::Zip(e)
    }
}

/// Тип медиа-файла
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Image,
    Gif,
    Video,
    Unknown,
}

impl MediaType {
    /// Определяет тип медиа по расширению файла
    pub fn from_path<P: AsRef<Path>>(path: P) -> Self {
        let ext = path
            .as_ref()
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "gif" => MediaType::Gif,
            "mp4" | "webm" | "avi" | "mov" | "mkv" | "flv" | "wmv" | "m4v" | "ogv" | "mpeg"
            | "mpg" | "tts" => MediaType::Video,
            "jpg" | "jpeg" | "png" | "bmp" | "tiff" | "tif" | "webp" | "ico" | "svg" | "avif"
            | "heic" | "heif" => MediaType::Image,
            _ => MediaType::Unknown,
        }
    }

    pub fn is_gif(&self) -> bool {
        matches!(self, MediaType::Gif)
    }

    pub fn is_video(&self) -> bool {
        matches!(self, MediaType::Video)
    }
}

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "webm", "avi", "mov", "mkv", "flv", "wmv", "m4v", "ogv", "mpeg", "mpg", "tts",
];

/// Проверяет, является ли файл видео по расширению
pub fn is_video_file<P: AsRef<Path>>(path: P) -> bool {
    MediaType::from_path(path).is_video()
}

/// Проверяет, является ли файл изображением (не видео) по расширению
pub fn is_image_file<P: AsRef<Path>>(path: P) -> bool {
    let path = path.as_ref();
    is_media_file(path) && !is_video_file(path)
}

/// Расширение контейнера для временного видеофайла (для FFmpeg).
pub fn video_container_extension(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("mp4");

    if ext.eq_ignore_ascii_case("mkv") {
        return "mkv";
    }
    if ext.eq_ignore_ascii_case("webm") {
        return "webm";
    }
    if ext.eq_ignore_ascii_case("avi") {
        return "avi";
    }
    if ext.eq_ignore_ascii_case("mov") {
        return "mov";
    }
    if ext.eq_ignore_ascii_case("flv") {
        return "flv";
    }
    if ext.eq_ignore_ascii_case("wmv") {
        return "wmv";
    }
    if ext.eq_ignore_ascii_case("m4v") {
        return "m4v";
    }
    if ext.eq_ignore_ascii_case("ogv") {
        return "ogv";
    }
    if ext.eq_ignore_ascii_case("mpeg") || ext.eq_ignore_ascii_case("mpg") {
        return "mp4";
    }
    if ext.eq_ignore_ascii_case("mp4") {
        return "mp4";
    }

    "mp4"
}

/// Путь к временному видеофайлу с корректным расширением.
pub fn temp_video_path(prefix: &str, name_hint: &str) -> std::path::PathBuf {
    let ext = video_container_extension(name_hint);
    std::env::temp_dir().join(format!("{prefix}_{pid}.{ext}", pid = std::process::id()))
}

/// Проверяет, является ли файл медиа (изображение, GIF или видео) по расширению
pub fn is_media_file<P: AsRef<Path>>(path: P) -> bool {
    const IMAGE_EXTENSIONS: &[&str] = &[
        "jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp", "ico", "svg", "avif", "heic",
        "heif",
    ];

    if let Some(ext) = path.as_ref().extension() {
        let ext = ext.to_str().unwrap_or("").to_lowercase();
        IMAGE_EXTENSIONS.contains(&ext.as_str()) || VIDEO_EXTENSIONS.contains(&ext.as_str())
    } else {
        false
    }
}
/// Версия формата .ail (v2 с встроенным превью)
pub const AIL_VERSION: u32 = 2;
/// Магическое число для файла .ail ("AIL!")
pub const AIL_MAGIC: &[u8; 4] = b"AIL!";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn mkv_is_video_file() {
        assert!(is_video_file(Path::new("movie.mkv")));
        assert!(is_media_file(Path::new("movie.mkv")));
        assert!(!is_video_file(Path::new("photo.png")));
    }

    #[test]
    fn video_container_extension_for_mkv() {
        assert_eq!(video_container_extension("clip.mkv"), "mkv");
        assert_eq!(video_container_extension("path/to/video.MKV"), "mkv");
        assert_eq!(video_container_extension("unknown.bin"), "mp4");
    }
}
