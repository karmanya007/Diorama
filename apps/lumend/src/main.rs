//! `lumend` — Lumen wallpaper daemon (Phase 1 walking skeleton).
//!
//! Shows a static image on every monitor behind the desktop icons. Renders only
//! when something changes (static content → ~0% GPU when idle, P2) and rebuilds
//! its windows/surfaces when Explorer restarts (P9), monitors change (P8), or
//! the GPU device is lost (P11).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use lumen_gpu::{Backend, Gpu, GpuOptions, OutputSurface};
use lumen_platform::{OutputInfo, Platform, WallpaperWindow};
use lumen_render::{FitMode, ImageBlit, Rect, display_size};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Image to show (PNG, JPEG, …).
    #[arg(long, required_unless_present = "list_outputs")]
    image: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "fill")]
    fit: FitArg,
    /// `per-output`: the whole image on every monitor. `span`: one image across all monitors.
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

/// How the image is placed: fit mode plus per-output vs span layout.
#[derive(Debug, Clone, Copy)]
struct Look {
    fit: FitMode,
    layout: LayoutArg,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BackendArg {
    Auto,
    Dx12,
    Vulkan,
}

/// One monitor's wallpaper. Field order matters: the surface must drop before its window.
struct Output {
    info: OutputInfo,
    surface: OutputSurface,
    _window: WallpaperWindow,
}

/// Keeps windows, surfaces and GPU resources alive while the wallpaper is shown.
struct Session {
    _outputs: Vec<Output>,
    _blit: ImageBlit,
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

    let image_path = args.image.context("--image is required")?;
    let image = image::open(&image_path)
        .with_context(|| format!("loading {}", image_path.display()))?
        .into_rgba8();
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
    let mut session = Some(build(&platform, &gpu, &image, look)?);

    while !stop.load(Ordering::SeqCst) && deadline.is_none_or(|d| Instant::now() < d) {
        let events = platform.poll_events(Duration::from_millis(250));
        let device_lost = gpu.is_lost();
        if events.is_empty() && !device_lost {
            continue;
        }
        tracing::info!(?events, device_lost, "rebuilding wallpaper");
        // Surfaces/windows go first; they reference the old device and old Explorer windows.
        drop(session.take());
        if device_lost {
            gpu = Gpu::new(gpu_options).context("GPU re-init after device loss")?;
        }
        // Explorer may still be starting after a restart; retry briefly.
        session = Some(retry(
            || build(&platform, &gpu, &image, look),
            10,
            Duration::from_millis(500),
        )?);
    }

    drop(session);
    tracing::info!("exiting");
    Ok(())
}

fn build(platform: &Platform, gpu: &Gpu, image: &image::RgbaImage, look: Look) -> Result<Session> {
    let infos = platform.outputs().context("enumerating outputs")?;
    let canvas = Rect::bounding(infos.iter().map(rect_of)).context("no outputs found")?;

    // Downscale once to the largest size the image is ever displayed at (sharp, less VRAM).
    let sizes: Vec<(u32, u32)> = match look.layout {
        LayoutArg::Span => vec![canvas.size()],
        LayoutArg::PerOutput => infos.iter().map(|o| (o.width, o.height)).collect(),
    };
    let (w, h) = display_size(look.fit, image.dimensions(), &sizes);
    let pixels = if (w, h) == image.dimensions() {
        image.clone()
    } else {
        image::imageops::resize(image, w, h, image::imageops::FilterType::Lanczos3)
    };

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
    let blit = ImageBlit::new(gpu, format, pixels.as_raw(), w, h);
    for out in &mut outputs {
        let target = rect_of(&out.info);
        let canvas = match look.layout {
            LayoutArg::Span => canvas,
            LayoutArg::PerOutput => target,
        };
        render(gpu, &blit, out, canvas, target, look.fit)?;
    }
    tracing::info!(
        outputs = outputs.len(),
        image = ?(w, h),
        layout = ?look.layout,
        "wallpaper shown on {}",
        outputs.iter().map(|o| o.info.device.as_str()).collect::<Vec<_>>().join(", ")
    );
    Ok(Session {
        _outputs: outputs,
        _blit: blit,
    })
}

fn rect_of(o: &OutputInfo) -> Rect {
    Rect {
        x: o.x,
        y: o.y,
        width: o.width,
        height: o.height,
    }
}

fn render(
    gpu: &Gpu,
    blit: &ImageBlit,
    out: &mut Output,
    canvas: Rect,
    target: Rect,
    fit: FitMode,
) -> Result<()> {
    let Some(frame) = out.surface.acquire(gpu)? else {
        tracing::debug!(output = %out.info.device, "frame skipped (occluded)");
        return Ok(());
    };
    let view = frame
        .texture
        .create_view(&lumen_gpu::wgpu::TextureViewDescriptor::default());
    blit.draw(gpu, &view, canvas, target, fit, [0.0, 0.0, 0.0, 1.0]);
    gpu.queue.present(frame);
    Ok(())
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
