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
    MediaWatcher, Platform, ProcessMonitor, SystemMonitor, Waker, WallpaperWindow, autostart,
    cache_dir, config_dir, day_fraction, detach_console, no_console, set_autostart,
    set_os_wallpaper, steam_dir, ui_language,
};

#[cfg(not(windows))]
mod unsupported;

#[cfg(not(windows))]
pub use unsupported::{
    MediaWatcher, Platform, ProcessMonitor, SystemMonitor, Waker, WallpaperWindow, autostart,
    cache_dir, config_dir, day_fraction, detach_console, no_console, set_autostart,
    set_os_wallpaper, steam_dir, ui_language,
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

/// Load of a group of processes (see `ProcessMonitor`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ProcessLoad {
    /// Percent of all processors.
    pub cpu: f32,
    /// Percent: their busiest GPU engine.
    pub gpu: f32,
    /// Dedicated video memory and working sets, in bytes.
    pub vram: u64,
    pub ram: u64,
    pub processes: u32,
}

/// One reading of the machine's load (for wallpapers that show it).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemMetrics {
    /// Percent of all processors, and per processor.
    pub cpu: f32,
    pub cpu_per_core: Vec<f32>,
    /// Physical memory in bytes.
    pub memory_used: u64,
    pub memory_total: u64,
    /// Percent: the busiest engine of the main GPU (as Task Manager shows it).
    pub gpu: Option<f32>,
    /// The main GPU's dedicated memory in bytes.
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    /// Fixed drives.
    pub disks: Vec<DiskUsage>,
    /// Network bytes per second, all physical interfaces.
    pub upload: f64,
    pub download: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiskUsage {
    /// Upper case, e.g. `C`.
    pub letter: char,
    /// Bytes.
    pub used: u64,
    pub total: u64,
}

/// Mouse state on the desktop, polled (wallpapers get no mouse messages).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DesktopPointer {
    /// The cursor is over the desktop, not over an application window.
    pub over_desktop: bool,
    /// Left button held while over the desktop.
    pub left_down: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
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
    /// Chosen from the notification-area icon.
    Tray(TrayCommand),
}

/// A choice from the notification-area icon (its menu, or a click on it:
/// `OpenUi`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    OpenUi,
    Next,
    Pause,
    Resume,
    Quit,
}

/// What the notification-area icon's menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrayState {
    /// Paused by the user: the menu offers Resume instead of Pause.
    pub paused: bool,
    /// A playlist is running (Next wallpaper is enabled).
    pub can_next: bool,
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
