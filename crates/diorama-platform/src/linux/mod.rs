//! Linux backend (Phase 7). Wayland compositors with `wlr-layer-shell`
//! (Sway, Hyprland, KDE, niri, COSMIC…): each wallpaper is a layer surface
//! on the background layer of its output; battery from the kernel, lock
//! from logind, media information from MPRIS. X11 and GNOME come later.

mod media;
mod session;
mod system;
mod wayland;

pub use media::MediaWatcher;
pub use system::{autostart, day_fraction, set_autostart, steam_dir};
pub use wayland::{Platform, Waker, WallpaperWindow};
