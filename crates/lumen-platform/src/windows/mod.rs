//! Windows backend.

mod desktop;
mod outputs;
mod system;
mod windows_list;

pub use system::{
    autostart, cache_dir, config_dir, detach_console, set_autostart, set_os_wallpaper,
};

use std::cell::{Cell, RefCell};
use std::mem::size_of;
use std::num::NonZeroIsize;
use std::time::Duration;

use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::MapWindowPoints;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, SetPriorityClass,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GWLP_USERDATA, GetWindowLongPtrW, HWND_TOP, MA_NOACTIVATE, MSG, MsgWaitForMultipleObjects,
    PM_REMOVE, PeekMessageW, PostMessageW, QS_ALLINPUT, RegisterClassExW, RegisterWindowMessageW,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, TranslateMessage, WM_APP,
    WM_DESTROY, WM_DISPLAYCHANGE, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_WTSSESSION_CHANGE,
    WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    WS_VISIBLE, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
};
use windows::core::{PCWSTR, w};

use crate::{Coverage, DesktopEvent, Error, OutputInfo, Result, SurfaceHandles, classify};

const CONTROL_CLASS: PCWSTR = w!("LumenControl");
const WALLPAPER_CLASS: PCWSTR = w!("LumenWallpaper");
/// Marker in GWLP_USERDATA meaning "we are destroying this window on purpose".
const CLOSING: isize = 1;

thread_local! {
    /// Events raised by window procedures, drained by `Platform::poll_events`.
    static EVENTS: RefCell<Vec<DesktopEvent>> = const { RefCell::new(Vec::new()) };
    /// Message id of the `TaskbarCreated` broadcast (Explorer started/restarted).
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

fn push_event(e: DesktopEvent) {
    EVENTS.with(|ev| {
        let mut ev = ev.borrow_mut();
        if !ev.contains(&e) {
            ev.push(e);
        }
    });
}

fn os_err(call: &'static str, e: &windows::core::Error) -> Error {
    Error::Os {
        call,
        message: e.message(),
    }
}

/// Owns the hidden control window and window classes. Single-threaded: create,
/// use and drop on the thread that pumps messages.
#[derive(Debug)]
pub struct Platform {
    hinstance: HINSTANCE,
    control: HWND,
}

impl Platform {
    pub fn new() -> Result<Self> {
        // SAFETY: Win32 setup calls; all pointers reference live locals or statics.
        unsafe {
            // Physical-pixel coordinates everywhere. Fails harmlessly if already set (e.g. by a manifest).
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

            let hinstance: HINSTANCE = GetModuleHandleW(PCWSTR::null())
                .map_err(|e| os_err("GetModuleHandleW", &e))?
                .into();
            register_class(hinstance, CONTROL_CLASS, control_proc)?;
            register_class(hinstance, WALLPAPER_CLASS, wallpaper_proc)?;
            TASKBAR_CREATED.with(|c| c.set(RegisterWindowMessageW(w!("TaskbarCreated"))));

            // Top-level (not message-only) so it receives TaskbarCreated / WM_DISPLAYCHANGE broadcasts.
            let control = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                CONTROL_CLASS,
                w!("Lumen"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinstance),
                None,
            )
            .map_err(|e| os_err("CreateWindowExW(control)", &e))?;
            // Lock/unlock notifications (pause while the lock screen is up).
            if let Err(e) = WTSRegisterSessionNotification(control, NOTIFY_FOR_THIS_SESSION) {
                tracing::warn!("session notifications unavailable: {}", e.message());
            }

            Ok(Self { hinstance, control })
        }
    }

    pub fn outputs(&self) -> Result<Vec<OutputInfo>> {
        outputs::enumerate()
    }

    /// How much other applications hide each output (P5/P6). Cheap enough to
    /// call a few times per second.
    pub fn coverage(&self, outputs: &[OutputInfo]) -> Vec<Coverage> {
        classify(outputs, &windows_list::app_windows())
    }

    /// A handle other threads can use to interrupt [`Platform::poll_events`].
    pub fn waker(&self) -> Waker {
        Waker {
            control: self.control.0 as isize,
        }
    }

    /// True when running on battery power.
    pub fn on_battery(&self) -> bool {
        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: out-param is a valid local.
        unsafe { GetSystemPowerStatus(&mut status) }.is_ok() && status.ACLineStatus == 0
    }

    /// Below-normal CPU priority; WDDM also schedules GPU work from lower-priority
    /// processes behind foreground games (P5).
    pub fn lower_priority(&self) {
        // SAFETY: changes only this process's priority class.
        if let Err(e) =
            unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) }
        {
            tracing::warn!("could not lower process priority: {}", e.message());
        }
    }

    /// Creates a window covering `output`, parented behind the desktop icons.
    pub fn create_wallpaper(&self, output: &OutputInfo) -> Result<WallpaperWindow> {
        let (host, layout) = desktop::wallpaper_host()?;
        tracing::debug!(?layout, output = %output.device, "attaching wallpaper window");

        // Monitor rect is in screen coordinates; child windows are positioned in parent client coordinates.
        let mut origin = [POINT {
            x: output.x,
            y: output.y,
        }];
        // SAFETY: plain Win32 calls with valid handles; class registered in `new`.
        unsafe {
            MapWindowPoints(None, Some(host), &mut origin);
            let hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE,
                WALLPAPER_CLASS,
                w!("Lumen wallpaper"),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                origin[0].x,
                origin[0].y,
                output.width as i32,
                output.height as i32,
                Some(host),
                None,
                Some(self.hinstance),
                None,
            )
            .map_err(|e| os_err("CreateWindowExW(wallpaper)", &e))?;
            // Above any other wallpaper host child (e.g. another wallpaper app), still below the icons.
            SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                origin[0].x,
                origin[0].y,
                output.width as i32,
                output.height as i32,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .map_err(|e| os_err("SetWindowPos", &e))?;

            Ok(WallpaperWindow {
                hwnd,
                hinstance: self.hinstance,
                width: output.width,
                height: output.height,
            })
        }
    }

    /// Pumps window messages for up to `timeout` and returns desktop events
    /// raised meanwhile. Returns early as soon as any message arrives.
    pub fn poll_events(&self, timeout: Duration) -> Vec<DesktopEvent> {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: standard message loop on the thread that owns our windows.
        unsafe {
            MsgWaitForMultipleObjects(None, false, ms, QS_ALLINPUT);
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        EVENTS.with(|ev| std::mem::take(&mut *ev.borrow_mut()))
    }
}

impl Drop for Platform {
    fn drop(&mut self) {
        // SAFETY: control window was created by us on this thread.
        unsafe {
            let _ = WTSUnRegisterSessionNotification(self.control);
            let _ = DestroyWindow(self.control);
        }
    }
}

/// Wakes the thread blocked in [`Platform::poll_events`]. Cheap to clone; usable from any thread.
#[derive(Debug, Clone)]
pub struct Waker {
    /// Control window handle stored as an integer: HWNDs are process-wide ids, and
    /// PostMessageW is documented as safe to call from any thread.
    control: isize,
}

impl Waker {
    pub fn wake(&self) {
        // SAFETY: posting a no-op message; if the window is gone the call just fails.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.control as *mut _)),
                WM_LUMEN_WAKE,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

/// Posted by [`Waker::wake`]; its only job is to end the message wait.
const WM_LUMEN_WAKE: u32 = WM_APP + 1;

/// A window behind the desktop icons covering one output. Destroyed on drop.
/// Drop GPU surfaces created from [`WallpaperWindow::handles`] first.
#[derive(Debug)]
pub struct WallpaperWindow {
    hwnd: HWND,
    hinstance: HINSTANCE,
    width: u32,
    height: u32,
}

impl WallpaperWindow {
    pub fn handles(&self) -> SurfaceHandles {
        // HWNDs are never null for a created window; fall back defensively anyway.
        let hwnd = NonZeroIsize::new(self.hwnd.0 as isize).unwrap_or(NonZeroIsize::MIN);
        let mut window = Win32WindowHandle::new(hwnd);
        window.hinstance = NonZeroIsize::new(self.hinstance.0 as isize);
        SurfaceHandles {
            window: RawWindowHandle::Win32(window),
            display: RawDisplayHandle::Windows(WindowsDisplayHandle::new()),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Drop for WallpaperWindow {
    fn drop(&mut self) {
        // SAFETY: hwnd was created by us; if Explorer already destroyed it these calls fail harmlessly.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, CLOSING);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe fn register_class(
    hinstance: HINSTANCE,
    name: PCWSTR,
    proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
) -> Result<()> {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(proc),
        hInstance: hinstance,
        lpszClassName: name,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialised; re-registration returns 0, which we tolerate
    // (a second Platform in the same process reuses the class).
    let atom = unsafe { RegisterClassExW(&class) };
    if atom == 0 {
        let err = windows::core::Error::from_thread();
        // ERROR_CLASS_ALREADY_EXISTS (1410)
        if err.code().0 as u32 & 0xFFFF != 1410 {
            return Err(os_err("RegisterClassExW", &err));
        }
    }
    Ok(())
}

unsafe extern "system" fn control_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == TASKBAR_CREATED.with(Cell::get) && msg != 0 {
        tracing::info!("Explorer restarted (TaskbarCreated)");
        push_event(DesktopEvent::DesktopReset);
        return LRESULT(0);
    }
    if msg == WM_DISPLAYCHANGE {
        push_event(DesktopEvent::OutputsChanged);
    }
    if msg == WM_WTSSESSION_CHANGE {
        match wp.0 as u32 {
            WTS_SESSION_LOCK => push_event(DesktopEvent::SessionLocked),
            WTS_SESSION_UNLOCK => push_event(DesktopEvent::SessionUnlocked),
            _ => {}
        }
    }
    // SAFETY: default handling for our own window.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn wallpaper_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // The GPU covers every pixel; skipping GDI erase avoids a flash of the class background.
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_DESTROY => {
            // SAFETY: reading our own window's user data.
            let intentional = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } == CLOSING;
            if !intentional {
                // Parent (Explorer's WorkerW) went away: wallpapers must be recreated.
                tracing::warn!("wallpaper window destroyed externally");
                push_event(DesktopEvent::DesktopReset);
            }
            LRESULT(0)
        }
        // SAFETY: default handling for our own window.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
