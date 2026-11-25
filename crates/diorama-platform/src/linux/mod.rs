//! Linux backend (Phase 7). The display side is Wayland where the
//! compositor has `wlr-layer-shell` (Sway, Hyprland, KDE, niri, COSMIC…):
//! each wallpaper is a layer surface on the background layer of its
//! output; else X11, with EWMH desktop windows. Shared by both: battery from
//! the kernel, lock from logind, media information from MPRIS, the tray icon
//! through StatusNotifierItem. GNOME comes later.
//!
//! Single-threaded like the Windows backend: create, use and drop the
//! [`Platform`] on the thread that calls [`Platform::poll_events`].

mod dropper;
mod media;
mod metrics;
mod session;
mod system;
mod tray;
mod wayland;
mod x11;

use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use dropper::pick_color;
pub use media::MediaWatcher;
pub use metrics::{ProcessMonitor, SystemMonitor};
pub use system::{autostart, day_fraction, default_font, restart, set_autostart, steam_dir};

use crate::{
    Coverage, DesktopEvent, DesktopPointer, Error, HotkeyModifiers, OutputInfo, Result,
    SurfaceHandles,
};

/// Nice value of a lowered process (0 is normal, 19 the lowest).
const NICE: i32 = 5;

/// The desktop session Diorama draws into.
#[derive(Debug)]
enum Display {
    Wayland(Box<wayland::Display>),
    X11(Box<x11::Display>),
}

/// Calls `$call` on whichever display is in use.
macro_rules! on_display {
    ($display:expr, $d:ident => $call:expr) => {
        match $display {
            Display::Wayland($d) => $call,
            Display::X11($d) => $call,
        }
    };
}

/// The desktop session and what Diorama keeps open on it.
#[derive(Debug)]
pub struct Platform {
    display: Display,
    /// Read end of the waker's socket pair (polled with the display).
    wake_rx: UnixStream,
    wake_tx: Arc<UnixStream>,
    /// Events from other threads (logind, the tray).
    shared: Arc<Mutex<Vec<DesktopEvent>>>,
    tray: RefCell<Option<tray::Tray>>,
}

/// The session is Wayland's: an X server here is Xwayland.
fn under_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("XDG_SESSION_TYPE").is_some_and(|t| t == "wayland")
}

fn io_err(call: &'static str) -> impl FnOnce(std::io::Error) -> Error {
    move |e| Error::Os {
        call,
        message: e.to_string(),
    }
}

impl Platform {
    /// Wayland when `WAYLAND_DISPLAY` is set, else X11 (`DISPLAY`);
    /// `DIORAMA_DISPLAY=x11` or `=wayland` chooses.
    pub fn new() -> Result<Self> {
        let forced = std::env::var("DIORAMA_DISPLAY").unwrap_or_default();
        let wayland = match forced.as_str() {
            "x11" => false,
            "wayland" => true,
            _ => std::env::var_os("WAYLAND_DISPLAY").is_some(),
        };
        let display = if wayland {
            Display::Wayland(Box::new(wayland::Display::new()?))
        } else {
            Display::X11(Box::new(x11::Display::new()?))
        };
        let (wake_rx, wake_tx) = UnixStream::pair().map_err(io_err("socketpair"))?;
        wake_rx
            .set_nonblocking(true)
            .map_err(io_err("socketpair"))?;
        wake_tx
            .set_nonblocking(true)
            .map_err(io_err("socketpair"))?;
        let wake_tx = Arc::new(wake_tx);
        let shared = Arc::new(Mutex::new(Vec::new()));
        session::watch(Arc::clone(&shared), Waker(Arc::clone(&wake_tx)));
        Ok(Self {
            display,
            wake_rx,
            wake_tx,
            shared,
            tray: RefCell::new(None),
        })
    }

    pub fn outputs(&self) -> Result<Vec<OutputInfo>> {
        on_display!(&self.display, d => d.outputs())
    }

    /// A wallpaper surface covering `output`, behind every window.
    pub fn create_wallpaper(&self, output: &OutputInfo) -> Result<WallpaperWindow> {
        Ok(match &self.display {
            Display::Wayland(d) => WallpaperWindow::Wayland(d.create_wallpaper(output)?),
            Display::X11(d) => WallpaperWindow::X11(d.create_wallpaper(output, false)?),
        })
    }

    /// Like [`Self::create_wallpaper`], but under the wallpapers already
    /// shown until [`WallpaperWindow::raise`].
    pub fn create_wallpaper_below(&self, output: &OutputInfo) -> Result<WallpaperWindow> {
        Ok(match &self.display {
            // A layer surface appears only with its first frame.
            Display::Wayland(d) => WallpaperWindow::Wayland(d.create_wallpaper(output)?),
            Display::X11(d) => WallpaperWindow::X11(d.create_wallpaper(output, true)?),
        })
    }

    pub fn create_offscreen(&self, _width: u32, _height: u32) -> Result<WallpaperWindow> {
        Err(Error::Unsupported)
    }

    /// Dispatches display events for up to `timeout` and returns desktop
    /// events raised meanwhile. Returns early when anything arrives or the
    /// [`Waker`] fires.
    pub fn poll_events(&self, timeout: Duration) -> Vec<DesktopEvent> {
        let mut events = on_display!(&self.display, d => d.poll_events(timeout, &self.wake_rx));
        // Drain the waker.
        let mut buf = [0u8; 64];
        while matches!((&self.wake_rx).read(&mut buf), Ok(n) if n > 0) {}
        if let Ok(mut shared) = self.shared.lock() {
            events.append(&mut shared);
        }
        events
    }

    /// What covers each output (fullscreen or maximized windows on it).
    pub fn coverage(&self, outputs: &[OutputInfo]) -> Vec<Coverage> {
        on_display!(&self.display, d => d.coverage(outputs))
    }

    pub fn on_battery(&self) -> bool {
        session::on_battery(Path::new("/sys/class/power_supply"))
    }

    /// The cursor in virtual-desktop pixels (on Wayland: where it last was
    /// over a wallpaper).
    pub fn cursor_position(&self) -> Option<(i32, i32)> {
        on_display!(&self.display, d => d.cursor_position())
    }

    /// Whether the cursor is over bare desktop and the left button is down
    /// there. A press or release since the last call counts for one call
    /// each, so a click shorter than a frame is still seen.
    pub fn desktop_pointer(&self) -> DesktopPointer {
        on_display!(&self.display, d => d.desktop_pointer())
    }

    /// A higher nice value for this thread and the threads and processes it
    /// starts from now on (Linux keeps it per thread), about what Windows'
    /// below-normal class does.
    pub fn lower_priority(&self) {
        if let Err(e) = rustix::process::setpriority_process(None, NICE) {
            tracing::warn!("could not lower process priority: {e}");
        }
    }

    /// Panels scale the pixmap to their size; 64 stays sharp on HiDPI.
    pub fn tray_icon_size(&self) -> u32 {
        64
    }

    /// Shows (or replaces) the tray icon: `rgba` is `size`×`size`.
    pub fn show_tray(&self, rgba: &[u8], size: u32, state: crate::TrayState) -> Result<()> {
        // The old icon leaves the panel (and frees its bus name) first.
        self.tray.borrow_mut().take();
        let tray = tray::Tray::show(rgba, size, state, Arc::clone(&self.shared), self.waker())?;
        *self.tray.borrow_mut() = Some(tray);
        Ok(())
    }

    pub fn set_tray_state(&self, state: crate::TrayState) {
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.set_state(state);
        }
    }

    /// Whether programs can set global hotkeys here: on X11 only. Wayland
    /// has no such thing, and Xwayland sees no keys meant for Wayland windows.
    pub fn hotkeys_supported(&self) -> bool {
        match &self.display {
            Display::Wayland(_) => false,
            Display::X11(_) => !under_wayland(),
        }
    }

    /// From now on `modifiers` with one of `digits` (1-9) raises
    /// [`DesktopEvent::Hotkey`], and no others do. Returns the digits whose
    /// hotkey another program has.
    pub fn set_hotkeys(&self, modifiers: HotkeyModifiers, digits: &[u8]) -> Vec<u8> {
        match &self.display {
            Display::X11(d) if self.hotkeys_supported() => d.set_hotkeys(modifiers, digits),
            _ => Vec::new(),
        }
    }

    pub fn waker(&self) -> Waker {
        Waker(Arc::clone(&self.wake_tx))
    }
}

/// Wakes the thread blocked in [`Platform::poll_events`]. Cheap to clone;
/// usable from any thread.
#[derive(Debug, Clone)]
pub struct Waker(Arc<UnixStream>);

impl Waker {
    pub fn wake(&self) {
        // A full socket already means a pending wake-up.
        let _ = (&*self.0).write(&[1]);
    }
}

/// A wallpaper surface behind every window, covering one output. Destroyed
/// on drop; drop GPU surfaces created from [`WallpaperWindow::handles`]
/// first.
#[derive(Debug)]
pub enum WallpaperWindow {
    Wayland(wayland::WallpaperWindow),
    X11(x11::WallpaperWindow),
}

impl WallpaperWindow {
    /// Shows a window made with `create_wallpaper_below` above the others.
    pub fn raise(&self) {
        match self {
            Self::Wayland(w) => w.raise(),
            Self::X11(w) => w.raise(),
        }
    }

    pub fn handles(&self) -> SurfaceHandles {
        match self {
            Self::Wayland(w) => w.handles(),
            Self::X11(w) => w.handles(),
        }
    }

    /// Physical pixels.
    pub fn size(&self) -> (u32, u32) {
        match self {
            Self::Wayland(w) => w.size(),
            Self::X11(w) => w.size(),
        }
    }

    pub fn keep_in_place(&self) -> bool {
        match self {
            Self::Wayland(w) => w.keep_in_place(),
            Self::X11(w) => w.keep_in_place(),
        }
    }
}
