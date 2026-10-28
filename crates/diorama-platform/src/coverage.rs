//! Is anything covering each output? OS-neutral classification over window
//! rectangles; backends supply the window list (pain points P5/P6).

use crate::OutputInfo;

/// How much of an output is hidden by other applications. Ordered: a larger
/// value hides more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Coverage {
    /// The wallpaper is (at least partly) visible.
    #[default]
    Visible,
    /// A maximized window fills the work area; only the area behind the taskbar shows.
    Maximized,
    /// A window covers the whole output (games, video players, presentations).
    Fullscreen,
}

/// A top-level application window, in virtual-desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub maximized: bool,
}

/// Coverage of each output (same order as `outputs`).
pub fn classify(outputs: &[OutputInfo], windows: &[WindowRect]) -> Vec<Coverage> {
    outputs
        .iter()
        .map(|o| {
            let (ol, ot) = (o.x, o.y);
            let (or, ob) = (o.x + o.width as i32, o.y + o.height as i32);
            windows
                .iter()
                .map(|w| {
                    if w.left <= ol && w.top <= ot && w.right >= or && w.bottom >= ob {
                        Coverage::Fullscreen
                    } else {
                        let (cx, cy) = ((w.left + w.right) / 2, (w.top + w.bottom) / 2);
                        let centred_here = cx >= ol && cx < or && cy >= ot && cy < ob;
                        if w.maximized && centred_here {
                            Coverage::Maximized
                        } else {
                            Coverage::Visible
                        }
                    }
                })
                .max()
                .unwrap_or_default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputId;

    fn output(x: i32, y: i32, width: u32, height: u32) -> OutputInfo {
        OutputInfo {
            id: OutputId(format!("{x},{y}")),
            name: String::new(),
            manufacturer: String::new(),
            product_code: 0,
            device: String::new(),
            x,
            y,
            width,
            height,
            refresh_mhz: 0,
            primary: false,
        }
    }

    fn window(left: i32, top: i32, right: i32, bottom: i32, maximized: bool) -> WindowRect {
        WindowRect {
            left,
            top,
            right,
            bottom,
            maximized,
        }
    }

    // The user's real layout.
    fn outputs() -> Vec<OutputInfo> {
        vec![output(0, 0, 1920, 1080), output(-1080, -546, 1080, 1920)]
    }

    #[test]
    fn nothing_open_is_visible() {
        assert_eq!(
            classify(&outputs(), &[]),
            [Coverage::Visible, Coverage::Visible]
        );
    }

    #[test]
    fn fullscreen_is_per_output() {
        let game = window(0, 0, 1920, 1080, false);
        assert_eq!(
            classify(&outputs(), &[game]),
            [Coverage::Fullscreen, Coverage::Visible]
        );
    }

    #[test]
    fn maximized_counts_on_its_own_monitor_only() {
        // Maximized on the portrait monitor: work area excludes the taskbar.
        let editor = window(-1080, -546, 0, 1326, true);
        assert_eq!(
            classify(&outputs(), &[editor]),
            [Coverage::Visible, Coverage::Maximized]
        );
    }

    #[test]
    fn restored_window_does_not_pause() {
        let small = window(100, 100, 900, 700, false);
        assert_eq!(
            classify(&outputs(), &[small]),
            [Coverage::Visible, Coverage::Visible]
        );
    }

    #[test]
    fn strongest_coverage_wins() {
        let max = window(0, 0, 1920, 1040, true);
        let full = window(0, 0, 1920, 1080, false);
        assert_eq!(
            classify(&outputs()[..1], &[max, full]),
            [Coverage::Fullscreen]
        );
    }

    #[test]
    fn spanning_window_covers_both() {
        let span = window(-1080, -546, 1920, 1374, false);
        assert_eq!(
            classify(&outputs(), &[span]),
            [Coverage::Fullscreen, Coverage::Fullscreen]
        );
    }
}
