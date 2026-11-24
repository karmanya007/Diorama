//! X11: outputs from RandR (identity from EDID), wallpapers as EWMH desktop
//! windows (`_NET_WM_WINDOW_TYPE_DESKTOP`: the window manager keeps them
//! below every other window, on every workspace) where the window manager
//! supports them, else as override-redirect windows it never sees, kept at
//! the bottom of the stack (i3, no window manager), what covers each output
//! from `_NET_CLIENT_LIST` and `_NET_WM_STATE`, and the cursor from the X
//! server, which (unlike Wayland) tells any client where it is. Left-button
//! presses come from XInput 2 raw events, so clicks shorter than a frame
//! are not lost.
//!
//! The connection is libxcb's, whose handle Vulkan's XCB surfaces take.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::num::NonZeroU32;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::Duration;

use raw_window_handle::{RawDisplayHandle, RawWindowHandle, XcbDisplayHandle, XcbWindowHandle};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, KeyButMask,
    PropMode, StackMode, Window, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;

use crate::{
    Coverage, DesktopEvent, DesktopPointer, Error, OutputId, OutputInfo, Result, SurfaceHandles,
    WindowRect,
};

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        _NET_CLIENT_LIST,
        _NET_CURRENT_DESKTOP,
        _NET_SUPPORTED,
        _NET_WM_DESKTOP,
        _NET_WM_NAME,
        _NET_WM_STATE,
        _NET_WM_STATE_BELOW,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_SKIP_PAGER,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_STATE_STICKY,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DESKTOP,
        _NET_WM_WINDOW_TYPE_DOCK,
        UTF8_STRING,
        EDID,
    }
}

/// X's left mouse button.
const BUTTON_LEFT: u32 = 1;
/// `_NET_WM_DESKTOP` of windows shown on every workspace.
const ALL_DESKTOPS: u32 = 0xFFFF_FFFF;

fn x_err(call: &'static str) -> impl FnOnce(String) -> Error {
    move |message| Error::Os { call, message }
}

/// Turns any x11rb error into ours.
macro_rules! x {
    ($call:literal, $e:expr) => {
        $e.map_err(|e| x_err($call)(e.to_string()))
    };
}

/// Left-button presses (`true`) and releases on the desktop, in order, not
/// yet seen by `desktop_pointer`.
#[derive(Debug, Default)]
struct Pointer {
    edges: VecDeque<bool>,
    /// The press that started on the desktop has not been released yet.
    pressed_here: bool,
}

/// The X connection and Diorama's windows on it.
pub struct Display {
    conn: Rc<XCBConnection>,
    screen: usize,
    root: Window,
    atoms: Atoms,
    /// Diorama's live wallpaper windows (shared with them).
    ours: Rc<RefCell<HashSet<Window>>>,
    pointer: RefCell<Pointer>,
    /// The window manager supports EWMH desktop windows.
    managed: bool,
}

impl std::fmt::Debug for Display {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("x11::Display")
            .field("screen", &self.screen)
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Display {
    pub fn new() -> Result<Self> {
        let (conn, screen) = x!("xcb_connect", XCBConnection::connect(None))?;
        let root = conn
            .setup()
            .roots
            .get(screen)
            .ok_or(Error::DesktopNotFound("X screen"))?
            .root;
        let atoms = x!("intern atoms", Atoms::new(&conn))?;
        let atoms = x!("intern atoms", atoms.reply())?;
        let managed = conn
            .get_property(false, root, atoms._NET_SUPPORTED, AtomEnum::ATOM, 0, 4096)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| {
                r.value32()
                    .map(|mut v| v.any(|a| a == atoms._NET_WM_WINDOW_TYPE_DESKTOP))
            })
            .unwrap_or(false);
        tracing::info!(
            "X11 wallpapers: {}",
            if managed {
                "desktop windows (EWMH)"
            } else {
                "unmanaged windows at the bottom (the window manager has no desktop windows)"
            }
        );
        // Monitor changes (RandR 1.2+).
        x!("RandR", conn.randr_query_version(1, 5).map(|c| c.reply()))?
            .map_err(|e| x_err("RandR")(e.to_string()))?;
        x!(
            "randr_select_input",
            conn.randr_select_input(
                root,
                randr::NotifyMask::SCREEN_CHANGE
                    | randr::NotifyMask::OUTPUT_CHANGE
                    | randr::NotifyMask::CRTC_CHANGE,
            )
        )?;
        // Button presses anywhere, without grabbing them (XInput 2 raw
        // events); without XInput 2 only the held state is seen.
        let xi = conn
            // 2.1+: raw events arrive even while another client grabs the
            // pointer (window managers do on clicks).
            .xinput_xi_query_version(2, 2)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|v| v.major_version >= 2);
        if xi {
            let mask = xinput::EventMask {
                deviceid: xinput::Device::ALL_MASTER.into(),
                mask: vec![
                    xinput::XIEventMask::RAW_BUTTON_PRESS | xinput::XIEventMask::RAW_BUTTON_RELEASE,
                ],
            };
            if let Err(e) = conn.xinput_xi_select_events(root, &[mask]) {
                tracing::debug!("XInput raw buttons: {e}");
            }
        } else {
            tracing::info!("no XInput 2: clicks shorter than a frame may be missed");
        }
        x!("xcb_flush", conn.flush())?;
        Ok(Self {
            conn: Rc::new(conn),
            screen,
            root,
            atoms,
            ours: Rc::new(RefCell::new(HashSet::new())),
            pointer: RefCell::new(Pointer::default()),
            managed,
        })
    }

    /// Connected monitors that show part of the screen.
    pub fn outputs(&self) -> Result<Vec<OutputInfo>> {
        let conn = &*self.conn;
        let res = x!(
            "randr_get_screen_resources_current",
            conn.randr_get_screen_resources_current(self.root)
                .map(|c| c.reply())
        )?
        .map_err(|e| x_err("randr_get_screen_resources_current")(e.to_string()))?;
        let primary = conn
            .randr_get_output_primary(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or(0, |p| p.output);
        let mut outputs = Vec::new();
        for &output in &res.outputs {
            let Some(info) = conn
                .randr_get_output_info(output, res.config_timestamp)
                .ok()
                .and_then(|c| c.reply().ok())
            else {
                continue;
            };
            if info.connection != randr::Connection::CONNECTED || info.crtc == 0 {
                continue;
            }
            let Some(crtc) = conn
                .randr_get_crtc_info(info.crtc, res.config_timestamp)
                .ok()
                .and_then(|c| c.reply().ok())
            else {
                continue;
            };
            if crtc.width == 0 || crtc.height == 0 {
                continue;
            }
            let connector = String::from_utf8_lossy(&info.name).into_owned();
            let edid = conn
                .randr_get_output_property(
                    output,
                    self.atoms.EDID,
                    AtomEnum::ANY,
                    0,
                    128,
                    false,
                    false,
                )
                .ok()
                .and_then(|c| c.reply().ok())
                .and_then(|p| Edid::parse(&p.data));
            let refresh = res
                .modes
                .iter()
                .find(|m| m.id == crtc.mode)
                .and_then(|m| {
                    let dots = u64::from(m.htotal) * u64::from(m.vtotal);
                    (u64::from(m.dot_clock) * 1000).checked_div(dots)
                })
                .map_or(0, |mhz| u32::try_from(mhz).unwrap_or(0));
            outputs.push(OutputInfo {
                id: OutputId(match &edid {
                    Some(e) => format!(
                        "{}|{:04X}|{}|{connector}",
                        e.manufacturer, e.product, e.serial
                    ),
                    None => format!("x11|{connector}"),
                }),
                name: edid
                    .as_ref()
                    .and_then(|e| e.name.clone())
                    .unwrap_or_else(|| connector.clone()),
                manufacturer: edid
                    .as_ref()
                    .map(|e| e.manufacturer.clone())
                    .unwrap_or_default(),
                product_code: edid.as_ref().map_or(0, |e| e.product),
                device: connector,
                x: i32::from(crtc.x),
                y: i32::from(crtc.y),
                width: u32::from(crtc.width),
                height: u32::from(crtc.height),
                refresh_mhz: refresh,
                primary: output == primary,
            });
        }
        outputs.sort_by_key(|o| (o.x, o.y));
        // No primary set: the one at the origin (else the first) stands in.
        if !outputs.iter().any(|o| o.primary) {
            let i = outputs
                .iter()
                .position(|o| (o.x, o.y) == (0, 0))
                .unwrap_or(0);
            if let Some(o) = outputs.get_mut(i) {
                o.primary = true;
            }
        }
        Ok(outputs)
    }

    /// A desktop window on `output`; with `below`, under Diorama's other
    /// windows until [`WallpaperWindow::raise`].
    pub fn create_wallpaper(&self, output: &OutputInfo, below: bool) -> Result<WallpaperWindow> {
        let conn = &*self.conn;
        let screen = conn
            .setup()
            .roots
            .get(self.screen)
            .ok_or(Error::DesktopNotFound("X screen"))?;
        let window = x!("xcb_generate_id", conn.generate_id())?;
        let (x, y) = (
            i16::try_from(output.x).unwrap_or(0),
            i16::try_from(output.y).unwrap_or(0),
        );
        let (w, h) = (
            u16::try_from(output.width).unwrap_or(u16::MAX),
            u16::try_from(output.height).unwrap_or(u16::MAX),
        );
        x!(
            "create_window",
            conn.create_window(
                screen.root_depth,
                window,
                self.root,
                x,
                y,
                w,
                h,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new()
                    .background_pixel(screen.black_pixel)
                    .override_redirect(u32::from(!self.managed))
                    .event_mask(EventMask::STRUCTURE_NOTIFY),
            )
        )?;
        let a = &self.atoms;
        let set32 = |property, kind: AtomEnum, values: &[u32]| {
            x!(
                "change_property",
                conn.change_property32(PropMode::REPLACE, window, property, kind, values)
            )
            .map(|_| ())
        };
        if self.managed {
            set32(
                a._NET_WM_WINDOW_TYPE,
                AtomEnum::ATOM,
                &[a._NET_WM_WINDOW_TYPE_DESKTOP],
            )?;
            set32(
                a._NET_WM_STATE,
                AtomEnum::ATOM,
                &[
                    a._NET_WM_STATE_BELOW,
                    a._NET_WM_STATE_STICKY,
                    a._NET_WM_STATE_SKIP_TASKBAR,
                    a._NET_WM_STATE_SKIP_PAGER,
                ],
            )?;
            set32(a._NET_WM_DESKTOP, AtomEnum::CARDINAL, &[ALL_DESKTOPS])?;
        }
        x!(
            "change_property",
            conn.change_property8(
                PropMode::REPLACE,
                window,
                a._NET_WM_NAME,
                a.UTF8_STRING,
                b"Diorama"
            )
        )?;
        x!(
            "change_property",
            conn.change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                b"diorama\0Diorama\0"
            )
        )?;
        x!("map_window", conn.map_window(window))?;
        // Window managers may place desktop windows themselves: ask again
        // for the output's rectangle, and the stacking wanted.
        let mut config = ConfigureWindowAux::new()
            .x(i32::from(x))
            .y(i32::from(y))
            .width(u32::from(w))
            .height(u32::from(h));
        // Unmanaged windows go to the very bottom, under every window.
        if below || !self.managed {
            config = config.stack_mode(StackMode::BELOW);
        }
        x!("configure_window", conn.configure_window(window, &config))?;
        self.ours.borrow_mut().insert(window);
        let raw = NonNull::new(conn.get_raw_xcb_connection())
            .ok_or(Error::DesktopNotFound("XCB connection"))?;
        let wallpaper = WallpaperWindow {
            conn: Rc::clone(&self.conn),
            ours: Rc::clone(&self.ours),
            window,
            root: self.root,
            managed: self.managed,
            raw,
            screen: i32::try_from(self.screen).unwrap_or(0),
            size: (output.width, output.height),
        };
        if !below && !self.managed {
            wallpaper.raise();
        }
        x!("xcb_flush", conn.flush())?;
        Ok(wallpaper)
    }

    /// Waits up to `timeout` for X events (or `wake`) and returns desktop
    /// events raised meanwhile.
    pub fn poll_events(&self, timeout: Duration, wake: &UnixStream) -> Vec<DesktopEvent> {
        let conn = &*self.conn;
        let _ = conn.flush();
        // SAFETY: the connection's socket stays open while `conn` lives.
        let xfd = unsafe { BorrowedFd::borrow_raw(conn.as_raw_fd()) };
        let mut fds = [
            rustix::event::PollFd::new(&xfd, rustix::event::PollFlags::IN),
            rustix::event::PollFd::new(wake, rustix::event::PollFlags::IN),
        ];
        let timeout = rustix::event::Timespec::try_from(timeout).ok();
        // Events may already be queued in libxcb.
        let mut events = Vec::new();
        if !self.drain(&mut events) {
            let _ = rustix::event::poll(&mut fds, timeout.as_ref());
            self.drain(&mut events);
        }
        events
    }

    /// Handles every queued X event; true if there was any.
    fn drain(&self, events: &mut Vec<DesktopEvent>) -> bool {
        let mut any = false;
        let push = |e: DesktopEvent, events: &mut Vec<DesktopEvent>| {
            if !events.contains(&e) {
                events.push(e);
            }
        };
        loop {
            let event = match self.conn.poll_for_event() {
                Ok(Some(event)) => event,
                Ok(None) => return any,
                Err(e) => {
                    tracing::warn!("X connection lost: {e}");
                    push(DesktopEvent::DisplayLost, events);
                    return true;
                }
            };
            any = true;
            match event {
                Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_) => {
                    push(DesktopEvent::OutputsChanged, events);
                }
                // Destroyed by someone else (we forget ours before destroying).
                Event::DestroyNotify(e) if self.ours.borrow().contains(&e.window) => {
                    push(DesktopEvent::DesktopReset, events);
                }
                Event::XinputRawButtonPress(e) if e.detail == BUTTON_LEFT => {
                    if self.over_desktop() {
                        let mut p = self.pointer.borrow_mut();
                        p.pressed_here = true;
                        p.edges.push_back(true);
                        tracing::trace!("left press on the desktop");
                    }
                }
                Event::XinputRawButtonRelease(e) if e.detail == BUTTON_LEFT => {
                    let mut p = self.pointer.borrow_mut();
                    if std::mem::take(&mut p.pressed_here) {
                        p.edges.push_back(false);
                        tracing::trace!("left release");
                    }
                }
                _ => {}
            }
        }
    }

    /// What covers each output: client windows on the current workspace,
    /// with fullscreen and maximized as the window manager reports them.
    pub fn coverage(&self, outputs: &[OutputInfo]) -> Vec<Coverage> {
        let a = &self.atoms;
        let current = self
            .cardinals(self.root, a._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL)
            .first()
            .copied();
        let ours = self.ours.borrow();
        let windows: Vec<WindowRect> = self
            .cardinals(self.root, a._NET_CLIENT_LIST, AtomEnum::WINDOW)
            .into_iter()
            .filter(|w| !ours.contains(w))
            .filter_map(|w| {
                let kinds = self.cardinals(w, a._NET_WM_WINDOW_TYPE, AtomEnum::ATOM);
                if kinds.contains(&a._NET_WM_WINDOW_TYPE_DESKTOP)
                    || kinds.contains(&a._NET_WM_WINDOW_TYPE_DOCK)
                {
                    return None;
                }
                let state = self.cardinals(w, a._NET_WM_STATE, AtomEnum::ATOM);
                if state.contains(&a._NET_WM_STATE_HIDDEN) {
                    return None;
                }
                let desktop = self
                    .cardinals(w, a._NET_WM_DESKTOP, AtomEnum::CARDINAL)
                    .first()
                    .copied();
                if let (Some(d), Some(c)) = (desktop, current)
                    && d != c
                    && d != ALL_DESKTOPS
                {
                    return None;
                }
                let conn = &*self.conn;
                let geometry = conn.get_geometry(w).ok()?.reply().ok()?;
                let at = conn
                    .translate_coordinates(w, self.root, 0, 0)
                    .ok()?
                    .reply()
                    .ok()?;
                let (left, top) = (i32::from(at.dst_x), i32::from(at.dst_y));
                let rect = WindowRect {
                    left,
                    top,
                    right: left + i32::from(geometry.width),
                    bottom: top + i32::from(geometry.height),
                    maximized: state.contains(&a._NET_WM_STATE_MAXIMIZED_VERT)
                        && state.contains(&a._NET_WM_STATE_MAXIMIZED_HORZ),
                };
                Some(as_window_manager_says(
                    rect,
                    state.contains(&a._NET_WM_STATE_FULLSCREEN),
                    outputs,
                ))
            })
            .collect();
        crate::classify(outputs, &windows)
    }

    /// A 32-bit list property (atoms, windows, cardinals); empty if unset.
    fn cardinals(&self, window: Window, property: u32, kind: AtomEnum) -> Vec<u32> {
        self.conn
            .get_property(false, window, property, kind, 0, 1024)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(Iterator::collect))
            .unwrap_or_default()
    }

    /// The cursor in screen pixels.
    pub fn cursor_position(&self) -> Option<(i32, i32)> {
        let p = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((i32::from(p.root_x), i32::from(p.root_y)))
    }

    /// No application window under the cursor: the root, one of Diorama's
    /// windows (or the frame a window manager put around it), or another
    /// desktop window (a file manager's desktop icons).
    fn over_desktop(&self) -> bool {
        let Some(child) = self
            .conn
            .query_pointer(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|p| p.child)
        else {
            return false;
        };
        if child == x11rb::NONE || self.ours.borrow().contains(&child) {
            return true;
        }
        let a = &self.atoms;
        if self
            .cardinals(child, a._NET_WM_WINDOW_TYPE, AtomEnum::ATOM)
            .contains(&a._NET_WM_WINDOW_TYPE_DESKTOP)
        {
            return true;
        }
        // A frame around one of ours.
        self.ours
            .borrow()
            .iter()
            .any(|&w| self.top_level(w) == Some(child))
    }

    /// The root's child that holds `window`.
    fn top_level(&self, mut window: Window) -> Option<Window> {
        for _ in 0..8 {
            let tree = self.conn.query_tree(window).ok()?.reply().ok()?;
            if tree.parent == self.root || tree.parent == x11rb::NONE {
                return Some(window);
            }
            window = tree.parent;
        }
        None
    }

    pub fn desktop_pointer(&self) -> DesktopPointer {
        if let Some(down) = self.pointer.borrow_mut().edges.pop_front() {
            return DesktopPointer {
                over_desktop: true,
                left_down: down,
            };
        }
        let over = self.over_desktop();
        let held = self
            .conn
            .query_pointer(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|p| p.mask.contains(KeyButMask::BUTTON1));
        DesktopPointer {
            over_desktop: over,
            left_down: over && held,
        }
    }
}

/// `rect` as coverage should count it: a window the window manager calls
/// fullscreen covers its whole output (terminals size themselves in whole
/// characters, a few pixels short), and one filling most of its output
/// counts as maximized (tiling window managers such as i3 have no maximize;
/// a tiled window fills the workspace).
fn as_window_manager_says(
    mut rect: WindowRect,
    fullscreen: bool,
    outputs: &[OutputInfo],
) -> WindowRect {
    let (cx, cy) = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
    let Some(o) = outputs.iter().find(|o| {
        cx >= o.x && cx < o.x + o.width as i32 && cy >= o.y && cy < o.y + o.height as i32
    }) else {
        return rect;
    };
    if fullscreen {
        rect.left = rect.left.min(o.x);
        rect.top = rect.top.min(o.y);
        rect.right = rect.right.max(o.x + o.width as i32);
        rect.bottom = rect.bottom.max(o.y + o.height as i32);
    } else {
        let area = |w: i32, h: i32| i64::from(w.max(0)) * i64::from(h.max(0));
        let covered = area(
            rect.right.min(o.x + o.width as i32) - rect.left.max(o.x),
            rect.bottom.min(o.y + o.height as i32) - rect.top.max(o.y),
        );
        let output = area(o.width as i32, o.height as i32);
        rect.maximized |= covered * 100 >= output * 85;
    }
    rect
}

/// What a monitor's EDID says about it.
#[derive(Debug, Clone, PartialEq)]
struct Edid {
    /// Three-letter PNP code (`DEL`, `GSM`).
    manufacturer: String,
    product: u16,
    serial: u32,
    /// The monitor name descriptor, if any.
    name: Option<String>,
}

impl Edid {
    fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 128 || data[..8] != [0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0] {
            return None;
        }
        let code = u16::from_be_bytes([data[8], data[9]]);
        let letter = |shift: u16| char::from(b'A' - 1 + ((code >> shift) & 0x1F) as u8);
        let manufacturer = [letter(10), letter(5), letter(0)].iter().collect();
        let product = u16::from_le_bytes([data[10], data[11]]);
        let serial = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
        let name = [54, 72, 90, 108].iter().find_map(|&at| {
            let d = &data[at..at + 18];
            (d[..3] == [0, 0, 0] && d[3] == 0xFC).then(|| {
                String::from_utf8_lossy(&d[5..])
                    .split('\n')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            })
        });
        Some(Self {
            manufacturer,
            product,
            serial,
            name,
        })
    }
}

/// A desktop window covering one output. Destroyed on drop; drop GPU
/// surfaces created from [`WallpaperWindow::handles`] first.
pub struct WallpaperWindow {
    conn: Rc<XCBConnection>,
    ours: Rc<RefCell<HashSet<Window>>>,
    window: Window,
    root: Window,
    /// An EWMH desktop window (else unmanaged, at the bottom).
    managed: bool,
    raw: NonNull<std::ffi::c_void>,
    screen: i32,
    size: (u32, u32),
}

impl std::fmt::Debug for WallpaperWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("x11::WallpaperWindow")
            .field("window", &self.window)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl WallpaperWindow {
    /// Above Diorama's other wallpaper windows, still below applications:
    /// the window manager keeps desktop windows below them; an unmanaged
    /// window goes just above the highest of Diorama's others.
    pub fn raise(&self) {
        let config = if self.managed {
            Some(ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))
        } else {
            let ours = self.ours.borrow();
            // Bottom to top.
            let children = self
                .conn
                .query_tree(self.root)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|t| t.children)
                .unwrap_or_default();
            children
                .iter()
                .rev()
                .find(|w| **w != self.window && ours.contains(w))
                .map(|&top| {
                    ConfigureWindowAux::new()
                        .sibling(top)
                        .stack_mode(StackMode::ABOVE)
                })
        };
        if let Some(config) = config {
            let _ = self.conn.configure_window(self.window, &config);
            let _ = self.conn.flush();
        }
    }

    pub fn handles(&self) -> SurfaceHandles {
        // An X window id is never 0 (`NONE`).
        let window = NonZeroU32::new(self.window).unwrap_or(NonZeroU32::MIN);
        SurfaceHandles {
            window: RawWindowHandle::Xcb(XcbWindowHandle::new(window)),
            display: RawDisplayHandle::Xcb(XcbDisplayHandle::new(Some(self.raw), self.screen)),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The window manager keeps desktop windows where they were put.
    pub fn keep_in_place(&self) -> bool {
        false
    }
}

impl Drop for WallpaperWindow {
    fn drop(&mut self) {
        // Forgotten first: its DestroyNotify is not a desktop reset.
        self.ours.borrow_mut().remove(&self.window);
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_manager_states_shape_coverage() {
        let output = OutputInfo {
            width: 1280,
            height: 720,
            ..OutputInfo::default()
        };
        let outputs = [output];
        let rect = |left, top, right, bottom| WindowRect {
            left,
            top,
            right,
            bottom,
            maximized: false,
        };
        // An xterm made fullscreen, 2 px short of the output.
        let full = as_window_manager_says(rect(0, 0, 1278, 718), true, &outputs);
        assert_eq!(crate::classify(&outputs, &[full]), [Coverage::Fullscreen]);
        // A tiled window filling the workspace above i3's bar.
        let tiled = as_window_manager_says(rect(0, 18, 1280, 697), false, &outputs);
        assert_eq!(crate::classify(&outputs, &[tiled]), [Coverage::Maximized]);
        // A normal window.
        let small = as_window_manager_says(rect(100, 100, 700, 500), false, &outputs);
        assert_eq!(crate::classify(&outputs, &[small]), [Coverage::Visible]);
    }

    #[test]
    fn edid_identity_and_name() {
        let mut e = [0u8; 128];
        e[..8].copy_from_slice(&[0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]);
        // "DEL": D=4, E=5, L=12 → 0b0_00100_00101_01100.
        e[8..10].copy_from_slice(&0x10ACu16.to_be_bytes());
        e[10..12].copy_from_slice(&0xA0B1u16.to_le_bytes());
        e[12..16].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        e[72..77].copy_from_slice(&[0, 0, 0, 0xFC, 0]);
        e[77..90].copy_from_slice(b"DELL U2720Q\n ");
        let edid = Edid::parse(&e).unwrap();
        assert_eq!(edid.manufacturer, "DEL");
        assert_eq!((edid.product, edid.serial), (0xA0B1, 0x1234_5678));
        assert_eq!(edid.name.as_deref(), Some("DELL U2720Q"));
        assert_eq!(Edid::parse(&e[..100]), None);
    }
}
