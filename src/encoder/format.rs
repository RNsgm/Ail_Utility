use std::io::{Read, Write};
use std::path::Path;

use super::{EncoderError, AIL_MAGIC, AIL_VERSION};

#[cfg(feature = "ffmpeg")]
const MAX_THUMB_PACKETS: usize = 8_000;
#[cfg(feature = "ffmpeg")]
const THUMB_PACKET_LOG_STEP: usize = 500;

const MAX_ENTRY_NAME_LEN: usize = 1024;

fn crypto_err(error: crate::crypto::CryptoError) -> EncoderError {
    EncoderError::Crypto(error.to_string())
}

/// Заголовок .ail файла (v2 с превью)
#[derive(Debug, Clone)]
pub struct AilHeader {
    /// Магическое число (4 байта): "AIL!"
    pub magic: [u8; 4],
    /// Версия формата (4 байта, u32) - теперь версия 2
    pub version: u32,
    /// Флаг шифрования (1 байт, 0 - нет, 1 - AES-256-GCM)
    pub encrypted: u8,
    /// Зарезервировано (3 байта)
    pub reserved: [u8; 3],
    /// Количество изображений (4 байта, u32)
    pub entry_count: u32,
    /// Смещение секции превью от начала файла (8 байт, u64)
    pub thumbnail_offset: u64,
    /// Размер секции превью (8 байт, u64)
    pub thumbnail_size: u64,
}

impl AilHeader {
    /// Размер заголовка в байтах
    pub const SIZE: usize = 4 + 4 + 1 + 3 + 4 + 8 + 8; // 32 байта

    /// Создает новый заголовок
    pub fn new(entry_count: u32, thumbnail_offset: u64, thumbnail_size: u64) -> Self {
        Self {
            magic: *AIL_MAGIC,
            version: AIL_VERSION,
            encrypted: 1, // По умолчанию шифруем
            reserved: [0; 3],
            entry_count,
            thumbnail_offset,
            thumbnail_size,
        }
    }
    /// Сериализует заголовок в байты
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        buf[0..4].copy_from_slice(&self.magic);
        buf[4..8].copy_from_slice(&self.version.to_le_bytes());
        buf[8] = self.encrypted;
        buf[9..12].copy_from_slice(&self.reserved);
        buf[12..16].copy_from_slice(&self.entry_count.to_le_bytes());
        buf[16..24].copy_from_slice(&self.thumbnail_offset.to_le_bytes());
        buf[24..32].copy_from_slice(&self.thumbnail_size.to_le_bytes());
        buf
    }

    /// Десериализует заголовок из байт
    pub fn from_bytes(bytes: &[u8; Self::SIZE]) -> Result<Self, EncoderError> {
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[0..4]);

        if &magic != AIL_MAGIC {
            return Err(EncoderError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid AIL magic number",
            )));
        }

        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let encrypted = bytes[8];
        let mut reserved = [0u8; 3];
        reserved.copy_from_slice(&bytes[9..12]);
        let entry_count = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let thumbnail_offset = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let thumbnail_size = u64::from_le_bytes(bytes[24..32].try_into().unwrap());

        Ok(Self {
            magic,
            version,
            encrypted,
            reserved,
            entry_count,
            thumbnail_offset,
            thumbnail_size,
        })
    }
    /// Записывает заголовок в поток
    pub fn write<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(&self.to_bytes())
    }

    /// Читает заголовок из потока
    pub fn read<R: Read>(reader: &mut R) -> Result<Self, EncoderError> {
        let mut buf = [0u8; Self::SIZE];
        reader.read_exact(&mut buf)?;
        Self::from_bytes(&buf)
    }
}

/// Тип сжатия данных
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompressionType {
    /// Без сжатия
    None = 0,
    /// Zstd сжатие
    Zstd = 1,
    /// LZ4 сжатие
    Lz4 = 2,
}

impl CompressionType {
    /// Преобразует u8 в CompressionType
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(CompressionType::None),
            1 => Some(CompressionType::Zstd),
            2 => Some(CompressionType::Lz4),
            _ => None,
        }
    }
}

/// Встроенное превью для быстрого отображения
#[derive(Debug, Clone)]
pub struct AilThumbnail {
    /// Ширина превью (4 байта, u32)
    pub width: u32,
    /// Высота превью (4 байта, u32)
    pub height: u32,
    /// Тип сжатия превью (1 байт)
    pub compression: CompressionType,
    /// Зарезервировано (3 байта)
    pub reserved: [u8; 3],
    /// Размер сжатых данных (8 байт, u64)
    pub compressed_size: u64,
    /// Данные превью (сжатые JPEG, размер ~20-50KB)
    pub data: Vec<u8>,
}

impl AilThumbnail {
    /// Размер заголовка превью (без данных)
    pub const HEADER_SIZE: usize = 4 + 4 + 1 + 3 + 8; // 20 байт

    /// Создает превью из изображения
    pub fn from_image(image_data: &[u8], max_size: u32) -> Result<Self, EncoderError> {
        use image::ImageReader;
        use std::io::Cursor;

        // Декодируем изображение
        let img = ImageReader::new(Cursor::new(image_data))
            .with_guessed_format()
            .map_err(|e| {
                EncoderError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Failed to read image: {}", e),
                ))
            })?
            .decode()
            .map_err(|e| {
                EncoderError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Failed to decode image: {}", e),
                ))
            })?;

        // Масштабируем
        let (orig_w, orig_h) = (img.width(), img.height());
        let (new_w, new_h) = if orig_w > orig_h {
            let scale = max_size as f32 / orig_w as f32;
            (max_size, (orig_h as f32 * scale) as u32)
        } else {
            let scale = max_size as f32 / orig_h as f32;
            ((orig_w as f32 * scale) as u32, max_size)
        };

        let thumbnail = img.resize(new_w, new_h, image::imageops::FilterType::Lanczos3);

        // Кодируем в JPEG с качеством 60%
        let mut jpeg_data = Vec::new();
        let mut cursor = Cursor::new(&mut jpeg_data);
        thumbnail
            .write_to(&mut cursor, image::ImageFormat::Jpeg)
            .map_err(|e| {
                EncoderError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Failed to encode thumbnail: {}", e),
                ))
            })?;

        // Сжимаем данные (уровень 3 для скорости)
        let mut thumb_data = zstd::encode_all(&jpeg_data[..], 3)?;

        // Шифруем превью
        let key = crate::crypto::get_or_create_key();
        thumb_data = crate::crypto::encrypt(&thumb_data, &key).map_err(crypto_err)?;

        Ok(Self {
            width: new_w,
            height: new_h,
            compression: CompressionType::Zstd,
            reserved: [0; 3],
            compressed_size: thumb_data.len() as u64,
            data: thumb_data,
        })
    }

    /// Создает превью из первого кадра GIF
    pub fn from_gif(gif_data: &[u8], max_size: u32) -> Result<Self, EncoderError> {
        use image::AnimationDecoder;
        use std::io::Cursor;

        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(gif_data)).map_err(|e| {
            EncoderError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to read GIF: {}", e),
            ))
        })?;

        let mut frames = decoder.into_frames();
        let first_frame = frames
            .next()
            .ok_or_else(|| {
                EncoderError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "GIF has no frames",
                ))
            })?
            .map_err(|e| {
                EncoderError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Failed to decode GIF frame: {}", e),
                ))
            })?;

        let img = first_frame.into_buffer();
        let dynamic_img = image::DynamicImage::ImageRgba8(img);
        Self::from_dynamic_image(&dynamic_img, max_size)
    }

    /// Создает превью из первого кадра видео по пути на диске (без копии в temp).
    #[cfg(feature = "ffmpeg")]
    pub fn from_video_path(path: &Path, max_size: u32) -> Result<Self, EncoderError> {
        let meta = std::fs::metadata(path).ok();
        crate::log_info!(
            "AilThumbnail::from_video_path: path={} size={}",
            path.display(),
            meta.map(|m| m.len()).unwrap_or(0)
        );

        let thumb = crate::ffmpeg_util::with_ffmpeg(|| Self::extract_first_video_frame(path, max_size))
            .map_err(|e| {
                crate::log_error!("AilThumbnail::from_video_path failed: {e}");
                EncoderError::Io(std::io::Error::new(std::io::ErrorKind::Other, e))
            })?;

        crate::log_info!("AilThumbnail::from_video_path: OK");
        Ok(thumb)
    }

    /// Создает превью из буфера видео (пишет во временный файл).
    #[cfg(feature = "ffmpeg")]
    pub fn from_video(
        video_data: &[u8],
        max_size: u32,
        name_hint: Option<&str>,
    ) -> Result<Self, EncoderError> {
        let hint = name_hint.unwrap_or("video.mp4");
        let temp_path = crate::encoder::temp_video_path("ail_video_thumb", hint);
        crate::log_info!(
            "AilThumbnail::from_video: hint={hint} data_len={} temp={}",
            video_data.len(),
            temp_path.display()
        );
        std::fs::write(&temp_path, video_data).map_err(|e| {
            EncoderError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to write temp file: {}", e),
            ))
        })?;

        let result = Self::from_video_path(&temp_path, max_size);
        let _ = std::fs::remove_file(&temp_path);
        result
    }

    #[cfg(feature = "ffmpeg")]
    fn extract_first_video_frame(path: &Path, max_size: u32) -> Result<Self, String> {
        use ffmpeg_next::codec;
        use ffmpeg_next::format;
        use ffmpeg_next::frame;
        use ffmpeg_next::software::scaling;
        use ffmpeg_next::util::format::Pixel;

        crate::log_info!("AilThumbnail: opening {}", path.display());
        let mut ictx = format::input(path).map_err(|e| e.to_string())?;

        let video_stream_index = ictx
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .ok_or_else(|| "No video stream found".to_string())?
            .index();

        let stream = ictx
            .stream(video_stream_index)
            .ok_or_else(|| "Failed to get video stream".to_string())?;
        let decoder_params = stream.parameters();
        let decoder = codec::context::Context::from_parameters(decoder_params)
            .map_err(|e| format!("Failed to create decoder context: {e}"))?;
        let mut decoder = decoder
            .decoder()
            .video()
            .map_err(|e| format!("Failed to get video decoder: {e}"))?;

        crate::log_info!(
            "AilThumbnail: stream={video_stream_index} {}x{} fmt={:?}",
            decoder.width(),
            decoder.height(),
            decoder.format()
        );

        let mut scaler = scaling::Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::RGB24,
            decoder.width(),
            decoder.height(),
            scaling::Flags::BILINEAR,
        )
        .map_err(|e| format!("Failed to create scaler: {e}"))?;

        let mut packet_count = 0usize;
        for (_stream, packet) in ictx.packets() {
            packet_count += 1;
            if packet_count == 1 || packet_count % THUMB_PACKET_LOG_STEP == 0 {
                crate::log_info!("AilThumbnail: packets={packet_count}");
            }
            if packet_count > MAX_THUMB_PACKETS {
                return Err(format!(
                    "No decodable video frame within {MAX_THUMB_PACKETS} packets"
                ));
            }

            if _stream.index() != video_stream_index {
                continue;
            }

            let mut decoded = frame::Video::empty();
            if decoder.send_packet(&packet).is_err() {
                continue;
            }
            if decoder.receive_frame(&mut decoded).is_err() {
                continue;
            }

            crate::log_info!("AilThumbnail: decoded frame at packet {packet_count}");

            let mut rgb_frame = frame::Video::empty();
            scaler.run(&decoded, &mut rgb_frame).ok();

            let width = decoder.width() as usize;
            let height = decoder.height() as usize;
            let stride = rgb_frame.stride(0) as usize;

            let mut rgb_data = Vec::new();
            for y in 0..height {
                let offset = y * stride;
                for x in 0..width {
                    let r = rgb_frame.data(0)[offset + x * 3];
                    let g = rgb_frame.data(0)[offset + x * 3 + 1];
                    let b = rgb_frame.data(0)[offset + x * 3 + 2];
                    rgb_data.extend_from_slice(&[r, g, b]);
                }
            }

            let img = image::RgbImage::from_raw(width as u32, height as u32, rgb_data)
                .ok_or_else(|| "Failed to create image from frame".to_string())?;
            let dynamic_img = image::DynamicImage::ImageRgb8(img);
            return Self::from_dynamic_image(&dynamic_img, max_size).map_err(|e| e.to_string());
        }

        Err("Failed to extract video frame".to_string())
    }

    /// Внутренний метод для создания превью из DynamicImage
    fn from_dynamic_image(img: &image::DynamicImage, max_size: u32) -> Result<Self, EncoderError> {
        use std::io::Cursor;

        let (orig_w, orig_h) = (img.width(), img.height());
        let (new_w, new_h) = if orig_w > orig_h {
            let scale = max_size as f32 / orig_w as f32;
            (max_size, (orig_h as f32 * scale) as u32)
        } else {
            let scale = max_size as f32 / orig_h as f32;
            ((orig_w as f32 * scale) as u32, max_size)
        };

        let thumbnail = img.resize(new_w, new_h, image::imageops::FilterType::Lanczos3);

        // Кодируем в JPEG
        let mut jpeg_data = Vec::new();
        let mut cursor = Cursor::new(&mut jpeg_data);
        thumbnail
            .write_to(&mut cursor, image::ImageFormat::Jpeg)
            .map_err(|e| {
                EncoderError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Failed to encode thumbnail: {}", e),
                ))
            })?;

        // Сжимаем и шифруем
        let mut thumb_data = zstd::encode_all(&jpeg_data[..], 3)?;
        let key = crate::crypto::get_or_create_key();
        thumb_data = crate::crypto::encrypt(&thumb_data, &key).map_err(crypto_err)?;

        Ok(Self {
            width: new_w,
            height: new_h,
            compression: CompressionType::Zstd,
            reserved: [0; 3],
            compressed_size: thumb_data.len() as u64,
            data: thumb_data,
        })
    }
    /// Сериализует превью в байты (только заголовок)
    pub fn header_to_bytes(&self) -> [u8; Self::HEADER_SIZE] {
        let mut buf = [0u8; Self::HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.width.to_le_bytes());
        buf[4..8].copy_from_slice(&self.height.to_le_bytes());
        buf[8] = self.compression as u8;
        buf[9..12].copy_from_slice(&self.reserved);
        buf[12..20].copy_from_slice(&self.compressed_size.to_le_bytes());
        buf
    }

    /// Полный размер записи (заголовок + данные)
    pub fn total_size(&self) -> usize {
        Self::HEADER_SIZE + self.data.len()
    }
}
/// Тип записи в .ail файле
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EntryType {
    /// Статичное изображение
    Image = 0,
    /// GIF-анимация (кадры как отдельные изображения)
    Gif = 1,
    /// Видео (хранится как один бинарный blob)
    Video = 2,
}

impl EntryType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(EntryType::Image),
            1 => Some(EntryType::Gif),
            2 => Some(EntryType::Video),
            _ => None,
        }
    }
}

/// Запись в .ail файле
#[derive(Debug, Clone)]
pub struct AilEntry {
    /// Оригинальное имя файла (переменная длина, null-terminated UTF-8)
    pub original_name: String,
    /// Тип записи (1 байт)
    pub entry_type: EntryType,
    /// Тип сжатия (1 байт)
    pub compression: CompressionType,
    /// Зарезервировано (2 байта)
    pub reserved: [u8; 2],
    /// Размер оригинальных данных (8 байт, u64)
    pub original_size: u64,
    /// Размер сжатых данных (8 байт, u64)
    pub compressed_size: u64,
    /// Смещение данных от начала файла (8 байт, u64)
    pub data_offset: u64,
    /// MD5 хеш оригинальных данных (16 байт)
    pub md5_hash: [u8; 16],
}
impl AilEntry {
    /// Минимальный размер записи (без имени файла)
    pub const MIN_SIZE: usize = 1 + 1 + 2 + 8 + 8 + 8 + 16; // 44 байта + имя файла

    /// Создает новую запись
    pub fn new(
        original_name: String,
        compression: CompressionType,
        original_size: u64,
        compressed_size: u64,
        data_offset: u64,
        md5_hash: [u8; 16],
    ) -> Self {
        Self {
            original_name,
            entry_type: EntryType::Image,
            compression,
            reserved: [0; 2],
            original_size,
            compressed_size,
            data_offset,
            md5_hash,
        }
    }

    /// Создает запись видео
    pub fn new_video(
        original_name: String,
        compression: CompressionType,
        original_size: u64,
        compressed_size: u64,
        data_offset: u64,
        md5_hash: [u8; 16],
    ) -> Self {
        Self {
            original_name,
            entry_type: EntryType::Video,
            compression,
            reserved: [0; 2],
            original_size,
            compressed_size,
            data_offset,
            md5_hash,
        }
    }

    /// Сериализует запись в байты
    pub fn to_bytes(&self) -> Vec<u8> {
        let name_bytes = self.original_name.as_bytes();
        let mut buf = Vec::with_capacity(Self::MIN_SIZE + name_bytes.len() + 1);

        // Имя файла (null-terminated)
        buf.extend_from_slice(name_bytes);
        buf.push(0);

        // Тип записи
        buf.push(self.entry_type as u8);

        // Тип сжатия
        buf.push(self.compression as u8);

        // Зарезервировано (2 bytes)
        buf.extend_from_slice(&self.reserved);

        // Размеры и смещение
        buf.extend_from_slice(&self.original_size.to_le_bytes());
        buf.extend_from_slice(&self.compressed_size.to_le_bytes());
        buf.extend_from_slice(&self.data_offset.to_le_bytes());

        // MD5 хеш
        buf.extend_from_slice(&self.md5_hash);

        buf
    }

    /// Записывает запись в поток
    pub fn write<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(&self.to_bytes())
    }
}

/// Возвращает смещение начала индекса для заголовка v2.
pub fn index_offset_from_header(header: &AilHeader) -> u64 {
    if header.thumbnail_size > 0 && header.thumbnail_offset == AilHeader::SIZE as u64 {
        header.thumbnail_offset + header.thumbnail_size
    } else {
        AilHeader::SIZE as u64
    }
}

/// Читает записи индекса из потока.
pub fn read_index_entries<R: Read>(
    reader: &mut R,
    count: u32,
) -> Result<Vec<AilEntry>, EncoderError> {
    let mut entries = Vec::with_capacity(count as usize);

    for _ in 0..count {
        let mut name_bytes = Vec::new();
        let mut found_null = false;

        for _ in 0..MAX_ENTRY_NAME_LEN {
            let mut byte = [0u8; 1];
            reader.read_exact(&mut byte)?;
            if byte[0] == 0 {
                found_null = true;
                break;
            }
            name_bytes.push(byte[0]);
        }

        if !found_null {
            return Err(EncoderError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Filename too long or not null-terminated",
            )));
        }

        let original_name = String::from_utf8(name_bytes).map_err(|e| {
            EncoderError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid UTF-8 in entry name: {e}"),
            ))
        })?;

        let mut buf = [0u8; AilEntry::MIN_SIZE];
        reader.read_exact(&mut buf)?;

        let entry_type = EntryType::from_u8(buf[0]).unwrap_or(EntryType::Image);
        let compression = CompressionType::from_u8(buf[1]).ok_or_else(|| {
            EncoderError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid compression type for entry {original_name}"),
            ))
        })?;

        let original_size = u64::from_le_bytes(buf[4..12].try_into().unwrap());
        let compressed_size = u64::from_le_bytes(buf[12..20].try_into().unwrap());
        let data_offset = u64::from_le_bytes(buf[20..28].try_into().unwrap());

        let mut md5_hash = [0u8; 16];
        md5_hash.copy_from_slice(&buf[28..44]);

        entries.push(AilEntry {
            original_name,
            entry_type,
            compression,
            reserved: [buf[2], buf[3]],
            original_size,
            compressed_size,
            data_offset,
            md5_hash,
        });
    }

    Ok(entries)
}

/// Индекс записей в .ail файле
#[derive(Debug, Clone)]
pub struct AilIndex {
    /// Записи об изображениях
    pub entries: Vec<AilEntry>,
}

impl AilIndex {
    /// Создает новый индекс
    pub fn new(entries: Vec<AilEntry>) -> Self {
        Self { entries }
    }

    /// Вычисляет размер индекса в байтах
    pub fn size(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| (AilEntry::MIN_SIZE + e.original_name.len() + 1) as u64)
            .sum()
    }

    /// Записывает индекс в поток
    pub fn write<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        for entry in &self.entries {
            entry.write(writer)?;
        }
        Ok(())
    }
}

/// Помощник для построения .ail файла (v2 с превью)
pub struct AilBuilder {
    entries: Vec<AilEntry>,
    data_chunks: Vec<Vec<u8>>,
    thumbnail: Option<AilThumbnail>,
    thumbnail_max_size: u32,
}

impl AilBuilder {
    /// Создает новый билдер
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            data_chunks: Vec::new(),
            thumbnail: None,
            thumbnail_max_size: 300,
        }
    }

    /// Задает максимальный размер стороны превью
    pub fn with_thumbnail_size(mut self, size: u32) -> Self {
        self.thumbnail_max_size = size;
        self
    }

    /// Добавляет изображение в билдер
    /// Если это первое изображение, также создает превью для галереи
    pub fn add_image(
        &mut self,
        original_name: String,
        data: Vec<u8>,
        compression: CompressionType,
    ) -> Result<(), EncoderError> {
        let original_size = data.len() as u64;

        // Создаем превью из первого изображения (если еще не создано)
        if self.thumbnail.is_none() {
            match AilThumbnail::from_image(&data, self.thumbnail_max_size) {
                Ok(thumb) => {
                    self.thumbnail = Some(thumb);
                }
                Err(e) => {
                    eprintln!("Warning: Failed to create thumbnail: {}", e);
                }
            }
        }

        // Сжимаем данные
        let mut data_to_store = match compression {
            CompressionType::None => data.clone(),
            CompressionType::Zstd => zstd::encode_all(&data[..], 3)?,
            CompressionType::Lz4 => {
                let mut encoder = lz4::EncoderBuilder::new().build(Vec::new())?;
                std::io::copy(&mut &data[..], &mut encoder)?;
                let (compressed, result) = encoder.finish();
                result?;
                compressed
            }
        };

        // Шифруем данные (AES-256-GCM)
        let key = crate::crypto::get_or_create_key();
        data_to_store = crate::crypto::encrypt(&data_to_store, &key).map_err(crypto_err)?;

        let compressed_size = data_to_store.len() as u64;

        // Вычисляем MD5 хеш
        let md5_hash = md5::compute(&data).into();

        self.data_chunks.push(data_to_store);

        let entry = AilEntry::new(
            original_name,
            compression,
            original_size,
            compressed_size,
            0, // Будет вычислено позже
            md5_hash,
        );
        self.entries.push(entry);

        Ok(())
    }

    /// Добавляет видео с диска: превью читается по пути, данные — одним чтением файла.
    pub fn add_video_blob_from_path(
        &mut self,
        video_path: &Path,
        original_name: String,
        compression: CompressionType,
    ) -> Result<(), EncoderError> {
        if self.thumbnail.is_none() {
            #[cfg(feature = "ffmpeg")]
            {
                match AilThumbnail::from_video_path(video_path, self.thumbnail_max_size) {
                    Ok(thumb) => {
                        self.thumbnail = Some(thumb);
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to create video thumbnail: {e}");
                    }
                }
            }
        }

        let data = std::fs::read(video_path)?;
        self.add_video_blob_data(original_name, data, compression)
    }

    /// Добавляет видео как один бинарный blob
    pub fn add_video_blob(
        &mut self,
        original_name: String,
        data: Vec<u8>,
        compression: CompressionType,
    ) -> Result<(), EncoderError> {
        if self.thumbnail.is_none() {
            #[cfg(feature = "ffmpeg")]
            {
                match AilThumbnail::from_video(&data, self.thumbnail_max_size, Some(&original_name))
                {
                    Ok(thumb) => {
                        self.thumbnail = Some(thumb);
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to create video thumbnail: {e}");
                    }
                }
            }
        }

        self.add_video_blob_data(original_name, data, compression)
    }

    fn add_video_blob_data(
        &mut self,
        original_name: String,
        data: Vec<u8>,
        compression: CompressionType,
    ) -> Result<(), EncoderError> {
        let original_size = data.len() as u64;

        let mut data_to_store = match compression {
            CompressionType::None => data.clone(),
            CompressionType::Zstd => zstd::encode_all(&data[..], 3)?,
            CompressionType::Lz4 => {
                let mut encoder = lz4::EncoderBuilder::new().build(Vec::new())?;
                std::io::copy(&mut &data[..], &mut encoder)?;
                let (compressed, result) = encoder.finish();
                result?;
                compressed
            }
        };

        // Шифруем данные (AES-256-GCM)
        let key = crate::crypto::get_or_create_key();
        data_to_store = crate::crypto::encrypt(&data_to_store, &key).map_err(crypto_err)?;

        let compressed_size = data_to_store.len() as u64;
        let md5_hash = md5::compute(&data).into();

        self.data_chunks.push(data_to_store);

        let entry = AilEntry::new_video(
            original_name,
            compression,
            original_size,
            compressed_size,
            0, // Будет вычислено позже
            md5_hash,
        );
        self.entries.push(entry);

        Ok(())
    }

    /// Собирает .ail файл и записывает его в поток    
    pub fn build<W: Write>(mut self, writer: &mut W) -> Result<(), EncoderError> {
        let entry_count = self.entries.len() as u32;

        // Размер секции превью
        let thumbnail_size = self
            .thumbnail
            .as_ref()
            .map(|t| t.total_size() as u64)
            .unwrap_or(0);

        // Структура файла:
        // [Header][Thumbnail][Index][Data...]

        let header_size = AilHeader::SIZE as u64;
        let thumbnail_offset = header_size;
        let index_offset = thumbnail_offset + thumbnail_size;
        let index_size: u64 = self
            .entries
            .iter()
            .map(|e| (AilEntry::MIN_SIZE + e.original_name.len() + 1) as u64)
            .sum();
        let data_offset = index_offset + index_size;

        // Создаем заголовок
        let header = AilHeader::new(entry_count, thumbnail_offset, thumbnail_size);

        // Обновляем смещения данных для записей
        let mut current_offset = data_offset;
        for (entry, chunk) in self.entries.iter_mut().zip(&self.data_chunks) {
            entry.data_offset = current_offset;
            current_offset += chunk.len() as u64;
        }

        // Записываем заголовок
        header.write(writer)?;

        // Записываем секцию превью
        if let Some(thumb) = &self.thumbnail {
            writer.write_all(&thumb.header_to_bytes())?;
            writer.write_all(&thumb.data)?;
        }

        // Записываем индекс
        let index = AilIndex::new(self.entries);
        index.write(writer)?;

        // Записываем данные
        for chunk in &self.data_chunks {
            writer.write_all(chunk)?;
        }

        Ok(())
    }
}

impl Default for AilBuilder {
    fn default() -> Self {
        Self::new()
    }
}
