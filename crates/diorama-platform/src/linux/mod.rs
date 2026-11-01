//! Linux backend (Phase 7). Wayland compositors with `wlr-layer-shell`
//! (Sway, Hyprland, KDE, niri, COSMIC…): each wallpaper is a layer surface
//! on the background layer of its output. X11 and GNOME come later.

mod wayland;

pub use wayland::{Platform, Waker, WallpaperWindow};
