use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::presenter::DecodedFrame;

pub const READ_AHEAD_SECS: f64 = 30.0;

/// Общий буфер кадров: декодер пишет, UI читает.
#[derive(Clone)]
pub struct SharedFrameStore {
    inner: Arc<Mutex<FrameStore>>,
}

pub struct FrameStore {
    frames: VecDeque<DecodedFrame>,
    eof: bool,
    max_frames: usize,
}

impl SharedFrameStore {
    pub fn new(fps: f64) -> Self {
        let max_frames = (READ_AHEAD_SECS * fps.max(1.0)).ceil() as usize + 2;
        Self {
            inner: Arc::new(Mutex::new(FrameStore {
                frames: VecDeque::new(),
                eof: false,
                max_frames: max_frames.max(30),
            })),
        }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, FrameStore> {
        self.inner.lock().expect("frame store lock poisoned")
    }
}

impl FrameStore {
    pub fn clear(&mut self) {
        self.frames.clear();
        self.eof = false;
    }

    pub fn set_eof(&mut self) {
        self.eof = true;
    }

    pub fn is_eof(&self) -> bool {
        self.eof
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn can_accept(&self) -> bool {
        self.frames.len() < self.max_frames
    }

    pub fn try_push(&mut self, frame: DecodedFrame) -> Result<(), DecodedFrame> {
        if !self.can_accept() {
            return Err(frame);
        }
        self.frames.push_back(frame);
        Ok(())
    }

    pub fn trim_before(&mut self, clock: f64) {
        let cutoff = clock - 1.0;
        while self
            .frames
            .front()
            .is_some_and(|frame| frame.pts < cutoff)
        {
            self.frames.pop_front();
        }
    }

    pub fn take_first(&mut self) -> Option<DecodedFrame> {
        self.frames.pop_front()
    }

    pub fn pop_due(&mut self, clock: f64, displayed_pts: f64) -> Option<DecodedFrame> {
        let mut best_idx = None;
        let mut best_pts = displayed_pts;

        for (idx, frame) in self.frames.iter().enumerate() {
            if frame.pts <= clock + 0.001 && frame.pts > best_pts + 0.0001 {
                best_pts = frame.pts;
                best_idx = Some(idx);
            }
        }

        let idx = best_idx?;
        let frame = self.frames.remove(idx)?;
        self.frames.retain(|queued| queued.pts >= frame.pts - 0.0001);
        Some(frame)
    }

    pub fn next_pts_after(&self, displayed_pts: f64) -> Option<f64> {
        self.frames
            .iter()
            .filter(|frame| frame.pts > displayed_pts + 0.0001)
            .map(|frame| frame.pts)
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    }

    pub fn last_pts(&self) -> Option<f64> {
        self.frames.back().map(|frame| frame.pts)
    }
}
