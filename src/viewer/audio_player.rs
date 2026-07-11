use crate::viewer::thread_util::spawn_worker;
use crate::{log_error, log_info};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// ~2 s of stereo audio at 48 kHz.
const AUDIO_BUFFER_SAMPLES: usize = 48_000 * 2 * 2;
/// Keep the decode thread this far ahead of playback.
const AUDIO_AHEAD_SECS: f64 = 1.5;
/// Don't start speakers until at least this much audio is buffered.
const AUDIO_PREFILL_SECS: f64 = 0.75;

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

        let player = Self {
            cancel: cancel.clone(),
            volume,
            buffer: buffer.clone(),
            sample_rate,
            channels,
            _stream: stream,
        };
        player.spawn_decode(path, start_time);
        player.wait_prefill();
        player._stream.play().ok()?;
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
        let _ = self._stream.pause();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.spawn_decode(path, start_time);
        self.wait_prefill();
        let _ = self._stream.play();
    }

    fn wait_prefill(&self) {
        let prefill_samples = (self.sample_rate as f64
            * self.channels as f64
            * AUDIO_PREFILL_SECS) as usize;
        for _ in 0..300 {
            if self.cancel.load(Ordering::Relaxed) {
                return;
            }
            let ready = self
                .buffer
                .lock()
                .map(|guard| guard.len() >= prefill_samples)
                .unwrap_or(false);
            if ready {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn spawn_decode(&self, path: PathBuf, start_time: f64) {
        log_info!(
            "AudioPlayer: decode {} @ {start_time:.3}s",
            path.display()
        );

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
struct AudioDecoderState {
    input: ffmpeg_next::format::context::Input,
    stream_index: usize,
    decoder: ffmpeg_next::decoder::Audio,
    resampler: ffmpeg_next::software::resampling::Context,
    out_channels: usize,
}

#[cfg(feature = "ffmpeg")]
impl AudioDecoderState {
    fn open(
        path: &PathBuf,
        start_time: f64,
        sample_rate: u32,
        channels: usize,
    ) -> Result<Self, String> {
        use ffmpeg_next::{codec, format, media, software::resampling, util::format::Sample};
        use ffmpeg_next::util::format::sample::Type;

        let mut input = format::input(path).map_err(|e| e.to_string())?;
        let stream_index = input
            .streams()
            .best(media::Type::Audio)
            .ok_or_else(|| "No audio stream".to_string())?
            .index();
        let stream = input
            .stream(stream_index)
            .ok_or_else(|| "Missing audio stream".to_string())?;
        let decoder_ctx = codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| e.to_string())?;
        let mut decoder = decoder_ctx.decoder().audio().map_err(|e| e.to_string())?;

        if start_time > 0.0 {
            let ts = (start_time * 1_000_000.0).round() as i64;
            input
                .seek(ts, ..ts)
                .map_err(|e| format!("audio seek failed: {e}"))?;
            decoder.flush();
        }

        let out_channels = channels.max(2) as u16;
        let out_layout = if out_channels >= 2 {
            ffmpeg_next::util::channel_layout::ChannelLayout::STEREO
        } else {
            ffmpeg_next::util::channel_layout::ChannelLayout::MONO
        };

        let resampler = resampling::Context::get(
            decoder.format(),
            decoder.channel_layout(),
            decoder.rate(),
            Sample::F32(Type::Packed),
            out_layout,
            sample_rate,
        )
        .map_err(|e| e.to_string())?;

        Ok(Self {
            input,
            stream_index,
            decoder,
            resampler,
            out_channels: out_channels as usize,
        })
    }

    fn decode_chunk(&mut self) -> Result<Option<Vec<f32>>, String> {
        use ffmpeg_next::{frame, Error, Packet};

        loop {
            let mut decoded = frame::Audio::empty();
            while self.decoder.receive_frame(&mut decoded).is_ok() {
                if let Some(samples) = self.convert_frame(&decoded)? {
                    return Ok(Some(samples));
                }
                decoded = frame::Audio::empty();
            }

            let mut packet = Packet::empty();
            loop {
                match packet.read(&mut self.input) {
                    Ok(()) => break,
                    Err(Error::Eof) => return Ok(None),
                    Err(_) => continue,
                }
            }

            if packet.stream() != self.stream_index {
                continue;
            }

            if self.decoder.send_packet(&packet).is_err() {
                continue;
            }
        }
    }

    fn decode_batch(&mut self, max_samples: usize) -> Result<Option<Vec<f32>>, String> {
        let mut batch = Vec::with_capacity(max_samples.min(16_384));

        while batch.len() < max_samples {
            let Some(chunk) = self.decode_chunk()? else {
                break;
            };
            batch.extend(chunk);
        }

        if batch.is_empty() {
            Ok(None)
        } else {
            Ok(Some(batch))
        }
    }

    fn convert_frame(
        &mut self,
        decoded: &ffmpeg_next::frame::Audio,
    ) -> Result<Option<Vec<f32>>, String> {
        let mut converted = ffmpeg_next::frame::Audio::empty();
        if self.resampler.run(decoded, &mut converted).is_err() {
            return Ok(None);
        }

        let sample_count = converted.samples();
        let ch = self.out_channels;
        let byte_len = sample_count * ch * 4;
        let data = converted.data(0);
        if data.len() < byte_len {
            return Ok(None);
        }

        let mut samples = Vec::with_capacity(sample_count * ch);
        for chunk in data[..byte_len].chunks_exact(4) {
            samples.push(f32::from_le_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3],
            ]));
        }

        Ok(Some(samples))
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
    let mut decoder = match crate::ffmpeg_util::with_ffmpeg(|| {
        AudioDecoderState::open(path, start_time, sample_rate, channels)
    }) {
        Ok(decoder) => decoder,
        Err(e) => {
            log_error!("audio decode open failed for {}: {e}", path.display());
            return;
        }
    };

    let max_ahead = (sample_rate as f64 * channels as f64 * AUDIO_AHEAD_SECS) as usize;
    let batch_target = (sample_rate as f64 * channels as f64 * 0.1) as usize;

    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let ahead = buffer.lock().map(|guard| guard.len()).unwrap_or(max_ahead);
            if ahead < max_ahead {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }

        let batch = match crate::ffmpeg_util::with_ffmpeg(|| {
            decoder.decode_batch(batch_target.max(4_096))
        }) {
            Ok(batch) => batch,
            Err(e) => {
                log_error!("audio decode batch failed for {}: {e}", path.display());
                break;
            }
        };

        let Some(samples) = batch else {
            break;
        };

        if let Ok(mut guard) = buffer.lock() {
            for sample in samples {
                guard.push_back(sample);
            }
            while guard.len() > AUDIO_BUFFER_SAMPLES {
                guard.pop_front();
            }
        }
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
