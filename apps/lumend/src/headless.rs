//! `lumend --scene <dir> --frame-out <png>`: render one scene frame offscreen
//! at a fixed time and save it (golden tests, RenderDoc comparisons).

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use lumen_core::Limits;
use lumen_gpu::shader::ShaderCompiler;
use lumen_gpu::{Gpu, GpuOptions};
use lumen_render::scene::{FrameInputs, SceneRenderer};

/// Default `<WE install>/assets` (built-in shaders, effects, `util/*`).
pub const DEFAULT_WE_ASSETS: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine\assets";

pub fn shader_compiler() -> Result<ShaderCompiler> {
    let cache = lumen_platform::cache_dir().map(|d| d.join("shaders"));
    ShaderCompiler::locate(cache).context("locating the HLSL compiler")
}

pub fn load_scene(dir: &Path, assets: Option<&Path>) -> Result<lumen_scene::Scene> {
    let started = Instant::now();
    let scene = lumen_we::scene::import(dir, assets, &Limits::default())
        .with_context(|| format!("importing {}", dir.display()))?;
    tracing::info!(
        layers = scene.layers.len(),
        programs = scene.programs.len(),
        textures = scene.textures.len(),
        ms = started.elapsed().as_millis(),
        "scene imported"
    );
    for s in &scene.skipped {
        tracing::warn!("not yet supported: {s}");
    }
    Ok(scene)
}

pub fn render_to_png(
    scene_dir: &Path,
    assets: Option<&Path>,
    gpu_options: GpuOptions,
    time: f32,
    out: &PathBuf,
    dump_targets: bool,
) -> Result<()> {
    let scene = load_scene(scene_dir, assets)?;
    let gpu = Gpu::new(gpu_options)?;
    let compiler = shader_compiler()?;
    let started = Instant::now();
    let mut renderer = SceneRenderer::new(&gpu, &scene, &compiler)?;
    tracing::info!(ms = started.elapsed().as_millis(), "scene renderer built");
    renderer.render(
        &gpu,
        &FrameInputs {
            time,
            ..FrameInputs::default()
        },
    );
    if dump_targets {
        for (i, (label, [tw, th], rgba)) in renderer.read_targets(&gpu).into_iter().enumerate() {
            let path = out.with_extension(format!("t{i}.png"));
            image::save_buffer(&path, &rgba, tw, th, image::ExtendedColorType::Rgba8)?;
            println!("target {i} {label:?} → {}", path.display());
        }
    }
    let [w, h] = renderer.size();
    let rgba = renderer
        .read_output(&gpu)
        .context("reading back the frame")?;
    image::save_buffer(out, &rgba, w, h, image::ExtendedColorType::Rgba8)
        .with_context(|| format!("writing {}", out.display()))?;
    println!("wrote {} ({w}×{h}, t = {time} s)", out.display());
    Ok(())
}
