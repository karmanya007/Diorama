//! When to pause a monitor's wallpaper (P5/P6/P17).

use clap::ValueEnum;
use lumen_platform::{Coverage, OutputInfo, Platform};

use crate::session::Session;

/// What to do with a monitor's wallpaper in a given situation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Action {
    Keep,
    Pause,
}

#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub fullscreen: Action,
    pub maximized: Action,
    pub battery: Action,
    pub locked: Action,
}

/// Inputs to the pause decision, refreshed from the platform and the user.
#[derive(Debug, Clone, Copy, Default)]
pub struct Conditions {
    pub on_battery: bool,
    pub locked: bool,
    /// `lumen pause` until `lumen resume`.
    pub user_paused: bool,
}

impl Policy {
    pub fn pauses(self, coverage: Coverage, c: Conditions) -> bool {
        let pause = |a: Action| a == Action::Pause;
        c.user_paused
            || (c.locked && pause(self.locked))
            || (c.on_battery && pause(self.battery))
            || match coverage {
                Coverage::Fullscreen => pause(self.fullscreen),
                Coverage::Maximized => pause(self.maximized),
                Coverage::Visible => false,
            }
    }
}

/// Re-evaluates which outputs are paused. Logs changes; returns true if any output resumed.
pub fn update_pauses(
    platform: &Platform,
    session: &mut Session,
    policy: Policy,
    c: Conditions,
) -> bool {
    let infos: Vec<OutputInfo> = session.outputs.iter().map(|o| o.info.clone()).collect();
    let coverage = platform.coverage(&infos);
    let mut resumed = false;
    for (out, cov) in session.outputs.iter_mut().zip(coverage) {
        let paused = policy.pauses(cov, c);
        if paused != out.paused || cov != out.coverage {
            tracing::info!(
                output = %out.info.device,
                coverage = ?cov,
                on_battery = c.on_battery,
                locked = c.locked,
                user_paused = c.user_paused,
                "{}",
                if paused { "paused" } else { "playing" }
            );
        }
        resumed |= out.paused && !paused;
        out.paused = paused;
        out.coverage = cov;
    }
    resumed
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULTS: Policy = Policy {
        fullscreen: Action::Pause,
        maximized: Action::Pause,
        battery: Action::Keep,
        locked: Action::Pause,
    };

    #[test]
    fn defaults() {
        let c = Conditions::default();
        assert!(!DEFAULTS.pauses(Coverage::Visible, c));
        assert!(DEFAULTS.pauses(Coverage::Maximized, c));
        assert!(DEFAULTS.pauses(Coverage::Fullscreen, c));
        let battery = Conditions {
            on_battery: true,
            ..c
        };
        assert!(!DEFAULTS.pauses(Coverage::Visible, battery));
        let locked = Conditions { locked: true, ..c };
        assert!(DEFAULTS.pauses(Coverage::Visible, locked));
    }

    #[test]
    fn user_pause_overrides_keep() {
        let keep_all = Policy {
            fullscreen: Action::Keep,
            maximized: Action::Keep,
            battery: Action::Keep,
            locked: Action::Keep,
        };
        let c = Conditions {
            user_paused: true,
            ..Conditions::default()
        };
        assert!(keep_all.pauses(Coverage::Visible, c));
    }
}
