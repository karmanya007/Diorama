//! Drawing documents off-screen.

#![allow(clippy::unwrap_used, dead_code, reason = "test")]

use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use diorama_core::Limits;
use diorama_gpu::shader::ShaderCompiler;
use diorama_gpu::{Gpu, GpuOptions};
pub use diorama_project::Host;
use diorama_project::{Memory, Values, compile};
use diorama_render::scene::{FrameInputs, SceneRenderer};
use serde_json::{Value, json};

pub struct System;

impl Host for System {
    fn default_font(&self) -> Option<Arc<[u8]>> {
        let candidates = [
            "C:/Windows/Fonts/arial.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ];
        let font = candidates.iter().find_map(|f| std::fs::read(f).ok())?;
        Some(font.into())
    }

    fn system_font(&self, family: &str, _bold: bool, _italic: bool) -> Option<(Arc<[u8]>, u32)> {
        (family == "Known").then(|| Some((self.default_font()?, 0)))?
    }

    fn video_size(&self, _file: &Path) -> Option<[u32; 2]> {
        None
    }
}

/// One device for all pictures, drawn one at a time.
static GPU: LazyLock<Option<Mutex<Gpu>>> =
    LazyLock::new(|| Gpu::new(GpuOptions::default()).ok().map(Mutex::new));

pub const SIZE: [usize; 2] = [64, 32];
pub const MIDDLE: [f32; 2] = [0.5, 0.5];

/// 4×2: red, green, blue and white above their halves in alpha.
fn swatch() -> Vec<u8> {
    let colors = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
    let mut rgba = Vec::new();
    for alpha in [255, 128] {
        for [r, g, b] in colors {
            rgba.extend([r, g, b, alpha]);
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(
        &mut out,
        &rgba,
        4,
        2,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .unwrap();
    out.into_inner()
}

#[derive(PartialEq)]
pub struct Picture(pub Vec<u8>);

impl Picture {
    /// RGB at (x, y), y down.
    pub fn at(&self, x: usize, y: usize) -> [u8; 3] {
        let i = (y * SIZE[0] + x) * 4;
        [self.0[i], self.0[i + 1], self.0[i + 2]]
    }

    pub fn assert(&self, x: usize, y: usize, rgb: [u8; 3], what: &str) {
        let got = self.at(x, y);
        let near = got.iter().zip(&rgb).all(|(a, b)| a.abs_diff(*b) <= 2);
        assert!(near, "{what} at ({x}, {y}): {got:?}, not {rgb:?}");
    }

    /// The largest difference of any channel anywhere.
    pub fn difference(&self, other: &Self) -> u8 {
        let pairs = self.0.iter().zip(&other.0);
        pairs.map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0)
    }
}

/// `None` without a GPU.
pub fn draw(background: &str, layers: Value, pointer: [f32; 2]) -> Option<Picture> {
    draw_at(background, layers, pointer, 0.0)
}

pub fn draw_at(background: &str, layers: Value, pointer: [f32; 2], time: f32) -> Option<Picture> {
    let Some(gpu) = GPU.as_ref() else {
        eprintln!("no GPU; skipping");
        return None;
    };
    let gpu = gpu.lock().unwrap_or_else(PoisonError::into_inner);
    let inputs = FrameInputs {
        pointer,
        time,
        ..FrameInputs::default()
    };
    Some(render(&gpu, background, &layers, &inputs))
}

/// With sound: how loud each band is.
pub fn draw_loud(background: &str, layers: Value, loud: impl Fn(usize) -> f32) -> Option<Picture> {
    let gpu = GPU.as_ref()?;
    let gpu = gpu.lock().unwrap_or_else(PoisonError::into_inner);
    let inputs = FrameInputs {
        audio: [std::array::from_fn(&loud); 2],
        ..FrameInputs::default()
    };
    Some(render(&gpu, background, &layers, &inputs))
}

fn renderer(gpu: &Gpu, background: &str, layers: &Value) -> SceneRenderer {
    let document = json!({
        "version": 1, "size": SIZE, "background": background, "layers": layers,
        "parallax": {"amount": 0.25, "smoothing": 0},
    });
    let mut files = Memory::default();
    files.0.insert(
        "wallpaper.json".into(),
        serde_json::to_vec(&document).unwrap(),
    );
    files.0.insert("assets/swatch.png".into(), swatch());
    let scene = compile(&files, &Values::new(), &System, &Limits::default()).unwrap();
    assert!(scene.skipped.is_empty(), "{:?}", scene.skipped);
    let compiler = ShaderCompiler::without_dxc();
    SceneRenderer::new(gpu, &scene, &compiler, None).unwrap()
}

fn render(gpu: &Gpu, background: &str, layers: &Value, inputs: &FrameInputs) -> Picture {
    let mut renderer = renderer(gpu, background, layers);
    renderer.render(gpu, inputs);
    Picture(renderer.read_output(gpu).unwrap())
}

/// Whether `layers` drawn at `first` need no drawing at `second` (time and
/// cursor); `None` without a GPU.
pub fn idle(layers: &Value, first: (f32, [f32; 2]), second: (f32, [f32; 2])) -> Option<bool> {
    let gpu = GPU.as_ref()?;
    let gpu = gpu.lock().unwrap_or_else(PoisonError::into_inner);
    let mut renderer = renderer(&gpu, "#000", layers);
    let inputs = |(time, pointer)| FrameInputs {
        time,
        pointer,
        ..FrameInputs::default()
    };
    assert!(!renderer.idle(&inputs(first)), "nothing drawn yet");
    renderer.render(&gpu, &inputs(first));
    Some(renderer.idle(&inputs(second)))
}
