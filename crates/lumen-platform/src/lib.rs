//! OS integration layer (L3). The only crate allowed to contain OS-specific
//! code (`scripts/check-layers.sh` enforces this).
//!
//! Exposes outputs (monitors) with stable identities, wallpaper surfaces that
//! sit behind the desktop icons, and desktop events. It hands out raw window
//! handles and never knows about the renderer.

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

mod coverage;

pub use coverage::{Coverage, WindowRect, classify};

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{
    MediaWatcher, Platform, Waker, WallpaperWindow, autostart, cache_dir, config_dir, day_fraction,
    detach_console, set_autostart, set_os_wallpaper,
};

#[cfg(not(windows))]
mod unsupported;

#[cfg(not(windows))]
pub use unsupported::{
    MediaWatcher, Platform, Waker, WallpaperWindow, autostart, cache_dir, config_dir, day_fraction,
    detach_console, set_autostart, set_os_wallpaper,
};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("platform not supported yet")]
    Unsupported,
    #[error("desktop window not found: {0}")]
    DesktopNotFound(&'static str),
    #[error("{call} failed: {message}")]
    Os { call: &'static str, message: String },
}

/// Identity that survives reboots and Windows renumbering displays (P8).
/// On Windows: the monitor device interface path from `QueryDisplayConfig`,
/// which encodes EDID manufacturer/product and the connector instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputId(pub String);

/// What the system says is playing (any player using the OS media controls).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaInfo {
    /// Media integration is available on this system.
    pub enabled: bool,
    /// A player session exists.
    pub active: bool,
    pub title: String,
    pub artist: String,
    pub subtitle: String,
    pub album_title: String,
    pub album_artist: String,
    /// Comma-separated.
    pub genres: String,
    pub playback: MediaPlayback,
    /// Seconds.
    pub position: f64,
    pub duration: f64,
    /// Encoded image (PNG/JPEG…) as the player provides it.
    pub thumbnail: Option<std::sync::Arc<Vec<u8>>>,
    /// Changes when the track (and so the thumbnail) changes.
    pub thumbnail_key: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MediaPlayback {
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// Mouse state on the desktop, polled (wallpapers get no mouse messages).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DesktopPointer {
    /// The cursor is over the desktop, not over an application window.
    pub over_desktop: bool,
    /// Left button held while over the desktop.
    pub left_down: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputInfo {
    pub id: OutputId,
    /// Human-readable monitor name (EDID friendly name), may be empty.
    pub name: String,
    /// Three-letter EDID manufacturer code, e.g. `GSM`.
    pub manufacturer: String,
    pub product_code: u16,
    /// OS device name, e.g. `\\.\DISPLAY1` (volatile; for logs only).
    pub device: String,
    /// Position of the top-left corner in virtual-desktop physical pixels.
    pub x: i32,
    pub y: i32,
    /// Size in physical pixels.
    pub width: u32,
    pub height: u32,
    /// Refresh rate in millihertz (e.g. 59_940), 0 if unknown.
    pub refresh_mhz: u32,
    pub primary: bool,
}

/// Something that happened to the desktop since the last poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopEvent {
    /// Explorer restarted or our windows were destroyed: recreate all wallpaper surfaces.
    DesktopReset,
    /// Monitors were added/removed/moved/rotated or resolution changed.
    OutputsChanged,
    /// The user session was locked (lock screen shown).
    SessionLocked,
    SessionUnlocked,
}

impl DesktopEvent {
    /// Whether wallpaper windows/surfaces must be recreated.
    pub fn needs_rebuild(self) -> bool {
        matches!(self, Self::DesktopReset | Self::OutputsChanged)
    }
}

/// Raw handles for creating a GPU surface on a wallpaper window.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceHandles {
    pub window: RawWindowHandle,
    pub display: RawDisplayHandle,
}
