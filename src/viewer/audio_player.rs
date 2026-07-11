use crate::viewer::thread_util::spawn_worker;
use crate::{log_error, log_info};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const AUDIO_BUFFER_SAMPLES: usize = 48_000 * 2;

pub struct AudioPlayer {
    cancel: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    sample_rate: u32,
    channels: usize,
    _stream: cpal::Stream,
}

impl AudioPlayer {
    pub fn start(path: PathBuf, start_time: f64) -> Option<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let config = device.default_output_config().ok()?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;

        let buffer = Arc::new(Mutex::new(VecDeque::with_capacity(AUDIO_BUFFER_SAMPLES)));
        let buffer_cb = buffer.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let volume = Arc::new(AtomicU32::new(1000));
        let volume_cb = volume.clone();

        let stream = device
            .build_output_stream(
                &config.into(),
                move |data: &mut [f32], _| {
                    let vol = volume_cb.load(Ordering::Relaxed) as f32 / 1000.0;
                    let mut guard = match buffer_cb.lock() {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                    for sample in data.iter_mut() {
                        let v = guard.pop_front().unwrap_or(0.0) * vol;
                        *sample = v.clamp(-1.0, 1.0);
                    }
                },
                move |err| {
                    log_error!("Audio output error: {err}");
                },
                None,
            )
            .ok()?;
        stream.play().ok()?;

        let player = Self {
            cancel,
            volume,
            buffer,
            sample_rate,
            channels,
            _stream: stream,
        };
        player.spawn_decode(path, start_time);
        Some(player)
    }

    pub fn set_volume(&self, volume: f32) {
        let v = (volume.clamp(0.0, 1.0) * 1000.0) as u32;
        self.volume.store(v, Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        self.volume.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn pause(&self) {
        let _ = self._stream.pause();
    }

    pub fn resume(&self) {
        let _ = self._stream.play();
    }

    pub fn restart(&mut self, path: PathBuf, start_time: f64) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Ok(mut guard) = self.buffer.lock() {
            guard.clear();
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        self.spawn_decode(path, start_time);
    }

    fn spawn_decode(&self, path: PathBuf, start_time: f64) {
        let cancel = self.cancel.clone();
        let buffer = self.buffer.clone();
        let sample_rate = self.sample_rate;
        let channels = self.channels;
        spawn_worker("audio_decode", move || {
            decode_audio_to_buffer(
                &path,
                start_time,
                sample_rate,
                channels,
                buffer,
                cancel,
            );
        });
        log_info!(
            "AudioPlayer: decode {} @ {start_time:.3}s",
            path.display()
        );
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.clear();
        }
    }
}

#[cfg(feature = "ffmpeg")]
fn decode_audio_to_buffer(
    path: &PathBuf,
    start_time: f64,
    sample_rate: u32,
    channels: usize,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    cancel: Arc<AtomicBool>,
) {
    use ffmpeg_next::{codec, format, frame, media, software::resampling, util::format::Sample};
    use ffmpeg_next::util::format::sample::Type;

    let result = crate::ffmpeg_util::with_ffmpeg(|| {
        let mut input = format::input(path).map_err(|e| e.to_string())?;

        let stream_index = input
            .streams()
            .best(media::Type::Audio)
            .ok_or_else(|| "No audio stream".to_string())?
            .index();

        let stream = input
            .stream(stream_index)
            .ok_or_else(|| "Missing audio stream".to_string())?;
        let mut decoder_ctx = codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| e.to_string())?;
        let mut decoder = decoder_ctx.decoder().audio().map_err(|e| e.to_string())?;

        if start_time > 0.0 {
            let ts = (start_time * 1_000_000.0).round() as i64;
            input
                .seek(ts, ..ts)
                .map_err(|e| format!("audio seek failed: {e}"))?;
            decoder.flush();
        }

        let out_layout = ffmpeg_next::util::channel_layout::ChannelLayout::STEREO;
        let out_channels = channels.max(2) as u16;

        let mut resampler = resampling::Context::get(
            decoder.format(),
            decoder.channel_layout(),
            decoder.rate(),
            Sample::F32(Type::Packed),
            out_layout,
            sample_rate,
        )
        .map_err(|e| e.to_string())?;

        for (stream, packet) in input.packets() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if stream.index() != stream_index {
                continue;
            }

            if decoder.send_packet(&packet).is_err() {
                continue;
            }

            let mut decoded = frame::Audio::empty();
            while decoder.receive_frame(&mut decoded).is_ok() {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(());
                }

                let mut converted = frame::Audio::empty();
                if resampler.run(&decoded, &mut converted).is_err() {
                    continue;
                }

                let sample_count = converted.samples();
                let ch = out_channels as usize;
                let byte_len = sample_count * ch * 4;
                let data = converted.data(0);
                if data.len() < byte_len {
                    continue;
                }

                let mut samples = Vec::with_capacity(sample_count * ch);
                for chunk in data[..byte_len].chunks_exact(4) {
                    samples.push(f32::from_le_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3],
                    ]));
                }

                if let Ok(mut guard) = buffer.lock() {
                    for s in samples {
                        if guard.len() >= AUDIO_BUFFER_SAMPLES {
                            guard.pop_front();
                        }
                        guard.push_back(s);
                    }
                }
            }
        }

        Ok(())
    });

    if let Err(e) = result {
        log_error!("decode_audio_to_buffer failed for {}: {e}", path.display());
    }
}

#[cfg(not(feature = "ffmpeg"))]
fn decode_audio_to_buffer(
    _path: &PathBuf,
    _start_time: f64,
    _sample_rate: u32,
    _channels: usize,
    _buffer: Arc<Mutex<VecDeque<f32>>>,
    _cancel: Arc<AtomicBool>,
) {
}
