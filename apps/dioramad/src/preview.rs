//! `dioramad --preview <dir>`: draws a wallpaper off-screen for the editor
//! and writes its frames to standard output (ADR-009, `diorama_ipc::preview`).
//! Ends when standard input does.

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use diorama_gpu::shader::ShaderCompiler;
use diorama_gpu::{Gpu, GpuOptions, wgpu};
use diorama_ipc::preview::{Command, Event, MAX_SIDE, write_event, write_frame};
use diorama_render::scene::{FrameInputs, SceneRenderer};
use diorama_render::{FitMode, ImageBlit, Rect, read_rgba8, render_size};

use crate::headless::{load_scene, shader_compiler};
use crate::scripts::{SceneScripts, ScriptFrame, day_fraction_at, now_ms};

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
    let scene = load_scene(dir, assets, &choices)?;
    let gpu = Gpu::new(gpu_options)?;
    let compiler = shader_compiler(&scene)?;
    let mut view = View {
        size: SIZE,
        fps: FPS,
        pointer: [0.5, 0.5],
    };
    let mut stage = Stage::new(&gpu, &scene, &compiler, view.size)?;
    let mut scripts = SceneScripts::new(&scene, now_ms(), false, None, None);
    let mut videos = crate::videos::SceneVideos::start(&scene);
    let started = Instant::now();
    let mut next = started;
    let ready = Event::Ready {
        width: scene.width,
        height: scene.height,
    };
    write_event(out, &ready).context("writing to the UI")?;
    loop {
        // Commands until the next frame is due (or any, with none wanted).
        let due = view.interval().map(|_| next);
        let wait = due.map_or(Duration::from_secs(1), |d| {
            d.saturating_duration_since(Instant::now())
        });
        match commands.recv_timeout(wait) {
            Ok(command) => {
                if view.apply(&command) {
                    stage = Stage::new(&gpu, &scene, &compiler, view.size)?;
                }
                continue;
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
        stage.frame(&gpu, &inputs);
        // Waiting for the GPU shows the frame soonest; nothing else is to do.
        let rgba = read_rgba8(&gpu, &stage.target);
        anyhow::ensure!(!gpu.is_lost(), "the GPU was reset");
        let rgba = rgba.context("reading back the frame")?;
        let [w, h] = view.size;
        write_frame(out, w, h, &rgba).context("writing to the UI")?;
    }
}

/// The scene and the picture of it that goes to the UI.
struct Stage {
    renderer: SceneRenderer,
    blit: ImageBlit,
    target: wgpu::Texture,
    clear: [f32; 4],
}

impl Stage {
    fn new(
        gpu: &Gpu,
        scene: &diorama_scene::Scene,
        compiler: &ShaderCompiler,
        size: [u32; 2],
    ) -> Result<Self> {
        let [w, h] = size;
        let max = gpu.device.limits().max_texture_dimension_2d;
        let fit = FitMode::Fit;
        let (rw, rh) = render_size(fit, (scene.width, scene.height), &[(w, h)], max);
        let renderer = SceneRenderer::new(gpu, scene, compiler, Some([rw, rh]))?;
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
}
