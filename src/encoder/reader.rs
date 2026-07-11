use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

#[cfg(feature = "ffmpeg")]
use crate::encoder::temp_video_path;
#[cfg(feature = "ffmpeg")]
use crate::encoder::video_convert::{convert_to_mp4, is_tts_file, mp4_output_path};
use crate::encoder::{AilBuilder, ArchiveType, CompressionType, EncoderError, MediaType};
use crate::log_info;

/// Настройки кодирования
#[derive(Debug, Clone)]
pub struct EncodeOptions {
    /// Тип сжатия для данных
    pub compression: CompressionType,
    /// Сортировать ли изображения по имени перед кодированием
    pub sort_by_name: bool,
    /// Базовое имя для файлов без расширения
    pub base_name: Option<String>,
    /// Размер превью (по умолчанию 300)
    pub thumbnail_size: u32,
    /// Пропускать, если выходной `.ail` уже существует
    pub skip_existing: bool,
}

/// Результат одного задания кодирования
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeStatus {
    Created,
    Skipped,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            compression: CompressionType::Zstd,
            sort_by_name: true,
            base_name: None,
            thumbnail_size: 300,
            skip_existing: true,
        }
    }
}
impl EncodeOptions {
    /// Создает настройки с отключенным сжатием
    pub fn no_compression() -> Self {
        Self {
            compression: CompressionType::None,
            ..Default::default()
        }
    }

    /// Создает настройки с LZ4 сжатием
    pub fn lz4() -> Self {
        Self {
            compression: CompressionType::Lz4,
            ..Default::default()
        }
    }

    /// Создает настройки с Zstd сжатием
    pub fn zstd() -> Self {
        Self {
            compression: CompressionType::Zstd,
            ..Default::default()
        }
    }

    /// С размером превью
    pub fn with_thumbnail_size(mut self, size: u32) -> Self {
        self.thumbnail_size = size;
        self
    }
}

/// Кодирует архив (zip или rar) в файл .ail
pub fn encode_archive_to_ail<P: AsRef<Path>, Q: AsRef<Path>>(
    archive_path: P,
    output_path: Q,
    options: Option<EncodeOptions>,
) -> Result<(), EncoderError> {
    let options = options.unwrap_or_default();
    let archive_type =
        ArchiveType::from_path(&archive_path).ok_or(EncoderError::InvalidArchiveType)?;

    let mut builder = AilBuilder::new().with_thumbnail_size(options.thumbnail_size);
    let mut images: Vec<(String, Vec<u8>)> = Vec::new();

    match archive_type {
        ArchiveType::Zip => {
            read_zip_archive(&archive_path, &mut images)?;
        }
        ArchiveType::Rar => {
            read_rar_archive(&archive_path, &mut images)?;
        }
        ArchiveType::SevenZip => {
            read_7z_archive(&archive_path, &mut images)?;
        }
    }

    if images.is_empty() {
        return Err(EncoderError::NoImagesFound);
    }

    if options.sort_by_name {
        images.sort_by(|a, b| alphanumeric_sort::compare_str(&a.0, &b.0));
    }

    for (name, data) in images {
        builder.add_image(name, data, options.compression)?;
    }

    // Создаем выходной файл
    let output_file = File::create(output_path)?;
    let mut writer = std::io::BufWriter::new(output_file);

    builder.build(&mut writer)?;
    writer.flush()?;

    Ok(())
}

/// Кодирует директорию с изображениями в файл .ail (видео в папке кодируются отдельно)
pub fn encode_from_directory<P: AsRef<Path>, Q: AsRef<Path>>(
    dir_path: P,
    output_path: Q,
    options: Option<EncodeOptions>,
) -> Result<(), EncoderError> {
    let options = options.unwrap_or_default();
    let mut builder = AilBuilder::new().with_thumbnail_size(options.thumbnail_size);
    let mut images: Vec<(String, Vec<u8>)> = Vec::new();

    let dir = std::fs::read_dir(dir_path)?;

    for entry in dir {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        if !crate::encoder::is_image_file(&path) {
            continue;
        }

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let data = std::fs::read(&path)?;
        images.push((name, data));
    }

    if images.is_empty() {
        return Err(EncoderError::NoImagesFound);
    }

    if options.sort_by_name {
        images.sort_by(|a, b| alphanumeric_sort::compare_str(&a.0, &b.0));
    }

    for (name, data) in images {
        builder.add_image(name, data, options.compression)?;
    }

    // Создаем выходной файл
    let output_file = File::create(output_path)?;
    let mut writer = std::io::BufWriter::new(output_file);

    builder.build(&mut writer)?;
    writer.flush()?;

    Ok(())
}

/// Читает ZIP архив и извлекает изображения
fn read_zip_archive<P: AsRef<Path>>(
    path: P,
    images: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), EncoderError> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().to_string();

        if !crate::encoder::is_image_file(&name) {
            continue;
        }

        if file.is_dir() {
            continue;
        }

        let mut data = Vec::new();
        file.read_to_end(&mut data)?;

        if !data.is_empty() {
            images.push((name, data));
        }
    }

    Ok(())
}

/// Читает RAR архив и извлекает изображения
#[cfg(feature = "unrar")]
fn read_rar_archive<P: AsRef<Path>>(
    path: P,
    images: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), EncoderError> {
    let path = path.as_ref();

    // Используем Process режим для чтения содержимого файлов
    let archive = unrar::Archive::new(&path);

    let open_archive = archive
        .open_for_processing()
        .map_err(|e| EncoderError::Rar(format!("Failed to open RAR: {:?}", e)))?;

    read_rar_entries(open_archive, images)?;

    Ok(())
}

#[cfg(feature = "unrar")]
fn read_rar_entries(
    mut open_archive: unrar::OpenArchive<unrar::Process, unrar::CursorBeforeHeader>,
    images: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), EncoderError> {
    loop {
        // Читаем заголовок - получаем OpenArchive с CursorBeforeFile
        let archive_with_file = match open_archive.read_header() {
            Ok(Some(archive)) => archive,
            Ok(None) => break, // Конец архива
            Err(e) => return Err(EncoderError::Rar(format!("RAR read header error: {:?}", e))),
        };

        let entry = archive_with_file.entry();
        let filename = entry.filename.to_string_lossy().to_string();

        // Проверяем, является ли файл изображением
        if crate::encoder::is_image_file(&filename) && !entry.is_directory() {
            let (data, next_archive) = archive_with_file
                .read()
                .map_err(|e| EncoderError::Rar(format!("RAR read data error: {:?}", e)))?;

            if !data.is_empty() {
                images.push((filename, data));
            }

            open_archive = next_archive;
        } else {
            // Пропускаем файл
            open_archive = archive_with_file
                .skip()
                .map_err(|e| EncoderError::Rar(format!("RAR skip error: {:?}", e)))?;
        }
    }

    Ok(())
}

#[cfg(not(feature = "unrar"))]
fn read_rar_archive<P: AsRef<Path>>(
    _path: P,
    _images: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), EncoderError> {
    Err(EncoderError::Rar(
        "RAR support requires building with the unrar feature".to_string(),
    ))
}

/// Читает 7z архив и извлекает изображения
fn read_7z_archive<P: AsRef<Path>>(
    path: P,
    images: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), EncoderError> {
    sevenz_rust::decompress_file_with_extract_fn(path, "", |entry, reader, _| {
        let name = entry.name().to_string();
        if crate::encoder::is_image_file(&name) && !entry.is_directory() {
            let mut data = Vec::new();
            if reader.read_to_end(&mut data).is_ok() && !data.is_empty() {
                images.push((name, data));
            }
        }
        Ok(true)
    })
    .map_err(|e| EncoderError::SevenZip(e.to_string()))?;
    Ok(())
}

/// Извлекает кадры из GIF
fn decode_gif_frames(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>, EncoderError> {
    use image::codecs::gif::GifDecoder;
    use image::AnimationDecoder;
    use std::io::Cursor;

    let decoder = GifDecoder::new(Cursor::new(data)).map_err(|e| {
        EncoderError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to read GIF: {}", e),
        ))
    })?;

    let mut frames = Vec::new();
    for (frame_num, frame_result) in decoder.into_frames().enumerate() {
        let frame = frame_result.map_err(|e| {
            EncoderError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to decode GIF frame: {}", e),
            ))
        })?;

        let buffer = frame.into_buffer();
        let dynamic = image::DynamicImage::ImageRgba8(buffer);

        let mut png_data = Vec::new();
        let mut cursor = Cursor::new(&mut png_data);
        dynamic
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| {
                EncoderError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Failed to encode frame as PNG: {}", e),
                ))
            })?;

        frames.push((format!("frame_{frame_num:04}.png"), png_data));
    }

    if frames.is_empty() {
        return Err(EncoderError::NoImagesFound);
    }

    Ok(frames)
}

/// Извлекает кадры из видео через FFmpeg (каждый N-й кадр, макс 300)
#[cfg(feature = "ffmpeg")]
fn decode_video_frames(
    data: &[u8],
    name_hint: &str,
) -> Result<Vec<(String, Vec<u8>)>, EncoderError> {
    use ffmpeg_next::{codec, format, frame, software::scaling, util::format::Pixel};
    use std::io::Cursor;

    const FRAME_SKIP: usize = 30;
    const MAX_FRAMES: usize = 300;

    let temp_path = temp_video_path("ail_encode_vid", name_hint);
    std::fs::write(&temp_path, data).map_err(|e| {
        EncoderError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to write temp file: {}", e),
        ))
    })?;

    let result = with_ffmpeg_enc(|| {
        log_info!(
            "decode_video_frames(encode): temp={} hint={name_hint} data_len={}",
            temp_path.display(),
            data.len()
        );
        let mut ictx = format::input(&temp_path).map_err(|e| {
            EncoderError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to open video: {}", e),
            ))
        })?;

        let video_stream_index = match ictx.streams().best(ffmpeg_next::media::Type::Video) {
            Some(stream) => stream.index(),
            None => {
                let _ = std::fs::remove_file(&temp_path);
                return Err(EncoderError::NoImagesFound);
            }
        };

        let stream = ictx.stream(video_stream_index).ok_or_else(|| {
            let _ = std::fs::remove_file(&temp_path);
            EncoderError::NoImagesFound
        })?;

        let fps = stream.avg_frame_rate();
        let _ = fps;

        let decoder_params = stream.parameters();
        let decoder =
            codec::context::Context::from_parameters(decoder_params.clone()).map_err(|_| {
                let _ = std::fs::remove_file(&temp_path);
                EncoderError::NoImagesFound
            })?;
        let mut decoder = decoder.decoder().video().map_err(|_| {
            let _ = std::fs::remove_file(&temp_path);
            EncoderError::NoImagesFound
        })?;

        let mut scaler = scaling::Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::RGB24,
            decoder.width(),
            decoder.height(),
            scaling::Flags::BILINEAR,
        )
        .map_err(|_| {
            let _ = std::fs::remove_file(&temp_path);
            EncoderError::NoImagesFound
        })?;

        let mut frames = Vec::new();
        let mut frame_num = 0;
        let mut decoded_count = 0;

        for (_stream, packet) in ictx.packets() {
            if _stream.index() != video_stream_index {
                continue;
            }

            let mut decoded = frame::Video::empty();
            if decoder.send_packet(&packet).is_err() {
                continue;
            }

            while decoder.receive_frame(&mut decoded).is_ok() {
                decoded_count += 1;

                if decoded_count % FRAME_SKIP != 0 {
                    continue;
                }
                if frames.len() >= MAX_FRAMES {
                    break;
                }

                let mut rgb_frame = frame::Video::empty();
                if scaler.run(&decoded, &mut rgb_frame).is_err() {
                    continue;
                }

                let width = decoder.width() as usize;
                let height = decoder.height() as usize;
                let stride = rgb_frame.stride(0) as usize;

                let mut rgba_data = Vec::with_capacity(width * height * 4);
                for y in 0..height {
                    let offset = y * stride;
                    for x in 0..width {
                        let r = rgb_frame.data(0)[offset + x * 3];
                        let g = rgb_frame.data(0)[offset + x * 3 + 1];
                        let b = rgb_frame.data(0)[offset + x * 3 + 2];
                        rgba_data.extend_from_slice(&[r, g, b, 255]);
                    }
                }

                let color_image =
                    image::RgbaImage::from_raw(width as u32, height as u32, rgba_data).ok_or_else(
                        || {
                            EncoderError::Io(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "Failed to create image buffer",
                            ))
                        },
                    )?;
                let dynamic = image::DynamicImage::ImageRgba8(color_image);

                let mut png_data = Vec::new();
                let mut cursor = Cursor::new(&mut png_data);
                dynamic
                    .write_to(&mut cursor, image::ImageFormat::Png)
                    .map_err(|e| {
                        EncoderError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("Failed to encode frame as PNG: {}", e),
                        ))
                    })?;

                frames.push((format!("frame_{:04}.png", frame_num), png_data));
                frame_num += 1;
            }

            if frames.len() >= MAX_FRAMES {
                break;
            }
        }

        if frames.is_empty() {
            return Err(EncoderError::NoImagesFound);
        }

        Ok(frames)
    });

    let _ = std::fs::remove_file(&temp_path);
    result
}

#[cfg(feature = "ffmpeg")]
fn with_ffmpeg_enc<F, R>(f: F) -> Result<R, EncoderError>
where
    F: FnOnce() -> Result<R, EncoderError>,
{
    crate::ffmpeg_util::with_ffmpeg(|| f().map_err(|e| e.to_string()))
        .map_err(|e| EncoderError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))
}

#[cfg(feature = "ffmpeg")]
fn encode_video_file_to_ail(
    input_path: &Path,
    output_path: &Path,
    options: &EncodeOptions,
) -> Result<(), EncoderError> {
    log_info!(
        "encode_video_file_to_ail: input={} output={}",
        input_path.display(),
        output_path.display()
    );
    let converted_path = if is_tts_file(input_path) {
        let temp = std::env::temp_dir().join(format!(
            "ail_convert_{}_{}.mp4",
            std::process::id(),
            input_path.file_name().unwrap_or_default().to_string_lossy()
        ));
        log_info!("encode_video: converting TTS -> {}", temp.display());
        convert_to_mp4(input_path, &temp)?;
        Some(temp)
    } else {
        None
    };

    let video_path = converted_path.as_deref().unwrap_or(input_path);
    let meta = std::fs::metadata(video_path).ok();
    log_info!(
        "encode_video: reading {} ({} bytes)",
        video_path.display(),
        meta.map(|m| m.len()).unwrap_or(0)
    );

    let data = std::fs::read(video_path)?;
    let name = if converted_path.is_some() {
        mp4_output_path(input_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("video.mp4")
            .to_string()
    } else {
        input_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("video.mp4")
            .to_string()
    };

    let mut builder = AilBuilder::new().with_thumbnail_size(options.thumbnail_size);
    log_info!("encode_video: built blob name={name}");
    builder.add_video_blob(name, data, options.compression)?;

    let output_file = File::create(output_path)?;
    let mut writer = std::io::BufWriter::new(output_file);
    builder.build(&mut writer)?;
    writer.flush()?;
    log_info!("encode_video: OK -> {}", output_path.display());

    if let Some(temp) = converted_path {
        let _ = std::fs::remove_file(temp);
    }

    Ok(())
}

/// Кодирует архив, медиа-файл или папку в .ail
pub fn encode_media_to_ail<P: AsRef<Path>, Q: AsRef<Path>>(
    input_path: P,
    output_path: Q,
    options: Option<EncodeOptions>,
) -> Result<EncodeStatus, EncoderError> {
    let options = options.unwrap_or_default();
    let input_path = input_path.as_ref();
    let output_path = output_path.as_ref();

    if options.skip_existing && output_path.is_file() {
        log_info!(
            "encode_media_to_ail: SKIP existing {}",
            output_path.display()
        );
        return Ok(EncodeStatus::Skipped);
    }

    log_info!(
        "encode_media_to_ail: input={} output={}",
        input_path.display(),
        output_path.display()
    );

    if input_path.is_dir() {
        encode_from_directory(input_path, output_path, Some(options))?;
        return Ok(EncodeStatus::Created);
    }

    if matches!(MediaType::from_path(input_path), MediaType::Image) {
        let data = std::fs::read(input_path)?;
        let name = input_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image")
            .to_string();

        let mut builder = AilBuilder::new().with_thumbnail_size(options.thumbnail_size);
        builder.add_image(name, data, options.compression)?;

        let output_file = File::create(output_path)?;
        let mut writer = std::io::BufWriter::new(output_file);
        builder.build(&mut writer)?;
        writer.flush()?;
        return Ok(EncodeStatus::Created);
    }

    #[cfg(feature = "ffmpeg")]
    if MediaType::from_path(input_path).is_video() || is_tts_file(input_path) {
        encode_video_file_to_ail(input_path, output_path, &options)?;
        return Ok(EncodeStatus::Created);
    }

    let ext = input_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mut builder = AilBuilder::new().with_thumbnail_size(options.thumbnail_size);
    let mut images: Vec<(String, Vec<u8>)> = Vec::new();

    match ext.as_str() {
        "zip" | "rar" | "7z" => {
            let archive_type =
                ArchiveType::from_path(input_path).ok_or(EncoderError::InvalidArchiveType)?;
            match archive_type {
                ArchiveType::Zip => read_zip_archive(input_path, &mut images)?,
                ArchiveType::Rar => read_rar_archive(input_path, &mut images)?,
                ArchiveType::SevenZip => read_7z_archive(input_path, &mut images)?,
            }
        }
        "gif" => {
            let data = std::fs::read(input_path)?;
            images = decode_gif_frames(&data)?;
        }
        _ => {
            return Err(EncoderError::InvalidArchiveType);
        }
    }

    if images.is_empty() {
        return Err(EncoderError::NoImagesFound);
    }

    if options.sort_by_name {
        images.sort_by(|a, b| alphanumeric_sort::compare_str(&a.0, &b.0));
    }

    for (name, data) in images {
        builder.add_image(name, data, options.compression)?;
    }

    let output_file = File::create(output_path)?;
    let mut writer = std::io::BufWriter::new(output_file);
    builder.build(&mut writer)?;
    writer.flush()?;

    Ok(EncodeStatus::Created)
}
/// Результат пакетного кодирования
pub type BatchEncodeResult = Vec<(String, Result<EncodeStatus, EncoderError>)>;

/// Пакетное кодирование нескольких файлов
pub fn batch_encode<P: AsRef<Path>, Q: AsRef<Path>>(
    input_paths: &[P],
    output_dir: Q,
    options: Option<EncodeOptions>,
) -> Result<BatchEncodeResult, EncoderError> {
    let output_dir = output_dir.as_ref();
    std::fs::create_dir_all(output_dir)?;

    let mut results = Vec::new();

    for input_path in input_paths {
        let input_path = input_path.as_ref();
        let file_stem = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");

        let output_path = output_dir.join(format!("{}.ail", file_stem));

        let result = encode_media_to_ail(input_path, &output_path, options.clone());
        results.push((input_path.to_string_lossy().to_string(), result));
    }

    Ok(results)
}

/// Получает список изображений из архива без кодирования
pub fn list_media_in_archive<P: AsRef<Path>>(archive_path: P) -> Result<Vec<String>, EncoderError> {
    let archive_type =
        ArchiveType::from_path(&archive_path).ok_or(EncoderError::InvalidArchiveType)?;

    let mut images = Vec::new();

    match archive_type {
        ArchiveType::Zip => {
            let file = File::open(archive_path)?;
            let mut archive = zip::ZipArchive::new(file)?;

            for i in 0..archive.len() {
                let file = archive.by_index(i)?;
                let name = file.name().to_string();

                if crate::encoder::is_image_file(&name) && !file.is_dir() {
                    images.push(name);
                }
            }
        }
        ArchiveType::Rar => {
            #[cfg(feature = "unrar")]
            {
                let archive = unrar::Archive::new(&archive_path);
                let open_archive = archive
                    .open_for_listing()
                    .map_err(|e| EncoderError::Rar(format!("Failed to open RAR: {:?}", e)))?;

                for entry_result in open_archive {
                    let entry = entry_result
                        .map_err(|e| EncoderError::Rar(format!("RAR list error: {:?}", e)))?;
                    let filename = entry.filename.to_string_lossy().to_string();

                    if crate::encoder::is_image_file(&filename) && !entry.is_directory() {
                        images.push(filename);
                    }
                }
            }
            #[cfg(not(feature = "unrar"))]
            {
                return Err(EncoderError::Rar(
                    "RAR support requires building with the unrar feature".to_string(),
                ));
            }
        }
        ArchiveType::SevenZip => {
            sevenz_rust::decompress_file_with_extract_fn(&archive_path, "", |entry, _reader, _| {
                let name = entry.name().to_string();
                if crate::encoder::is_image_file(&name) && !entry.is_directory() {
                    images.push(name);
                }
                Ok(true)
            })
            .map_err(|e| EncoderError::SevenZip(e.to_string()))?;
        }
    }

    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn skips_existing_output_when_enabled() {
        let dir = TempDir::new().unwrap();
        let input = dir.path().join("page.png");
        let output = dir.path().join("page.ail");
        fs::write(&input, b"png").unwrap();
        fs::write(&output, b"existing ail").unwrap();

        let status = encode_media_to_ail(
            &input,
            &output,
            Some(EncodeOptions {
                skip_existing: true,
                ..Default::default()
            }),
        )
        .unwrap();

        assert_eq!(status, EncodeStatus::Skipped);
        assert_eq!(fs::read(&output).unwrap(), b"existing ail");
    }

    #[test]
    fn overwrites_existing_output_when_skip_disabled() {
        let dir = TempDir::new().unwrap();
        let input = dir.path().join("page.png");
        let output = dir.path().join("page.ail");
        fs::write(&input, [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]).unwrap();
        fs::write(&output, b"existing ail").unwrap();

        let status = encode_media_to_ail(
            &input,
            &output,
            Some(EncodeOptions {
                skip_existing: false,
                compression: CompressionType::None,
                ..Default::default()
            }),
        )
        .unwrap();

        assert_eq!(status, EncodeStatus::Created);
        assert_ne!(fs::read(&output).unwrap(), b"existing ail");
    }
}
