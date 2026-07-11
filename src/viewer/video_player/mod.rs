mod buffer;
mod clock;
mod decoder;
mod presenter;

use std::path::PathBuf;
use std::time::Duration;

use egui::{Context, TextureHandle};
use crate::viewer::audio_player::AudioPlayer;
use crate::{log_info};

use buffer::FrameQueue;
use clock::PlaybackClock;
use decoder::{probe_duration, probe_fps, VideoDecoder};
use presenter::{DecodedFrame, FramePresenter};

/// Проигрыватель видео: clock + декодер + очередь кадров + презентер + аудио.
pub struct VideoPlayer {
    path: PathBuf,
    clock: PlaybackClock,
    decoder: VideoDecoder,
    queue: FrameQueue,
    presenter: FramePresenter,
    awaiting_seek_frame: bool,
    stream_ended: bool,
    displayed_pts: f64,
    audio: Option<AudioPlayer>,
    fps: f64,
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
            queue: FrameQueue::new(),
            presenter: FramePresenter::new(),
            awaiting_seek_frame: true,
            stream_ended: false,
            displayed_pts: f64::NEG_INFINITY,
            audio: None,
            fps,
            looping: false,
            volume: 1.0,
        };

        ctx.request_repaint();
        player
    }

    pub fn update(&mut self, ctx: &Context) -> Option<&TextureHandle> {
        let now = self.clock.now();
        self.queue.ingest(self.decoder.receiver(), now);

        if self.awaiting_seek_frame {
            if let Some(frame) = self.queue.take_first() {
                self.display_frame(ctx, frame);
                self.awaiting_seek_frame = false;
                self.stream_ended = false;
            } else if self.queue.is_disconnected() {
                self.awaiting_seek_frame = false;
            } else {
                ctx.request_repaint();
                return self.presenter.texture();
            }
        } else if !self.clock.is_paused() && !self.stream_ended {
            if self.looping && self.clock.reached_end() {
                self.seek_to(ctx, 0.0);
            } else if let Some(frame) = self.queue.pop_due(now, self.displayed_pts) {
                self.display_frame(ctx, frame);
            }
        }

        if !self.clock.is_paused()
            && self.queue.is_disconnected()
            && self.queue.is_empty()
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
        self.queue.drain(self.decoder.receiver());

        self.decoder = VideoDecoder::start(self.path.clone(), target);
        self.clock.seek(target);
        self.awaiting_seek_frame = true;
        self.stream_ended = false;
        self.displayed_pts = target - 0.001;
        self.queue.reset_disconnect();

        self.sync_audio(target, was_paused);
        ctx.request_repaint();
    }

    fn display_frame(&mut self, ctx: &Context, frame: DecodedFrame) {
        self.displayed_pts = frame.pts;
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
            return;
        }

        let now = self.clock.now();
        if let Some(next_pts) = self.queue.next_pts_after(self.displayed_pts) {
            let wait = (next_pts - now).max(0.0);
            ctx.request_repaint_after(Duration::from_secs_f64(wait.min(0.05)));
        } else {
            ctx.request_repaint_after(Duration::from_secs_f64(1.0 / self.fps.max(1.0)));
        }
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.decoder.stop();
        self.queue.drain(self.decoder.receiver());
        self.audio = None;
        log_info!("VideoPlayer::drop removing {}", self.path.display());
        let _ = std::fs::remove_file(&self.path);
    }
}
