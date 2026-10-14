//! Windows, surfaces and GPU resources for the current desktop + wallpaper.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lumen_gpu::{Gpu, OutputSurface};
use lumen_ipc::{Fit, Layout, MediaKind};
use lumen_media::{Nv12Frame, VideoPlayer};
use lumen_platform::{Coverage, OutputInfo, Platform, WallpaperWindow};
use lumen_render::{FitMode, ImageBlit, Nv12Planes, Rect, VideoBlit, display_size};

/// How the picture is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub fit: Fit,
    pub layout: Layout,
}

impl Look {
    fn fit_mode(self) -> FitMode {
        match self.fit {
            Fit::Fill => FitMode::Fill,
            Fit::Fit => FitMode::Fit,
            Fit::Stretch => FitMode::Stretch,
        }
    }
}

/// Decoded content; lives across desktop rebuilds.
pub enum Content {
    Image(image::RgbaImage),
    Video(VideoPlayer),
}

/// The wallpaper the user asked for.
pub struct Wallpaper {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub look: Look,
    pub content: Content,
}

impl Wallpaper {
    pub fn load(path: &Path, kind: Option<MediaKind>, look: Look) -> Result<Self> {
        let kind = kind
            .or_else(|| MediaKind::from_path(path))
            .with_context(|| format!("unknown media type for {}", path.display()))?;
        let content = match kind {
            MediaKind::Image => Content::Image(
                image::open(path)
                    .with_context(|| format!("loading {}", path.display()))?
                    .into_rgba8(),
            ),
            MediaKind::Video => Content::Video(
                VideoPlayer::spawn(path).with_context(|| format!("opening {}", path.display()))?,
            ),
        };
        Ok(Self {
            path: path.to_path_buf(),
            kind,
            look,
            content,
        })
    }
}

enum Renderer {
    Image(ImageBlit),
    Video(VideoBlit),
}

/// One monitor's wallpaper. Field order matters: the surface must drop before its window.
pub struct Output {
    pub info: OutputInfo,
    /// Not receiving new frames (covered, locked, …); the last frame stays on screen.
    pub paused: bool,
    pub coverage: Coverage,
    surface: OutputSurface,
    _window: WallpaperWindow,
}

pub struct Session {
    pub outputs: Vec<Output>,
    canvas: Rect,
    renderer: Renderer,
    look: Look,
}

pub fn rect_of(o: &OutputInfo) -> Rect {
    Rect {
        x: o.x,
        y: o.y,
        width: o.width,
        height: o.height,
    }
}

impl Session {
    pub fn build(platform: &Platform, gpu: &Gpu, wallpaper: &Wallpaper) -> Result<Self> {
        let look = wallpaper.look;
        let infos = platform.outputs().context("enumerating outputs")?;
        let canvas = Rect::bounding(infos.iter().map(rect_of)).context("no outputs found")?;

        let mut outputs = Vec::new();
        for info in infos {
            let window = platform
                .create_wallpaper(&info)
                .with_context(|| format!("wallpaper window for {}", info.device))?;
            let handles = window.handles();
            // SAFETY: `window` is stored next to the surface in `Output` and dropped after it.
            let surface = unsafe {
                gpu.create_surface(handles.window, handles.display, info.width, info.height)
            }
            .with_context(|| format!("GPU surface for {}", info.device))?;
            outputs.push(Output {
                info,
                paused: false,
                coverage: Coverage::Visible,
                surface,
                _window: window,
            });
        }
        let format = outputs[0].surface.format();

        let renderer = match &wallpaper.content {
            Content::Image(image) => {
                // Downscale once to the largest size the image is ever displayed at (sharp, less VRAM).
                let sizes: Vec<(u32, u32)> = match look.layout {
                    Layout::Span => vec![canvas.size()],
                    Layout::PerOutput => outputs
                        .iter()
                        .map(|o| (o.info.width, o.info.height))
                        .collect(),
                };
                let (w, h) = display_size(look.fit_mode(), image.dimensions(), &sizes);
                let pixels = if (w, h) == image.dimensions() {
                    image.clone()
                } else {
                    image::imageops::resize(image, w, h, image::imageops::FilterType::Lanczos3)
                };
                Renderer::Image(ImageBlit::new(gpu, format, pixels.as_raw(), w, h))
            }
            Content::Video(player) => {
                let info = player.info();
                Renderer::Video(VideoBlit::new(gpu, format, info.width, info.height))
            }
        };

        let mut session = Self {
            outputs,
            canvas,
            renderer,
            look,
        };
        if matches!(session.renderer, Renderer::Image(_)) {
            session.present_all(gpu)?;
        }
        tracing::info!(
            outputs = session.outputs.len(),
            path = %wallpaper.path.display(),
            layout = ?look.layout,
            "wallpaper shown"
        );
        Ok(session)
    }

    pub fn show_video_frame(&mut self, gpu: &Gpu, frame: &Nv12Frame) -> Result<()> {
        if let Renderer::Video(blit) = &self.renderer {
            blit.upload(
                gpu,
                Nv12Planes {
                    width: frame.width,
                    height: frame.height,
                    stride: frame.stride,
                    luma: frame.luma(),
                    chroma: frame.chroma(),
                },
            );
        }
        self.present_all(gpu)
    }

    /// Draws the current content on every output that is not paused.
    pub fn present_all(&mut self, gpu: &Gpu) -> Result<()> {
        let fit = self.look.fit_mode();
        for out in self.outputs.iter_mut().filter(|o| !o.paused) {
            let target = rect_of(&out.info);
            let canvas = match self.look.layout {
                Layout::Span => self.canvas,
                Layout::PerOutput => target,
            };
            let Some(frame) = out.surface.acquire(gpu)? else {
                tracing::debug!(output = %out.info.device, "frame skipped (occluded)");
                continue;
            };
            let view = frame
                .texture
                .create_view(&lumen_gpu::wgpu::TextureViewDescriptor::default());
            let clear = [0.0, 0.0, 0.0, 1.0];
            match &self.renderer {
                Renderer::Image(blit) => blit.draw(gpu, &view, canvas, target, fit, clear),
                Renderer::Video(blit) => blit.draw(gpu, &view, canvas, target, fit, clear),
            }
            gpu.queue.present(frame);
        }
        Ok(())
    }
}
