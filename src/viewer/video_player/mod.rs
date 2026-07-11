mod buffer;
mod clock;
mod decoder;
mod presenter;
mod sync;

use std::path::PathBuf;
use std::time::Duration;

use egui::{Context, TextureHandle};
use crate::viewer::audio_player::AudioPlayer;
use crate::{log_info};

use buffer::{SharedFrameStore, READ_AHEAD_SECS};
use decoder::{probe_av_offset, probe_duration, probe_fps, VideoDecoder};
use presenter::{DecodedFrame, FramePresenter};
use sync::AvSync;

/// Проигрыватель видео: A/V sync + буфер на 30с + декодер + презентер.
pub struct VideoPlayer {
    path: PathBuf,
    sync: AvSync,
    decoder: VideoDecoder,
    store: SharedFrameStore,
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
        let av_offset = probe_av_offset(&path);
        log_info!(
            "VideoPlayer: duration={duration:.3}s fps={fps:.3} av_offset={av_offset:.3}s read_ahead={READ_AHEAD_SECS}s"
        );

        let store = SharedFrameStore::new(fps);
        let decoder = VideoDecoder::start(path.clone(), 0.0, store.clone());

        let player = Self {
            path,
            sync: AvSync::new(duration, av_offset),
            decoder,
            store,
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
        let now = self.sync.video_position(self.audio.as_ref());
        let mut seek_to_start = false;

        {
            let mut store = self.store.lock();
            store.trim_before(now);

            if self.awaiting_seek_frame {
                if let Some(frame) = store.take_first() {
                    drop(store);
                    self.display_frame(ctx, frame);
                    self.awaiting_seek_frame = false;
                    self.stream_ended = false;
                } else if store.is_eof() {
                    self.awaiting_seek_frame = false;
                } else {
                    drop(store);
                    ctx.request_repaint();
                    return self.presenter.texture();
                }
            } else if !self.sync.is_paused() && !self.stream_ended {
                if self.looping && self.sync.reached_end() {
                    seek_to_start = true;
                } else {
                    let mut frame_to_show = None;
                    while let Some(frame) = store.pop_due(now, self.displayed_pts) {
                        frame_to_show = Some(frame);
                    }
                    if let Some(frame) = frame_to_show {
                        drop(store);
                        self.display_frame(ctx, frame);
                    }
                }
            }
        }

        if seek_to_start {
            self.seek_to(ctx, 0.0);
            self.schedule_repaint(ctx);
            return self.presenter.texture();
        }

        let ended = {
            let store = self.store.lock();
            store.is_eof()
                && store.is_empty()
                && store.next_pts_after(self.displayed_pts).is_none()
        };
        if !self.sync.is_paused() && ended && !self.stream_ended {
            self.stream_ended = true;
            self.handle_end_of_stream(ctx);
        }

        if !self.awaiting_seek_frame {
            self.ensure_audio();
        }

        self.schedule_repaint(ctx);
        self.presenter.texture()
    }

    pub fn toggle_pause(&mut self) {
        let paused = self.sync.toggle_pause();
        if let Some(audio) = &self.audio {
            if paused {
                audio.pause();
            } else {
                audio.resume();
            }
        }
    }

    pub fn is_playing(&self) -> bool {
        !self.sync.is_paused()
    }

    pub fn is_paused(&self) -> bool {
        self.sync.is_paused()
    }

    pub fn elapsed(&self) -> f64 {
        self.sync.position(self.audio.as_ref())
    }

    pub fn duration(&self) -> f64 {
        self.sync.duration()
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(audio) = &self.audio {
            audio.set_volume(self.volume);
        }
    }

    pub fn seek_relative(&mut self, ctx: &Context, delta: f64) {
        let target = self.sync.clamp_target(self.sync.position(self.audio.as_ref()) + delta);
        self.seek_to(ctx, target);
    }

    pub fn seek_to(&mut self, ctx: &Context, target: f64) {
        let target = self.sync.clamp_target(target);
        let was_paused = self.sync.is_paused();

        log_info!("VideoPlayer::seek_to {target:.3}s file={}", self.path.display());

        self.decoder.stop();
        self.store.lock().clear();

        self.decoder = VideoDecoder::start(self.path.clone(), target, self.store.clone());
        self.sync.seek(target);
        self.sync.pause();
        self.awaiting_seek_frame = true;
        self.stream_ended = false;
        self.displayed_pts = target - 0.001;

        self.restart_audio(target, was_paused);
        ctx.request_repaint();
    }

    fn display_frame(&mut self, ctx: &Context, frame: DecodedFrame) {
        let pts = frame.pts;
        self.displayed_pts = pts;
        self.presenter.show(ctx, frame);
    }

    fn ensure_audio(&mut self) {
        if self.audio.is_some() {
            return;
        }

        let min_buffered = (self.fps.max(1.0) * 0.25) as usize;
        if self.store.lock().len() < min_buffered.max(8) {
            return;
        }

        let start = self.displayed_pts.max(0.0);
        self.sync.pause();

        let Some(audio) = AudioPlayer::start(self.path.clone(), start) else {
            self.sync.begin_at(start);
            return;
        };
        audio.set_volume(self.volume);

        self.audio = Some(audio);
        self.sync.begin_at(start);
    }

    fn restart_audio(&mut self, target: f64, was_paused: bool) {
        if let Some(audio) = &mut self.audio {
            self.sync.pause();
            audio.restart(self.path.clone(), target);
            audio.set_volume(self.volume);
            if was_paused {
                audio.pause();
                self.sync.pause();
            } else {
                audio.resume();
                self.sync.begin_at(target);
            }
        }
    }

    fn handle_end_of_stream(&mut self, ctx: &Context) {
        if self.looping && self.sync.duration() > 0.0 {
            self.seek_to(ctx, 0.0);
            return;
        }
        self.sync.pause();
        if let Some(audio) = &self.audio {
            audio.pause();
        }
    }

    fn schedule_repaint(&self, ctx: &Context) {
        if self.sync.is_paused() {
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        }

        let now = self.sync.video_position(self.audio.as_ref());
        let frame_step = 1.0 / self.fps.max(1.0);
        let store = self.store.lock();
        if let Some(next_pts) = store.next_pts_after(self.displayed_pts) {
            let wait = (next_pts - now).max(0.0).min(frame_step);
            ctx.request_repaint_after(Duration::from_secs_f64(wait.max(0.008)));
        } else if store.is_eof() {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.decoder.stop();
        self.store.lock().clear();
        self.audio = None;
        log_info!("VideoPlayer::drop removing {}", self.path.display());
        let _ = std::fs::remove_file(&self.path);
    }
}
