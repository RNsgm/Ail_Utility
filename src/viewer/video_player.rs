use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};
use egui::{Context, TextureHandle, TextureOptions};

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
        let duration = probe_duration(&temp_path);
        let (frame_tx, frame_rx) = channel();
        let path = temp_path.clone();

        std::thread::spawn(move || {
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
                let texture = ctx.load_texture("video_frame", image.clone(), TextureOptions::LINEAR);
                self.current_texture = Some(texture);
                self.next_frame = None;
            }
        }

        while self.next_frame.is_none() {
            match self.receiver.try_recv() {
                Ok((image, pts)) => {
                    if pts <= elapsed {
                        let texture = ctx.load_texture("video_frame", image, TextureOptions::LINEAR);
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

    pub fn ui(&mut self, _ctx: &Context, ui: &mut egui::Ui) {
        let available_width = ui.available_width();
        let slider_width = (available_width - 140.0).max(100.0);
        let mut elapsed = self.elapsed().clamp(0.0, self.duration);

        ui.horizontal(|ui| {
            if ui.button(if self.is_paused { "?" } else { "?" }).clicked() {
                self.toggle_pause();
            }

            if ui.button(if self.looping { "??" } else { "?" }).on_hover_text("Loop").clicked() {
                self.looping = !self.looping;
            }

            ui.label(format!("{} / {}", fmt_time(elapsed), fmt_time(self.duration)));

            ui.add_sized(
                [slider_width, 20.0],
                egui::Slider::new(&mut elapsed, 0.0..=self.duration)
                    .show_value(false)
                    .trailing_fill(true),
            );
        });
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

        let (frame_tx, frame_rx) = channel();
        let path = self.temp_path.clone();

        std::thread::spawn(move || {
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
        let _ = std::fs::remove_file(&self.temp_path);
    }
}

fn fmt_time(seconds: f64) -> String {
    let secs = seconds as u64;
    let mins = secs / 60;
    let hrs = mins / 60;
    let rem = secs % 60;
    if hrs > 0 {
        format!("{:02}:{:02}:{:02}", hrs, mins % 60, rem)
    } else {
        format!("{:02}:{:02}", mins, rem)
    }
}

fn probe_duration(path: &PathBuf) -> f64 {
    use ffmpeg_next::format;
    if ffmpeg_next::init().is_err() { return 0.0; }
    let ictx = match format::input(path) { Ok(c) => c, Err(_) => return 0.0 };
    ictx.duration() as f64 / 1_000_000.0
}

fn decode_video_frames(
    path: &PathBuf,
    sender: std::sync::mpsc::Sender<(egui::ColorImage, f64)>,
    start_time: f64,
) {
    use ffmpeg_next::{codec, format, frame, software::scaling, util::format::Pixel};

    if ffmpeg_next::init().is_err() { return; }
    let mut ictx = match format::input(path) { Ok(c) => c, Err(_) => return };

    let video_stream_index = match ictx.streams().best(ffmpeg_next::media::Type::Video) {
        Some(s) => s.index(), None => return,
    };
    let tb = {
        let stream = match ictx.stream(video_stream_index) { Some(s) => s, None => return };
        stream.time_base()
    };

    if start_time > 0.0 {
        let ts = (start_time * f64::from(tb.denominator()) / f64::from(tb.numerator())) as i64;
        let _ = ictx.seek(ts, ..);
    }

    let stream = match ictx.stream(video_stream_index) { Some(s) => s, None => return };
    let time_base = stream.time_base();

    let decoder_params = stream.parameters();
    let decoder = match codec::context::Context::from_parameters(decoder_params.clone()) {
        Ok(d) => d, Err(_) => return,
    };
    let mut decoder = match decoder.decoder().video() { Ok(d) => d, Err(_) => return };

    let mut scaler = match scaling::Context::get(
        decoder.format(), decoder.width(), decoder.height(),
        Pixel::RGB24, decoder.width(), decoder.height(),
        scaling::Flags::BILINEAR,
    ) { Ok(s) => s, Err(_) => return };

    for (_stream, packet) in ictx.packets() {
        if _stream.index() != video_stream_index { continue; }
        let mut decoded = frame::Video::empty();
        if decoder.send_packet(&packet).is_err() { continue; }

        while decoder.receive_frame(&mut decoded).is_ok() {
            let mut rgb_frame = frame::Video::empty();
            if scaler.run(&decoded, &mut rgb_frame).is_err() { continue; }

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

            let size = [width, height];
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &rgba_data);
            let pts = decoded.timestamp().map(|ts| {
                ts as f64 * f64::from(time_base.numerator()) / f64::from(time_base.denominator())
            }).unwrap_or(0.0);

            if sender.send((color_image, pts)).is_err() { return; }
        }
    }
}