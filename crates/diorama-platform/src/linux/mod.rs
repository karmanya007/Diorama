//! Linux backend (Phase 7). Wayland compositors with `wlr-layer-shell`
//! (Sway, Hyprland, KDE, niri, COSMIC…): each wallpaper is a layer surface
//! on the background layer of its output; battery from the kernel, lock
//! from logind. X11 and GNOME come later.

mod session;
mod wayland;

pub use wayland::{Platform, Waker, WallpaperWindow};
