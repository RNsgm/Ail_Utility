mod buffer;
mod clock;
mod decoder;
mod presenter;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Context, TextureHandle};
use crate::viewer::audio_player::AudioPlayer;
use crate::{log_info};

use buffer::FrameBuffer;
use clock::PlaybackClock;
use decoder::{probe_duration, probe_fps, VideoDecoder};
use presenter::{DecodedFrame, FramePresenter};

/// Проигрыватель видео: master clock + декодер + буфер кадров + презентер + аудио.
pub struct VideoPlayer {
    path: PathBuf,
    clock: PlaybackClock,
    decoder: VideoDecoder,
    buffer: FrameBuffer,
    presenter: FramePresenter,
    awaiting_seek_frame: bool,
    stream_ended: bool,
    audio: Option<AudioPlayer>,
    fps: f64,
    last_presented: Option<Instant>,
    pub looping: bool,
    pub volume: f32,
}

impl VideoPlayer {
    pub fn new(ctx: &Context, path: PathBuf) -> Self {
        log_info!(
            "VideoPlayer::new path={} size={} bytes",
            path.display(),
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        );
        let duration = probe_duration(&path);
        let fps = probe_fps(&path);
        log_info!("VideoPlayer: duration={duration:.3}s fps={fps:.3}");

        let player = Self {
            path: path.clone(),
            clock: PlaybackClock::new(duration),
            decoder: VideoDecoder::start(path, 0.0),
            buffer: FrameBuffer::new(),
            presenter: FramePresenter::new(),
            awaiting_seek_frame: true,
            stream_ended: false,
            audio: None,
            fps,
            last_presented: None,
            looping: false,
            volume: 1.0,
        };

        ctx.request_repaint();
        player
    }

    pub fn update(&mut self, ctx: &Context) -> Option<&TextureHandle> {
        self.buffer.ingest(self.decoder.receiver());

        if self.awaiting_seek_frame {
            if let Some(frame) = self.buffer.take_latest() {
                self.present_frame(ctx, frame);
                self.awaiting_seek_frame = false;
                self.stream_ended = false;
                self.last_presented = Some(Instant::now());
            } else if self.buffer.is_disconnected() {
                self.awaiting_seek_frame = false;
            } else {
                ctx.request_repaint();
                return self.presenter.texture();
            }
        }

        if !self.clock.is_paused() && !self.stream_ended {
            if self.looping && self.clock.reached_end() {
                self.seek_to(ctx, 0.0);
            } else if self.should_present_frame() {
                if let Some(frame) = self.buffer.take_latest() {
                    self.present_frame(ctx, frame);
                    self.last_presented = Some(Instant::now());
                }
            }
        }

        if !self.clock.is_paused()
            && self.buffer.is_disconnected()
            && !self.buffer.has_frame()
            && !self.stream_ended
        {
            self.stream_ended = true;
            self.handle_end_of_stream(ctx);
        }

        self.schedule_repaint(ctx);
        self.presenter.texture()
    }

    pub fn toggle_pause(&mut self) {
        let paused = self.clock.toggle();
        if let Some(audio) = &self.audio {
            if paused {
                audio.pause();
            } else {
                audio.resume();
            }
        }
    }

    pub fn is_playing(&self) -> bool {
        !self.clock.is_paused()
    }

    pub fn is_paused(&self) -> bool {
        self.clock.is_paused()
    }

    pub fn elapsed(&self) -> f64 {
        self.clock.now()
    }

    pub fn duration(&self) -> f64 {
        self.clock.duration()
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(audio) = &self.audio {
            audio.set_volume(self.volume);
        }
    }

    pub fn seek_relative(&mut self, ctx: &Context, delta: f64) {
        let target = self.clock.clamp_target(self.clock.now() + delta);
        self.seek_to(ctx, target);
    }

    pub fn seek_to(&mut self, ctx: &Context, target: f64) {
        let target = self.clock.clamp_target(target);
        let was_paused = self.clock.is_paused();

        log_info!("VideoPlayer::seek_to {target:.3}s file={}", self.path.display());

        self.decoder.stop();
        self.buffer.drain(self.decoder.receiver());

        self.decoder = VideoDecoder::start(self.path.clone(), target);
        self.clock.seek(target);
        self.awaiting_seek_frame = true;
        self.stream_ended = false;
        self.last_presented = None;
        self.buffer.reset_disconnect();

        self.sync_audio(target, was_paused);
        ctx.request_repaint();
    }

    fn should_present_frame(&self) -> bool {
        let interval = self.frame_interval();
        self.last_presented
            .map(|t| t.elapsed() >= interval)
            .unwrap_or(true)
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_secs_f64((1.0 / self.fps.max(1.0)).min(0.05))
    }

    fn present_frame(&mut self, ctx: &Context, frame: DecodedFrame) {
        self.presenter.show(ctx, frame);
        self.ensure_audio();
    }

    fn ensure_audio(&mut self) {
        if self.audio.is_some() {
            return;
        }
        let start = self.clock.now();
        let Some(audio) = AudioPlayer::start(self.path.clone(), start) else {
            return;
        };
        audio.set_volume(self.volume);
        if self.clock.is_paused() {
            audio.pause();
        }
        self.audio = Some(audio);
    }

    fn sync_audio(&mut self, target: f64, was_paused: bool) {
        if let Some(audio) = &mut self.audio {
            audio.restart(self.path.clone(), target);
            audio.set_volume(self.volume);
            if was_paused {
                audio.pause();
            } else {
                audio.resume();
            }
        }
    }

    fn handle_end_of_stream(&mut self, ctx: &Context) {
        if self.looping && self.clock.duration() > 0.0 {
            self.seek_to(ctx, 0.0);
            return;
        }
        self.clock.pause();
        if let Some(audio) = &self.audio {
            audio.pause();
        }
    }

    fn schedule_repaint(&self, ctx: &Context) {
        if self.clock.is_paused() {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(self.frame_interval());
        }
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.decoder.stop();
        self.buffer.drain(self.decoder.receiver());
        self.audio = None;
        log_info!("VideoPlayer::drop removing {}", self.path.display());
        let _ = std::fs::remove_file(&self.path);
    }
}
