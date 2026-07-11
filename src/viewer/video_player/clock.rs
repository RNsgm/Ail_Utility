use std::time::Instant;

/// Единый источник времени воспроизведения (master clock).
#[derive(Debug, Clone)]
pub struct PlaybackClock {
    anchor_secs: f64,
    anchor_instant: Instant,
    paused: bool,
    duration: f64,
}

impl PlaybackClock {
    pub fn new(duration: f64) -> Self {
        Self {
            anchor_secs: 0.0,
            anchor_instant: Instant::now(),
            paused: false,
            duration: duration.max(0.0),
        }
    }

    pub fn new_paused(duration: f64, position: f64) -> Self {
        Self {
            anchor_secs: position.max(0.0),
            anchor_instant: Instant::now(),
            paused: true,
            duration: duration.max(0.0),
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn now(&self) -> f64 {
        if self.paused {
            self.anchor_secs
        } else {
            self.anchor_secs + self.anchor_instant.elapsed().as_secs_f64()
        }
    }

    pub fn pause(&mut self) {
        if self.paused {
            return;
        }
        self.anchor_secs = self.now();
        self.paused = true;
    }

    pub fn resume(&mut self) {
        if !self.paused {
            return;
        }
        self.anchor_instant = Instant::now();
        self.paused = false;
    }

    pub fn toggle(&mut self) -> bool {
        if self.paused {
            self.resume();
        } else {
            self.pause();
        }
        self.paused
    }

    pub fn seek(&mut self, secs: f64) {
        self.anchor_secs = secs.clamp(0.0, self.duration.max(0.0));
        self.anchor_instant = Instant::now();
    }

    pub fn clamp_target(&self, secs: f64) -> f64 {
        secs.clamp(0.0, self.duration.max(0.0))
    }

    pub fn reached_end(&self) -> bool {
        self.duration > 0.0 && self.now() >= self.duration
    }
}
