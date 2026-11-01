//! Wayland: outputs from `wl_output` (+ `xdg-output` names), wallpapers as
//! `wlr-layer-shell` surfaces on the background layer, and what covers each
//! output from `wlr-foreign-toplevel-management` (P5/P6).
//!
//! Single-threaded like the Windows backend: create, use and drop the
//! [`Platform`] on the thread that calls [`Platform::poll_events`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};
use wayland_client::backend::ObjectData;
use wayland_client::globals::{GlobalList, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_surface};
use wayland_client::{Connection, EventQueue, Proxy, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use crate::{Coverage, DesktopEvent, Error, OutputId, OutputInfo, Result, SurfaceHandles};

/// Layer-shell namespace, so compositors (and their users) can tell our
/// surfaces apart.
const NAMESPACE: &str = "diorama";

fn wl_err(call: &'static str) -> impl FnOnce(String) -> Error {
    move |message| Error::Os { call, message }
}

/// The Wayland connection and everything bound on it.
#[derive(Debug)]
pub struct Platform {
    inner: RefCell<Inner>,
    /// Read end of the waker's socket pair (polled with the connection).
    wake_rx: UnixStream,
    wake_tx: Arc<UnixStream>,
}

#[derive(Debug)]
struct Inner {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
}

impl Platform {
    pub fn new() -> Result<Self> {
        let conn = Connection::connect_to_env()
            .map_err(|e| wl_err("wl_display_connect")(e.to_string()))?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)
            .map_err(|e| wl_err("wl_registry")(e.to_string()))?;
        let qh = queue.handle();
        let mut state = State::new(&globals, &qh)?;
        // Outputs announce themselves (and their names) over two roundtrips.
        for _ in 0..2 {
            queue
                .roundtrip(&mut state)
                .map_err(|e| wl_err("wl_display_roundtrip")(e.to_string()))?;
        }
        state.ready = true;
        state.events.clear();
        let (wake_rx, wake_tx) =
            UnixStream::pair().map_err(|e| wl_err("socketpair")(e.to_string()))?;
        wake_rx
            .set_nonblocking(true)
            .map_err(|e| wl_err("socketpair")(e.to_string()))?;
        wake_tx
            .set_nonblocking(true)
            .map_err(|e| wl_err("socketpair")(e.to_string()))?;
        Ok(Self {
            inner: RefCell::new(Inner { conn, queue, state }),
            wake_rx,
            wake_tx: Arc::new(wake_tx),
        })
    }

    pub fn outputs(&self) -> Result<Vec<OutputInfo>> {
        let inner = self.inner.borrow();
        let state = &inner.state;
        let mut outputs: Vec<OutputInfo> = state
            .output_state
            .outputs()
            .filter_map(|o| state.output_info(&o))
            .collect();
        outputs.sort_by_key(|o| (o.x, o.y));
        // Wayland has no primary output; the one at the origin (else the
        // first) stands in.
        let primary = outputs
            .iter()
            .position(|o| (o.x, o.y) == (0, 0))
            .unwrap_or(0);
        if let Some(o) = outputs.get_mut(primary) {
            o.primary = true;
        }
        Ok(outputs)
    }

    /// A layer surface covering `output` on the background layer (below
    /// every window, including the compositor's own wallpaper client).
    pub fn create_wallpaper(&self, output: &OutputInfo) -> Result<WallpaperWindow> {
        let mut inner = self.inner.borrow_mut();
        let Inner { conn, queue, state } = &mut *inner;
        let qh = queue.handle();
        let wl_output = state
            .output_state
            .outputs()
            .find(|o| state.output_info(o).is_some_and(|i| i.id == output.id))
            .ok_or(Error::DesktopNotFound("Wayland output"))?;
        let scale = state
            .output_state
            .info(&wl_output)
            .map_or(1, |i| i.scale_factor.max(1));
        let surface = state.compositor.create_surface(&qh);
        surface.set_buffer_scale(scale);
        let layer = state.layer_shell.create_layer_surface(
            &qh,
            surface,
            Layer::Background,
            Some(NAMESPACE),
            Some(&wl_output),
        );
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(0, 0);
        layer.commit();
        let id = layer.wl_surface().id();
        state.configured.insert(id.clone(), None);
        // Nothing may be drawn before the first configure.
        while matches!(state.configured.get(&id), Some(None)) {
            queue
                .blocking_dispatch(state)
                .map_err(|e| wl_err("wl_display_dispatch")(e.to_string()))?;
        }
        let Some(Some((w, h))) = state.configured.get(&id).copied() else {
            return Err(Error::DesktopNotFound(
                "layer surface closed before configure",
            ));
        };
        let display = NonNull::new(conn.backend().display_ptr().cast())
            .ok_or(Error::DesktopNotFound("Wayland display"))?;
        Ok(WallpaperWindow {
            size: (w * scale as u32, h * scale as u32),
            display,
            layer,
            state_ref: Rc::clone(&state.closed),
        })
    }

    /// The same as [`Self::create_wallpaper`]: a layer surface appears only
    /// with its first frame, so the wallpaper it replaces stays visible
    /// until then without being stacked above.
    pub fn create_wallpaper_below(&self, output: &OutputInfo) -> Result<WallpaperWindow> {
        self.create_wallpaper(output)
    }

    pub fn create_offscreen(&self, _width: u32, _height: u32) -> Result<WallpaperWindow> {
        Err(Error::Unsupported)
    }

    /// Dispatches Wayland events for up to `timeout` and returns desktop
    /// events raised meanwhile. Returns early when anything arrives or the
    /// [`Waker`] fires.
    pub fn poll_events(&self, timeout: Duration) -> Vec<DesktopEvent> {
        let mut inner = self.inner.borrow_mut();
        let Inner { queue, state, .. } = &mut *inner;
        if let Err(e) = dispatch(queue, state, &self.wake_rx, timeout) {
            tracing::warn!("Wayland connection: {e}");
        }
        // Drain the waker.
        let mut buf = [0u8; 64];
        while matches!((&self.wake_rx).read(&mut buf), Ok(n) if n > 0) {}
        std::mem::take(&mut state.events)
    }

    /// What covers each output: windows the compositor reports fullscreen or
    /// maximized on it. Without `wlr-foreign-toplevel` (GNOME), nothing does.
    pub fn coverage(&self, outputs: &[OutputInfo]) -> Vec<Coverage> {
        let inner = self.inner.borrow();
        let state = &inner.state;
        outputs
            .iter()
            .map(|o| {
                state
                    .toplevels
                    .values()
                    .filter(|t| !t.minimized)
                    .filter(|t| {
                        t.outputs
                            .iter()
                            .any(|wo| state.output_info(wo).is_some_and(|i| i.id == o.id))
                    })
                    .map(|t| {
                        if t.fullscreen {
                            Coverage::Fullscreen
                        } else if t.maximized {
                            Coverage::Maximized
                        } else {
                            Coverage::Visible
                        }
                    })
                    .max()
                    .unwrap_or_default()
            })
            .collect()
    }

    pub fn on_battery(&self) -> bool {
        false
    }

    /// Wayland clients never see the global cursor position.
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
        Waker(Arc::clone(&self.wake_tx))
    }
}

/// Flushes requests, then waits up to `timeout` for the connection or the
/// waker, and dispatches what arrived.
fn dispatch(
    queue: &mut EventQueue<State>,
    state: &mut State,
    wake: &UnixStream,
    timeout: Duration,
) -> std::result::Result<(), String> {
    queue.flush().map_err(|e| e.to_string())?;
    queue.dispatch_pending(state).map_err(|e| e.to_string())?;
    if !state.events.is_empty() {
        return Ok(());
    }
    let Some(guard) = queue.prepare_read() else {
        // Events were queued meanwhile.
        queue.dispatch_pending(state).map_err(|e| e.to_string())?;
        return Ok(());
    };
    let (conn_fd, wake_fd) = (guard.connection_fd(), wake.as_fd());
    let mut fds = [
        rustix::event::PollFd::new(&conn_fd, rustix::event::PollFlags::IN),
        rustix::event::PollFd::new(&wake_fd, rustix::event::PollFlags::IN),
    ];
    let timeout = rustix::event::Timespec::try_from(timeout).ok();
    match rustix::event::poll(&mut fds, timeout.as_ref()) {
        Ok(_) if fds[0].revents().contains(rustix::event::PollFlags::IN) => {
            guard.read().map_err(|e| e.to_string())?;
        }
        Ok(_) | Err(rustix::io::Errno::INTR) => drop(guard),
        Err(e) => return Err(e.to_string()),
    }
    queue.dispatch_pending(state).map_err(|e| e.to_string())?;
    Ok(())
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

/// A layer surface behind every window, covering one output. Destroyed on
/// drop; drop GPU surfaces created from [`WallpaperWindow::handles`] first.
#[derive(Debug)]
pub struct WallpaperWindow {
    layer: LayerSurface,
    display: NonNull<std::ffi::c_void>,
    /// Buffer size: the configured (logical) size times the output's scale.
    size: (u32, u32),
    state_ref: Rc<RefCell<Vec<wayland_client::backend::ObjectId>>>,
}

impl WallpaperWindow {
    /// Layer surfaces of one layer have no order a client can set; a new
    /// one shows once it has drawn, which is what raising is for.
    pub fn raise(&self) {}

    pub fn handles(&self) -> SurfaceHandles {
        let surface = self.layer.wl_surface().id().as_ptr();
        // SAFETY: the surface pointer stays valid while `self.layer` lives;
        // a null pointer is impossible for a live proxy.
        let window = WaylandWindowHandle::new(unsafe { NonNull::new_unchecked(surface.cast()) });
        SurfaceHandles {
            window: RawWindowHandle::Wayland(window),
            display: RawDisplayHandle::Wayland(WaylandDisplayHandle::new(self.display)),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Layer surfaces stay on their output; the compositor moves them.
    pub fn keep_in_place(&self) -> bool {
        false
    }
}

impl Drop for WallpaperWindow {
    fn drop(&mut self) {
        let id = self.layer.wl_surface().id();
        self.state_ref.borrow_mut().retain(|c| *c != id);
    }
}

/// One application window as `wlr-foreign-toplevel` reports it.
#[derive(Debug, Default)]
struct Toplevel {
    outputs: Vec<wl_output::WlOutput>,
    maximized: bool,
    minimized: bool,
    fullscreen: bool,
    /// Changes since the last `done` event.
    pending: Option<(bool, bool, bool)>,
}

#[derive(Debug)]
struct State {
    registry: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    /// Configured (logical) size of each layer surface; `None` until the
    /// first configure.
    configured: HashMap<wayland_client::backend::ObjectId, Option<(u32, u32)>>,
    /// Surfaces the compositor closed (shared with their windows).
    closed: Rc<RefCell<Vec<wayland_client::backend::ObjectId>>>,
    toplevels: HashMap<wayland_client::backend::ObjectId, Toplevel>,
    /// Desktop events not yet returned by `poll_events`.
    events: Vec<DesktopEvent>,
    /// Initial roundtrips done: output changes from now on are events.
    ready: bool,
}

impl State {
    fn new(globals: &GlobalList, qh: &QueueHandle<Self>) -> Result<Self> {
        let compositor = CompositorState::bind(globals, qh)
            .map_err(|e| wl_err("bind wl_compositor")(e.to_string()))?;
        let layer_shell = LayerShell::bind(globals, qh).map_err(|e| {
            wl_err("bind zwlr_layer_shell_v1")(format!(
                "{e} (this compositor has no layer-shell; GNOME needs Diorama's extension)"
            ))
        })?;
        // Optional: without it nothing ever counts as covering an output.
        if globals
            .bind::<ZwlrForeignToplevelManagerV1, _, _>(qh, 1..=3, ToplevelManagerData)
            .is_err()
        {
            tracing::info!("no wlr-foreign-toplevel: wallpapers will not pause behind windows");
        }
        Ok(Self {
            registry: RegistryState::new(globals),
            output_state: OutputState::new(globals, qh),
            compositor,
            layer_shell,
            configured: HashMap::new(),
            closed: Rc::new(RefCell::new(Vec::new())),
            toplevels: HashMap::new(),
            events: Vec::new(),
            ready: false,
        })
    }

    /// Diorama's view of a Wayland output. Its identity is make, model and
    /// connector name (`DP-1`): stable across restarts as long as the
    /// monitor stays on its port (Wayland exposes no EDID serial).
    fn output_info(&self, output: &wl_output::WlOutput) -> Option<OutputInfo> {
        let info = self.output_state.info(output)?;
        let scale = info.scale_factor.max(1);
        let (lw, lh) = info.logical_size?;
        let (lx, ly) = info.logical_position.unwrap_or((0, 0));
        let mode = info.modes.iter().find(|m| m.current);
        let connector = info.name.clone().unwrap_or_default();
        Some(OutputInfo {
            id: OutputId(format!("{}|{}|{}", info.make, info.model, connector)),
            name: info
                .description
                .clone()
                .unwrap_or_else(|| info.model.clone()),
            manufacturer: info.make.clone(),
            product_code: 0,
            device: connector,
            x: lx * scale,
            y: ly * scale,
            width: (lw * scale).max(0) as u32,
            height: (lh * scale).max(0) as u32,
            refresh_mhz: mode.map_or(0, |m| m.refresh_rate.max(0) as u32),
            primary: false,
        })
    }

    fn outputs_changed(&mut self) {
        if self.ready && !self.events.contains(&DesktopEvent::OutputsChanged) {
            self.events.push(DesktopEvent::OutputsChanged);
        }
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
        // Reported when one of our surfaces enters an output; real scale
        // changes arrive as output updates.
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.outputs_changed();
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.outputs_changed();
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.outputs_changed();
    }
}

impl LayerShellHandler for State {
    /// The compositor removed the surface (output gone, compositor policy):
    /// every wallpaper is rebuilt.
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        let id = layer.wl_surface().id();
        self.configured.remove(&id);
        self.closed.borrow_mut().push(id);
        if !self.events.contains(&DesktopEvent::DesktopReset) {
            self.events.push(DesktopEvent::DesktopReset);
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let (w, h) = configure.new_size;
        let size = Some((w.max(1), h.max(1)));
        let id = layer.wl_surface().id();
        let previous = self.configured.insert(id, size);
        // A resize of a live surface means its output changed.
        if matches!(previous, Some(Some(old)) if Some(old) != size) {
            self.outputs_changed();
        }
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

/// Handler data of the `wlr-foreign-toplevel` manager.
#[derive(Debug)]
struct ToplevelManagerData;

/// Handler data of one toplevel handle.
#[derive(Debug)]
struct ToplevelData;

impl Dispatch2<ZwlrForeignToplevelManagerV1, State> for ToplevelManagerData {
    fn event(
        &self,
        state: &mut State,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state.toplevels.insert(toplevel.id(), Toplevel::default());
        }
    }

    fn event_created_child(opcode: u16, qh: &QueueHandle<State>) -> Arc<dyn ObjectData> {
        debug_assert_eq!(
            opcode,
            zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE
        );
        qh.make_data::<ZwlrForeignToplevelHandleV1, _>(ToplevelData)
    }
}

impl Dispatch2<ZwlrForeignToplevelHandleV1, State> for ToplevelData {
    fn event(
        &self,
        state: &mut State,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::{Event, State as S};
        let id = handle.id();
        let Some(t) = state.toplevels.get_mut(&id) else {
            return;
        };
        match event {
            Event::State { state: bytes } => {
                let states: Vec<u32> = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| u32::from_ne_bytes(*c))
                    .collect();
                let has = |s: S| states.contains(&(s as u32));
                t.pending = Some((has(S::Maximized), has(S::Minimized), has(S::Fullscreen)));
            }
            Event::OutputEnter { output } => {
                if !t.outputs.contains(&output) {
                    t.outputs.push(output);
                }
            }
            Event::OutputLeave { output } => t.outputs.retain(|o| *o != output),
            Event::Done => {
                if let Some((maximized, minimized, fullscreen)) = t.pending.take() {
                    (t.maximized, t.minimized, t.fullscreen) = (maximized, minimized, fullscreen);
                }
            }
            Event::Closed => {
                state.toplevels.remove(&id);
                handle.destroy();
            }
            _ => {}
        }
    }
}

delegate_dispatch2!(State);
delegate_registry!(State);
