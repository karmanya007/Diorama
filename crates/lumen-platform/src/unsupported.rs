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

    pub fn lower_priority(&self) {}

    pub fn waker(&self) -> Waker {
        Waker
    }
}

/// `$XDG_CONFIG_HOME/lumen` or `~/.config/lumen`.
pub fn config_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|d| d.join("lumen"))
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
    pub fn handles(&self) -> SurfaceHandles {
        match *self {}
    }

    pub fn size(&self) -> (u32, u32) {
        match *self {}
    }
}

/// `$XDG_CACHE_HOME/lumen` or `~/.cache/lumen`.
pub fn cache_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .map(|d| d.join("lumen"))
}

/// Fraction of the day elapsed (UTC until the Linux backend reads the time zone).
pub fn day_fraction() -> f32 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    (secs % 86_400) as f32 / 86_400.0
}
