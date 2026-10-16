//! Finding the window that sits between the desktop background and the icons.
//!
//! Explorer creates a `WorkerW` for wallpaper hosting when `Progman` receives
//! the undocumented message `0x052C` (the same one Windows uses for slideshow
//! transitions). Its location depends on the Windows build:
//!
//! - **Windows 11 24H2+** (verified on build 26200): `WorkerW` is a *child* of
//!   `Progman`, next to `SHELLDLL_DefView` (the icons).
//! - **Older builds:** `WorkerW` is a *top-level* window right after the
//!   top-level window that hosts `SHELLDLL_DefView`.
//!
//! Wallpaper Engine itself parents its window into this `WorkerW` (see
//! `docs/re-notes/` and PROGRESS.md). Pain point P9.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GA_ROOT, GetAncestor, GetClassNameW, SMTO_NORMAL,
    SendMessageTimeoutW,
};
use windows::core::{BOOL, PCWSTR, w};

use crate::{Error, Result};

const SPAWN_WORKERW: u32 = 0x052C;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Layout {
    /// WorkerW is a child of Progman (Windows 11 24H2+).
    ChildOfProgman,
    /// WorkerW is a top-level window behind the icons (older builds).
    TopLevel,
}

/// Returns the window to parent wallpaper windows into, and which layout was found.
pub(super) fn wallpaper_host() -> Result<(HWND, Layout)> {
    // SAFETY: plain Win32 calls with valid string literals and out-params.
    unsafe {
        let progman = FindWindowW(w!("Progman"), PCWSTR::null())
            .map_err(|_| Error::DesktopNotFound("Progman"))?;

        // Ask Explorer to create the WorkerW if it does not exist yet. Harmless if it does.
        let _ = SendMessageTimeoutW(
            progman,
            SPAWN_WORKERW,
            WPARAM(0),
            LPARAM(0),
            SMTO_NORMAL,
            1000,
            None,
        );

        if let Ok(worker) = FindWindowExW(Some(progman), None, w!("WorkerW"), PCWSTR::null()) {
            return Ok((worker, Layout::ChildOfProgman));
        }

        let mut found = HWND::default();
        let _ = EnumWindows(Some(find_legacy_worker), LPARAM((&raw mut found) as isize));
        if found.is_invalid() {
            Err(Error::DesktopNotFound("WorkerW"))
        } else {
            Ok((found, Layout::TopLevel))
        }
    }
}

unsafe extern "system" fn find_legacy_worker(hwnd: HWND, out: LPARAM) -> BOOL {
    // SAFETY: called synchronously by EnumWindows with the LPARAM we passed,
    // which points at a live HWND on the caller's stack.
    unsafe {
        let hosts_icons =
            FindWindowExW(Some(hwnd), None, w!("SHELLDLL_DefView"), PCWSTR::null()).is_ok();
        if hosts_icons
            && let Ok(worker) = FindWindowExW(None, Some(hwnd), w!("WorkerW"), PCWSTR::null())
        {
            *(out.0 as *mut HWND) = worker;
            return BOOL::from(false); // stop enumerating
        }
    }
    BOOL::from(true)
}

/// `hwnd` belongs to the desktop (icons view, Explorer's desktop windows or
/// a wallpaper window), not to an application window.
pub(super) fn is_desktop_window(hwnd: HWND) -> bool {
    // SAFETY: class-name and ancestor queries on any window handle.
    unsafe {
        let root = GetAncestor(hwnd, GA_ROOT);
        let mut name = [0u16; 64];
        let n = GetClassNameW(root, &mut name);
        let class = String::from_utf16_lossy(&name[..usize::try_from(n).unwrap_or(0)]);
        matches!(class.as_str(), "Progman" | "WorkerW")
    }
}
