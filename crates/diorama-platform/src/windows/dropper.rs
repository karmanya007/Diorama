//! Picking a colour from anywhere on the screen: a still of the whole
//! desktop in a topmost window, with a magnifier at the cursor. A click picks;
//! Escape, a right click, another window coming to the front or a minute
//! without a choice cancel. The still stays in memory and goes with the window.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLACK_BRUSH, BeginPaint, BitBlt, CAPTUREBLT,
    COLORONCOLOR, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, EndPaint, FillRect, FrameRect, GdiFlush, GetDC, GetStockObject,
    HBITMAP, HBRUSH, HDC, HGDIOBJ, InvalidateRect, PAINTSTRUCT, ReleaseDC, SRCCOPY, SelectObject,
    SetBkColor, SetStretchBltMode, SetTextColor, StretchBlt, TextOutW, WHITE_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetCapture, SetFocus, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    GetSystemMetrics, IDC_CROSS, LoadCursorW, MSG, PostQuitMessage, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOW, SetCursor,
    SetForegroundWindow, SetTimer, ShowWindow, TranslateMessage, WA_INACTIVE, WM_ACTIVATE,
    WM_CAPTURECHANGED, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WM_RBUTTONUP, WM_SETCURSOR, WM_TIMER, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::{os_err, register_class};
use crate::{Error, Result};

const CLASS: PCWSTR = w!("DioramaDropper");
/// Without a choice by then, it is cancelled.
const PATIENCE_MS: u32 = 60_000;
/// Screen pixels across the magnifier (odd: one is in the middle), and how
/// large each is drawn.
const CELLS: i32 = 15;
const ZOOM: i32 = 10;
const LABEL: i32 = 22;
/// From the cursor to the magnifier.
const GAP: i32 = 24;

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

struct State {
    still: Still,
    /// The cursor, in the window's own pixels.
    at: POINT,
    picked: Option<[u8; 3]>,
    done: bool,
}

/// Lets the user pick a colour on the screen; `None` if they did not.
/// Blocks; not for a thread with windows of its own.
pub fn pick_color() -> Result<Option<[u8; 3]>> {
    static BUSY: AtomicBool = AtomicBool::new(false);
    if BUSY.swap(true, Ordering::SeqCst) {
        return Ok(None);
    }
    // Messages and state of its own.
    let picked = std::thread::spawn(run).join();
    BUSY.store(false, Ordering::SeqCst);
    picked.unwrap_or(Err(Error::Os {
        call: "pick_color",
        message: "it failed".into(),
    }))
}

fn run() -> Result<Option<[u8; 3]>> {
    // SAFETY: Win32 calls on a thread of our own; pointers are to locals.
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance: HINSTANCE = GetModuleHandleW(PCWSTR::null())
            .map_err(|e| os_err("GetModuleHandleW", &e))?
            .into();
        register_class(instance, CLASS, window_proc)?;
        let [x, y, width, height] = [
            SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
            SM_CXVIRTUALSCREEN,
            SM_CYVIRTUALSCREEN,
        ]
        .map(|m| GetSystemMetrics(m));
        let still = Still::of_screen(x, y, width, height)?;
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                still,
                at: POINT {
                    x: at.x - x,
                    y: at.y - y,
                },
                picked: None,
                done: false,
            });
        });
        let window = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            CLASS,
            w!("Diorama colour"),
            WS_POPUP,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(instance),
            None,
        )
        .map_err(|e| os_err("CreateWindowExW(dropper)", &e))?;
        SetTimer(Some(window), 1, PATIENCE_MS, None);
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetForegroundWindow(window);
        let _ = SetFocus(Some(window));
        SetCapture(window);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        Ok(STATE.with(|s| s.borrow_mut().take().and_then(|s| s.picked)))
    }
}

/// Ends it, once, with what was picked.
fn finish(window: HWND, pick: bool) {
    let end = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(state) = s.as_mut().filter(|s| !s.done) else {
            return false;
        };
        state.done = true;
        if pick {
            state.picked = state.still.pixel(state.at.x, state.at.y);
        }
        true
    });
    if end {
        // SAFETY: our own window, on its thread.
        let _ = unsafe { DestroyWindow(window) };
    }
}

/// The cursor in a mouse message.
fn point(lp: LPARAM) -> POINT {
    POINT {
        x: i32::from(lp.0 as u16 as i16),
        y: i32::from((lp.0 >> 16) as u16 as i16),
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            paint(window);
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETCURSOR => {
            // SAFETY: a system cursor.
            unsafe {
                if let Ok(cursor) = LoadCursorW(None, IDC_CROSS) {
                    SetCursor(Some(cursor));
                }
            }
            return LRESULT(1);
        }
        WM_MOUSEMOVE => {
            moved(window, point(lp));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            moved(window, point(lp));
            finish(window, true);
            return LRESULT(0);
        }
        WM_RBUTTONUP | WM_TIMER => {
            finish(window, false);
            return LRESULT(0);
        }
        WM_KEYDOWN if wp.0 == usize::from(VK_ESCAPE.0) => {
            finish(window, false);
            return LRESULT(0);
        }
        WM_ACTIVATE if wp.0 as u32 & 0xFFFF == WA_INACTIVE => finish(window, false),
        WM_CAPTURECHANGED => finish(window, false),
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: default handling for our own window.
    unsafe { DefWindowProcW(window, message, wp, lp) }
}

fn moved(window: HWND, to: POINT) {
    let changed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let state = s.as_mut()?;
        let size = (state.still.width, state.still.height);
        let before = magnifier(state.at, size);
        state.at = to;
        Some([before, magnifier(to, size)])
    });
    for rect in changed.into_iter().flatten() {
        // SAFETY: our own window; the rectangle is a local.
        let _ = unsafe { InvalidateRect(Some(window), Some(&rect), false) };
    }
}

/// Where the magnifier is for a cursor `at`: beside it, inside the window.
fn magnifier(at: POINT, (width, height): (i32, i32)) -> RECT {
    let (w, h) = (CELLS * ZOOM, CELLS * ZOOM + LABEL);
    let side = |at: i32, size: i32, room: i32| {
        if at + GAP + size <= room {
            at + GAP
        } else {
            (at - GAP - size).max(0)
        }
    };
    let (left, top) = (side(at.x, w, width), side(at.y, h, height));
    RECT {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}

fn paint(window: HWND) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: GDI objects made and freed here; the window is ours.
    unsafe {
        let dc = BeginPaint(window, &mut ps);
        STATE.with(|s| {
            if let Some(state) = s.borrow().as_ref() {
                draw(state, dc, &ps.rcPaint);
            }
        });
        let _ = EndPaint(window, &ps);
    }
}

/// Draws `area` in one go (no flicker).
unsafe fn draw(state: &State, dc: HDC, area: &RECT) {
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    if w <= 0 || h <= 0 {
        return;
    }
    // SAFETY: every object made here is freed here; `dc` is the caller's.
    unsafe {
        let buffer = CreateCompatibleDC(Some(dc));
        let bitmap = CreateCompatibleBitmap(dc, w, h);
        let old = SelectObject(buffer, bitmap.into());
        let still = &state.still;
        let _ = BitBlt(
            buffer,
            0,
            0,
            w,
            h,
            Some(still.dc),
            area.left,
            area.top,
            SRCCOPY,
        );

        let m = magnifier(state.at, (still.width, still.height));
        let (x, y) = (m.left - area.left, m.top - area.top);
        let side = CELLS * ZOOM;
        let black = HBRUSH(GetStockObject(BLACK_BRUSH).0);
        let white = HBRUSH(GetStockObject(WHITE_BRUSH).0);
        let rect = |l: i32, t: i32, w: i32, h: i32| RECT {
            left: x + l,
            top: y + t,
            right: x + l + w,
            bottom: y + t + h,
        };
        FillRect(buffer, &rect(0, 0, side, side + LABEL), black);
        SetStretchBltMode(buffer, COLORONCOLOR);
        let _ = StretchBlt(
            buffer,
            x,
            y,
            side,
            side,
            Some(still.dc),
            state.at.x - CELLS / 2,
            state.at.y - CELLS / 2,
            CELLS,
            CELLS,
            SRCCOPY,
        );
        // The pixel that would be picked.
        let middle = CELLS / 2 * ZOOM;
        FrameRect(
            buffer,
            &rect(middle - 1, middle - 1, ZOOM + 2, ZOOM + 2),
            black,
        );
        FrameRect(buffer, &rect(middle, middle, ZOOM, ZOOM), white);
        FrameRect(buffer, &rect(0, 0, side, side + LABEL), white);
        if let Some([r, g, b]) = still.pixel(state.at.x, state.at.y) {
            let text: Vec<u16> = format!("#{r:02x}{g:02x}{b:02x}").encode_utf16().collect();
            SetBkColor(buffer, COLORREF(0));
            SetTextColor(buffer, COLORREF(0x00FF_FFFF));
            let _ = TextOutW(buffer, x + 8, y + side + 3, &text);
        }
        let _ = BitBlt(dc, area.left, area.top, w, h, Some(buffer), 0, 0, SRCCOPY);
        SelectObject(buffer, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(buffer);
    }
}

/// A picture in memory: what the screen showed.
struct Still {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    /// `width` × `height` BGRA, rows from the top; the bitmap's own memory.
    bits: *const u8,
    width: i32,
    height: i32,
}

impl Still {
    fn new(width: i32, height: i32) -> Result<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: rows from the top.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        // SAFETY: the bitmap is selected into a DC of ours and freed with it.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                Ok(bitmap) if !bits.is_null() => bitmap,
                Ok(_) | Err(_) => {
                    let _ = DeleteDC(dc);
                    return Err(Error::Os {
                        call: "CreateDIBSection",
                        message: format!("{width}×{height}"),
                    });
                }
            };
            let old = SelectObject(dc, bitmap.into());
            Ok(Self {
                dc,
                bitmap,
                old,
                bits: bits.cast(),
                width,
                height,
            })
        }
    }

    fn of_screen(x: i32, y: i32, width: i32, height: i32) -> Result<Self> {
        let still = Self::new(width.max(1), height.max(1))?;
        // SAFETY: the screen's DC is released right after the copy.
        unsafe {
            let screen = GetDC(None);
            let copied = BitBlt(
                still.dc,
                0,
                0,
                width,
                height,
                Some(screen),
                x,
                y,
                SRCCOPY | CAPTUREBLT,
            );
            ReleaseDC(None, screen);
            copied.map_err(|e| os_err("BitBlt", &e))?;
            let _ = GdiFlush();
        }
        Ok(still)
    }

    fn pixel(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return None;
        }
        let at = (y as usize * self.width as usize + x as usize) * 4;
        // SAFETY: inside the bitmap's memory, which lives as long as `self`.
        let [b, g, r] = unsafe { [0, 1, 2].map(|i| *self.bits.add(at + i)) };
        Some([r, g, b])
    }
}

impl Drop for Still {
    fn drop(&mut self) {
        // SAFETY: made in `new`, used nowhere else.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_are_read_where_they_were_drawn() {
        let still = Still::new(40, 20).unwrap();
        // SAFETY: drawing into our own bitmap.
        unsafe {
            let brush = windows::Win32::Graphics::Gdi::CreateSolidBrush(COLORREF(0x0030_2010));
            let rect = RECT {
                left: 5,
                top: 7,
                right: 6,
                bottom: 8,
            };
            FillRect(still.dc, &rect, brush);
            let _ = DeleteObject(brush.into());
            let _ = GdiFlush();
        }
        assert_eq!(still.pixel(5, 7), Some([0x10, 0x20, 0x30]));
        assert_eq!(still.pixel(6, 7), Some([0, 0, 0]));
        assert_eq!(still.pixel(40, 0), None);
        assert_eq!(still.pixel(-1, 0), None);
    }

    #[test]
    fn the_magnifier_stays_on_the_screen_and_off_the_cursor() {
        let size = (1920, 1080);
        for (x, y) in [(0, 0), (1919, 0), (0, 1079), (1919, 1079), (960, 540)] {
            let m = magnifier(POINT { x, y }, size);
            assert!(m.left >= 0 && m.top >= 0 && m.right <= 1920 && m.bottom <= 1080);
            assert!(x < m.left || x >= m.right, "{x} {m:?}");
        }
        // Windows left of and above the main monitor have negative positions
        // in messages only; the window's own pixels start at 0.
        let p = point(LPARAM(((5 << 16) | 0xFFFF) as isize));
        assert_eq!((p.x, p.y), (-1, 5));
    }

    /// What a user sees, drawn without any screen or window.
    #[test]
    fn the_magnifier_shows_the_pixel_in_the_middle() {
        let state = State {
            still: Still::new(400, 400).unwrap(),
            at: POINT { x: 100, y: 100 },
            picked: None,
            done: false,
        };
        let out = Still::new(400, 400).unwrap();
        // SAFETY: drawing between our own bitmaps.
        unsafe {
            let brush = windows::Win32::Graphics::Gdi::CreateSolidBrush(COLORREF(0x0000_00FF));
            let rect = RECT {
                left: 100,
                top: 100,
                right: 101,
                bottom: 101,
            };
            FillRect(state.still.dc, &rect, brush);
            let _ = DeleteObject(brush.into());
            let all = RECT {
                left: 0,
                top: 0,
                right: 400,
                bottom: 400,
            };
            draw(&state, out.dc, &all);
            let _ = GdiFlush();
        }
        let m = magnifier(state.at, (400, 400));
        let middle = (m.left + CELLS * ZOOM / 2, m.top + CELLS * ZOOM / 2);
        assert_eq!(out.pixel(middle.0, middle.1), Some([255, 0, 0]));
        assert_eq!(out.pixel(middle.0 + ZOOM, middle.1), Some([0, 0, 0]));
        assert_eq!(out.pixel(m.left, m.top), Some([255, 255, 255]), "its edge");
        assert_eq!(
            out.pixel(100, 100),
            Some([255, 0, 0]),
            "the screen under it"
        );
    }
}
