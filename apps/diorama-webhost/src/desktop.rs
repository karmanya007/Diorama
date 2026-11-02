//! Where the pages live: windows behind the desktop, one per output, and the
//! event loop that drives them.
//!
//! Windows (and macOS later): diorama-platform's wallpaper windows, with the
//! WebView built as a child of each. Linux: GTK windows made layer-shell
//! background surfaces (`gtk-layer-shell`), since WebKitGTK lives in GTK
//! widgets; the GTK main loop runs the pages and diorama-platform still
//! reports outputs and their changes.

#[cfg(target_os = "linux")]
pub use self::gtk_desktop::{Desktop, Surface};
#[cfg(not(target_os = "linux"))]
pub use self::native::{Desktop, Surface};

#[cfg(not(target_os = "linux"))]
mod native {
    use std::time::Duration;

    use anyhow::Result;
    use diorama_platform::{DesktopEvent, OutputInfo, Platform, WallpaperWindow};
    use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
    use wry::{WebView, WebViewBuilder};

    pub use diorama_platform::Waker;

    /// A raw handle as `HasWindowHandle` for wry.
    struct Parent(RawWindowHandle);

    impl HasWindowHandle for Parent {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            // SAFETY: the handle comes from a WallpaperWindow that outlives
            // the WebView built on it (both live in the same `Screen`).
            Ok(unsafe { WindowHandle::borrow_raw(self.0) })
        }
    }

    pub struct Desktop(Platform);

    impl Desktop {
        pub fn new() -> Result<Self> {
            Ok(Self(Platform::new()?))
        }

        pub fn waker(&self) -> Waker {
            self.0.waker()
        }

        pub fn outputs(&self) -> Result<Vec<OutputInfo>> {
            Ok(self.0.outputs()?)
        }

        pub fn poll_events(&self, timeout: Duration) -> Vec<DesktopEvent> {
            self.0.poll_events(timeout)
        }

        /// A window behind the desktop on `output`; `below` the other
        /// wallpaper windows until [`Surface::raise`].
        pub fn window(&self, output: &OutputInfo, below: bool) -> Result<Surface> {
            Ok(Surface(if below {
                self.0.create_wallpaper_below(output)?
            } else {
                self.0.create_wallpaper(output)?
            }))
        }

        pub fn offscreen(&self, width: u32, height: u32) -> Result<Surface> {
            Ok(Surface(self.0.create_offscreen(width, height)?))
        }
    }

    pub struct Surface(WallpaperWindow);

    impl Surface {
        pub fn raise(&self) {
            self.0.raise();
        }

        pub fn keep_in_place(&self) {
            self.0.keep_in_place();
        }

        /// Physical pixels.
        pub fn size(&self) -> (u32, u32) {
            self.0.size()
        }

        pub fn build(&self, builder: WebViewBuilder<'_>) -> wry::Result<WebView> {
            builder.build_as_child(&Parent(self.0.handles().window))
        }
    }
}

#[cfg(target_os = "linux")]
mod gtk_desktop {
    use std::cell::Cell;
    use std::time::Duration;

    use anyhow::{Context, Result};
    use diorama_platform::{DesktopEvent, OutputInfo, Platform};
    use gtk::glib;
    use gtk::prelude::*;
    use gtk_layer_shell::{Edge, Layer, LayerShell};
    use wry::{WebView, WebViewBuilder, WebViewBuilderExtUnix};

    /// Wakes [`Desktop::poll_events`] from another thread.
    #[derive(Clone)]
    pub struct Waker(glib::MainContext);

    impl Waker {
        pub fn wake(&self) {
            self.0.wakeup();
        }
    }

    pub struct Desktop {
        platform: Platform,
        context: glib::MainContext,
    }

    impl Desktop {
        pub fn new() -> Result<Self> {
            gtk::init().context("GTK init")?;
            Ok(Self {
                platform: Platform::new()?,
                context: glib::MainContext::default(),
            })
        }

        pub fn waker(&self) -> Waker {
            Waker(self.context.clone())
        }

        pub fn outputs(&self) -> Result<Vec<OutputInfo>> {
            Ok(self.platform.outputs()?)
        }

        /// Runs GTK (the pages) until something happens or `timeout`
        /// passes; returns output changes meanwhile.
        pub fn poll_events(&self, timeout: Duration) -> Vec<DesktopEvent> {
            let timer = glib::timeout_source_new(timeout, None, glib::Priority::DEFAULT, || {
                glib::ControlFlow::Break
            });
            timer.attach(Some(&self.context));
            self.context.iteration(true);
            while self.context.pending() {
                self.context.iteration(false);
            }
            timer.destroy();
            self.platform.poll_events(Duration::ZERO)
        }

        /// A background layer surface on `output`; with `below` it appears
        /// only on [`Surface::raise`] (layer surfaces of one layer have no
        /// order, so the page it replaces stays until then).
        pub fn window(&self, output: &OutputInfo, below: bool) -> Result<Surface> {
            let window = gtk::Window::new(gtk::WindowType::Toplevel);
            window.init_layer_shell();
            window.set_layer(Layer::Background);
            window.set_namespace("diorama");
            window.set_keyboard_interactivity(false);
            for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
                window.set_anchor(edge, true);
            }
            window.set_exclusive_zone(-1);
            if let Some(monitor) = monitor(output) {
                window.set_monitor(&monitor);
            }
            let surface = Surface {
                window,
                size: (output.width, output.height),
                shown: Cell::new(false),
            };
            if !below {
                surface.raise();
            }
            Ok(surface)
        }

        /// A window that never reaches the screen (snapshots).
        pub fn offscreen(&self, width: u32, height: u32) -> Result<Surface> {
            let window = gtk::OffscreenWindow::new();
            window.set_default_size(
                i32::try_from(width).unwrap_or(1920),
                i32::try_from(height).unwrap_or(1080),
            );
            let surface = Surface {
                window: window.upcast(),
                size: (width, height),
                shown: Cell::new(false),
            };
            surface.raise();
            Ok(surface)
        }
    }

    /// GDK's monitor for `output` (same position; GDK reports logical
    /// pixels).
    fn monitor(output: &OutputInfo) -> Option<gtk::gdk::Monitor> {
        let display = gtk::gdk::Display::default()?;
        (0..display.n_monitors())
            .filter_map(|i| display.monitor(i))
            .find(|m| {
                let (g, scale) = (m.geometry(), m.scale_factor());
                (g.x() * scale, g.y() * scale) == (output.x, output.y)
            })
    }

    pub struct Surface {
        window: gtk::Window,
        size: (u32, u32),
        shown: Cell<bool>,
    }

    impl Surface {
        pub fn raise(&self) {
            if !self.shown.replace(true) {
                self.window.show_all();
            }
        }

        /// The compositor keeps layer surfaces on their output.
        pub fn keep_in_place(&self) {}

        /// Physical pixels.
        pub fn size(&self) -> (u32, u32) {
            self.size
        }

        pub fn build(&self, builder: WebViewBuilder<'_>) -> wry::Result<WebView> {
            let webview = builder.build_gtk(&self.window)?;
            if self.shown.get() {
                self.window.show_all();
            }
            Ok(webview)
        }
    }

    impl Drop for Surface {
        fn drop(&mut self) {
            // SAFETY: the window is ours; its WebView (dropped before it,
            // see `Screen`) no longer uses it.
            unsafe { self.window.destroy() };
        }
    }
}
