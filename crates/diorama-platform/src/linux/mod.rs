//! Linux backend (Phase 7). Wayland compositors with `wlr-layer-shell`
//! (Sway, Hyprland, KDE, niri, COSMIC…): each wallpaper is a layer surface
//! on the background layer of its output; battery from the kernel, lock
//! from logind, media information from MPRIS, the tray icon through
//! StatusNotifierItem. X11 and GNOME come later.

mod media;
mod metrics;
mod session;
mod system;
mod tray;
mod wayland;

pub use media::MediaWatcher;
pub use metrics::{ProcessMonitor, SystemMonitor};
pub use system::{autostart, day_fraction, set_autostart, steam_dir};
pub use wayland::{Platform, Waker, WallpaperWindow};
