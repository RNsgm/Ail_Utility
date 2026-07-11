use crate::viewer::thread_util::spawn_worker;
use crate::{log_error, log_info, log_warn};
use egui::{Context, TextureHandle, TextureOptions};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::time::{Duration, Instant};

const FRAME_BUFFER: usize = 3;

pub struct VideoPlayer {
    receiver: Receiver<(egui::ColorImage, f64)>,
    current_texture: Option<TextureHandle>,
    next_frame: Option<(egui::ColorImage, f64)>,
    base_time: Instant,
    total_pause: Duration,
    paused_at: Option<Instant>,
    pub is_paused: bool,
    temp_path: PathBuf,
    pub duration: f64,
    pub looping: bool,
    pub seek_target: Option<f64>,
}

impl VideoPlayer {
    pub fn new(ctx: &Context, temp_path: PathBuf) -> Self {
        log_info!(
            "VideoPlayer::new path={} size={} bytes",
            temp_path.display(),
            std::fs::metadata(&temp_path).map(|m| m.len()).unwrap_or(0)
        );
        let duration = probe_duration(&temp_path);
        log_info!("VideoPlayer: probed duration={duration:.3}s");
        let (frame_tx, frame_rx) = std::sync::mpsc::sync_channel(FRAME_BUFFER);
        let path = temp_path.clone();

        spawn_worker("video_decode", move || {
            decode_video_frames(&path, frame_tx, 0.0);
        });

        let player = Self {
            receiver: frame_rx,
            current_texture: None,
            next_frame: None,
            base_time: Instant::now(),
            total_pause: Duration::ZERO,
            paused_at: None,
            is_paused: false,
            temp_path,
            duration,
            looping: false,
            seek_target: None,
        };

        ctx.request_repaint();
        player
    }

    pub fn update(&mut self, ctx: &Context) -> Option<&TextureHandle> {
        if let Some(target) = self.seek_target.take() {
            self.seek_to(ctx, target);
        }

        if self.is_paused {
            return self.current_texture.as_ref();
        }

        let mut elapsed = self.elapsed();

        if self.looping && self.duration > 0.0 && elapsed >= self.duration {
            self.seek_to(ctx, 0.0);
            elapsed = 0.0;
        }

        if let Some((ref image, pts)) = self.next_frame {
            if pts <= elapsed {
                let texture =
                    ctx.load_texture("video_frame", image.clone(), TextureOptions::LINEAR);
                self.current_texture = Some(texture);
                self.next_frame = None;
            }
        }

        while self.next_frame.is_none() {
            match self.receiver.try_recv() {
                Ok((image, pts)) => {
                    if pts <= elapsed {
                        let texture =
                            ctx.load_texture("video_frame", image, TextureOptions::LINEAR);
                        self.current_texture = Some(texture);
                    } else {
                        self.next_frame = Some((image, pts));
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        ctx.request_repaint();
        self.current_texture.as_ref()
    }

    pub fn toggle_pause(&mut self) {
        if self.is_paused {
            if let Some(paused) = self.paused_at {
                self.total_pause += Instant::now().duration_since(paused);
                self.paused_at = None;
            }
            self.is_paused = false;
        } else {
            self.paused_at = Some(Instant::now());
            self.is_paused = true;
        }
    }

    pub fn is_playing(&self) -> bool {
        !self.is_paused
    }

    pub fn elapsed(&self) -> f64 {
        let now = Instant::now();
        let current_pause = if let Some(paused) = self.paused_at {
            now.duration_since(paused)
        } else {
            Duration::ZERO
        };
        let total = self.total_pause + current_pause;
        now.duration_since(self.base_time).as_secs_f64() - total.as_secs_f64()
    }

    pub fn seek_to(&mut self, ctx: &Context, target: f64) {
        let target = target.clamp(0.0, self.duration);
        log_info!(
            "VideoPlayer::seek_to {target:.3}s file={}",
            self.temp_path.display()
        );

        let (frame_tx, frame_rx) = std::sync::mpsc::sync_channel(FRAME_BUFFER);
        let path = self.temp_path.clone();

        spawn_worker("video_seek", move || {
            decode_video_frames(&path, frame_tx, target);
        });

        self.receiver = frame_rx;
        self.next_frame = None;
        self.base_time = Instant::now() - Duration::from_secs_f64(target);
        self.total_pause = Duration::ZERO;
        self.paused_at = None;
        self.is_paused = false;

        ctx.request_repaint();
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        log_info!("VideoPlayer::drop removing {}", self.temp_path.display());
        let _ = std::fs::remove_file(&self.temp_path);
    }
}

fn probe_duration(path: &PathBuf) -> f64 {
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

fn decode_video_frames(
    path: &PathBuf,
    sender: SyncSender<(egui::ColorImage, f64)>,
    start_time: f64,
) {
    #[cfg(feature = "ffmpeg")]
    {
        use ffmpeg_next::{codec, format, frame, software::scaling, util::format::Pixel};

        let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        log_info!(
            "decode_video_frames: start path={} size={file_size} start_time={start_time:.3}s",
            path.display()
        );

        let result = crate::ffmpeg_util::with_ffmpeg(|| {
            let mut ictx = format::input(path).map_err(|e| e.to_string())?;

            let video_stream_index = ictx
                .streams()
                .best(ffmpeg_next::media::Type::Video)
                .ok_or_else(|| "No video stream".to_string())?
                .index();

            let tb = ictx
                .stream(video_stream_index)
                .ok_or_else(|| "Missing video stream".to_string())?
                .time_base();

            if start_time > 0.0 {
                let ts =
                    (start_time * f64::from(tb.denominator()) / f64::from(tb.numerator())) as i64;
                log_info!("decode_video_frames: seek to ts={ts}");
                let _ = ictx.seek(ts, ..);
            }

            let stream = ictx
                .stream(video_stream_index)
                .ok_or_else(|| "Missing video stream".to_string())?;
            let time_base = stream.time_base();

            let decoder_params = stream.parameters();
            let decoder = codec::context::Context::from_parameters(decoder_params.clone())
                .map_err(|e| e.to_string())?;
            let mut decoder = decoder.decoder().video().map_err(|e| e.to_string())?;

            log_info!(
                "decode_video_frames: stream #{video_stream_index} {}x{} format={:?}",
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
            .map_err(|e| e.to_string())?;

            let mut frames_sent = 0u64;
            let mut packets = 0u64;

            for (_stream, packet) in ictx.packets() {
                if _stream.index() != video_stream_index {
                    continue;
                }
                packets += 1;

                let mut decoded = frame::Video::empty();
                if decoder.send_packet(&packet).is_err() {
                    log_warn!("decode_video_frames: send_packet failed at packet #{packets}");
                    continue;
                }

                while decoder.receive_frame(&mut decoded).is_ok() {
                    let pts = decoded
                        .timestamp()
                        .map(|ts| {
                            ts as f64 * f64::from(time_base.numerator())
                                / f64::from(time_base.denominator())
                        })
                        .unwrap_or(0.0);

                    if pts + 0.001 < start_time {
                        continue;
                    }

                    let mut rgb_frame = frame::Video::empty();
                    if scaler.run(&decoded, &mut rgb_frame).is_err() {
                        log_warn!("decode_video_frames: scaler failed at pts={pts:.3}");
                        continue;
                    }

                    let width = decoder.width() as usize;
                    let height = decoder.height() as usize;
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

                    let color_image =
                        egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba_data);

                    if sender.send((color_image, pts)).is_err() {
                        log_info!(
                            "decode_video_frames: receiver closed after {frames_sent} frames"
                        );
                        return Ok(());
                    }

                    frames_sent += 1;
                    if frames_sent == 1 || frames_sent % 30 == 0 {
                        log_info!("decode_video_frames: sent frame #{frames_sent} pts={pts:.3}s");
                    }
                }
            }

            log_info!(
                "decode_video_frames: done path={} packets={packets} frames_sent={frames_sent}",
                path.display()
            );
            Ok(())
        });

        if let Err(e) = result {
            log_error!("decode_video_frames failed for {}: {e}", path.display());
        }
    }

    #[cfg(not(feature = "ffmpeg"))]
    {
        let _ = (path, sender, start_time);
    }
}
