use std::sync::mpsc::{Receiver, TryRecvError};

use super::presenter::DecodedFrame;

/// Буфер кадров между декодером и UI: выбирает кадр по master clock.
pub struct FrameBuffer {
    pending: Option<DecodedFrame>,
    disconnected: bool,
}

impl FrameBuffer {
    pub fn new() -> Self {
        Self {
            pending: None,
            disconnected: false,
        }
    }

    pub fn clear(&mut self) {
        self.pending = None;
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }

    pub fn reset_disconnect(&mut self) {
        self.disconnected = false;
    }

    pub fn drain(&mut self, receiver: &Receiver<DecodedFrame>) {
        while receiver.try_recv().is_ok() {}
        self.pending = None;
        self.disconnected = false;
    }

    /// После seek: взять самый поздний кадр из очереди.
    pub fn take_after_seek(&mut self, receiver: &Receiver<DecodedFrame>) -> Option<DecodedFrame> {
        let mut best = None;
        loop {
            match receiver.try_recv() {
                Ok(frame) => {
                    let replace = best
                        .as_ref()
                        .map(|prev: &DecodedFrame| frame.pts >= prev.pts)
                        .unwrap_or(true);
                    if replace {
                        best = Some(frame);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.disconnected = true;
                    break;
                }
            }
        }
        best
    }

    /// Взять самый свежий кадр с pts ≤ clock.
    pub fn take_due(&mut self, receiver: &Receiver<DecodedFrame>, clock: f64) -> Option<DecodedFrame> {
        let mut latest_due = None;

        if let Some(pending) = self.pending.take() {
            if pending.pts <= clock + 0.001 {
                latest_due = Some(pending);
            } else {
                self.pending = Some(pending);
            }
        }

        loop {
            match receiver.try_recv() {
                Ok(frame) => {
                    if frame.pts <= clock + 0.001 {
                        latest_due = Some(frame);
                    } else if self.pending.is_none() {
                        self.pending = Some(frame);
                        break;
                    } else {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.disconnected = true;
                    break;
                }
            }
        }

        latest_due
    }

    pub fn next_pts(&self) -> Option<f64> {
        self.pending.as_ref().map(|f| f.pts)
    }
}
