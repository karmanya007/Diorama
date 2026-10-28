//! Notification-area (tray) icon on the control window: a menu to open the
//! UI, move playlists on, pause or resume, and quit.

use std::cell::RefCell;
use std::mem::size_of;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NINF_KEY, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetSystemMetrics,
    HICON, ICONINFO, MF_GRAYED, MF_SEPARATOR, MF_STRING, PostMessageW, SM_CXSMICON,
    SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP,
    WM_CONTEXTMENU, WM_NULL,
};
use windows::core::{PCWSTR, w};

use super::{os_err, push_event};
use crate::{DesktopEvent, Result, TrayCommand, TrayState};

/// Callback message of the icon (NOTIFYICON_VERSION_4 layout).
pub(super) const WM_DIORAMA_TRAY: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;
/// Enter or space on the focused icon.
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;

struct Tray {
    control: HWND,
    icon: HICON,
    state: TrayState,
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

/// Side of the small icon the notification area shows, in pixels.
pub(super) fn icon_size() -> u32 {
    // SAFETY: plain metric query.
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
    u32::try_from(size).unwrap_or(16).max(16)
}

/// Adds (or replaces) the icon: `rgba` is `size`×`size`, straight alpha.
pub(super) fn show(control: HWND, rgba: &[u8], size: u32, state: TrayState) -> Result<()> {
    hide();
    let icon = icon_from_rgba(rgba, size)?;
    TRAY.with(|t| {
        *t.borrow_mut() = Some(Tray {
            control,
            icon,
            state,
        });
    });
    add();
    Ok(())
}

/// What the menu offers; the tooltip says when wallpapers are paused.
pub(super) fn set_state(state: TrayState) {
    let changed = TRAY.with(|t| {
        let mut t = t.borrow_mut();
        let Some(tray) = t.as_mut() else { return false };
        let changed = tray.state.paused != state.paused;
        tray.state = state;
        changed
    });
    if changed {
        notify(NIM_MODIFY);
    }
}

/// Puts the icon back after Explorer restarted (TaskbarCreated).
pub(super) fn add() {
    if notify(NIM_ADD) {
        notify(NIM_SETVERSION);
    }
}

pub(super) fn hide() {
    notify(NIM_DELETE);
    if let Some(tray) = TRAY.with(|t| t.borrow_mut().take()) {
        // SAFETY: icon created by `icon_from_rgba`, no longer shown.
        unsafe {
            let _ = DestroyIcon(tray.icon);
        }
    }
}

/// Handles the icon's callback message.
pub(super) fn on_message(wp: WPARAM, lp: LPARAM) {
    let event = (lp.0 & 0xFFFF) as u32;
    match event {
        NIN_SELECT | NIN_KEYSELECT => push_event(DesktopEvent::Tray(TrayCommand::OpenUi)),
        WM_CONTEXTMENU => {
            // Anchor point in screen coordinates.
            let x = i32::from((wp.0 & 0xFFFF) as i16);
            let y = i32::from(((wp.0 >> 16) & 0xFFFF) as i16);
            if let Some(command) = menu(x, y) {
                push_event(DesktopEvent::Tray(command));
            }
        }
        _ => {}
    }
}

fn menu(x: i32, y: i32) -> Option<TrayCommand> {
    let (control, state) = TRAY.with(|t| t.borrow().as_ref().map(|t| (t.control, t.state)))?;
    // `None`: a separator.
    let pause = if state.paused {
        (TrayCommand::Resume, w!("Resume"), true)
    } else {
        (TrayCommand::Pause, w!("Pause"), true)
    };
    let items = [
        Some((TrayCommand::OpenUi, w!("Open Diorama"), true)),
        Some((TrayCommand::Next, w!("Next wallpaper"), state.can_next)),
        Some(pause),
        None,
        Some((TrayCommand::Quit, w!("Quit"), true)),
    ];
    // SAFETY: menu created, shown and destroyed here on the window's thread.
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for (i, item) in items.iter().enumerate() {
            let _ = match item {
                Some((_, text, true)) => AppendMenuW(menu, MF_STRING, i + 1, *text),
                Some((_, text, false)) => AppendMenuW(menu, MF_STRING | MF_GRAYED, i + 1, *text),
                None => AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()),
            };
        }
        // The menu closes when clicking elsewhere only if our window is in
        // the foreground (documented TrackPopupMenu caveat).
        let _ = SetForegroundWindow(control);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            x,
            y,
            None,
            control,
            None,
        )
        .0;
        let _ = PostMessageW(Some(control), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        let index = usize::try_from(chosen).ok()?.checked_sub(1)?;
        items.get(index).copied().flatten().map(|item| item.0)
    }
}

/// Sends the icon's data with `message`; `false` if the shell refused it
/// (no taskbar yet, or no icon to show).
fn notify(message: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE) -> bool {
    TRAY.with(|t| {
        let t = t.borrow();
        let Some(tray) = t.as_ref() else { return false };
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: tray.control,
            uID: ICON_ID,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: WM_DIORAMA_TRAY,
            hIcon: tray.icon,
            ..Default::default()
        };
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let tip = if tray.state.paused {
            "Diorama (paused)"
        } else {
            "Diorama"
        };
        for (dst, src) in data.szTip.iter_mut().zip(tip.encode_utf16()) {
            *dst = src;
        }
        // SAFETY: `data` is a fully initialised NOTIFYICONDATAW.
        unsafe { Shell_NotifyIconW(message, &data).as_bool() }
    })
}

fn icon_from_rgba(rgba: &[u8], size: u32) -> Result<HICON> {
    let side = i32::try_from(size).unwrap_or(0);
    if side == 0 || rgba.len() != (size * size * 4) as usize {
        return Err(crate::Error::Os {
            call: "CreateIconIndirect",
            message: "icon pixels do not match its size".into(),
        });
    }
    // 32-bit colour bitmaps are BGRA; the alpha channel makes the mask moot
    // but one is still required.
    let bgra: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| [b, g, r, a])
        .collect();
    let mask = vec![0u8; (size.div_ceil(16) * 2 * size) as usize];
    // SAFETY: bitmaps sized to the buffers passed; deleted after the icon
    // has copied them.
    unsafe {
        let color = CreateBitmap(side, side, 1, 32, Some(bgra.as_ptr().cast()));
        let mono = CreateBitmap(side, side, 1, 1, Some(mask.as_ptr().cast()));
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mono,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mono.into());
        icon.map_err(|e| os_err("CreateIconIndirect", &e))
    }
}
