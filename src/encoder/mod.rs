pub mod format;
pub mod reader;
pub mod video_convert;

pub use format::{
    read_index_entries, AilBuilder, AilEntry, AilHeader, AilThumbnail, CompressionType, EntryType,
};
pub use reader::{
    batch_encode, encode_archive_to_ail, encode_from_directory, encode_media_to_ail,
    list_media_in_archive, BatchEncodeResult, EncodeOptions,
};
pub use video_convert::{
    collect_convertible_files, convert_to_mp4, convert_to_mp4_if_needed, is_ts_extension,
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
            | "mpg" | "ts" | "mts" | "m2ts" | "tsv" => MediaType::Video,
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

/// Проверяет, является ли файл медиа (изображение, GIF или видео) по расширению
pub fn is_media_file<P: AsRef<Path>>(path: P) -> bool {
    const MEDIA_EXTENSIONS: &[&str] = &[
        // Изображения
        "jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp", "ico", "svg", "avif", "heic",
        "heif", // Видео
        "mp4", "webm", "avi", "mov", "mkv", "flv", "wmv", "m4v", "ogv", "mpeg", "mpg", "ts", "mts",
        "m2ts", "tsv",
    ];

    if let Some(ext) = path.as_ref().extension() {
        let ext = ext.to_str().unwrap_or("").to_lowercase();
        MEDIA_EXTENSIONS.contains(&ext.as_str())
    } else {
        false
    }
}
/// Версия формата .ail (v2 с встроенным превью)
pub const AIL_VERSION: u32 = 2;
/// Магическое число для файла .ail ("AIL!")
pub const AIL_MAGIC: &[u8; 4] = b"AIL!";
