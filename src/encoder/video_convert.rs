use std::path::{Path, PathBuf};

use crate::encoder::EncoderError;

const TS_EXTENSIONS: &[&str] = &["ts", "mts", "m2ts", "tsv"];

/// Расширения MPEG-TS и похожих контейнеров.
pub fn is_ts_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| TS_EXTENSIONS.iter().any(|s| ext.eq_ignore_ascii_case(s)))
        .unwrap_or(false)
}

/// Нужна ли конвертация в MP4 (по расширению или формату контейнера).
pub fn needs_mp4_conversion(path: &Path) -> bool {
    if is_ts_extension(path) {
        return true;
    }

    probe_container_name(path)
        .map(|name| is_mpegts_format(&name))
        .unwrap_or(false)
}

fn is_mpegts_format(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("mpegts") || name == "mpeg2ts" || name == "ts"
}

#[cfg(feature = "ffmpeg")]
fn probe_container_name(path: &Path) -> Result<String, EncoderError> {
    use ffmpeg_next::format;

    ffmpeg_next::init()
        .map_err(|e| EncoderError::VideoConvert(format!("Failed to initialize FFmpeg: {e}")))?;

    let ictx = format::input(path).map_err(|e| {
        EncoderError::VideoConvert(format!("Failed to probe {}: {e}", path.display()))
    })?;

    Ok(ictx.format().name().to_string())
}

#[cfg(not(feature = "ffmpeg"))]
fn probe_container_name(_path: &Path) -> Result<String, EncoderError> {
    Ok(String::new())
}

/// Путь для выходного MP4 рядом с исходником.
pub fn mp4_output_path(input: &Path) -> PathBuf {
    input.with_extension("mp4")
}

/// Ремультиплексирует TS/MPEG-TS в MP4 без перекодирования.
#[cfg(feature = "ffmpeg")]
pub fn convert_to_mp4<P: AsRef<Path>, Q: AsRef<Path>>(
    input: P,
    output: Q,
) -> Result<(), EncoderError> {
    use ffmpeg_next::{codec, encoder, format, media, Rational};

    let input = input.as_ref();
    let output = output.as_ref();

    if output.exists() {
        return Err(EncoderError::VideoConvert(format!(
            "Output already exists: {}",
            output.display()
        )));
    }

    ffmpeg_next::init()
        .map_err(|e| EncoderError::VideoConvert(format!("Failed to initialize FFmpeg: {e}")))?;

    let mut ictx = format::input(input).map_err(|e| {
        EncoderError::VideoConvert(format!("Failed to open {}: {e}", input.display()))
    })?;

    let mut octx = format::output(output).map_err(|e| {
        EncoderError::VideoConvert(format!("Failed to create output {}: {e}", output.display()))
    })?;

    let stream_count = ictx.nb_streams() as usize;
    let mut stream_mapping = vec![-1i32; stream_count];
    let mut ist_time_bases = vec![Rational(0, 1); stream_count];
    let mut ost_index = 0i32;

    for (ist_index, ist) in ictx.streams().enumerate() {
        let medium = ist.parameters().medium();
        if medium != media::Type::Audio
            && medium != media::Type::Video
            && medium != media::Type::Subtitle
        {
            continue;
        }

        stream_mapping[ist_index] = ost_index;
        ist_time_bases[ist_index] = ist.time_base();

        let mut ost = octx
            .add_stream(encoder::find(codec::Id::None))
            .map_err(|e| EncoderError::VideoConvert(format!("Failed to add stream: {e}")))?;
        ost.set_parameters(ist.parameters());

        // Сброс codec_tag для совместимости при смене контейнера.
        unsafe {
            (*ost.parameters().as_mut_ptr()).codec_tag = 0;
        }

        ost_index += 1;
    }

    if ost_index == 0 {
        return Err(EncoderError::VideoConvert(format!(
            "No audio/video streams found in {}",
            input.display()
        )));
    }

    octx.set_metadata(ictx.metadata().to_owned());
    octx.write_header()
        .map_err(|e| EncoderError::VideoConvert(format!("Failed to write header: {e}")))?;

    for (stream, mut packet) in ictx.packets() {
        let ist_index = stream.index();
        let mapped = stream_mapping[ist_index];
        if mapped < 0 {
            continue;
        }

        let ost = octx
            .stream(mapped as usize)
            .ok_or_else(|| EncoderError::VideoConvert("Output stream missing".to_string()))?;

        packet.rescale_ts(ist_time_bases[ist_index], ost.time_base());
        packet.set_position(-1);
        packet.set_stream(mapped as usize);
        packet
            .write_interleaved(&mut octx)
            .map_err(|e| EncoderError::VideoConvert(format!("Failed to write packet: {e}")))?;
    }

    octx.write_trailer()
        .map_err(|e| EncoderError::VideoConvert(format!("Failed to finalize MP4: {e}")))?;

    Ok(())
}

#[cfg(not(feature = "ffmpeg"))]
pub fn convert_to_mp4<P: AsRef<Path>, Q: AsRef<Path>>(
    _input: P,
    _output: Q,
) -> Result<(), EncoderError> {
    Err(EncoderError::VideoConvert(
        "Video conversion requires the ffmpeg feature".to_string(),
    ))
}

/// Конвертирует в MP4 рядом с исходником, если нужно.
pub fn convert_to_mp4_if_needed(input: &Path) -> Result<Option<PathBuf>, EncoderError> {
    if !needs_mp4_conversion(input) {
        return Ok(None);
    }

    let output = mp4_output_path(input);
    convert_to_mp4(input, &output)?;
    Ok(Some(output))
}

/// Рекурсивно собирает файлы, требующие конвертации.
pub fn collect_convertible_files(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_convertible_files_inner(path, &mut files);
    files.sort();
    files.dedup();
    files
}

fn collect_convertible_files_inner(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        if needs_mp4_conversion(path) {
            out.push(path.to_path_buf());
        }
        return;
    }

    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            collect_convertible_files_inner(&entry_path, out);
        } else if needs_mp4_conversion(&entry_path) {
            out.push(entry_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ts_extensions() {
        assert!(is_ts_extension(Path::new("video.ts")));
        assert!(is_ts_extension(Path::new("clip.MTS")));
        assert!(!is_ts_extension(Path::new("movie.mp4")));
    }

    #[test]
    fn mpegts_format_names() {
        assert!(is_mpegts_format("mpegts"));
        assert!(is_mpegts_format("mpeg2ts"));
        assert!(!is_mpegts_format("mp4"));
    }
}
