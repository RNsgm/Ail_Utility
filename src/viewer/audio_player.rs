use crate::viewer::thread_util::spawn_worker;
use crate::{log_error, log_info};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// ~2 s of stereo audio at 48 kHz.
const AUDIO_BUFFER_SAMPLES: usize = 48_000 * 2 * 2;
/// Keep the decode thread this far ahead of playback.
const AUDIO_AHEAD_SECS: f64 = 1.5;
/// Don't start speakers until at least this much audio is buffered.
const AUDIO_PREFILL_SECS: f64 = 0.25;

/// Typical WASAPI/cpal output queue depth when buffer size is unknown.
const DEFAULT_OUTPUT_LATENCY_SECS: f64 = 0.2;

pub struct AudioPlayer {
    cancel: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    samples_played: Arc<AtomicU64>,
    /// PTS (сек) самого старого сэмпла в буфере, f64::to_bits().
    timeline_pts: Arc<AtomicU64>,
    sample_rate: u32,
    channels: usize,
    /// Задержка между подачей сэмпла в cpal и звуком в колонках.
    output_latency_secs: f64,
    _stream: cpal::Stream,
}

impl AudioPlayer {
    pub fn start(path: PathBuf, start_time: f64) -> Option<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let config = device.default_output_config().ok()?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let output_latency_secs = estimate_output_latency_secs(&config);
        log_info!("AudioPlayer: output_latency={output_latency_secs:.3}s");

        let buffer = Arc::new(Mutex::new(VecDeque::with_capacity(AUDIO_BUFFER_SAMPLES)));
        let buffer_cb = buffer.clone();
        let samples_played = Arc::new(AtomicU64::new(0));
        let samples_cb = samples_played.clone();
        let timeline_pts = Arc::new(AtomicU64::new(start_time.to_bits()));
        let timeline_decode = timeline_pts.clone();
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
                        if let Some(v) = guard.pop_front() {
                            samples_cb.fetch_add(1, Ordering::Relaxed);
                            *sample = f32::clamp(v * vol, -1.0, 1.0);
                        } else {
                            *sample = 0.0;
                        }
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
            samples_played,
            timeline_pts,
            sample_rate,
            channels,
            output_latency_secs,
            _stream: stream,
        };
        player.spawn_decode(path, start_time, timeline_decode);
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

    pub fn playback_time(&self) -> f64 {
        self.stream_position()
    }

    /// Позиция звука, который пользователь слышит сейчас (с учётом буфера DAC).
    pub fn heard_time(&self) -> f64 {
        (self.stream_position() - self.output_latency_secs).max(0.0)
    }

    pub fn output_latency_secs(&self) -> f64 {
        self.output_latency_secs
    }

    fn stream_position(&self) -> f64 {
        let origin = f64::from_bits(self.timeline_pts.load(Ordering::Relaxed));
        let samples = self.samples_played.load(Ordering::Relaxed) as f64;
        let rate = self.sample_rate as f64 * self.channels as f64;
        if rate <= 0.0 {
            return origin;
        }
        origin + samples / rate
    }

    pub fn restart(&mut self, path: PathBuf, start_time: f64) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Ok(mut guard) = self.buffer.lock() {
            guard.clear();
        }
        self.samples_played.store(0, Ordering::Relaxed);
        self.timeline_pts
            .store(start_time.to_bits(), Ordering::Relaxed);
        let _ = self._stream.pause();
        self.cancel = Arc::new(AtomicBool::new(false));
        let timeline_decode = self.timeline_pts.clone();
        self.spawn_decode(path, start_time, timeline_decode);
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

    fn spawn_decode(
        &self,
        path: PathBuf,
        start_time: f64,
        timeline_pts: Arc<AtomicU64>,
    ) {
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
                timeline_pts,
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

fn estimate_output_latency_secs(config: &cpal::SupportedStreamConfig) -> f64 {
    let rate = config.sample_rate().0 as f64;
    let period_secs = match config.buffer_size() {
        cpal::SupportedBufferSize::Range { max, .. } => *max as f64 / rate,
        cpal::SupportedBufferSize::Unknown => 0.0,
    };
    if period_secs > 0.0 {
        // WASAPI обычно держит 2–4 периода в очереди до ЦАП.
        (period_secs * 3.0).clamp(0.08, 0.35)
    } else {
        DEFAULT_OUTPUT_LATENCY_SECS
    }
}

#[cfg(feature = "ffmpeg")]
struct AudioDecoderState {
    input: ffmpeg_next::format::context::Input,
    stream_index: usize,
    decoder: ffmpeg_next::decoder::Audio,
    resampler: ffmpeg_next::software::resampling::Context,
    time_base: ffmpeg_next::util::rational::Rational,
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
        let time_base = stream.time_base();
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
            time_base,
            out_channels: out_channels as usize,
        })
    }

    fn frame_pts(&self, decoded: &ffmpeg_next::frame::Audio) -> Option<f64> {
        decoded
            .pts()
            .map(|ts| ts_to_seconds(ts, self.time_base))
    }

    fn decode_chunk(&mut self) -> Result<Option<(f64, Vec<f32>)>, String> {
        use ffmpeg_next::{frame, Error, Packet};

        loop {
            let mut decoded = frame::Audio::empty();
            while self.decoder.receive_frame(&mut decoded).is_ok() {
                if let Some(samples) = self.convert_frame(&decoded)? {
                    let pts = self.frame_pts(&decoded).unwrap_or(0.0);
                    return Ok(Some((pts, samples)));
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

    fn decode_batch(&mut self, max_samples: usize) -> Result<Option<(f64, Vec<f32>)>, String> {
        let mut batch = Vec::with_capacity(max_samples.min(16_384));
        let mut batch_pts = None;

        while batch.len() < max_samples {
            let Some((pts, chunk)) = self.decode_chunk()? else {
                break;
            };
            if batch_pts.is_none() {
                batch_pts = Some(pts);
            }
            batch.extend(chunk);
        }

        if batch.is_empty() {
            Ok(None)
        } else {
            Ok(Some((batch_pts.unwrap_or(0.0), batch)))
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
fn ts_to_seconds(ts: i64, time_base: ffmpeg_next::util::rational::Rational) -> f64 {
    ts as f64 * f64::from(time_base.numerator()) / f64::from(time_base.denominator())
}

#[cfg(feature = "ffmpeg")]
fn decode_audio_to_buffer(
    path: &PathBuf,
    start_time: f64,
    sample_rate: u32,
    channels: usize,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    timeline_pts: Arc<AtomicU64>,
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

        let Some((pts, samples)) = batch else {
            break;
        };

        if pts + 0.04 < start_time {
            continue;
        }

        if let Ok(mut guard) = buffer.lock() {
            if guard.is_empty() {
                timeline_pts.store(pts.to_bits(), Ordering::Relaxed);
            }
            for sample in samples {
                guard.push_back(sample);
            }
            let sample_dt = 1.0 / (sample_rate as f64 * channels as f64);
            while guard.len() > AUDIO_BUFFER_SAMPLES {
                guard.pop_front();
                let current = f64::from_bits(timeline_pts.load(Ordering::Relaxed));
                timeline_pts.store((current + sample_dt).to_bits(), Ordering::Relaxed);
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
    _timeline_pts: Arc<AtomicU64>,
    _cancel: Arc<AtomicBool>,
) {
}
