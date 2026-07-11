use crate::viewer::audio_player::AudioPlayer;

use super::clock::PlaybackClock;

/// Синхронизация аудио и видео: аудио — единственный master clock.
pub struct AvSync {
    clock: PlaybackClock,
    /// video_start - audio_start из контейнера.
    av_offset_secs: f64,
    /// Видео показываем чуть раньше аудио-позиции (компенсация задержки вывода звука).
    video_lead_secs: f64,
}

impl AvSync {
    pub fn new(duration: f64, av_offset_secs: f64) -> Self {
        Self {
            clock: PlaybackClock::new_paused(duration, 0.0),
            av_offset_secs,
            video_lead_secs: 0.0,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.clock.is_paused()
    }

    pub fn duration(&self) -> f64 {
        self.clock.duration()
    }

    pub fn clamp_target(&self, secs: f64) -> f64 {
        self.clock.clamp_target(secs)
    }

    pub fn reached_end(&self) -> bool {
        self.clock.reached_end()
    }

    pub fn toggle_pause(&mut self) -> bool {
        self.clock.toggle()
    }

    pub fn pause(&mut self) {
        self.clock.pause();
    }

    pub fn resume(&mut self) {
        self.clock.resume();
    }

    pub fn seek(&mut self, secs: f64) {
        self.clock.seek(secs);
    }

    /// Позиция для UI-таймера.
    pub fn position(&self, audio: Option<&AudioPlayer>) -> f64 {
        self.master_position(audio)
    }

    /// Позиция для выбора кадра видео.
    pub fn video_position(&self, audio: Option<&AudioPlayer>) -> f64 {
        (self.master_position(audio) - self.video_lead_secs).max(0.0)
    }

    /// Запуск воспроизведения с заданной позиции (после буферизации).
    pub fn begin_at(&mut self, pts: f64) {
        self.clock.seek(pts);
        self.clock.resume();
    }

    fn master_position(&self, audio: Option<&AudioPlayer>) -> f64 {
        if let Some(audio) = audio {
            return audio.playback_time() + self.av_offset_secs;
        }
        self.clock.now()
    }
}
