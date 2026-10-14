//! GPU context (L2): one wgpu device shared by every output, plus per-output
//! presentation surfaces.
//!
//! Conventions from the RenderDoc captures (`docs/re-notes/renderdoc-captures.md`):
//! WE renders in gamma space into `R8G8B8A8_UNORM` targets, so surfaces use a
//! non-sRGB format to reproduce its colours.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

pub mod shader;

pub use wgpu;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no suitable GPU adapter: {0}")]
    NoAdapter(#[from] wgpu::RequestAdapterError),
    #[error("no GPU backend was tried")]
    NoBackend,
    #[error("device request failed: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
    #[error("surface creation failed: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
    #[error("surface has no supported formats on this adapter")]
    NoSurfaceFormat,
    #[error("surface lost and could not be reconfigured")]
    SurfaceLost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// Platform default: DX12 on Windows, Vulkan on Linux, Metal on macOS.
    #[default]
    Auto,
    Dx12,
    Vulkan,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GpuOptions {
    pub backend: Backend,
    /// Prefer an integrated GPU when there is a choice (battery, P5).
    pub low_power: bool,
}

/// The single device shared by all outputs.
#[derive(Debug)]
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    lost: Arc<AtomicBool>,
}

impl Gpu {
    pub fn new(options: GpuOptions) -> Result<Self> {
        let attempts: &[wgpu::Backends] = match options.backend {
            // DX12 first: native flip-model swapchains on Windows (P3); it simply finds no
            // adapter elsewhere, so this stays OS-agnostic.
            Backend::Auto => &[wgpu::Backends::DX12, wgpu::Backends::PRIMARY],
            Backend::Dx12 => &[wgpu::Backends::DX12],
            Backend::Vulkan => &[wgpu::Backends::VULKAN],
        };
        let power_preference = if options.low_power {
            wgpu::PowerPreference::LowPower
        } else {
            wgpu::PowerPreference::HighPerformance
        };

        let mut last_err = None;
        for &backends in attempts {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference,
                ..Default::default()
            })) {
                Ok(adapter) => return Self::with_adapter(instance, adapter),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.map_or(Error::NoBackend, Error::NoAdapter))
    }

    fn with_adapter(instance: wgpu::Instance, adapter: wgpu::Adapter) -> Result<Self> {
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("lumen"),
                ..Default::default()
            }))?;

        // Device loss (driver update, TDR, sleep on some drivers) must be recoverable (P11).
        let lost = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&lost);
        device.set_device_lost_callback(move |reason, message| {
            tracing::error!(?reason, %message, "GPU device lost");
            flag.store(true, Ordering::SeqCst);
        });

        // Validation errors would otherwise panic; content-driven pipelines can trigger them.
        device.on_uncaptured_error(Arc::new(|e| tracing::error!("wgpu: {e:?}")));

        let info = adapter.get_info();
        tracing::info!(adapter = %info.name, backend = ?info.backend, "GPU ready");
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            lost,
        })
    }

    /// True once the device is lost; the caller must drop and rebuild the `Gpu`
    /// and everything created from it. Call [`Gpu::maintain`] periodically when
    /// idle: wgpu only notices loss while polling or submitting.
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    /// Non-blocking device poll: runs completion callbacks and surfaces device loss.
    pub fn maintain(&self) {
        if let Err(e) = self.device.poll(wgpu::PollType::Poll) {
            tracing::warn!("device poll failed: {e}");
        }
    }

    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }

    /// Creates a presentation surface for a native window.
    ///
    /// # Safety
    /// `window` must stay valid until the returned [`OutputSurface`] is dropped.
    pub unsafe fn create_surface(
        &self,
        window: RawWindowHandle,
        display: RawDisplayHandle,
        width: u32,
        height: u32,
    ) -> Result<OutputSurface> {
        // SAFETY: forwarded caller contract — the window outlives the surface.
        let surface = unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(display),
                    raw_window_handle: window,
                })?
        };
        let caps = surface.get_capabilities(&self.adapter);
        let format = pick_format(&caps.formats).ok_or(Error::NoSurfaceFormat)?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: pick_present_mode(&caps.present_modes),
            // Fewest buffers in flight: lower latency and memory (P3).
            desired_maximum_frame_latency: 1,
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: Vec::new(),
            ..surface
                .get_default_config(&self.adapter, width.max(1), height.max(1))
                .ok_or(Error::NoSurfaceFormat)?
        };
        surface.configure(&self.device, &config);
        tracing::debug!(present_mode = ?config.present_mode, modes = ?caps.present_modes, "surface configured");
        Ok(OutputSurface { surface, config })
    }
}

/// Mailbox when available: vsynced without tearing, and a present never
/// waits for that monitor's vblank, so outputs with different refresh rates
/// cannot stall each other through the shared render loop (P7). FIFO
/// otherwise (still vsynced; only blocks when content outruns a monitor).
fn pick_present_mode(modes: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    if modes.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else {
        wgpu::PresentMode::Fifo
    }
}

/// Non-sRGB first (gamma-space rendering, matching WE), BGRA preferred for flip-model swapchains.
fn pick_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    use wgpu::TextureFormat::{Bgra8Unorm, Rgba8Unorm};
    [Bgra8Unorm, Rgba8Unorm]
        .into_iter()
        .find(|f| formats.contains(f))
        .or_else(|| formats.iter().copied().find(|f| !f.is_srgb()))
        .or_else(|| formats.first().copied())
}

/// One output's swapchain.
#[derive(Debug)]
pub struct OutputSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

impl OutputSurface {
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize(&mut self, gpu: &Gpu, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&gpu.device, &self.config);
    }

    /// Next frame to draw into. `Ok(None)` means skip this frame (occluded or timed out).
    pub fn acquire(&mut self, gpu: &Gpu) -> Result<Option<wgpu::SurfaceTexture>> {
        for _ in 0..2 {
            match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t) => return Ok(Some(t)),
                wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                    // Present this one; reconfigure before the next frame.
                    self.surface.configure(&gpu.device, &self.config);
                    return Ok(Some(t));
                }
                wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => {
                    return Ok(None);
                }
                wgpu::CurrentSurfaceTexture::Outdated
                | wgpu::CurrentSurfaceTexture::Lost
                | wgpu::CurrentSurfaceTexture::Validation => {
                    self.surface.configure(&gpu.device, &self.config);
                }
            }
        }
        Err(Error::SurfaceLost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::TextureFormat::*;

    #[test]
    fn prefers_mailbox_then_fifo() {
        use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
        assert_eq!(pick_present_mode(&[Fifo, Mailbox, Immediate]), Mailbox);
        assert_eq!(pick_present_mode(&[Fifo, Immediate]), Fifo);
    }

    #[test]
    fn prefers_non_srgb_bgra() {
        assert_eq!(pick_format(&[Bgra8UnormSrgb, Bgra8Unorm]), Some(Bgra8Unorm));
        assert_eq!(pick_format(&[Rgba8UnormSrgb, Rgba8Unorm]), Some(Rgba8Unorm));
        assert_eq!(pick_format(&[Rgba16Float]), Some(Rgba16Float));
        assert_eq!(pick_format(&[Bgra8UnormSrgb]), Some(Bgra8UnormSrgb));
        assert_eq!(pick_format(&[]), None);
    }
}
