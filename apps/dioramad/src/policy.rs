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

/// What a monitor's wallpaper does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Play,
    Pause,
    /// Pause and unload (see [`Action::Stop`]).
    Stop,
}

/// What `policy` does with a monitor with this coverage under these
/// conditions: any rule that applies and stops wins, then any that pauses.
pub fn decide(policy: Policy, coverage: Coverage, c: Conditions) -> State {
    let rules = [
        (c.locked, policy.locked),
        (c.on_battery, policy.battery),
        (coverage == Coverage::Fullscreen, policy.fullscreen),
        (coverage == Coverage::Maximized, policy.maximized),
    ];
    let applies = |action: Action| rules.iter().any(|&(on, a)| on && a == action);
    if applies(Action::Stop) {
        State::Stop
    } else if c.user_paused || applies(Action::Pause) {
        State::Pause
    } else {
        State::Play
    }
}

/// What changed when pauses were re-evaluated.
#[derive(Debug, Clone, Copy, Default)]
pub struct Update {
    /// An output resumed.
    pub resumed: bool,
    /// Every output stops: the wallpaper should be unloaded.
    pub stop: bool,
}

/// Re-evaluates which outputs are paused. Logs changes.
pub fn update_pauses(
    platform: &Platform,
    session: &mut Session,
    policy: Policy,
    c: Conditions,
) -> Update {
    let infos: Vec<OutputInfo> = session.outputs.iter().map(|o| o.info.clone()).collect();
    let coverage = platform.coverage(&infos);
    let mut update = Update {
        resumed: false,
        stop: !session.outputs.is_empty(),
    };
    for (out, cov) in session.outputs.iter_mut().zip(coverage) {
        // Display changes can leave the window off its output (see `keep_in_place`).
        update.resumed |= out.keep_in_place();
        let state = decide(policy, cov, c);
        let paused = state != State::Play;
        update.stop &= state == State::Stop;
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
        update.resumed |= out.paused && !paused;
        out.paused = paused;
        out.coverage = cov;
    }
    update
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULTS: Policy = Policy::BALANCED;

    #[test]
    fn defaults() {
        let c = Conditions::default();
        assert_eq!(decide(DEFAULTS, Coverage::Visible, c), State::Play);
        assert_eq!(decide(DEFAULTS, Coverage::Maximized, c), State::Pause);
        assert_eq!(decide(DEFAULTS, Coverage::Fullscreen, c), State::Pause);
        let battery = Conditions {
            on_battery: true,
            ..c
        };
        assert_eq!(decide(DEFAULTS, Coverage::Visible, battery), State::Play);
        let locked = Conditions { locked: true, ..c };
        assert_eq!(decide(DEFAULTS, Coverage::Visible, locked), State::Pause);
        let gaming = Policy::preset("Gaming").unwrap();
        assert_eq!(decide(gaming, Coverage::Visible, battery), State::Pause);
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
        assert_eq!(decide(keep_all, Coverage::Visible, c), State::Pause);
    }

    #[test]
    fn stop_wins_over_pause() {
        let gaming = Policy::preset("gaming").unwrap();
        let c = Conditions::default();
        assert_eq!(decide(gaming, Coverage::Fullscreen, c), State::Stop);
        assert_eq!(decide(gaming, Coverage::Maximized, c), State::Pause);
        let locked = Conditions {
            locked: true,
            user_paused: true,
            ..c
        };
        assert_eq!(decide(gaming, Coverage::Fullscreen, locked), State::Stop);
        assert_eq!(decide(gaming, Coverage::Visible, locked), State::Pause);
    }
}
