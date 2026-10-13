//! Placeholder until the Linux (Phase 7) and macOS (Phase 8) backends exist.
//! Keeps the workspace building and testable on those platforms.

use std::time::Duration;

use crate::{DesktopEvent, Error, OutputInfo, Result, SurfaceHandles};

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
