use std::sync::mpsc::{Receiver, TryRecvError};

use super::presenter::DecodedFrame;

/// Хранит последний декодированный кадр из очереди.
pub struct FrameBuffer {
    latest: Option<DecodedFrame>,
    disconnected: bool,
}

impl FrameBuffer {
    pub fn new() -> Self {
        Self {
            latest: None,
            disconnected: false,
        }
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }

    pub fn reset_disconnect(&mut self) {
        self.disconnected = false;
    }

    pub fn drain(&mut self, receiver: &Receiver<DecodedFrame>) {
        while receiver.try_recv().is_ok() {}
        self.latest = None;
        self.disconnected = false;
    }

    pub fn ingest(&mut self, receiver: &Receiver<DecodedFrame>) {
        loop {
            match receiver.try_recv() {
                Ok(frame) => self.latest = Some(frame),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.disconnected = true;
                    break;
                }
            }
        }
    }

    pub fn take_latest(&mut self) -> Option<DecodedFrame> {
        self.latest.take()
    }

    pub fn has_frame(&self) -> bool {
        self.latest.is_some()
    }
}
