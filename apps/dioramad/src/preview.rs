//! `dioramad --preview <dir>`: draws a wallpaper off-screen for the editor
//! and writes its frames to standard output (ADR-009, `diorama_ipc::preview`).
//! The document may come from the editor, unsaved. Ends when standard input
//! does.

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use diorama_core::Limits;
use diorama_gpu::shader::ShaderCompiler;
use diorama_gpu::{Gpu, GpuOptions, wgpu};
use diorama_ipc::preview::{Command, Event, LayerBox, MAX_SIDE, write_event, write_frame};
use diorama_project::{Compiled, Folder, LayerInfo};
use diorama_render::scene::{FrameInputs, SceneRenderer};
use diorama_render::{FitMode, ImageBlit, Rect, read_rgba8, render_size};
use diorama_scene::{Scene, VideoData};

use crate::headless::{ProjectHost, load_scene, shader_compiler};
use crate::scripts::{SceneScripts, ScriptFrame, day_fraction_at, now_ms};
use crate::videos::SceneVideos;

/// Until the UI says otherwise.
const SIZE: [u32; 2] = [960, 540];
const FPS: u32 = 30;

pub fn run(
    dir: &Path,
    assets: Option<&Path>,
    we_config: Option<&Path>,
    gpu_options: GpuOptions,
) -> Result<()> {
    let commands = commands();
    let mut out = std::io::stdout().lock();
    let result = draw(dir, assets, we_config, gpu_options, &commands, &mut out);
    if let Err(e) = &result {
        // The UI may be gone; its end is no news to it.
        let message = format!("{e:#}");
        let _ = write_event(&mut out, &Event::Error { message });
    }
    result
}

/// Standard input's commands; the channel closes with it.
fn commands() -> Receiver<Command> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { return };
            match serde_json::from_str(&line) {
                Ok(command) => {
                    if tx.send(command).is_err() {
                        return;
                    }
                }
                Err(e) => tracing::warn!("preview command: {e}"),
            }
        }
    });
    rx
}

struct View {
    size: [u32; 2],
    fps: u32,
    pointer: [f32; 2],
}

impl View {
    /// Applies `command`; true if frames get another size.
    fn apply(&mut self, command: &Command) -> bool {
        self.fps = command.fps.unwrap_or(self.fps).min(240);
        self.pointer = command.pointer.unwrap_or(self.pointer);
        let size = command
            .size
            .unwrap_or(self.size)
            .map(|side| side.clamp(1, MAX_SIDE));
        std::mem::replace(&mut self.size, size) != size
    }

    /// Time between frames; `None` when none are wanted.
    fn interval(&self) -> Option<Duration> {
        (self.fps > 0).then(|| Duration::from_secs_f64(1.0 / f64::from(self.fps)))
    }
}

fn draw(
    dir: &Path,
    assets: Option<&Path>,
    we_config: Option<&Path>,
    gpu_options: GpuOptions,
    commands: &Receiver<Command>,
    out: &mut impl Write,
) -> Result<()> {
    let choices = crate::properties::choices(we_config, dir, &Default::default());
    let project = diorama_project::is_project(dir);
    let folder = Folder::new(dir);
    let host = ProjectHost::default();
    let limits = Limits::default();
    let mut cache = diorama_project::Cache::default();
    let mut compile = |document: &serde_json::Value| {
        diorama_project::compile_document(document, &folder, &choices, &host, &limits, &mut cache)
    };
    let mut ids = Vec::new();
    let mut picture = if project {
        let (document, _) = diorama_project::edit::open(dir)?;
        ids = layer_ids(&document);
        compile(&document)?
    } else {
        Compiled {
            scene: load_scene(dir, assets, &choices)?,
            layers: Vec::new(),
        }
    };
    let gpu = Gpu::new(gpu_options)?;
    let compiler = shader_compiler(&picture.scene)?;
    let mut view = View {
        size: SIZE,
        fps: FPS,
        pointer: [0.5, 0.5],
    };
    let mut stage = Stage::new(&gpu, &picture.scene, &compiler, view.size, None)?;
    let mut scripts = SceneScripts::new(&picture.scene, now_ms(), false, None, None);
    let mut videos = SceneVideos::start(&picture.scene);
    let started = Instant::now();
    let mut next = started;
    report(out, &picture, &ids, &stage)?;
    loop {
        // Commands until the next frame is due (or any, with none wanted).
        let due = view.interval().map(|_| next);
        let wait = due.map_or(Duration::from_secs(1), |d| {
            d.saturating_duration_since(Instant::now())
        });
        match commands.recv_timeout(wait) {
            Ok(first) => {
                // While dragging, documents come faster than they are drawn:
                // only the newest counts.
                let mut resized = false;
                let mut document = None;
                for command in std::iter::once(first).chain(commands.try_iter()) {
                    resized |= view.apply(&command);
                    document = command.document.or(document);
                }
                let document = document.filter(|_| project);
                match document.as_ref().map(&mut compile) {
                    Some(Ok(new)) => {
                        let reuse = Some((&stage, &picture.scene));
                        let made = Stage::new(&gpu, &new.scene, &compiler, view.size, reuse)?;
                        if video_files(&new.scene) != video_files(&picture.scene) {
                            videos = SceneVideos::start(&new.scene);
                        }
                        (stage, picture) = (made, new);
                        ids = document.as_ref().map(layer_ids).unwrap_or_default();
                        report(out, &picture, &ids, &stage)?;
                    }
                    Some(Err(e)) => {
                        let event = Event::Document {
                            width: picture.scene.width,
                            height: picture.scene.height,
                            layers: Vec::new(),
                            problems: Vec::new(),
                            error: Some(e.to_string()),
                        };
                        write_event(out, &event).context("writing to the UI")?;
                    }
                    None if resized => {
                        let reuse = Some((&stage, &picture.scene));
                        stage = Stage::new(&gpu, &picture.scene, &compiler, view.size, reuse)?;
                    }
                    None => {}
                }
                // Commands that keep coming must not keep frames from going.
                if view.interval().is_none() || Instant::now() < next {
                    continue;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => {}
        }
        let Some(interval) = view.interval() else {
            continue;
        };
        // Late frames are dropped, not caught up with.
        next = (next + interval).max(Instant::now());
        let time = started.elapsed().as_secs_f32();
        let clock_ms = now_ms();
        if let Some(scripts) = &mut scripts {
            let frame = ScriptFrame {
                time,
                clock_ms,
                pointer: view.pointer,
                buttons: None,
                audio: [[0.0; 64]; 2],
            };
            scripts.frame(&gpu, &mut stage.renderer, None, &frame);
        }
        if let Some(videos) = &mut videos {
            videos.frame(&gpu, &stage.renderer, time);
        }
        let inputs = FrameInputs {
            time,
            pointer: view.pointer,
            daytime: day_fraction_at(clock_ms),
            audio: [[0.0; 64]; 2],
        };
        if scripts.is_none() && videos.is_none() && stage.renderer.idle(&inputs) {
            continue;
        }
        stage.frame(&gpu, &inputs);
        // Waiting for the GPU shows the frame soonest; nothing else is to do.
        let rgba = read_rgba8(&gpu, &stage.target);
        anyhow::ensure!(!gpu.is_lost(), "the GPU was reset");
        let rgba = rgba.context("reading back the frame")?;
        let [w, h] = view.size;
        write_frame(out, w, h, &rgba).context("writing to the UI")?;
    }
}

/// The videos of `scene`'s textures, which go on playing while they stay.
fn video_files(scene: &Scene) -> Vec<(usize, &VideoData)> {
    let textures = scene.textures.iter().enumerate();
    textures
        .filter_map(|(i, t)| Some((i, t.video.as_ref()?)))
        .collect()
}

/// The ids of a document's layers.
fn layer_ids(document: &serde_json::Value) -> Vec<String> {
    let layers = document["layers"].as_array().into_iter().flatten();
    layers
        .map(|l| l["id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// Tells the UI where the layers are and what is wrong with them.
fn report(out: &mut impl Write, picture: &Compiled, ids: &[String], stage: &Stage) -> Result<()> {
    let boxes = picture.layers.iter().enumerate();
    let event = Event::Document {
        width: picture.scene.width,
        height: picture.scene.height,
        layers: boxes
            .filter_map(|(i, info)| {
                let id = ids.get(info.document)?;
                layer_box(id, info, stage.renderer.text_box(i))
            })
            .collect(),
        problems: picture.scene.skipped.clone(),
        error: None,
    };
    write_event(out, &event).context("writing to the UI")
}

/// Where a layer is around its position; `text`: as it was laid out.
fn layer_box(id: &str, info: &LayerInfo, text: Option<([u32; 2], [f32; 2])>) -> Option<LayerBox> {
    let (size, [x, y]) = match (info.size, text) {
        (Some(size), _) => (size, [0.0, 0.0]),
        (None, Some((size, [x, y]))) => {
            let inner = [0, 1].map(|i| (size[i] as f32 - 2.0 * info.padding[i]).max(1.0));
            // Scenes count y up, documents down.
            (inner, [x, -y])
        }
        (None, None) => return None,
    };
    Some(LayerBox {
        id: id.to_owned(),
        x,
        y,
        width: size[0],
        height: size[1],
    })
}

/// The scene and the picture of it that goes to the UI.
struct Stage {
    renderer: SceneRenderer,
    blit: ImageBlit,
    target: wgpu::Texture,
    clear: [f32; 4],
}

impl Stage {
    /// `reuse`: the stage before, and its scene.
    fn new(
        gpu: &Gpu,
        scene: &Scene,
        compiler: &ShaderCompiler,
        size: [u32; 2],
        reuse: Option<(&Self, &Scene)>,
    ) -> Result<Self> {
        let [w, h] = size;
        let max = gpu.device.limits().max_texture_dimension_2d;
        let fit = FitMode::Fit;
        let (rw, rh) = render_size(fit, (scene.width, scene.height), &[(w, h)], max);
        let output = Some([rw, rh]);
        let renderer = match reuse {
            Some((stage, before)) => stage
                .renderer
                .rebuilt(before, gpu, scene, compiler, output)?,
            None => SceneRenderer::new(gpu, scene, compiler, output)?,
        };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("preview frame"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let [sw, sh] = renderer.size();
        let blit = ImageBlit::from_view(gpu, format, renderer.output(), sw, sh)
            .with_fit_size((scene.width, scene.height));
        let [r, g, b] = scene.scheme_color;
        Ok(Self {
            renderer,
            blit,
            target,
            clear: [r, g, b, 1.0],
        })
    }

    fn frame(&mut self, gpu: &Gpu, inputs: &FrameInputs) {
        self.renderer.render(gpu, inputs);
        let rect = Rect {
            x: 0,
            y: 0,
            width: self.target.width(),
            height: self.target.height(),
        };
        let view = self
            .target
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.blit
            .draw(gpu, &view, rect, rect, FitMode::Fit, self.clear);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_change_only_what_they_name() {
        let mut view = View {
            size: SIZE,
            fps: FPS,
            pointer: [0.5, 0.5],
        };
        assert!(!view.apply(&Command::default()));
        assert!(!view.apply(&Command {
            pointer: Some([0.1, 0.2]),
            fps: Some(0),
            ..Command::default()
        }));
        assert_eq!((view.pointer, view.interval()), ([0.1, 0.2], None));
        assert!(view.apply(&Command {
            size: Some([0, 99_999]),
            fps: Some(50),
            ..Command::default()
        }));
        assert_eq!(view.size, [1, MAX_SIDE]);
        assert_eq!(view.interval(), Some(Duration::from_millis(20)));
    }

    #[test]
    fn boxes_leave_out_padding() {
        let info = LayerInfo {
            document: 3,
            size: None,
            padding: [8.0, 10.0],
        };
        let b = layer_box("words", &info, Some(([116, 60], [5.0, 7.0]))).unwrap();
        assert_eq!((b.id.as_str(), b.width, b.height), ("words", 100.0, 40.0));
        assert_eq!((b.x, b.y), (5.0, -7.0));
        assert_eq!(layer_box("words", &info, None), None);
    }
}
