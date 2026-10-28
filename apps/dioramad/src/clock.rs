//! Maps video frame timestamps onto wall-clock time.

use std::time::{Duration, Instant};

use diorama_media::{Nv12Frame, VideoPlayer};

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

/// Scene time (`g_Time`) at a fixed frame rate; frozen while every output is paused.
#[derive(Debug)]
pub struct SceneClock {
    start: Instant,
    paused_at: Option<Instant>,
    interval: Duration,
    next_frame: Instant,
}

impl SceneClock {
    pub fn new(fps: u32) -> Self {
        let now = Instant::now();
        Self {
            start: now,
            paused_at: None,
            interval: Duration::from_secs(1) / fps.clamp(1, 1000),
            next_frame: now,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// A new frame rate cap; scene time carries on.
    pub fn set_fps(&mut self, fps: u32) {
        self.interval = Duration::from_secs(1) / fps.clamp(1, 1000);
        self.next_frame = Instant::now();
    }

    pub fn set_paused(&mut self, paused: bool) {
        match (paused, self.paused_at) {
            (true, None) => self.paused_at = Some(Instant::now()),
            (false, Some(at)) => {
                self.start += at.elapsed();
                self.paused_at = None;
                self.next_frame = Instant::now();
            }
            _ => {}
        }
    }

    /// Seconds of unpaused time since start.
    pub fn time(&self) -> f32 {
        let end = self.paused_at.unwrap_or_else(Instant::now);
        end.saturating_duration_since(self.start).as_secs_f32()
    }

    pub fn time_until_next(&self) -> Duration {
        self.next_frame.saturating_duration_since(Instant::now())
    }

    /// True when a frame is due; schedules the next one. Never queues a
    /// burst of catch-up frames after a stall.
    pub fn frame_due(&mut self) -> bool {
        let now = Instant::now();
        if self.is_paused() || now < self.next_frame {
            return false;
        }
        self.next_frame += self.interval;
        if self.next_frame < now {
            self.next_frame = now + self.interval;
        }
        true
    }
}

#[cfg(test)]
mod scene_tests {
    use super::*;

    #[test]
    fn paused_time_does_not_advance() {
        let mut c = SceneClock::new(30);
        assert!(c.frame_due());
        assert!(!c.frame_due(), "next frame is a full interval away");
        c.set_paused(true);
        let t = c.time();
        std::thread::sleep(Duration::from_millis(20));
        assert!((c.time() - t).abs() < 1e-6);
        assert!(!c.frame_due());
        c.set_paused(false);
        assert!(c.frame_due(), "resume draws immediately");
        assert!(c.time() < t + 0.015);
    }
}
