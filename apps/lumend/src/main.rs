//! `lumend` — Lumen wallpaper daemon (Phase 1).
//!
//! Shows an image or a looping video on every monitor behind the desktop
//! icons. Static images render once (≈0% GPU when idle, P2); video frames are
//! presented on their timestamps. Windows and surfaces are rebuilt when
//! Explorer restarts (P9), monitors change (P8) or the GPU device is lost (P11).
//! Video never opens an audio device (P10).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use lumen_gpu::{Backend, Gpu, GpuOptions, OutputSurface};
use lumen_media::{Nv12Frame, VideoPlayer};
use lumen_platform::{OutputInfo, Platform, WallpaperWindow};
use lumen_render::{FitMode, ImageBlit, Nv12Planes, Rect, VideoBlit, display_size};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Image to show (PNG, JPEG, …).
    #[arg(long, conflicts_with = "video", required_unless_present_any = ["video", "list_outputs"])]
    image: Option<PathBuf>,
    /// Video to loop (MP4, …). Muted.
    #[arg(long)]
    video: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "fill")]
    fit: FitArg,
    /// `per-output`: the whole picture on every monitor. `span`: one picture across all monitors.
    #[arg(long, value_enum, default_value = "per-output")]
    layout: LayoutArg,
    #[arg(long, value_enum, default_value = "auto")]
    backend: BackendArg,
    /// Prefer an integrated GPU.
    #[arg(long)]
    low_power: bool,
    /// Exit after this many seconds (for testing).
    #[arg(long)]
    duration: Option<u64>,
    /// Print detected outputs and exit.
    #[arg(long)]
    list_outputs: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum FitArg {
    Fill,
    Fit,
    Stretch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LayoutArg {
    PerOutput,
    Span,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BackendArg {
    Auto,
    Dx12,
    Vulkan,
}

/// How the picture is placed: fit mode plus per-output vs span layout.
#[derive(Debug, Clone, Copy)]
struct Look {
    fit: FitMode,
    layout: LayoutArg,
}

/// What is being shown. Lives across desktop rebuilds.
enum Content {
    Image(image::RgbaImage),
    Video(VideoPlayer),
}

/// GPU-side renderer for the content; recreated on every rebuild.
enum Renderer {
    Image(ImageBlit),
    Video(VideoBlit),
}

/// One monitor's wallpaper. Field order matters: the surface must drop before its window.
struct Output {
    info: OutputInfo,
    surface: OutputSurface,
    _window: WallpaperWindow,
}

/// Windows, surfaces and GPU resources for the current desktop.
struct Session {
    outputs: Vec<Output>,
    canvas: Rect,
    renderer: Renderer,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    let platform = Platform::new().context("platform init")?;

    if args.list_outputs {
        for o in platform.outputs()? {
            println!(
                "{}{} {}×{} at ({}, {}) {:.2} Hz {} {} [{}]",
                if o.primary { "* " } else { "  " },
                o.device,
                o.width,
                o.height,
                o.x,
                o.y,
                f64::from(o.refresh_mhz) / 1000.0,
                o.manufacturer,
                o.name,
                o.id.0
            );
        }
        return Ok(());
    }

    let content = match (&args.image, &args.video) {
        (Some(path), _) => Content::Image(
            image::open(path)
                .with_context(|| format!("loading {}", path.display()))?
                .into_rgba8(),
        ),
        (None, Some(path)) => Content::Video(
            VideoPlayer::spawn(path).with_context(|| format!("opening {}", path.display()))?,
        ),
        (None, None) => anyhow::bail!("--image or --video is required"),
    };
    let look = Look {
        fit: match args.fit {
            FitArg::Fill => FitMode::Fill,
            FitArg::Fit => FitMode::Fit,
            FitArg::Stretch => FitMode::Stretch,
        },
        layout: args.layout,
    };
    let gpu_options = GpuOptions {
        backend: match args.backend {
            BackendArg::Auto => Backend::Auto,
            BackendArg::Dx12 => Backend::Dx12,
            BackendArg::Vulkan => Backend::Vulkan,
        },
        low_power: args.low_power,
    };

    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst)).context("Ctrl+C handler")?;
    let deadline = args
        .duration
        .map(|s| Instant::now() + Duration::from_secs(s));

    let mut gpu = Gpu::new(gpu_options).context("GPU init")?;
    let mut session = Some(build(&platform, &gpu, &content, look)?);
    let mut clock = VideoClock::default();

    while !stop.load(Ordering::SeqCst) && deadline.is_none_or(|d| Instant::now() < d) {
        // Static content: sleep until an event. Video: until the next frame is due.
        let wait = match &content {
            Content::Image(_) => Duration::from_secs(1),
            Content::Video(player) => clock.time_until_next(player),
        };
        let events = platform.poll_events(wait);
        gpu.maintain();

        let device_lost = gpu.is_lost();
        if !events.is_empty() || device_lost {
            tracing::info!(?events, device_lost, "rebuilding wallpaper");
            // Surfaces/windows go first; they reference the old device and old Explorer windows.
            drop(session.take());
            if device_lost {
                gpu = Gpu::new(gpu_options).context("GPU re-init after device loss")?;
            }
            // Explorer may still be starting after a restart; retry briefly.
            session = Some(retry(
                || build(&platform, &gpu, &content, look),
                10,
                Duration::from_millis(500),
            )?);
            if let (Content::Video(_), Some(s)) = (&content, session.as_mut()) {
                // New textures are empty: show the most recent frame again.
                if let Some(frame) = clock.current.as_ref() {
                    show_video_frame(&gpu, s, frame, look)?;
                }
            }
            continue;
        }

        if let (Content::Video(player), Some(s)) = (&content, session.as_mut())
            && let Some(frame) = clock.due_frame(player)
        {
            show_video_frame(&gpu, s, frame, look)?;
        }
    }

    drop(session);
    tracing::info!("exiting");
    Ok(())
}

/// Maps frame timestamps onto wall-clock time.
#[derive(Default)]
struct VideoClock {
    start: Option<Instant>,
    /// Decoded but not yet due.
    pending: Option<Nv12Frame>,
    /// Last frame shown (re-uploaded after a rebuild).
    current: Option<Nv12Frame>,
}

impl VideoClock {
    fn now_pts(&mut self) -> Duration {
        self.start.get_or_insert_with(Instant::now).elapsed()
    }

    fn time_until_next(&mut self, player: &VideoPlayer) -> Duration {
        if self.pending.is_none() {
            self.pending = player.try_next();
        }
        let now = self.now_pts();
        match &self.pending {
            Some(f) => f.pts.saturating_sub(now),
            // Decoder is behind; check again shortly.
            None => Duration::from_millis(5),
        }
    }

    /// The newest frame whose time has come, dropping late ones.
    fn due_frame(&mut self, player: &VideoPlayer) -> Option<&Nv12Frame> {
        let now = self.now_pts();
        let mut due = None;
        loop {
            if self.pending.is_none() {
                self.pending = player.try_next();
            }
            match self.pending.take() {
                Some(f) if f.pts <= now => due = Some(f),
                other => {
                    self.pending = other;
                    break;
                }
            }
        }
        if let Some(f) = due {
            self.current = Some(f);
            return self.current.as_ref();
        }
        None
    }
}

fn build(platform: &Platform, gpu: &Gpu, content: &Content, look: Look) -> Result<Session> {
    let infos = platform.outputs().context("enumerating outputs")?;
    let canvas = Rect::bounding(infos.iter().map(rect_of)).context("no outputs found")?;

    let mut outputs = Vec::new();
    for info in infos {
        let window = platform
            .create_wallpaper(&info)
            .with_context(|| format!("wallpaper window for {}", info.device))?;
        let handles = window.handles();
        // SAFETY: `window` is stored next to the surface in `Output` and dropped after it.
        let surface =
            unsafe { gpu.create_surface(handles.window, handles.display, info.width, info.height) }
                .with_context(|| format!("GPU surface for {}", info.device))?;
        outputs.push(Output {
            info,
            surface,
            _window: window,
        });
    }
    let format = outputs[0].surface.format();

    let renderer = match content {
        Content::Image(image) => {
            // Downscale once to the largest size the image is ever displayed at (sharp, less VRAM).
            let sizes: Vec<(u32, u32)> = match look.layout {
                LayoutArg::Span => vec![canvas.size()],
                LayoutArg::PerOutput => outputs
                    .iter()
                    .map(|o| (o.info.width, o.info.height))
                    .collect(),
            };
            let (w, h) = display_size(look.fit, image.dimensions(), &sizes);
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

    let mut session = Session {
        outputs,
        canvas,
        renderer,
    };
    if matches!(session.renderer, Renderer::Image(_)) {
        present_all(gpu, &mut session, look)?;
    }
    tracing::info!(
        outputs = session.outputs.len(),
        layout = ?look.layout,
        "wallpaper shown on {}",
        session
            .outputs
            .iter()
            .map(|o| o.info.device.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(session)
}

fn show_video_frame(gpu: &Gpu, session: &mut Session, frame: &Nv12Frame, look: Look) -> Result<()> {
    if let Renderer::Video(blit) = &session.renderer {
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
    present_all(gpu, session, look)
}

fn present_all(gpu: &Gpu, session: &mut Session, look: Look) -> Result<()> {
    for out in &mut session.outputs {
        let target = rect_of(&out.info);
        let canvas = match look.layout {
            LayoutArg::Span => session.canvas,
            LayoutArg::PerOutput => target,
        };
        let Some(frame) = out.surface.acquire(gpu)? else {
            tracing::debug!(output = %out.info.device, "frame skipped (occluded)");
            continue;
        };
        let view = frame
            .texture
            .create_view(&lumen_gpu::wgpu::TextureViewDescriptor::default());
        let clear = [0.0, 0.0, 0.0, 1.0];
        match &session.renderer {
            Renderer::Image(blit) => blit.draw(gpu, &view, canvas, target, look.fit, clear),
            Renderer::Video(blit) => blit.draw(gpu, &view, canvas, target, look.fit, clear),
        }
        gpu.queue.present(frame);
    }
    Ok(())
}

fn rect_of(o: &OutputInfo) -> Rect {
    Rect {
        x: o.x,
        y: o.y,
        width: o.width,
        height: o.height,
    }
}

fn retry<T>(mut f: impl FnMut() -> Result<T>, attempts: u32, delay: Duration) -> Result<T> {
    let mut last = None;
    for _ in 0..attempts {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => {
                tracing::warn!("{e:#}; retrying");
                last = Some(e);
                std::thread::sleep(delay);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("no attempts made")))
}
