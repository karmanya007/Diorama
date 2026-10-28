//! Placeholder until the Linux (Phase 7) and macOS (Phase 8) backends exist.
//! Keeps the workspace building and testable on those platforms.

use std::time::Duration;

use crate::{Coverage, DesktopEvent, Error, OutputInfo, Result, SurfaceHandles};

#[derive(Debug)]
pub struct Platform;

impl Platform {
    pub fn new() -> Result<Self> {
        Err(Error::Unsupported)
    }

    pub fn outputs(&self) -> Result<Vec<OutputInfo>> {
        Err(Error::Unsupported)
    }

    pub fn create_wallpaper(&self, _output: &OutputInfo) -> Result<WallpaperWindow> {
        Err(Error::Unsupported)
    }

    pub fn create_wallpaper_below(&self, _output: &OutputInfo) -> Result<WallpaperWindow> {
        Err(Error::Unsupported)
    }

    pub fn create_offscreen(&self, _width: u32, _height: u32) -> Result<WallpaperWindow> {
        Err(Error::Unsupported)
    }

    pub fn poll_events(&self, _timeout: Duration) -> Vec<DesktopEvent> {
        Vec::new()
    }

    pub fn coverage(&self, outputs: &[OutputInfo]) -> Vec<Coverage> {
        vec![Coverage::Visible; outputs.len()]
    }

    pub fn on_battery(&self) -> bool {
        false
    }

    pub fn cursor_position(&self) -> Option<(i32, i32)> {
        None
    }

    pub fn desktop_pointer(&self) -> crate::DesktopPointer {
        crate::DesktopPointer::default()
    }

    pub fn lower_priority(&self) {}

    pub fn tray_icon_size(&self) -> u32 {
        16
    }

    pub fn show_tray(&self, _rgba: &[u8], _size: u32, _state: crate::TrayState) -> Result<()> {
        Err(Error::Unsupported)
    }

    pub fn set_tray_state(&self, _state: crate::TrayState) {}

    pub fn waker(&self) -> Waker {
        Waker
    }
}

/// `$XDG_CONFIG_HOME/diorama` or `~/.config/diorama`.
pub fn config_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|d| d.join("diorama"))
}

/// The settings folder before the project was renamed Diorama (0.1.0).
pub fn legacy_config_dir() -> Option<std::path::PathBuf> {
    config_dir().and_then(|d| d.parent().map(|p| p.join("lumen")))
}

pub fn set_autostart(_command: Option<&str>) -> Result<()> {
    Err(Error::Unsupported)
}

pub fn autostart() -> Result<Option<String>> {
    Err(Error::Unsupported)
}

pub fn detach_console() {}

pub fn set_os_wallpaper(_image: &std::path::Path, _span: bool) -> Result<()> {
    Err(Error::Unsupported)
}

#[derive(Debug, Clone)]
pub struct Waker;

impl Waker {
    pub fn wake(&self) {}
}

/// Uninhabited: no wallpaper windows exist on unsupported platforms.
#[derive(Debug)]
pub enum WallpaperWindow {}

impl WallpaperWindow {
    pub fn raise(&self) {
        match *self {}
    }

    pub fn handles(&self) -> SurfaceHandles {
        match *self {}
    }

    pub fn size(&self) -> (u32, u32) {
        match *self {}
    }

    pub fn keep_in_place(&self) -> bool {
        match *self {}
    }
}

/// `$XDG_CACHE_HOME/diorama` or `~/.cache/diorama`.
pub fn cache_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .map(|d| d.join("diorama"))
}

/// Starts `cmd` without a console window (nothing to do here).
pub fn no_console(_cmd: &mut std::process::Command) {}

/// Steam's install folder: `~/.local/share/Steam` or `~/.steam/steam`.
pub fn steam_dir() -> Option<std::path::PathBuf> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    [home.join(".local/share/Steam"), home.join(".steam/steam")]
        .into_iter()
        .find(|p| p.is_dir())
}

/// The user's language from `LC_ALL` / `LC_MESSAGES` / `LANG` (`de_DE.UTF-8`).
pub fn ui_language() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
}

/// Fraction of the day elapsed (UTC until the Linux backend reads the time zone).
pub fn day_fraction() -> f32 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    (secs % 86_400) as f32 / 86_400.0
}

/// No media session on this platform yet: reports media integration as off.
#[derive(Debug)]
pub struct MediaWatcher;

impl MediaWatcher {
    pub fn start() -> Self {
        Self
    }

    pub fn latest(&self) -> crate::MediaInfo {
        crate::MediaInfo::default()
    }
}

/// Uninhabited until a non-Windows backend exists.
#[derive(Debug)]
pub enum ProcessMonitor {}

impl ProcessMonitor {
    pub fn new() -> Result<Self> {
        Err(crate::Error::Unsupported)
    }
    pub fn sample(&mut self, _processes: &[u32], _trees: &[u32]) -> crate::ProcessLoad {
        match *self {}
    }
}

/// Uninhabited until a non-Windows backend exists.
#[derive(Debug)]
pub enum SystemMonitor {}

impl SystemMonitor {
    pub fn new() -> Result<Self> {
        Err(crate::Error::Unsupported)
    }
    pub fn sample(&mut self) -> crate::SystemMetrics {
        match *self {}
    }
}
