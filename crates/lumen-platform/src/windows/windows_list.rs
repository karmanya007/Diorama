//! Top-level application windows that can hide the wallpaper.

use std::mem::size_of;

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetWindowLongW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, IsZoomed, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};
use windows::core::BOOL;

use super::outputs::wide_to_string;
use crate::coverage::WindowRect;

/// Shell windows that sit over the desktop but are not applications.
const SHELL_CLASSES: [&str; 6] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
];

pub(super) fn app_windows() -> Vec<WindowRect> {
    let mut out: Vec<WindowRect> = Vec::new();
    // SAFETY: the callback only uses the LPARAM during this synchronous call.
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM((&raw mut out) as isize));
    }
    out
}

unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the Vec passed by `app_windows`; Win32 queries on a live HWND.
    unsafe {
        let list = &mut *(data.0 as *mut Vec<WindowRect>);
        if let Some(rect) = classify_window(hwnd) {
            list.push(rect);
        }
    }
    BOOL::from(true)
}

/// Returns the window's rectangle if it is a visible application window.
unsafe fn classify_window(hwnd: HWND) -> Option<WindowRect> {
    // SAFETY: read-only Win32 queries on a window handle from EnumWindows.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == std::process::id() {
            return None;
        }
        // Click-through overlays (GPU/recording overlays) and tool windows never hide the wallpaper meaningfully.
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & (WS_EX_TRANSPARENT.0 | WS_EX_TOOLWINDOW.0) != 0 {
            return None;
        }
        // Cloaked = hidden by DWM (other virtual desktops, suspended UWP apps).
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        );
        if cloaked != 0 {
            return None;
        }
        let mut class = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class);
        if len > 0 && SHELL_CLASSES.contains(&wide_to_string(&class).as_str()) {
            return None;
        }
        // Visible bounds without the invisible resize borders Windows adds around frames.
        let mut rect = RECT::default();
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut rect).cast(),
            size_of::<RECT>() as u32,
        )
        .ok()?;
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return None;
        }
        Some(WindowRect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
            maximized: IsZoomed(hwnd).as_bool(),
        })
    }
}
