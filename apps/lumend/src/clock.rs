//! Maps video frame timestamps onto wall-clock time.

use std::time::{Duration, Instant};

use lumen_media::{Nv12Frame, VideoPlayer};

/// Playback clock; can be frozen while every output is paused.
#[derive(Default)]
pub struct VideoClock {
    start: Option<Instant>,
    paused_at: Option<Instant>,
    /// Decoded but not yet due.
    pending: Option<Nv12Frame>,
    /// Last frame shown (re-uploaded after a rebuild or resume).
    pub current: Option<Nv12Frame>,
}

impl VideoClock {
    fn now_pts(&mut self) -> Duration {
        self.start.get_or_insert_with(Instant::now).elapsed()
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Freezes/unfreezes playback time so a resumed video continues where it stopped.
    pub fn set_paused(&mut self, paused: bool) {
        match (paused, self.paused_at) {
            (true, None) => self.paused_at = Some(Instant::now()),
            (false, Some(at)) => {
                if let Some(start) = self.start.as_mut() {
                    *start += at.elapsed();
                }
                self.paused_at = None;
            }
            _ => {}
        }
    }

    pub fn time_until_next(&mut self, player: &VideoPlayer) -> Duration {
        if self.pending.is_none() {
            self.pending = player.try_next();
        }
        let now = self.now_pts();
        match &self.pending {
            Some(f) => f.pts.saturating_sub(now),
            // Decoder is behind; check again shortly.
            None => Duration::from_millis(5),
        }
    }

    /// The newest frame whose time has come, dropping late ones.
    pub fn due_frame(&mut self, player: &VideoPlayer) -> Option<&Nv12Frame> {
        let now = self.now_pts();
        let mut due = None;
        loop {
            if self.pending.is_none() {
                self.pending = player.try_next();
            }
            match self.pending.take() {
                Some(f) if f.pts <= now => due = Some(f),
                other => {
                    self.pending = other;
                    break;
                }
            }
        }
        if let Some(f) = due {
            self.current = Some(f);
            return self.current.as_ref();
        }
        None
    }
}
