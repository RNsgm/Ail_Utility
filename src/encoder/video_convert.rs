use std::path::{Path, PathBuf};
use std::process::Command;

use crate::encoder::EncoderError;

/// Расширение TTS-обёртки (в т.ч. `video.mp4.tts`).
pub fn is_tts_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|name| name.to_ascii_lowercase().ends_with(".tts"))
        .unwrap_or(false)
}

/// Нужна ли конвертация в MP4.
pub fn needs_mp4_conversion(path: &Path) -> bool {
    is_tts_file(path)
}

/// Путь для MP4: `clip.mp4.tts` → `clip.mp4`, `clip.tts` → `clip.mp4`.
pub fn mp4_output_path(input: &Path) -> PathBuf {
    let file_name = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("output");

    if file_name.to_ascii_lowercase().ends_with(".tts") {
        let without_tts = &file_name[..file_name.len() - 4];
        if without_tts.to_ascii_lowercase().ends_with(".mp4") {
            return input.with_file_name(without_tts);
        }
        return input.with_file_name(format!("{without_tts}.mp4"));
    }

    input.with_extension("mp4")
}

/// Конвертирует TTS в MP4 (несколько стратегий подряд).
pub fn convert_to_mp4<P: AsRef<Path>, Q: AsRef<Path>>(
    input: P,
    output: Q,
) -> Result<(), EncoderError> {
    let input = input.as_ref();
    let output = output.as_ref();

    if output.exists() {
        return Err(EncoderError::VideoConvert(format!(
            "Output already exists: {}",
            output.display()
        )));
    }

    if !is_tts_file(input) {
        return Err(EncoderError::VideoConvert(format!(
            "Not a TTS file: {}",
            input.display()
        )));
    }

    let errors = [
        try_ffmpeg_remux(input, output),
        try_extract_embedded_mp4(input, output),
        try_ffmpeg_cli(input, output),
    ];

    if output.exists() {
        return Ok(());
    }

    let details: Vec<String> = errors
        .into_iter()
        .filter_map(|e| e.err().map(|e| e.to_string()))
        .collect();
    Err(EncoderError::VideoConvert(format!(
        "Failed to convert {}: {}",
        input.display(),
        details.join("; ")
    )))
}

pub fn convert_to_mp4_if_needed(input: &Path) -> Result<Option<PathBuf>, EncoderError> {
    if !needs_mp4_conversion(input) {
        return Ok(None);
    }

    let output = mp4_output_path(input);
    convert_to_mp4(input, &output)?;
    Ok(Some(output))
}

/// Рекурсивно собирает TTS-файлы.
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

#[cfg(feature = "ffmpeg")]
fn try_ffmpeg_remux(input: &Path, output: &Path) -> Result<(), EncoderError> {
    use ffmpeg_next::{codec, encoder, format, media, Rational};

    let _ = std::fs::remove_file(output);

    ffmpeg_next::init()
        .map_err(|e| EncoderError::VideoConvert(format!("FFmpeg init: {e}")))?;

    let mut ictx = format::input(input)
        .map_err(|e| EncoderError::VideoConvert(format!("Open input: {e}")))?;
    let mut octx = format::output(output)
        .map_err(|e| EncoderError::VideoConvert(format!("Create output: {e}")))?;

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
            .map_err(|e| EncoderError::VideoConvert(format!("Add stream: {e}")))?;
        ost.set_parameters(ist.parameters());
        unsafe {
            (*ost.parameters().as_mut_ptr()).codec_tag = 0;
        }
        ost_index += 1;
    }

    if ost_index == 0 {
        return Err(EncoderError::VideoConvert("No streams in TTS file".to_string()));
    }

    octx.set_metadata(ictx.metadata().to_owned());
    octx.write_header()
        .map_err(|e| EncoderError::VideoConvert(format!("Write header: {e}")))?;

    for (stream, mut packet) in ictx.packets() {
        let ist_index = stream.index();
        let mapped = stream_mapping[ist_index];
        if mapped < 0 {
            continue;
        }
        let ost = octx
            .stream(mapped as usize)
            .ok_or_else(|| EncoderError::VideoConvert("Missing output stream".to_string()))?;
        packet.rescale_ts(ist_time_bases[ist_index], ost.time_base());
        packet.set_position(-1);
        packet.set_stream(mapped as usize);
        packet
            .write_interleaved(&mut octx)
            .map_err(|e| EncoderError::VideoConvert(format!("Write packet: {e}")))?;
    }

    octx.write_trailer()
        .map_err(|e| EncoderError::VideoConvert(format!("Finalize: {e}")))?;
    Ok(())
}

#[cfg(not(feature = "ffmpeg"))]
fn try_ffmpeg_remux(_input: &Path, _output: &Path) -> Result<(), EncoderError> {
    Err(EncoderError::VideoConvert(
        "FFmpeg library not available".to_string(),
    ))
}

fn try_extract_embedded_mp4(input: &Path, output: &Path) -> Result<(), EncoderError> {
    let data = std::fs::read(input)?;
    let Some(start) = find_mp4_start(&data) else {
        return Err(EncoderError::VideoConvert(
            "No embedded MP4 (ftyp) found".to_string(),
        ));
    };

    let _ = std::fs::remove_file(output);
    std::fs::write(output, &data[start..])?;

    if output.metadata()?.len() < 1024 {
        let _ = std::fs::remove_file(output);
        return Err(EncoderError::VideoConvert(
            "Extracted MP4 too small".to_string(),
        ));
    }

    Ok(())
}

fn find_mp4_start(data: &[u8]) -> Option<usize> {
    for offset in 0..data.len().saturating_sub(8) {
        if &data[offset + 4..offset + 8] == b"ftyp" {
            return Some(offset);
        }
    }
    None
}

fn try_ffmpeg_cli(input: &Path, output: &Path) -> Result<(), EncoderError> {
    let _ = std::fs::remove_file(output);

    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
        ])
        .arg(input)
        .args([
            "-c",
            "copy",
            "-bsf:a",
            "aac_adtstoasc",
            "-movflags",
            "+faststart",
        ])
        .arg(output)
        .status()
        .map_err(|e| EncoderError::VideoConvert(format!("ffmpeg CLI not found: {e}")))?;

    if status.success() && output.exists() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(output);
        Err(EncoderError::VideoConvert(
            "ffmpeg CLI conversion failed".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_tts_extension() {
        assert!(is_tts_file(Path::new("video.tts")));
        assert!(is_tts_file(Path::new("cd58bc21aa3fc3355e311eac7b477072.mp4.tts")));
        assert!(!is_tts_file(Path::new("movie.mp4")));
    }

    #[test]
    fn mp4_output_strips_tts_suffix() {
        let input = Path::new("clip.mp4.tts");
        assert_eq!(
            mp4_output_path(input),
            Path::new("clip.mp4")
        );
        assert_eq!(
            mp4_output_path(Path::new("clip.tts")),
            Path::new("clip.mp4")
        );
    }

    #[test]
    fn finds_ftyp_atom() {
        let mut data = vec![0u8; 12];
        data[4..8].copy_from_slice(b"ftyp");
        assert_eq!(find_mp4_start(&data), Some(0));
    }
}
