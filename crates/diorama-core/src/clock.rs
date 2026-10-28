//! Injectable time source.
//!
//! Everything that animates reads time through [`Clock`] instead of calling
//! `Instant::now()` directly. This keeps headless rendering, video export and
//! golden-image tests deterministic: a test can render the exact `g_Time` that
//! a RenderDoc capture of Wallpaper Engine recorded.

use std::time::{Duration, Instant};

/// Elapsed time since the wallpaper started.
pub trait Clock: Send + Sync {
    fn elapsed(&self) -> Duration;
}

/// Real wall-clock time.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Clock that only moves when told to. Used for headless output and tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct ManualClock {
    now: Duration,
}

impl ManualClock {
    pub fn at(now: Duration) -> Self {
        Self { now }
    }

    pub fn set(&mut self, now: Duration) {
        self.now = now;
    }

    pub fn advance(&mut self, dt: Duration) {
        self.now = self.now.saturating_add(dt);
    }
}

impl Clock for ManualClock {
    fn elapsed(&self) -> Duration {
        self.now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_advances_only_when_told() {
        let mut clock = ManualClock::at(Duration::from_secs(2));
        assert_eq!(clock.elapsed(), Duration::from_secs(2));
        clock.advance(Duration::from_millis(500));
        assert_eq!(clock.elapsed(), Duration::from_millis(2500));
        clock.set(Duration::ZERO);
        assert_eq!(clock.elapsed(), Duration::ZERO);
    }
}
