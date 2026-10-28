//! When to pause a monitor's wallpaper (P5/P6/P17).

use diorama_ipc::{Action, Policy};
use diorama_platform::{Coverage, OutputInfo, Platform};

use crate::session::Session;

/// Inputs to the pause decision, refreshed from the platform and the user.
#[derive(Debug, Clone, Copy, Default)]
pub struct Conditions {
    pub on_battery: bool,
    pub locked: bool,
    /// `diorama pause` until `diorama resume`.
    pub user_paused: bool,
}

/// Whether `policy` pauses a monitor with this coverage under these conditions.
pub fn pauses(policy: Policy, coverage: Coverage, c: Conditions) -> bool {
    let pause = |a: Action| a == Action::Pause;
    c.user_paused
        || (c.locked && pause(policy.locked))
        || (c.on_battery && pause(policy.battery))
        || match coverage {
            Coverage::Fullscreen => pause(policy.fullscreen),
            Coverage::Maximized => pause(policy.maximized),
            Coverage::Visible => false,
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
        // Display changes can leave the window off its output (see `keep_in_place`).
        resumed |= out.keep_in_place();
        let paused = pauses(policy, cov, c);
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

    const DEFAULTS: Policy = Policy::BALANCED;

    #[test]
    fn defaults() {
        let c = Conditions::default();
        assert!(!pauses(DEFAULTS, Coverage::Visible, c));
        assert!(pauses(DEFAULTS, Coverage::Maximized, c));
        assert!(pauses(DEFAULTS, Coverage::Fullscreen, c));
        let battery = Conditions {
            on_battery: true,
            ..c
        };
        assert!(!pauses(DEFAULTS, Coverage::Visible, battery));
        let locked = Conditions { locked: true, ..c };
        assert!(pauses(DEFAULTS, Coverage::Visible, locked));
        let gaming = Policy::preset("Gaming").unwrap();
        assert!(pauses(gaming, Coverage::Visible, battery));
        assert_eq!(gaming.preset_name(), Some("gaming"));
        assert_eq!(
            Policy {
                fps: 31,
                ..DEFAULTS
            }
            .preset_name(),
            None
        );
    }

    #[test]
    fn user_pause_overrides_keep() {
        let keep_all = Policy {
            fullscreen: Action::Keep,
            maximized: Action::Keep,
            battery: Action::Keep,
            locked: Action::Keep,
            fps: 30,
        };
        let c = Conditions {
            user_paused: true,
            ..Conditions::default()
        };
        assert!(pauses(keep_all, Coverage::Visible, c));
    }
}
