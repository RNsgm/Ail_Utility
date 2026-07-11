use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, TryRecvError};

use super::presenter::DecodedFrame;

const MAX_QUEUED: usize = 64;

/// Очередь кадров между декодером и UI.
///
/// Декодер может опережать clock — поэтому храним несколько кадров и
/// выбираем подходящий по PTS, не блокируя поток декодирования.
pub struct FrameQueue {
    frames: VecDeque<DecodedFrame>,
    disconnected: bool,
}

impl FrameQueue {
    pub fn new() -> Self {
        Self {
            frames: VecDeque::new(),
            disconnected: false,
        }
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }

    pub fn reset_disconnect(&mut self) {
        self.disconnected = false;
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn drain(&mut self, receiver: &Receiver<DecodedFrame>) {
        while receiver.try_recv().is_ok() {}
        self.frames.clear();
        self.disconnected = false;
    }

    pub fn ingest(&mut self, receiver: &Receiver<DecodedFrame>, clock: f64) {
        loop {
            match receiver.try_recv() {
                Ok(frame) => self.frames.push_back(frame),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.disconnected = true;
                    break;
                }
            }
        }

        let cutoff = clock - 1.0;
        while self
            .frames
            .front()
            .is_some_and(|frame| frame.pts < cutoff)
        {
            self.frames.pop_front();
        }

        while self.frames.len() > MAX_QUEUED {
            self.frames.pop_front();
        }
    }

    /// Первый кадр после seek.
    pub fn take_first(&mut self) -> Option<DecodedFrame> {
        self.frames.pop_front()
    }

    /// Самый поздний кадр с PTS ≤ clock, который ещё не показывали.
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
}
