use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

use crate::viewer::thread_util::spawn_worker;
use crate::{log_error, log_info};

use super::presenter::DecodedFrame;

const FRAME_BUFFER: usize = 32;

pub struct VideoDecoder {
    receiver: Receiver<DecodedFrame>,
    cancel: Option<Arc<AtomicBool>>,
}

impl VideoDecoder {
    pub fn start(path: PathBuf, start_secs: f64) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel(FRAME_BUFFER);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker_path = path;

        spawn_worker("video_decode", move || {
            decode_loop(&worker_path, tx, start_secs, worker_cancel);
        });

        Self {
            receiver: rx,
            cancel: Some(cancel),
        }
    }

    pub fn receiver(&self) -> &Receiver<DecodedFrame> {
        &self.receiver
    }

    pub fn stop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

pub fn probe_fps(path: &PathBuf) -> f64 {
    #[cfg(feature = "ffmpeg")]
    {
        use ffmpeg_next::format;

        return crate::ffmpeg_util::with_ffmpeg(|| {
            let ictx = format::input(path).map_err(|e| e.to_string())?;
            let stream = ictx
                .streams()
                .best(ffmpeg_next::media::Type::Video)
                .ok_or_else(|| "No video stream".to_string())?;
            Ok(stream_fps(&stream))
        })
        .unwrap_or(30.0);
    }

    #[cfg(not(feature = "ffmpeg"))]
    {
        let _ = path;
        30.0
    }
}

#[cfg(feature = "ffmpeg")]
fn rational_fps(rate: ffmpeg_next::util::rational::Rational) -> Option<f64> {
    if rate.numerator() > 0 && rate.denominator() > 0 {
        let fps = rate.numerator() as f64 / rate.denominator() as f64;
        if (1.0..=240.0).contains(&fps) {
            return Some(fps);
        }
    }
    None
}

#[cfg(feature = "ffmpeg")]
fn stream_fps(stream: &ffmpeg_next::format::stream::Stream) -> f64 {
    rational_fps(stream.avg_frame_rate())
        .or_else(|| rational_fps(stream.rate()))
        .unwrap_or(30.0)
        .max(1.0)
}

pub fn probe_duration(path: &PathBuf) -> f64 {
    #[cfg(feature = "ffmpeg")]
    {
        use ffmpeg_next::format;

        log_info!("probe_duration: {}", path.display());
        return crate::ffmpeg_util::with_ffmpeg(|| {
            let ictx = format::input(path).map_err(|e| e.to_string())?;
            let dur = ictx.duration() as f64 / 1_000_000.0;
            log_info!("probe_duration: OK duration={dur:.3}s");
            Ok(dur)
        })
        .unwrap_or_else(|e| {
            log_error!("probe_duration failed for {}: {e}", path.display());
            0.0
        });
    }

    #[cfg(not(feature = "ffmpeg"))]
    {
        let _ = path;
        0.0
    }
}

fn decode_loop(
    path: &PathBuf,
    sender: SyncSender<DecodedFrame>,
    start_secs: f64,
    cancel: Arc<AtomicBool>,
) {
    #[cfg(feature = "ffmpeg")]
    {
        let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        log_info!(
            "video_decode: path={} size={file_size} start={start_secs:.3}s",
            path.display()
        );

        let mut decoder = match crate::ffmpeg_util::with_ffmpeg(|| DecoderState::open(path, start_secs))
        {
            Ok(state) => state,
            Err(e) => {
                log_error!("video_decode open failed for {}: {e}", path.display());
                return;
            }
        };

        let mut frames_sent = 0u64;

        loop {
            if cancel.load(Ordering::Relaxed) {
                log_info!("video_decode: cancelled path={}", path.display());
                break;
            }

            let frame = match crate::ffmpeg_util::with_ffmpeg(|| decoder.next_frame()) {
                Ok(frame) => frame,
                Err(e) => {
                    log_error!("video_decode frame error for {}: {e}", path.display());
                    break;
                }
            };

            let Some(mut pending) = frame else {
                log_info!(
                    "video_decode: EOF path={} frames_sent={frames_sent}",
                    path.display()
                );
                break;
            };

            loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }

                match sender.try_send(pending) {
                    Ok(()) => {
                        frames_sent += 1;
                        if frames_sent == 1 || frames_sent % 30 == 0 {
                            log_info!("video_decode: sent frame #{frames_sent}");
                        }
                        break;
                    }
                    Err(TrySendError::Full(frame)) => {
                        pending = frame;
                        std::thread::sleep(Duration::from_millis(4));
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        log_info!("video_decode: receiver closed after {frames_sent} frames");
                        return;
                    }
                }
            }
        }
    }

    #[cfg(not(feature = "ffmpeg"))]
    {
        let _ = (path, sender, start_secs, cancel);
    }
}

#[cfg(feature = "ffmpeg")]
struct DecoderState {
    input: ffmpeg_next::format::context::Input,
    stream_index: usize,
    decoder: ffmpeg_next::decoder::Video,
    scaler: ffmpeg_next::software::scaling::Context,
    time_base: ffmpeg_next::util::rational::Rational,
    start_secs: f64,
    fps: f64,
    frames_emitted: u64,
    waiting_keyframe: bool,
}

#[cfg(feature = "ffmpeg")]
fn ts_to_seconds(ts: i64, time_base: ffmpeg_next::util::rational::Rational) -> f64 {
    ts as f64 * f64::from(time_base.numerator()) / f64::from(time_base.denominator())
}

#[cfg(feature = "ffmpeg")]
fn seconds_to_av_time(seconds: f64) -> i64 {
    (seconds * 1_000_000.0).round() as i64
}

#[cfg(feature = "ffmpeg")]
impl DecoderState {
    fn open(path: &PathBuf, start_secs: f64) -> Result<Self, String> {
        use ffmpeg_next::{codec, format, software::scaling, util::format::Pixel};

        let mut input = format::input(path).map_err(|e| e.to_string())?;

        let stream_index = input
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .ok_or_else(|| "No video stream".to_string())?
            .index();

        let stream = input
            .stream(stream_index)
            .ok_or_else(|| "Missing video stream".to_string())?;
        let time_base = stream.time_base();
        let fps = stream_fps(&stream);

        let decoder_ctx = codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| e.to_string())?;
        let mut decoder = decoder_ctx.decoder().video().map_err(|e| e.to_string())?;

        let waiting_keyframe = start_secs > 0.0;
        if waiting_keyframe {
            let ts = seconds_to_av_time(start_secs);
            log_info!("DecoderState::open seek ts={ts} ({start_secs:.3}s)");
            input
                .seek(ts, ..ts)
                .map_err(|e| format!("seek failed: {e}"))?;
            decoder.flush();
        }

        log_info!(
            "DecoderState::open stream #{stream_index} {}x{} fps={fps:.3}",
            decoder.width(),
            decoder.height()
        );

        let scaler = scaling::Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::RGB24,
            decoder.width(),
            decoder.height(),
            scaling::Flags::BILINEAR,
        )
        .map_err(|e| e.to_string())?;

        Ok(Self {
            input,
            stream_index,
            decoder,
            scaler,
            time_base,
            start_secs,
            fps,
            frames_emitted: 0,
            waiting_keyframe,
        })
    }

    fn next_frame(&mut self) -> Result<Option<DecodedFrame>, String> {
        use ffmpeg_next::{frame, Error, Packet};

        loop {
            let mut decoded = frame::Video::empty();
            while self.decoder.receive_frame(&mut decoded).is_ok() {
                if let Some(frame) = self.finish_frame(decoded, None)? {
                    return Ok(Some(frame));
                }
                decoded = frame::Video::empty();
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

            let packet_pts = packet.pts().map(|ts| ts_to_seconds(ts, self.time_base));
            if self.decoder.send_packet(&packet).is_err() {
                continue;
            }
        }
    }

    fn finish_frame(
        &mut self,
        decoded: ffmpeg_next::frame::Video,
        packet_pts: Option<f64>,
    ) -> Result<Option<DecodedFrame>, String> {
        use ffmpeg_next::frame;

        if self.waiting_keyframe {
            if !decoded.is_key() {
                return Ok(None);
            }
            self.waiting_keyframe = false;
        }

        let raw_pts = decoded
            .pts()
            .map(|ts| ts_to_seconds(ts, self.time_base))
            .or_else(|| {
                decoded
                    .timestamp()
                    .map(|ts| ts_to_seconds(ts, self.time_base))
            })
            .or(packet_pts);

        if raw_pts.map(|p| p + 0.05 < self.start_secs).unwrap_or(false) {
            return Ok(None);
        }

        let pts = self.start_secs + self.frames_emitted as f64 / self.fps;
        self.frames_emitted += 1;

        let mut rgb_frame = frame::Video::empty();
        if self.scaler.run(&decoded, &mut rgb_frame).is_err() {
            return Ok(None);
        }

        let width = self.decoder.width() as usize;
        let height = self.decoder.height() as usize;
        let stride = rgb_frame.stride(0) as usize;

        let mut rgba_data = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            let offset = y * stride;
            for x in 0..width {
                rgba_data.extend_from_slice(&[
                    rgb_frame.data(0)[offset + x * 3],
                    rgb_frame.data(0)[offset + x * 3 + 1],
                    rgb_frame.data(0)[offset + x * 3 + 2],
                    255,
                ]);
            }
        }

        let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba_data);
        Ok(Some(DecodedFrame { image, pts }))
    }
}
