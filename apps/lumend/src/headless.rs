//! `lumend --scene <dir> --frame-out <png>`: render one scene frame offscreen
//! at a fixed time and save it (golden tests, RenderDoc comparisons).

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use lumen_core::Limits;
use lumen_gpu::shader::ShaderCompiler;
use lumen_gpu::wgpu;
use lumen_gpu::{Gpu, GpuOptions};
use lumen_render::scene::{FrameInputs, SceneRenderer};
use lumen_render::{FitMode, ImageBlit, Rect, read_rgba8, render_size};

/// Default `<WE install>/assets` (built-in shaders, effects, `util/*`).
pub const DEFAULT_WE_ASSETS: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine\assets";

pub fn shader_compiler() -> Result<ShaderCompiler> {
    let cache = lumen_platform::cache_dir().map(|d| d.join("shaders"));
    ShaderCompiler::locate(cache).context("locating the HLSL compiler")
}

/// Imports layers `scene`'s scripts create at runtime; `None` when no
/// script calls `createLayer` (the factory holds the package in memory).
pub fn layer_factory(
    dir: &Path,
    assets: Option<&Path>,
    choices: &crate::properties::Values,
    scene: &lumen_scene::Scene,
) -> Option<lumen_we::scene::LayerFactory> {
    if !scene
        .scripts
        .iter()
        .any(|s| s.source.contains("createLayer"))
    {
        return None;
    }
    lumen_we::scene::LayerFactory::new(dir, assets, &Limits::default(), choices, scene)
        .inspect_err(|e| tracing::warn!("scripts cannot create layers: {e}"))
        .ok()
}

/// Imports a WE scene with the user's property `choices` applied.
pub fn load_scene(
    dir: &Path,
    assets: Option<&Path>,
    choices: &crate::properties::Values,
) -> Result<lumen_scene::Scene> {
    let started = Instant::now();
    if !choices.is_empty() {
        tracing::info!(
            count = choices.len(),
            "applying the wallpaper's property choices"
        );
    }
    let mut scene = lumen_we::scene::import_with_choices(dir, assets, &Limits::default(), choices)
        .with_context(|| format!("importing {}", dir.display()))?;
    // Debugging aid: `LUMEN_HIDE_LAYERS` / `LUMEN_ONLY_LAYERS` (comma-separated
    // layer indices or name fragments) drop some layers / all others.
    let matches = |var: &str, i: usize, name: &str| {
        std::env::var(var).ok().map(|v| {
            v.split(',')
                .any(|p| p.parse::<usize>().map_or(name.contains(p), |n| n == i))
        })
    };
    for (i, l) in scene.layers.iter_mut().enumerate() {
        if matches("LUMEN_HIDE_LAYERS", i, &l.name) == Some(true)
            || matches("LUMEN_ONLY_LAYERS", i, &l.name) == Some(false)
        {
            // Unbuildable rather than hidden: scripts may show hidden layers.
            l.albedo.program = lumen_scene::ProgramId(usize::MAX);
            l.particles = None;
        }
    }
    tracing::info!(
        layers = scene.layers.len(),
        programs = scene.programs.len(),
        textures = scene.textures.len(),
        ms = started.elapsed().as_millis(),
        "scene imported"
    );
    // Debugging aid: `LUMEN_DUMP_PROGRAM=<name>` prints that program's HLSL.
    if let Ok(name) = std::env::var("LUMEN_DUMP_PROGRAM") {
        for p in scene.programs.iter().filter(|p| p.name == name) {
            println!(
                "// {} vertex
{}
// fragment
{}",
                p.name, p.vertex_hlsl, p.fragment_hlsl
            );
        }
    }
    if let Some(camera) = &scene.camera {
        tracing::debug!(?camera, view = ?camera.view(&scene.nodes), "perspective camera");
    }
    for (i, l) in scene.layers.iter().enumerate() {
        for (e, effect) in l.effects.iter().enumerate() {
            for (k, pass) in effect.passes.iter().enumerate() {
                tracing::trace!(
                    layer = i,
                    effect = e,
                    pass = k,
                    name = %effect.name,
                    program = scene.programs.get(pass.program.0).map_or("?", |p| p.name.as_str()),
                    constants = ?pass.constants,
                    "effect pass"
                );
            }
        }
        tracing::debug!(
            i,
            name = %l.name,
            size = ?l.size,
            transform = ?l.transform,
            visible = l.visible,
            color = ?l.color,
            blend = ?l.blend,
            effects = ?l.effects.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            program = scene.programs.get(l.albedo.program.0).map_or("?", |p| p.name.as_str()),
            textures = ?l.albedo.textures.iter().map(|(slot, t)| match t {
                lumen_scene::TextureBinding::Asset(id) => {
                    format!("{slot}:{}", scene.textures.get(id.0).map_or("?", |t| t.name.as_str()))
                }
                other => format!("{slot}:{other:?}"),
            }).collect::<Vec<_>>(),
            attributes = ?scene.programs.get(l.albedo.program.0).map(|p| {
                p.attributes.iter().map(|a| (a.name.as_str(), a.location, a.components)).collect::<Vec<_>>()
            }),
            "layer"
        );
    }
    for s in &scene.skipped {
        tracing::warn!("not yet supported: {s}");
    }
    Ok(scene)
}

/// What `--frame-out` renders.
pub struct FrameRequest<'a> {
    pub time: f32,
    /// Wall clock for scripts, Unix milliseconds.
    pub clock_ms: f64,
    pub pointer: [f32; 2],
    pub out: &'a PathBuf,
    pub dump_targets: bool,
    /// Present into a frame of this size with this fit (else the raw scene).
    pub present: Option<([u32; 2], FitMode)>,
}

pub fn render_to_png(
    scene_dir: &Path,
    assets: Option<&Path>,
    we_config: Option<&Path>,
    gpu_options: GpuOptions,
    request: &FrameRequest<'_>,
) -> Result<()> {
    let FrameRequest {
        time,
        clock_ms,
        pointer,
        out,
        dump_targets,
        present,
    } = *request;
    let choices = crate::properties::choices(we_config, scene_dir, &Default::default());
    let scene = load_scene(scene_dir, assets, &choices)?;
    let gpu = Gpu::new(gpu_options)?;
    let compiler = shader_compiler()?;
    let started = Instant::now();
    // Like live playback: render at the size the frame needs.
    let output = present.map(|([w, h], mode)| {
        let max = gpu.device.limits().max_texture_dimension_2d;
        let (rw, rh) = render_size(mode, (scene.width, scene.height), &[(w, h)], max);
        [rw, rh]
    });
    let mut renderer = SceneRenderer::new(&gpu, &scene, &compiler, output)?;
    tracing::info!(ms = started.elapsed().as_millis(), "scene renderer built");
    let factory = layer_factory(scene_dir, assets, &choices, &scene).map(|f| (f, compiler.clone()));
    // Run scripts from the scene's start (up to a minute back) at 30 fps:
    // scripts that ease, integrate or count frames end where they would have
    // while playing (3509243656 spins its sky by 3°/s of runtime). They load
    // at the warm-up's wall clock (3129248394 times its log by `Date.now()`
    // taken at module load).
    let warm_up = 60.0f32.min(time);
    // Debugging aid: `LUMEN_FAKE_AUDIO=<0..1>` plays a synthetic beat into
    // scripts, shaders and emitters (headless frames are otherwise silent),
    // to check what a scene does with audio.
    let fake_audio = std::env::var("LUMEN_FAKE_AUDIO")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| v.clamp(0.0, 1.0));
    let audio_at = |t: f32| fake_audio.map_or([[0.0; 64]; 2], |level| fake_spectrum(level, t));
    let start_clock = clock_ms - f64::from(warm_up) * 1000.0;
    // Headless frames never read or write saved localStorage.
    if let Some(mut scripts) =
        crate::scripts::SceneScripts::new(&scene, start_clock, false, factory, None)
    {
        let frames = (warm_up * 30.0).ceil() as u32;
        for i in 0..=frames {
            let t = time - warm_up + warm_up * i as f32 / frames.max(1) as f32;
            let clock = clock_ms - f64::from(time - t) * 1000.0;
            let frame = crate::scripts::ScriptFrame {
                time: t,
                clock_ms: clock,
                pointer,
                buttons: None,
                audio: audio_at(t),
            };
            scripts.frame(&gpu, &mut renderer, None, &frame);
        }
    }
    if let Some(mut videos) = crate::videos::SceneVideos::start(&scene) {
        videos.seek(&gpu, &renderer, time);
    }
    renderer.render(
        &gpu,
        &FrameInputs {
            time,
            pointer,
            daytime: crate::scripts::day_fraction_at(clock_ms),
            audio: audio_at(time),
        },
    );
    if dump_targets {
        for (i, (label, [tw, th], rgba)) in renderer.read_targets(&gpu).into_iter().enumerate() {
            let path = out.with_extension(format!("t{i}.png"));
            image::save_buffer(&path, &rgba, tw, th, image::ExtendedColorType::Rgba8)?;
            println!("target {i} {label:?} → {}", path.display());
        }
    }
    let ([w, h], rgba) = match present {
        // The frame as a monitor of that size shows it (fit applied).
        Some(([fw, fh], mode)) => {
            let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("presented frame"),
                size: wgpu::Extent3d {
                    width: fw,
                    height: fh,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let [sw, sh] = renderer.size();
            let blit = ImageBlit::from_view(
                &gpu,
                wgpu::TextureFormat::Rgba8Unorm,
                renderer.output(),
                sw,
                sh,
            )
            .with_fit_size((scene.width, scene.height));
            let rect = Rect {
                x: 0,
                y: 0,
                width: fw,
                height: fh,
            };
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            let [r, g, b] = scene.scheme_color;
            blit.draw(&gpu, &view, rect, rect, mode, [r, g, b, 1.0]);
            let rgba = read_rgba8(&gpu, &target).context("reading back the frame")?;
            ([fw, fh], rgba)
        }
        None => (
            renderer.size(),
            renderer
                .read_output(&gpu)
                .context("reading back the frame")?,
        ),
    };
    image::save_buffer(out, &rgba, w, h, image::ExtendedColorType::Rgba8)
        .with_context(|| format!("writing {}", out.display()))?;
    println!("wrote {} ({w}×{h}, t = {time} s)", out.display());
    Ok(())
}

/// A 2 Hz bass-heavy beat at `level` (0..1) for [`render_to_png`]'s
/// `LUMEN_FAKE_AUDIO`: every band peaks on the beat, low bands loudest.
fn fake_spectrum(level: f32, time: f32) -> [[f32; 64]; 2] {
    let beat = (0.5 + 0.5 * (std::f32::consts::TAU * 2.0 * time).cos()).powi(4);
    let band = |i: usize| level * beat * (1.0 - i as f32 / 96.0);
    let bands = std::array::from_fn(band);
    [bands, bands]
}
