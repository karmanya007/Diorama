//! Scene IR renderer (Phase 2): image layers, effect chains, composite.
//!
//! Mirrors the pass structure in the RenderDoc captures
//! (`docs/re-notes/renderdoc-captures.md`):
//! 1. each layer draws its image into its own target (layer-local ortho MVP,
//!    no blending);
//! 2. effect passes ping-pong between the layer's two targets, or write named
//!    effect FBOs (`target`), with a near-identity MVP;
//! 3. finished layers are composited back to front into a scene-sized target
//!    using the layer's material blending.
//!
//! The ping-pong schedule is static, so every bind group is built once and a
//! frame only rewrites the small per-pass uniform buffers (time, pointer…).

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use lumen_gpu::shader::{ShaderCompiler, ShaderError, Stage, UniformLayout};
use lumen_gpu::{Gpu, wgpu};
use lumen_scene::{self as ir, Blend, LayerKind, TextureBinding};
use wgpu::util::DeviceExt as _;

use crate::text::TextState;

/// Scene targets use WE's gamma-space RGBA8 (no sRGB conversion)...
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// ...or half floats for HDR scenes, where values above 1 feed the bloom.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Scene depth buffer of perspective scenes (reversed: near = 1).
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[derive(Debug, thiserror::Error)]
pub enum SceneError {
    #[error("scene has zero size")]
    Empty,
    #[error("shader compiler: {0}")]
    Shader(#[from] ShaderError),
}

/// Per-frame engine inputs.
#[derive(Debug, Clone, Copy)]
pub struct FrameInputs {
    /// Seconds since the wallpaper started (`g_Time`).
    pub time: f32,
    /// Fraction of the local day, 0..1 (`g_Daytime`).
    pub daytime: f32,
    /// Cursor in scene UV, 0..1, y down (`g_PointerPosition`).
    pub pointer: [f32; 2],
    /// Audio spectrum, 64 bands per channel (left, right), 0..1, bass first
    /// (`g_AudioSpectrum{16,32,64}{Left,Right}`, audio-driven emitters).
    pub audio: [[f32; 64]; 2],
}

/// The 64 bands folded to `n` by averaging neighbours.
fn fold_bands(bands: &[f32; 64], n: usize) -> Vec<f32> {
    let group = 64 / n.clamp(1, 64);
    bands
        .chunks(group.max(1))
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect()
}

/// Runtime changes to one layer (from scripts). `None` / empty = unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerUpdate {
    /// Layer transform (`node_world ∘ local`, see [`ir::ImageLayer`]).
    pub transform: Option<[f32; 6]>,
    /// 3D placement in perspective scenes ([`ir::ImageLayer::world`]).
    pub world: Option<Rows>,
    /// Object colour and alpha (`g_Color4`).
    pub color: Option<[f32; 4]>,
    pub visible: Option<bool>,
    pub text: Option<String>,
    /// (effect index, visible).
    pub effects: Vec<(usize, bool)>,
    /// (effect index, pass index, uniform, value).
    pub constants: Vec<(usize, usize, String, Vec<f32>)>,
    /// Frame of the layer's animated texture, chosen by a script (it then
    /// stops following scene time).
    pub texture_frame: Option<u32>,
}

/// What scripts may change about a layer; layers are (re)built from this.
#[derive(Debug, Clone)]
struct LayerState {
    transform: [f32; 6],
    /// 3D placement in perspective scenes.
    world: Option<Rows>,
    color: [f32; 4],
    visible: bool,
    text: Option<String>,
    effects: Vec<bool>,
    constants: BTreeMap<(usize, usize, String), Vec<f32>>,
    /// Script-chosen animated texture frame (`None`: follows scene time).
    texture_frame: Option<u32>,
}

impl Default for FrameInputs {
    fn default() -> Self {
        Self {
            time: 0.0,
            daytime: 0.5,
            pointer: [0.5, 0.5],
            audio: [[0.0; 64]; 2],
        }
    }
}

struct Target {
    label: String,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
}

struct AssetTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Allocation size, then meaningful image size (`g_TextureNResolution`).
    resolution: [f32; 4],
    sampler: usize,
}

struct Program {
    name: String,
    vs: wgpu::ShaderModule,
    fs: wgpu::ShaderModule,
    vs_entry: String,
    fs_entry: String,
    vs_uniforms: UniformLayout,
    fs_uniforms: UniformLayout,
    textures_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    texture_count: u32,
    texture_bindings: BTreeMap<u32, u32>,
    attributes: Vec<ir::Attribute>,
    pipelines: HashMap<Blend, wgpu::RenderPipeline>,
}

struct GpuPass {
    program: usize,
    pipeline: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    vertex_count: u32,
    uniforms: wgpu::BindGroup,
    textures: wgpu::BindGroup,
    vs_buffer: wgpu::Buffer,
    fs_buffer: wgpu::Buffer,
    vs_data: Vec<u8>,
    fs_data: Vec<u8>,
    target: usize,
    /// Draw onto the target's current contents instead of clearing it
    /// (blended passes into effect FBOs, e.g. additive bloom upsampling).
    load: bool,
    /// Placement and camera of a pass drawn into the scene or a group target
    /// (moved by parallax each frame); `None` for passes into layer targets.
    placement: Option<(Rows, Rows)>,
    /// Indexed draw (models): `vertex_count` is then the index count.
    indices: Option<wgpu::Buffer>,
    /// Draws with the scene depth buffer (models).
    depth: bool,
    /// Uniform data changed outside the frame loop; upload before drawing.
    dirty: bool,
    /// (effect, pass) this GPU pass draws; `None` for albedo / composite.
    source: Option<(usize, usize)>,
    /// Asset whose animation frames drive `g_Texture0Translation/Rotation`.
    sprite: Option<usize>,
    /// Puppet albedo: the vertices are re-posed every frame (uv scale of the
    /// padded texture).
    puppet_uv: Option<[f32; 2]>,
}

struct GpuLayer {
    /// Index in the IR scene's layers.
    index: usize,
    /// Where it draws (kept for rebuilds).
    frame: Frame,
    visible: bool,
    /// Targets created for this layer (freed when it is rebuilt).
    owned: Vec<usize>,
    /// Albedo then effect passes. A layer without effects has only its albedo
    /// pass, drawn straight into the scene like WE does (no layer target).
    passes: Vec<GpuPass>,
    /// Copy of the frame under the layer into its first target, before the
    /// albedo (`copy_background`): bind group, quad, target, frame camera.
    background: Option<(wgpu::BindGroup, wgpu::Buffer, usize, Rows)>,
    /// Final layer target → its frame (layers with effects only): bind group,
    /// quad, frame target and frame camera.
    composite: Option<(wgpu::BindGroup, wgpu::Buffer, usize, Rows)>,
    /// Layers drawn into this composition layer's target, after the first
    /// `children_at` passes (its albedo) and before its effects.
    children: Vec<GpuLayer>,
    /// Particle simulation feeding this layer's passes (one per system).
    particles: Option<crate::particles::ParticleGpu>,
    children_at: usize,
    blend: Blend,
    /// Samples `_rt_FullFrameBuffer`: snapshot the scene before running.
    needs_framebuffer: bool,
    size: [u32; 2],
    /// Layer-local → scene (pixels, or world units in perspective scenes).
    transform: Rows,
    parallax_depth: [f32; 2],
    /// Parallax offset currently baked into uniforms / vertices.
    offset: [f32; 2],
}

/// `targets[SCENE]` is the composited scene, `targets[FRAMEBUFFER]` its
/// snapshot for layers that read what is below them.
const SCENE: usize = 0;
const FRAMEBUFFER: usize = 1;

pub struct SceneRenderer {
    size: [u32; 2],
    shake: Option<ir::Shake>,
    format: wgpu::TextureFormat,
    parallax: Option<ir::Parallax>,
    /// Smoothed cursor used for parallax, and the time it was last updated.
    parallax_pointer: [f32; 2],
    last_time: Option<f32>,
    /// Scene pixels (world units in perspective scenes) → clip space.
    camera: Rows,
    /// `g_EyePosition` of perspective scenes.
    eye: Option<[f32; 3]>,
    /// Camera right, up and backward axes in world space (perspective
    /// scenes): particles face the camera along them.
    camera_axes: Option<[[f32; 3]; 3]>,
    /// Depth buffer of the scene target (perspective scenes).
    depth: Option<wgpu::TextureView>,
    clear: wgpu::Color,
    targets: Vec<Target>,
    programs: Vec<Option<Program>>,
    layers: Vec<GpuLayer>,
    composite_pipelines: HashMap<Blend, wgpu::RenderPipeline>,
    /// Background copies into text targets: colour only, alpha stays 0 (as
    /// WE: the copy only shows where effects raise alpha).
    background_pipeline: wgpu::RenderPipeline,
    /// Build resources kept so layers can be rebuilt at runtime.
    shared: Arc<Shared>,
    /// The imported scene without texture pixels (layers are rebuilt from it).
    ir: Arc<ir::Scene>,
    /// Current, script-driven state of every IR layer.
    state: Vec<LayerState>,
    /// Text layout and glyph atlas per text layer (IR index).
    texts: HashMap<usize, TextState>,
    /// Target slots released by rebuilt layers.
    free_targets: Vec<usize>,
}

/// Resources every layer build needs.
struct Shared {
    assets: Vec<AssetTexture>,
    /// Animation frames per asset (empty for still textures).
    frames: Vec<Vec<ir::TextureFrame>>,
    placeholder: AssetTexture,
    empty: AssetTexture,
    samplers: Vec<wgpu::Sampler>,
    uniforms_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
}

impl std::fmt::Debug for SceneRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneRenderer")
            .field("size", &self.size)
            .field("layers", &self.layers.len())
            .finish_non_exhaustive()
    }
}

const SAMPLERS: [(wgpu::AddressMode, wgpu::FilterMode); 4] = [
    (wgpu::AddressMode::ClampToEdge, wgpu::FilterMode::Linear),
    (wgpu::AddressMode::Repeat, wgpu::FilterMode::Linear),
    (wgpu::AddressMode::ClampToEdge, wgpu::FilterMode::Nearest),
    (wgpu::AddressMode::Repeat, wgpu::FilterMode::Nearest),
];
const CLAMP_LINEAR: usize = 0;

/// Row-major 4×4 (as WE and RenderDoc list it, translation in row 3) →
/// upload order. `mul(v, M)` reads naga column j as HLSL column j
/// (pinned by `lumen-gpu/tests/matrix_convention.rs`), so upload columns.
fn upload_matrix(rows: [[f32; 4]; 4]) -> [f32; 16] {
    std::array::from_fn(|i| rows[i % 4][i / 4])
}

type Rows = [[f32; 4]; 4];

/// Row-vector product: `mul(mul(v, a), b) == mul(v, mat_mul(a, b))`.
fn mat_mul(a: Rows, b: Rows) -> Rows {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..4).map(|k| a[r][k] * b[k][c]).sum()))
}

const IDENTITY_ROWS: Rows = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Inverse of [`model_rows`] (scene pixels → layer-local pixels).
fn inverse_model_rows(t: [f32; 6]) -> Rows {
    let [a, b, c, d, e, f] = t;
    let det = a * d - b * c;
    let inv = if det.abs() < 1e-12 { 0.0 } else { 1.0 / det };
    // Inverse linear part [[d, -b], [-c, a]] / det; translation follows.
    let (ia, ib, ic, id) = (d * inv, -b * inv, -c * inv, a * inv);
    [
        [ia, ib, 0.0, 0.0],
        [ic, id, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [-(e * ia + f * ic), -(e * ib + f * id), 0.0, 1.0],
    ]
}

/// Layer world transform (layer-local pixels, centred → scene pixels).
fn model_rows(t: [f32; 6]) -> Rows {
    [
        [t[0], t[1], 0.0, 0.0],
        [t[2], t[3], 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [t[4], t[5], 0.0, 1.0],
    ]
}

/// Orthographic `0..w × 0..h` → clip space with D3D depth, as captured for
/// both the scene camera and layer targets.
fn ortho_rows(w: f32, h: f32) -> Rows {
    [
        [2.0 / w, 0.0, 0.0, 0.0],
        [0.0, 2.0 / h, 0.0, 0.0],
        [0.0, 0.0, 0.0005, 0.0],
        [-1.0, -1.0, 0.5, 1.0],
    ]
}

/// The scene camera: ortho over the whole scene. (`general.zoom` is not a
/// plain ortho zoom: applying it either way lowers SSIM on capture 914.)
fn camera_rows(scene: &ir::Scene, output: [u32; 2]) -> Rows {
    match scene.camera {
        // WE renders perspective scenes at the swapchain's aspect.
        Some(camera) => {
            let (view, _) = camera.view(&scene.nodes);
            let aspect = output[0] as f32 / output[1].max(1) as f32;
            mat_mul(view, camera.projection(aspect))
        }
        None => ortho_rows(scene.width as f32, scene.height as f32),
    }
}

/// Centred `-w/2..w/2 × -h/2..h/2` → clip space (layer targets of lit
/// layers in capture 914; equivalent to the `0..w` form of capture 289).
fn centred_ortho_rows(w: f32, h: f32) -> Rows {
    [
        [2.0 / w, 0.0, 0.0, 0.0],
        [0.0, 2.0 / h, 0.0, 0.0],
        [0.0, 0.0, 0.0005, 0.0],
        [0.0, 0.0, 0.5, 1.0],
    ]
}

/// Effect passes draw a clip-space quad through this near-identity matrix.
const EFFECT_ROWS: Rows = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 0.0005, 0.0],
    [0.0, 0.0, 0.5, 1.0],
];

fn blend_state(blend: Blend) -> Option<wgpu::BlendState> {
    match blend {
        Blend::Opaque => None,
        Blend::Translucent => Some(wgpu::BlendState::ALPHA_BLENDING),
        Blend::Additive => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        }),
    }
}

fn create_target(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 2],
    format: wgpu::TextureFormat,
) -> Target {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Target {
        label: label.to_owned(),
        texture,
        view,
        size,
    }
}

/// What a pass samples in one texture slot.
enum Source {
    Asset(usize),
    Target(usize),
    Atlas,
    Placeholder,
    /// Transparent black: another layer's image that is not drawn.
    Empty,
}

impl SceneRenderer {
    /// `output` is the size of the composited image (default: the scene's
    /// own size); layer targets keep their layer sizes either way.
    pub fn new(
        gpu: &Gpu,
        scene: &ir::Scene,
        compiler: &ShaderCompiler,
        output: Option<[u32; 2]>,
    ) -> Result<Self, SceneError> {
        if scene.width == 0 || scene.height == 0 {
            return Err(SceneError::Empty);
        }
        let output = output
            .unwrap_or([scene.width, scene.height])
            .map(|v| v.max(1));
        let device = &gpu.device;
        let format = if scene.hdr { HDR_FORMAT } else { FORMAT };
        let samplers: Vec<wgpu::Sampler> = SAMPLERS
            .iter()
            .map(|(address, filter)| {
                device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("scene sampler"),
                    address_mode_u: *address,
                    address_mode_v: *address,
                    mag_filter: *filter,
                    min_filter: *filter,
                    // Trilinear for minified textures (WE ships mip chains).
                    mipmap_filter: match filter {
                        wgpu::FilterMode::Linear => wgpu::MipmapFilterMode::Linear,
                        wgpu::FilterMode::Nearest => wgpu::MipmapFilterMode::Nearest,
                    },
                    ..Default::default()
                })
            })
            .collect();

        let assets: Vec<AssetTexture> = scene.textures.iter().map(|t| upload(gpu, t)).collect();
        let placeholder = upload(
            gpu,
            &ir::Texture {
                name: "placeholder".into(),
                width: 1,
                height: 1,
                image_width: 1,
                image_height: 1,
                rgba: vec![255; 4],
                mips: Vec::new(),
                clamp: true,
                nearest: false,
                frames: Vec::new(),
                video: None,
            },
        );

        let empty = upload(
            gpu,
            &ir::Texture {
                name: "empty".into(),
                width: 1,
                height: 1,
                image_width: 1,
                image_height: 1,
                rgba: vec![0; 4],
                mips: Vec::new(),
                clamp: true,
                nearest: false,
                frames: Vec::new(),
                video: None,
            },
        );

        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("we uniforms"),
            entries: &[0, 1].map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        });

        let mut programs = Vec::with_capacity(scene.programs.len());
        for p in &scene.programs {
            match build_program(gpu, compiler, p, &uniforms_layout, format) {
                Ok(program) => programs.push(Some(program)),
                Err(e) => {
                    tracing::warn!(program = %p.name, "shader skipped: {e}");
                    programs.push(None);
                }
            }
        }

        let composite = CompositeShared::new(device, format);
        let shared = Arc::new(Shared {
            frames: scene.textures.iter().map(|t| t.frames.clone()).collect(),
            assets,
            placeholder,
            empty,
            samplers,
            uniforms_layout,
            composite_layout: composite.layout,
        });
        let ir = Arc::new(stripped(scene));
        let state = scene
            .layers
            .iter()
            .map(|l| LayerState {
                transform: l.transform,
                world: l.world,
                color: l.color,
                visible: l.visible,
                text: None,
                effects: l.effects.iter().map(|e| e.visible).collect(),
                constants: BTreeMap::new(),
                texture_frame: None,
            })
            .collect();
        let mut renderer = Self {
            size: [scene.width, scene.height],
            shake: scene.shake,
            format,
            parallax: scene.parallax,
            parallax_pointer: [0.5, 0.5],
            last_time: None,
            camera: camera_rows(scene, output),
            eye: scene.camera.map(|c| c.view(&scene.nodes).1),
            camera_axes: scene.camera.and_then(|c| {
                let world = lumen_scene::space::invert_mat4(c.view(&scene.nodes).0)?;
                Some(std::array::from_fn(|r| {
                    [world[r][0], world[r][1], world[r][2]]
                }))
            }),
            depth: scene.camera.map(|_| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("scene depth"),
                        size: wgpu::Extent3d {
                            width: output[0],
                            height: output[1],
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: DEPTH_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default())
            }),
            clear: wgpu::Color {
                r: f64::from(scene.clear_color[0]),
                g: f64::from(scene.clear_color[1]),
                b: f64::from(scene.clear_color[2]),
                a: f64::from(scene.clear_color[3]),
            },
            targets: vec![
                create_target(device, "scene", output, format),
                create_target(device, "framebuffer", output, format),
            ],
            programs,
            layers: Vec::new(),
            composite_pipelines: composite.pipelines,
            background_pipeline: composite.background,
            shared,
            ir,
            state,
            texts: HashMap::new(),
            free_targets: Vec::new(),
        };

        let top = Frame {
            target: SCENE,
            camera: renderer.camera,
        };
        let (shared, ir) = (renderer.shared.clone(), renderer.ir.clone());
        let ctx = BuildContext {
            gpu,
            scene: &ir,
            assets: &shared.assets,
            placeholder: &shared.placeholder,
            empty: &shared.empty,
            samplers: &shared.samplers,
            uniforms_layout: &shared.uniforms_layout,
        };
        // Opaque models draw first (capture of 3509243656: every model
        // before the image layers listed above it in the scene).
        let later = |l: &ir::ImageLayer| !(l.model.is_some() && l.albedo.blend == Blend::Opaque);
        let mut order: Vec<usize> = (0..ir.layers.len()).collect();
        order.sort_by_key(|&i| later(&ir.layers[i]));
        for i in order {
            if ir.layers[i].group.is_none()
                && let Some(gpu_layer) =
                    renderer.build_layer(&ctx, i, top, &shared.composite_layout)
            {
                renderer.layers.push(gpu_layer);
            }
        }
        Ok(renderer)
    }

    /// Applies script changes to layer `index` (IR order). Transform, colour,
    /// visibility and constants update in place; text and effect visibility
    /// rebuild the layer.
    pub fn update_layer(&mut self, gpu: &Gpu, index: usize, update: &LayerUpdate) {
        let Some(state) = self.state.get_mut(index) else {
            return;
        };
        let mut rebuild = false;
        if let Some(text) = &update.text
            && state.text.as_ref() != Some(text)
        {
            state.text = Some(text.clone());
            rebuild = true;
        }
        for &(effect, visible) in &update.effects {
            if let Some(v) = state.effects.get_mut(effect)
                && *v != visible
            {
                *v = visible;
                rebuild = true;
            }
        }
        if let Some(t) = update.transform {
            state.transform = t;
        }
        if let Some(w) = update.world {
            state.world = Some(w);
        }
        let placement = state.world.unwrap_or_else(|| model_rows(state.transform));
        if let Some(c) = update.color {
            state.color = c;
        }
        if let Some(v) = update.visible {
            state.visible = v;
        }
        if update.texture_frame.is_some() {
            state.texture_frame = update.texture_frame;
        }
        for (effect, pass, uniform, value) in &update.constants {
            state
                .constants
                .insert((*effect, *pass, uniform.clone()), value.clone());
        }
        if rebuild {
            self.rebuild_layer(gpu, index);
            return;
        }

        let anchor = self.text_anchor(index);
        let programs = &self.programs;
        let Some(layer) = find_layer(&mut self.layers, index) else {
            return;
        };
        if let Some(v) = update.visible {
            layer.visible = v;
        }
        if update.transform.is_some() || update.world.is_some() {
            let t = mat_mul(model_rows(anchor), placement);
            layer.transform = t;
            for pass in &mut layer.passes {
                if let Some((base, _)) = &mut pass.placement {
                    *base = t;
                }
            }
            // Recompute placements, composite and background quads next frame.
            layer.offset = [f32::NAN; 2];
        }
        let write = |pass: &mut GpuPass, name: &str, value: &[f32]| {
            let Some(Some(program)) = programs.get(pass.program) else {
                return;
            };
            pass.dirty |= program.vs_uniforms.write(&mut pass.vs_data, name, value);
            pass.dirty |= program.fs_uniforms.write(&mut pass.fs_data, name, value);
        };
        if let Some(c) = update.color
            && let Some(albedo) = layer.passes.first_mut()
        {
            write(albedo, "g_Color4", &c);
            write(albedo, "g_Color", &c[..3]);
            write(albedo, "g_Alpha", &c[3..]);
        }
        for (effect, pass, uniform, value) in &update.constants {
            if let Some(p) = layer
                .passes
                .iter_mut()
                .find(|p| p.source == Some((*effect, *pass)))
            {
                write(p, uniform, value);
            }
        }
    }

    /// Replaces the pixels of scene texture `texture` (a video frame), tightly
    /// packed RGBA8 of `width × height`, clipped to the texture's size.
    pub fn write_texture(&self, gpu: &Gpu, texture: usize, width: u32, height: u32, rgba: &[u8]) {
        let Some(asset) = self.shared.assets.get(texture) else {
            return;
        };
        let size = asset.texture.size();
        let (w, h) = (width.min(size.width), height.min(size.height));
        if w == 0 || h == 0 || rgba.len() < (width as usize) * (height as usize) * 4 {
            return;
        }
        gpu.queue.write_texture(
            asset.texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Offset from a text layer's anchor to the centre of its text box.
    fn text_anchor(&self, index: usize) -> [f32; 6] {
        self.texts
            .get(&index)
            .map_or(lumen_scene::IDENTITY_AFFINE, |t| {
                lumen_scene::affine(t.centre, 0.0, [1.0, 1.0])
            })
    }

    /// Rebuilds layer `index` (and its group children) from its state.
    fn rebuild_layer(&mut self, gpu: &Gpu, index: usize) {
        let Some(frame) = find_layer(&mut self.layers, index).map(|l| l.frame) else {
            return;
        };
        let (shared, ir) = (self.shared.clone(), self.ir.clone());
        let ctx = BuildContext {
            gpu,
            scene: &ir,
            assets: &shared.assets,
            placeholder: &shared.placeholder,
            empty: &shared.empty,
            samplers: &shared.samplers,
            uniforms_layout: &shared.uniforms_layout,
        };
        let Some(new) = self.build_layer(&ctx, index, frame, &shared.composite_layout) else {
            return;
        };
        let Some(slot) = find_layer(&mut self.layers, index) else {
            return;
        };
        let old = std::mem::replace(slot, new);
        let mut freed = Vec::new();
        collect_targets(&old, &mut freed);
        for t in freed {
            self.targets[t] = create_target(&gpu.device, "free", [1, 1], self.format);
            self.free_targets.push(t);
        }
    }

    /// A target slot (reusing freed ones), recorded in `owned`.
    fn alloc_target(
        &mut self,
        device: &wgpu::Device,
        label: &str,
        size: [u32; 2],
        owned: &mut Vec<usize>,
    ) -> usize {
        let target = create_target(device, label, size, self.format);
        let index = match self.free_targets.pop() {
            Some(i) => {
                self.targets[i] = target;
                i
            }
            None => {
                self.targets.push(target);
                self.targets.len() - 1
            }
        };
        owned.push(index);
        index
    }

    /// A particle layer: one draw per system of the tree, each with a vertex
    /// buffer the simulation refills every frame.
    fn build_particles(
        &mut self,
        ctx: &BuildContext<'_>,
        index: usize,
        frame: Frame,
        layer: &ir::ImageLayer,
        tree: &ir::ParticleSystem,
    ) -> Option<GpuLayer> {
        use crate::particles::{self as pt, ParticleGpu, SystemDraw};
        let device = &ctx.gpu.device;
        let mut passes = Vec::new();
        let mut draws = Vec::new();
        let mut needs_framebuffer = false;
        let sim = lumen_particles::Simulation::new(tree, pt::seed(index, &layer.name));
        for (system_index, system) in pt::systems(tree).into_iter().enumerate() {
            let plan = PassPlan {
                pass: &system.draw,
                blend: system.draw.blend,
                previous: None,
                fbos: &HashMap::new(),
                target: frame.target,
                geometry: Geometry::Direct {
                    size: [1, 1],
                    model: placement(layer),
                    camera: frame.camera,
                },
                color: [1.0; 4],
                mesh: None,
                puppet: None,
                model: None,
                atlas: None,
            };
            let Some(mut pass) = self.build_pass(ctx, &plan) else {
                continue;
            };
            let Some(Some(program)) = self.programs.get(pass.program) else {
                continue;
            };
            needs_framebuffer |= system
                .draw
                .textures
                .values()
                .any(|t| *t == TextureBinding::FrameBuffer);
            pass.load |= frame.target != SCENE;
            let stride: u64 = program
                .attributes
                .iter()
                .map(|a| u64::from(a.components.clamp(1, 4)))
                .sum();
            let capacity = u32::try_from(sim.capacity(system_index))
                .unwrap_or(u32::MAX)
                .clamp(1, 1 << 20)
                .saturating_mul(6);
            pass.vertices = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("particle vertices"),
                size: u64::from(capacity) * stride.max(1) * 4,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            pass.vertex_count = 0;
            let (var0, var1) = pt::render_vars(system);
            let inverse = upload_matrix(inverse_model_rows(layer.transform));
            for (layout, buf) in [
                (&program.vs_uniforms, &mut pass.vs_data),
                (&program.fs_uniforms, &mut pass.fs_data),
            ] {
                layout.write(buf, "g_RenderVar0", &var0);
                layout.write(buf, "g_RenderVar1", &var1);
                // Screen-facing quads in an orthographic view.
                layout.write(buf, "g_OrientationRight", &[1.0, 0.0, 0.0]);
                layout.write(buf, "g_OrientationUp", &[0.0, 1.0, 0.0]);
                layout.write(buf, "g_OrientationForward", &[0.0, 0.0, 1.0]);
                layout.write(buf, "g_ViewRight", &[1.0, 0.0, 0.0]);
                layout.write(buf, "g_ViewUp", &[0.0, 1.0, 0.0]);
                layout.write(buf, "g_ModelMatrixInverse", &inverse);
            }
            pass.dirty = true;
            draws.push(SystemDraw {
                system: system_index,
                pass: passes.len(),
                capacity,
            });
            passes.push(pass);
        }
        Some(GpuLayer {
            index,
            frame,
            visible: layer.visible,
            owned: Vec::new(),
            passes,
            background: None,
            particles: Some(ParticleGpu {
                sim,
                draws,
                scratch: Vec::new(),
            }),
            composite: None,
            children: Vec::new(),
            children_at: 0,
            blend: layer.blend,
            needs_framebuffer,
            size: layer.size,
            transform: placement(layer),
            parallax_depth: layer.parallax_depth,
            offset: [0.0, 0.0],
        })
    }

    /// Layer `index` as it should be built now: IR plus script state, with
    /// text laid out (size, centred transform, glyph mesh).
    fn effective_layer(&mut self, gpu: &Gpu, scene: &ir::Scene, index: usize) -> ir::ImageLayer {
        let mut layer = scene.layers[index].clone();
        let state = &self.state[index];
        layer.transform = state.transform;
        layer.world = state.world;
        layer.color = state.color;
        layer.visible = state.visible;
        for (e, visible) in layer.effects.iter_mut().zip(&state.effects) {
            e.visible = *visible;
        }
        for ((e, p, uniform), value) in &state.constants {
            if let Some(pass) = layer.effects.get_mut(*e).and_then(|e| e.passes.get_mut(*p)) {
                pass.constants.insert(uniform.clone(), value.clone());
            }
        }
        if let Some(spec) = &layer.text {
            let text = state.text.as_deref().unwrap_or(&spec.text);
            let text_state = match self.texts.entry(index) {
                std::collections::hash_map::Entry::Occupied(e) => Some(e.into_mut()),
                std::collections::hash_map::Entry::Vacant(v) => scene
                    .fonts
                    .get(spec.font)
                    .and_then(TextState::new)
                    .map(|t| v.insert(t)),
            };
            if let Some(t) = text_state {
                let laid = t.layout(gpu, spec, text);
                layer.size = laid.size;
                let centre = lumen_scene::affine(laid.centre, 0.0, [1.0, 1.0]);
                layer.transform = lumen_scene::compose(layer.transform, centre);
                layer.world = layer.world.map(|w| mat_mul(model_rows(centre), w));
                layer.mesh = Some(laid.mesh);
            } else {
                layer.mesh = Some(Vec::new());
            }
        }
        layer
    }

    /// Size of [`SceneRenderer::output`].
    pub fn size(&self) -> [u32; 2] {
        self.targets[SCENE].size
    }

    /// The composited scene ([`FORMAT`] or [`HDR_FORMAT`]); sample it to display.
    pub fn output(&self) -> &wgpu::TextureView {
        &self.targets[SCENE].view
    }

    fn build_layer(
        &mut self,
        ctx: &BuildContext<'_>,
        index: usize,
        frame: Frame,
        composite_layout: &wgpu::BindGroupLayout,
    ) -> Option<GpuLayer> {
        let layer = &self.effective_layer(ctx.gpu, ctx.scene, index);
        // Fullscreen layers cover the scene's own rectangle, also in
        // perspective scenes.
        let camera = if layer.kind == LayerKind::Fullscreen && frame.target == SCENE {
            let [w, h] = self.size.map(|v| v as f32);
            ortho_rows(w, h)
        } else {
            frame.camera
        };
        if let Some(tree) = crate::particles::tree(layer) {
            return self.build_particles(ctx, index, frame, layer, tree);
        }
        let atlas = self
            .texts
            .get(&index)
            .and_then(|t| Some((t.view()?.clone(), t.resolution())));
        let atlas = atlas.as_ref().map(|(v, r)| (v, *r));
        let device = &ctx.gpu.device;
        let mut owned = Vec::new();
        // Drawing into a group target keeps what is already there.
        let into_group = frame.target != SCENE;
        let needs_framebuffer = std::iter::once(&layer.albedo)
            .chain(layer.composite.as_ref())
            .chain(layer.effects.iter().flat_map(|e| &e.passes))
            .any(|p| {
                p.textures
                    .values()
                    .any(|t| *t == TextureBinding::FrameBuffer)
            })
            || layer.copy_background;

        let effects: Vec<&ir::Effect> = layer.effects.iter().filter(|e| e.visible).collect();
        if effects.is_empty() && !layer.copy_background {
            let plan = PassPlan {
                pass: &layer.albedo,
                blend: layer.blend,
                previous: None,
                fbos: &HashMap::new(),
                target: frame.target,
                geometry: match layer.kind {
                    LayerKind::Fullscreen => Geometry::Fullscreen,
                    LayerKind::Image | LayerKind::Compose => Geometry::Direct {
                        size: layer.size,
                        model: placement(layer),
                        camera,
                    },
                },
                color: layer.color,
                mesh: layer.mesh.as_deref(),
                puppet: layer.puppet.as_deref(),
                model: layer.model.as_deref(),
                atlas,
            };
            let mut pass = self.build_pass(ctx, &plan)?;
            pass.load |= into_group;
            return Some(GpuLayer {
                index,
                frame,
                visible: layer.visible,
                owned,
                passes: vec![pass],
                background: None,
                particles: None,
                composite: None,
                children: Vec::new(),
                children_at: 0,
                blend: layer.blend,
                needs_framebuffer,
                size: layer.size,
                transform: placement(layer),
                parallax_depth: layer.parallax_depth,
                offset: [0.0, 0.0],
            });
        }

        let ping = [
            self.alloc_target(device, &format!("{} a", layer.name), layer.size, &mut owned),
            self.alloc_target(device, &format!("{} b", layer.name), layer.size, &mut owned),
        ];
        let mut current = ping[0];

        let mut passes = Vec::new();
        // Albedo: no blending inside the layer target (blend applies at
        // composite), except meshes (glyphs), which blend like WE's font draws.
        let albedo = PassPlan {
            pass: &layer.albedo,
            blend: if layer.mesh.is_some() {
                layer.blend
            } else {
                Blend::Opaque
            },
            previous: None,
            fbos: &HashMap::new(),
            target: ping[0],
            geometry: match layer.kind {
                LayerKind::Fullscreen => Geometry::Fullscreen,
                LayerKind::Image => Geometry::LayerTarget {
                    size: layer.size,
                    model: placement(layer),
                },
                // Placed in the scene for its frame-buffer lookup; the shader
                // itself fills the target from texcoords.
                LayerKind::Compose => Geometry::Direct {
                    size: layer.size,
                    model: placement(layer),
                    camera,
                },
            },
            color: layer.color,
            mesh: layer.mesh.as_deref(),
            puppet: layer.puppet.as_deref(),
            model: layer.model.as_deref(),
            atlas,
        };
        let mut albedo = self.build_pass(ctx, &albedo)?;
        albedo.load = layer.copy_background;
        passes.push(albedo);
        let background = layer.copy_background.then(|| {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("background copy"),
                layout: composite_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &self.targets[FRAMEBUFFER].view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&ctx.samplers[CLAMP_LINEAR]),
                    },
                ],
            });
            let quad = background_quad(layer.size, placement(layer), camera);
            let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("background quad"),
                contents: bytemuck::cast_slice(&quad),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            (bind_group, vertices, ping[0], camera)
        });

        // Children of a composition layer draw into its target (scene
        // coordinates mapped onto the layer's rect) before its effects.
        let group = Frame {
            target: ping[0],
            camera: mat_mul(
                inverse_model_rows(layer.transform),
                centred_ortho_rows(layer.size[0] as f32, layer.size[1] as f32),
            ),
        };
        let mut children = Vec::new();
        for (j, child) in ctx.scene.layers.iter().enumerate() {
            if child.group == Some(index)
                && let Some(c) = self.build_layer(ctx, j, group, composite_layout)
            {
                children.push(c);
            }
        }
        let children_at = passes.len();

        for (e, effect) in layer.effects.iter().enumerate() {
            if !effect.visible {
                continue;
            }
            let mut fbos = HashMap::new();
            for fbo in &effect.fbos {
                let size = layer.size.map(|s| (s / fbo.scale_div).max(1));
                let t = self.alloc_target(
                    device,
                    &format!("{} {}", layer.name, fbo.name),
                    size,
                    &mut owned,
                );
                fbos.insert(fbo.name.clone(), t);
            }
            for (pass_index, pass) in effect.passes.iter().enumerate() {
                let target = match &pass.target {
                    Some(name) => match fbos.get(name) {
                        Some(t) => *t,
                        None => {
                            tracing::warn!(effect = %effect.name, "unknown target {name}");
                            continue;
                        }
                    },
                    None => {
                        if current == ping[0] {
                            ping[1]
                        } else {
                            ping[0]
                        }
                    }
                };
                let plan = PassPlan {
                    pass,
                    blend: pass.blend,
                    previous: Some(current),
                    fbos: &fbos,
                    target,
                    geometry: Geometry::Effect,
                    color: [1.0; 4],
                    mesh: None,
                    puppet: None,
                    model: None,
                    atlas: None,
                };
                if let Some(mut p) = self.build_pass(ctx, &plan) {
                    p.source = Some((e, pass_index));
                    passes.push(p);
                    if pass.target.is_none() {
                        current = target;
                    }
                }
            }
        }

        if let Some(pass) = &layer.composite {
            let plan = PassPlan {
                pass,
                blend: layer.blend,
                previous: Some(current),
                fbos: &HashMap::new(),
                target: frame.target,
                geometry: Geometry::Direct {
                    size: layer.size,
                    model: placement(layer),
                    camera,
                },
                // Colour and alpha were applied by the albedo pass.
                color: [1.0; 4],
                mesh: None,
                puppet: None,
                model: None,
                atlas: None,
            };
            if let Some(mut p) = self.build_pass(ctx, &plan) {
                p.load |= into_group;
                passes.push(p);
                return Some(GpuLayer {
                    index,
                    frame,
                    visible: layer.visible,
                    owned,
                    passes,
                    background,
                    particles: None,
                    composite: None,
                    children,
                    children_at,
                    blend: layer.blend,
                    needs_framebuffer,
                    size: layer.size,
                    transform: placement(layer),
                    parallax_depth: layer.parallax_depth,
                    offset: [0.0, 0.0],
                });
            }
        }

        let composite = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite"),
            layout: composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.targets[current].view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&ctx.samplers[CLAMP_LINEAR]),
                },
            ],
        });
        let composite_vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("composite quad"),
            contents: bytemuck::cast_slice(&composite_quad(layer.size, placement(layer), camera)),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        Some(GpuLayer {
            index,
            frame,
            visible: layer.visible,
            owned,
            passes,
            background,
            particles: None,
            composite: Some((composite, composite_vertices, frame.target, camera)),
            children,
            children_at,
            blend: layer.blend,
            needs_framebuffer,
            size: layer.size,
            transform: placement(layer),
            parallax_depth: layer.parallax_depth,
            offset: [0.0, 0.0],
        })
    }

    fn build_pass(&self, ctx: &BuildContext<'_>, plan: &PassPlan<'_>) -> Option<GpuPass> {
        let device = &ctx.gpu.device;
        let index = plan.pass.program.0;
        let program = self.programs.get(index)?.as_ref()?;
        let pipeline = program.pipelines.get(&plan.blend)?.clone();
        let target_size = self.targets[plan.target].size;

        // Resolve every texture binding of the program.
        let mut sources: Vec<(u32, Source)> = Vec::new();
        for (slot, binding) in &program.texture_bindings {
            let source = match plan.pass.textures.get(slot) {
                Some(TextureBinding::Asset(id)) => Source::Asset(id.0),
                Some(TextureBinding::Previous) => {
                    // Without a running image (an albedo sampling another,
                    // undrawn layer's `_rt_imageLayerComposite_*`): empty.
                    plan.previous.map_or(Source::Empty, Source::Target)
                }
                Some(TextureBinding::Fbo(name)) => plan
                    .fbos
                    .get(name)
                    .map_or(Source::Placeholder, |t| Source::Target(*t)),
                Some(TextureBinding::FrameBuffer) => Source::Target(FRAMEBUFFER),
                Some(TextureBinding::TextAtlas) if plan.atlas.is_some() => Source::Atlas,
                Some(TextureBinding::TextAtlas) | None => Source::Placeholder,
            };
            let source = match source {
                Source::Target(t) if t == plan.target => {
                    tracing::warn!(program = %program.name, "pass samples its own target");
                    Source::Placeholder
                }
                Source::Asset(id) if id >= ctx.assets.len() => Source::Placeholder,
                s => s,
            };
            sources.push((*binding, source));
        }

        let mut entries = Vec::new();
        let mut resolutions: BTreeMap<u32, [f32; 4]> = BTreeMap::new();
        for i in 0..program.texture_count {
            let slot = program
                .texture_bindings
                .iter()
                .find(|(_, b)| **b == i)
                .map(|(s, _)| *s);
            let source = sources
                .iter()
                .find(|(b, _)| *b == i)
                .map_or(&Source::Placeholder, |(_, s)| s);
            let (view, sampler, resolution) = match source {
                Source::Asset(id) => {
                    let a = &ctx.assets[*id];
                    (&a.view, &ctx.samplers[a.sampler], a.resolution)
                }
                Source::Target(t) => {
                    let t = &self.targets[*t];
                    let [w, h] = t.size.map(|v| v as f32);
                    (&t.view, &ctx.samplers[CLAMP_LINEAR], [w, h, w, h])
                }
                Source::Atlas => match plan.atlas {
                    Some((view, resolution)) => (view, &ctx.samplers[CLAMP_LINEAR], resolution),
                    None => (
                        &ctx.placeholder.view,
                        &ctx.samplers[CLAMP_LINEAR],
                        ctx.placeholder.resolution,
                    ),
                },
                Source::Empty => (
                    &ctx.empty.view,
                    &ctx.samplers[CLAMP_LINEAR],
                    ctx.empty.resolution,
                ),
                Source::Placeholder => (
                    &ctx.placeholder.view,
                    &ctx.samplers[CLAMP_LINEAR],
                    ctx.placeholder.resolution,
                ),
            };
            if let Some(slot) = slot {
                resolutions.insert(slot, resolution);
            }
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * i,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * i + 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pass textures"),
            layout: &program.textures_layout,
            entries: &entries,
        });

        // Static uniforms: identity matrices, material constants, engine values.
        let mut data = [
            vec![0u8; program.vs_uniforms.size.max(16) as usize],
            vec![0u8; program.fs_uniforms.size.max(16) as usize],
        ];
        // Lit shaders use model and view-projection separately; keep them
        // consistent with the combined matrix.
        let scene_ortho = self.camera;
        let (model, view_projection) = match plan.geometry {
            Geometry::LayerTarget { size: [w, h], .. } => {
                (IDENTITY_ROWS, centred_ortho_rows(w as f32, h as f32))
            }
            Geometry::Direct { model, camera, .. } => (model, camera),
            Geometry::Effect | Geometry::Fullscreen => (IDENTITY_ROWS, EFFECT_ROWS),
        };
        let mvp = upload_matrix(mat_mul(model, view_projection));
        let [sw, sh] = self.size.map(|v| v as f32);
        let [tw, th] = target_size.map(|v| v as f32);
        for (layout, buf) in [&program.vs_uniforms, &program.fs_uniforms]
            .into_iter()
            .zip(data.iter_mut())
        {
            for m in layout
                .members
                .iter()
                .filter(|m| m.rows == m.columns && m.rows > 1)
            {
                let n = m.rows as usize;
                let identity: Vec<f32> = (0..n * n)
                    .map(|i| if i % (n + 1) == 0 { 1.0 } else { 0.0 })
                    .collect();
                layout.write(buf, &m.name, &identity);
            }
            // Defaults the pass may override (WE's bloom uses frame-buffer texels).
            layout.write(buf, "g_TexelSize", &[1.0 / tw, 1.0 / th]);
            layout.write(buf, "g_TexelSizeHalf", &[0.5 / tw, 0.5 / th]);
            for (name, value) in &plan.pass.constants {
                layout.write(buf, name, value);
            }
            layout.write(buf, "g_ModelViewProjectionMatrix", &mvp);
            layout.write(buf, "g_ModelMatrix", &upload_matrix(model));
            layout.write(
                buf,
                "g_ViewProjectionMatrix",
                &upload_matrix(view_projection),
            );
            // Scene placement for lighting while drawing into a layer target.
            let alt_model = match plan.geometry {
                Geometry::LayerTarget { model, .. } | Geometry::Direct { model, .. } => model,
                Geometry::Effect | Geometry::Fullscreen => IDENTITY_ROWS,
            };
            layout.write(buf, "g_AltModelMatrix", &upload_matrix(alt_model));
            layout.write(
                buf,
                "g_AltViewProjectionMatrix",
                &upload_matrix(scene_ortho),
            );
            // Observed in capture 914: eye above the scene centre at z = 2000.
            let eye = self.eye.unwrap_or([sw / 2.0, sh / 2.0, 2000.0]);
            layout.write(buf, "g_EyePosition", &eye);
            layout.write(buf, "g_NormalModelMatrix", &normal_matrix(model));
            let lights = &ctx.scene.point_lights;
            let origins: Vec<f32> = lights
                .iter()
                .flat_map(|l| [l.origin[0], l.origin[1], l.origin[2], l.exponent])
                .collect();
            let colors: Vec<f32> = lights
                .iter()
                .flat_map(|l| [l.color[0], l.color[1], l.color[2], l.radius])
                .collect();
            layout.write(buf, "g_LPoint_Origin", &origins);
            layout.write(buf, "g_LPoint_Color", &colors);
            let tubes = &ctx.scene.tube_lights;
            let pack = |f: fn(&ir::TubeLight) -> [f32; 4]| -> Vec<f32> {
                tubes.iter().flat_map(f).collect()
            };
            let [a, b] = [
                pack(|l| [l.a[0], l.a[1], l.a[2], l.exponent]),
                pack(|l| [l.b[0], l.b[1], l.b[2], 0.0]),
            ];
            layout.write(buf, "g_LTube_OriginA", &a);
            layout.write(buf, "g_LTube_OriginB", &b);
            layout.write(
                buf,
                "g_LTube_Color",
                &pack(|l| [l.color[0], l.color[1], l.color[2], l.radius]),
            );
            for (slot, res) in &resolutions {
                layout.write(buf, &format!("g_Texture{slot}Resolution"), res);
            }
            layout.write(buf, "g_Color4", &plan.color);
            layout.write(buf, "g_Color", &plan.color[..3]);
            layout.write(buf, "g_Alpha", &plan.color[3..]);
            layout.write(buf, "g_Screen", &[sw, sh, sw / sh]);
            layout.write(buf, "g_LightAmbientColor", &ctx.scene.ambient_color);
            layout.write(buf, "g_LightSkylightColor", &ctx.scene.skylight_color);
        }
        let [vs_data, fs_data] = data;
        let vs_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vs uniforms"),
            contents: &vs_data,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let fs_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fs uniforms"),
            contents: &fs_data,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pass uniforms"),
            layout: ctx.uniforms_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: vs_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: fs_buffer.as_entire_binding(),
                },
            ],
        });

        // Animated textures: the shader maps 0..1 onto the current frame.
        let sprite = match plan.pass.textures.get(&0) {
            Some(TextureBinding::Asset(id))
                if program.vs_uniforms.member("g_Texture0Rotation").is_some()
                    && self.shared.frames.get(id.0).is_some_and(|f| !f.is_empty()) =>
            {
                Some(id.0)
            }
            _ => None,
        };
        // Albedo UVs stop at the image inside a padded texture.
        let albedo = sprite.is_none()
            && matches!(
                plan.geometry,
                Geometry::LayerTarget { .. } | Geometry::Direct { .. }
            );
        let uv_max = match (albedo, plan.pass.textures.get(&0)) {
            (true, Some(TextureBinding::Asset(id))) => {
                ctx.assets.get(id.0).map_or([1.0, 1.0], |a| {
                    [
                        a.resolution[2] / a.resolution[0],
                        a.resolution[3] / a.resolution[1],
                    ]
                })
            }
            _ => [1.0, 1.0],
        };
        let skinned = plan.puppet.filter(|_| {
            matches!(
                plan.geometry,
                Geometry::LayerTarget { .. } | Geometry::Direct { .. }
            )
        });
        let mesh = plan
            .model
            .filter(|_| matches!(plan.geometry, Geometry::Direct { .. }));
        let data = match (skinned, mesh) {
            (_, Some(m)) => model_vertices(&program.attributes, m),
            (Some(p), None) => {
                let bind: Vec<[f32; 2]> = p.vertices.iter().map(|v| v.position).collect();
                puppet_vertices(&program.attributes, p, &bind, uv_max)
            }
            (None, None) => pass_vertices(&program.attributes, plan.geometry, uv_max, plan.mesh),
        };
        let stride: usize = program
            .attributes
            .iter()
            .map(|a| a.components.clamp(1, 4) as usize)
            .sum();
        let mut vertex_count = (data.len() / stride.max(1)) as u32;
        let indices = mesh.map(|m| {
            vertex_count = u32::try_from(m.indices.len()).unwrap_or(0);
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("model indices"),
                contents: bytemuck::cast_slice(&m.indices),
                usage: wgpu::BufferUsages::INDEX,
            })
        });
        let depth = mesh.is_some() && self.depth.is_some();
        let pipeline = match mesh {
            Some(m) if depth => create_pipeline(
                device,
                program,
                plan.blend,
                self.format,
                Some(ModelRaster {
                    cull: m.cull,
                    test: m.depth_test,
                    write: m.depth_write,
                }),
            ),
            _ => pipeline,
        };
        // An empty mesh (blank text) still clears its target; wgpu needs a
        // non-empty buffer to bind.
        let mut contents = data;
        contents.resize(contents.len().max(stride.max(1)), 0.0);
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("pass vertices"),
            contents: bytemuck::cast_slice(&contents),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        Some(GpuPass {
            program: index,
            pipeline,
            vertices,
            vertex_count,
            uniforms,
            textures,
            vs_buffer,
            fs_buffer,
            vs_data,
            fs_data,
            target: plan.target,
            load: plan.blend != Blend::Opaque && plan.pass.target.is_some(),
            placement: match plan.geometry {
                Geometry::Direct { model, camera, .. } => Some((model, camera)),
                _ => None,
            },
            indices,
            depth,
            dirty: false,
            source: None,
            puppet_uv: skinned.map(|_| uv_max),
            sprite,
        })
    }

    /// Blocking readback of [`SceneRenderer::output`] as tightly packed RGBA8
    /// (headless capture and golden tests; not for the frame loop).
    pub fn read_output(&self, gpu: &Gpu) -> Option<Vec<u8>> {
        read_target(gpu, &self.targets[SCENE])
    }

    /// Every intermediate target as (label, size, RGBA8), for debugging passes.
    pub fn read_targets(&self, gpu: &Gpu) -> Vec<(String, [u32; 2], Vec<u8>)> {
        self.targets
            .iter()
            .skip(FRAMEBUFFER + 1)
            .filter_map(|t| Some((t.label.clone(), t.size, read_target(gpu, t)?)))
            .collect()
    }

    /// Renders one frame into [`SceneRenderer::output`] and submits.
    pub fn render(&mut self, gpu: &Gpu, inputs: &FrameInputs) {
        self.render_impl(gpu, inputs);
    }
}

fn read_target(gpu: &Gpu, target: &Target) -> Option<Vec<u8>> {
    {
        let [w, h] = target.size;
        let texel_bytes = if target.texture.format() == HDR_FORMAT {
            8
        } else {
            4
        };
        let row = texel_bytes * w;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene readback"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let mapped = slice.get_mapped_range().ok()?;
        let mut out = Vec::with_capacity((row * h) as usize);
        for r in mapped.chunks_exact(padded as usize) {
            let r = &r[..row as usize];
            if texel_bytes == 8 {
                // Half-float channels → clamped 8-bit.
                out.extend(r.as_chunks::<2>().0.iter().map(|h| {
                    let v = f16_to_f32(u16::from_le_bytes(*h));
                    (v.clamp(0.0, 1.0) * 255.0).round() as u8
                }));
            } else {
                out.extend_from_slice(r);
            }
        }
        Some(out)
    }
}

impl SceneRenderer {
    fn render_impl(&mut self, gpu: &Gpu, inputs: &FrameInputs) {
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene"),
            });
        let shift = self.parallax_shift(inputs);
        let shake = self.shake_offset(inputs.time);
        // The scene target is cleared by whichever pass touches it first.
        let mut scene_load = wgpu::LoadOp::Clear(self.clear);
        let mut layers = std::mem::take(&mut self.layers);
        let mut audio = Vec::with_capacity(6);
        for (n, left, right) in [
            (16, "g_AudioSpectrum16Left", "g_AudioSpectrum16Right"),
            (32, "g_AudioSpectrum32Left", "g_AudioSpectrum32Right"),
            (64, "g_AudioSpectrum64Left", "g_AudioSpectrum64Right"),
        ] {
            audio.push((left, fold_bands(&inputs.audio[0], n)));
            audio.push((right, fold_bands(&inputs.audio[1], n)));
        }
        let audio_level = inputs.audio.iter().flatten().sum::<f32>() / 128.0;
        let mut frame = FrameState {
            gpu,
            inputs,
            audio,
            audio_level,
            shift,
            shake,
            scene_load: &mut scene_load,
            depth_load: wgpu::LoadOp::Clear(0.0),
        };
        for layer in &mut layers {
            self.encode_layer(&mut encoder, layer, &mut frame);
        }
        self.layers = layers;
        if let wgpu::LoadOp::Clear(_) = scene_load {
            scene_pass(&mut encoder, &self.targets[SCENE].view, scene_load, "clear");
        }
        gpu.queue.submit([encoder.finish()]);
    }
}

/// Where a layer is drawn: the scene, or a composition group's target.
#[derive(Clone, Copy)]
struct Frame {
    target: usize,
    camera: Rows,
}

/// Per-frame state shared while encoding layers.
struct FrameState<'a> {
    gpu: &'a Gpu,
    inputs: &'a FrameInputs,
    /// Spectrum uniforms for this frame (name, values).
    audio: Vec<(&'static str, Vec<f32>)>,
    /// Mean audio level 0..1 (audio-driven particle emitters).
    audio_level: f32,
    shift: [f32; 2],
    shake: [f32; 2],
    scene_load: &'a mut wgpu::LoadOp<wgpu::Color>,
    /// The scene depth buffer is cleared by the first model drawn.
    depth_load: wgpu::LoadOp<f32>,
}

impl SceneRenderer {
    /// Encodes one layer: its passes, its children (composition groups) after
    /// its albedo, then its composite into its frame.
    fn encode_layer(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        layer: &mut GpuLayer,
        f: &mut FrameState<'_>,
    ) {
        if !layer.visible {
            return;
        }
        let gpu = f.gpu;
        if let Some(pg) = &mut layer.particles {
            // Cursor in the system's local pixels (pointer-following control points).
            let [w, h] = self.size.map(|v| v as f32);
            let scene = [f.inputs.pointer[0] * w, (1.0 - f.inputs.pointer[1]) * h];
            let [a, b, c, d, e, g] = rows_affine(layer.transform);
            let det = a * d - b * c;
            if det.abs() > 1e-9 {
                let (x, y) = (scene[0] - e, scene[1] - g);
                pg.sim
                    .set_pointer(Some([(d * x - c * y) / det, (-b * x + a * y) / det, 0.0]));
            }
            pg.sim.set_audio_level(f.audio_level);
            pg.sim.advance_to(f.inputs.time);
            for draw in &pg.draws {
                let Some(pass) = layer.passes.get_mut(draw.pass) else {
                    continue;
                };
                let (Some(Some(program)), Some(system)) =
                    (self.programs.get(pass.program), pg.sim.system(draw.system))
                else {
                    continue;
                };
                // 3D scenes project particles through the camera; 2D ones
                // fake WE's perspective systems on the CPU.
                let view = crate::particles::view(rows_affine(layer.transform), self.size);
                let view = self.camera_axes.is_none().then_some(&view);
                if let Some(axes) = self.camera_axes {
                    let [right, up, forward] = facing(layer.transform, axes);
                    for (layout, buf) in [
                        (&program.vs_uniforms, &mut pass.vs_data),
                        (&program.fs_uniforms, &mut pass.fs_data),
                    ] {
                        layout.write(buf, "g_OrientationRight", &right);
                        layout.write(buf, "g_OrientationUp", &up);
                        layout.write(buf, "g_OrientationForward", &forward);
                        layout.write(buf, "g_ViewRight", &right);
                        layout.write(buf, "g_ViewUp", &up);
                    }
                    pass.dirty = true;
                }
                let n = crate::particles::fill(
                    &program.attributes,
                    system,
                    pg.sim.particles(draw.system),
                    draw.capacity,
                    view,
                    &mut pg.scratch,
                );
                tracing::trace!(
                    layer = layer.index,
                    system = draw.system,
                    particles = n / 6,
                    "particles drawn"
                );
                if n > 0 {
                    gpu.queue
                        .write_buffer(&pass.vertices, 0, bytemuck::cast_slice(&pg.scratch));
                }
                pass.vertex_count = n;
            }
        }
        let offset = [
            f.shift[0] * layer.parallax_depth[0] + f.shake[0],
            f.shift[1] * layer.parallax_depth[1] + f.shake[1],
        ];
        let moved = offset != layer.offset;
        if moved {
            layer.offset = offset;
            let t = offset_transform(layer.transform, offset);
            if let Some((_, vertices, _, camera)) = &layer.composite {
                let quad = composite_quad(layer.size, t, *camera);
                gpu.queue
                    .write_buffer(vertices, 0, bytemuck::cast_slice(&quad));
            }
            if let Some((_, background, _, camera)) = &layer.background {
                let quad = background_quad(layer.size, t, *camera);
                gpu.queue
                    .write_buffer(background, 0, bytemuck::cast_slice(&quad));
            }
        }
        if layer.needs_framebuffer {
            if let wgpu::LoadOp::Clear(_) = *f.scene_load {
                scene_pass(encoder, &self.targets[SCENE].view, *f.scene_load, "clear");
                *f.scene_load = wgpu::LoadOp::Load;
            }
            let [width, height] = self.targets[SCENE].size;
            let size = wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            };
            encoder.copy_texture_to_texture(
                self.targets[SCENE].texture.as_image_copy(),
                self.targets[FRAMEBUFFER].texture.as_image_copy(),
                size,
            );
        }
        if let Some((bind_group, vertices, target, _)) = &layer.background {
            let pipeline = &self.background_pipeline;
            let clear = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);
            let mut rp = scene_pass(encoder, &self.targets[*target].view, clear, "background");
            rp.set_pipeline(pipeline);
            rp.set_bind_group(0, bind_group, &[]);
            rp.set_vertex_buffer(0, vertices.slice(..));
            rp.draw(0..4, 0..1);
        }
        if let Some(puppet) = self
            .ir
            .layers
            .get(layer.index)
            .and_then(|l| l.puppet.as_deref())
        {
            let positions = puppet.pose(f.inputs.time);
            for pass in &layer.passes {
                let (Some(uv_max), Some(Some(program))) =
                    (pass.puppet_uv, self.programs.get(pass.program))
                else {
                    continue;
                };
                let data = puppet_vertices(&program.attributes, puppet, &positions, uv_max);
                gpu.queue
                    .write_buffer(&pass.vertices, 0, bytemuck::cast_slice(&data));
            }
        }
        let children_at = layer.children_at.min(layer.passes.len());
        let (before, after) = layer.passes.split_at_mut(children_at);
        let sprite_frame = self.state.get(layer.index).and_then(|s| s.texture_frame);
        for pass in before {
            self.encode_pass(encoder, pass, (moved, offset), sprite_frame, f);
        }
        for child in &mut layer.children {
            self.encode_layer(encoder, child, f);
        }
        for pass in after {
            self.encode_pass(encoder, pass, (moved, offset), sprite_frame, f);
        }
        if let Some((bind_group, vertices, target, _)) = &layer.composite
            && let Some(pipeline) = self.composite_pipelines.get(&layer.blend)
        {
            let load = if *target == SCENE {
                std::mem::replace(f.scene_load, wgpu::LoadOp::Load)
            } else {
                wgpu::LoadOp::Load
            };
            let mut rp = scene_pass(encoder, &self.targets[*target].view, load, "composite");
            rp.set_pipeline(pipeline);
            rp.set_bind_group(0, bind_group, &[]);
            rp.set_vertex_buffer(0, vertices.slice(..));
            rp.draw(0..4, 0..1);
        }
    }

    fn encode_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pass: &mut GpuPass,
        (moved, offset): (bool, [f32; 2]),
        sprite_frame: Option<u32>,
        f: &mut FrameState<'_>,
    ) {
        let Some(Some(program)) = self.programs.get(pass.program) else {
            return;
        };
        let inputs = f.inputs;
        for (layout, buf, gpu_buf) in [
            (&program.vs_uniforms, &mut pass.vs_data, &pass.vs_buffer),
            (&program.fs_uniforms, &mut pass.fs_data, &pass.fs_buffer),
        ] {
            let mut dirty = layout.write(buf, "g_Time", &[inputs.time]);
            if let (true, Some((base, camera))) = (moved, pass.placement) {
                let model = offset_transform(base, offset);
                let mvp = upload_matrix(mat_mul(model, camera));
                dirty |= layout.write(buf, "g_ModelViewProjectionMatrix", &mvp);
                dirty |= layout.write(buf, "g_ModelMatrix", &upload_matrix(model));
                dirty |= layout.write(buf, "g_NormalModelMatrix", &normal_matrix(model));
            }
            dirty |= layout.write(buf, "g_Daytime", &[inputs.daytime]);
            dirty |= layout.write(buf, "g_PointerPosition", &inputs.pointer);
            dirty |= layout.write(buf, "g_PointerPositionLast", &inputs.pointer);
            let frames = pass.sprite.and_then(|a| self.shared.frames.get(a));
            let frame = match sprite_frame {
                Some(i) => frames.and_then(|f| f.get(i as usize).or(f.last())),
                None => frames.and_then(|f| ir::TextureFrame::at(f, inputs.time)),
            };
            if let Some(frame) = frame {
                let [x, y] = frame.x_axis;
                let [u, v] = frame.y_axis;
                dirty |= layout.write(buf, "g_Texture0Translation", &frame.origin);
                dirty |= layout.write(buf, "g_Texture0Rotation", &[x, y, u, v]);
            }
            for (name, values) in &f.audio {
                dirty |= layout.write(buf, name, values);
            }
            if dirty || pass.dirty {
                f.gpu.queue.write_buffer(gpu_buf, 0, buf);
            }
        }
        pass.dirty = false;
        let load = if pass.target == SCENE {
            std::mem::replace(f.scene_load, wgpu::LoadOp::Load)
        } else if pass.load {
            wgpu::LoadOp::Load
        } else {
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
        };
        let depth = match (&self.depth, pass.depth) {
            (Some(view), true) => {
                let load = std::mem::replace(&mut f.depth_load, wgpu::LoadOp::Load);
                Some((view, load))
            }
            _ => None,
        };
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(&program.name),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.targets[pass.target].view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: depth.map(|(view, load)| {
                wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }
            }),
            ..Default::default()
        });
        rp.set_pipeline(&pass.pipeline);
        rp.set_bind_group(0, &pass.uniforms, &[]);
        rp.set_bind_group(1, &pass.textures, &[]);
        rp.set_vertex_buffer(0, pass.vertices.slice(..));
        match &pass.indices {
            Some(indices) if pass.vertex_count > 0 => {
                rp.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..pass.vertex_count, 0, 0..1);
            }
            None if pass.vertex_count > 0 => rp.draw(0..pass.vertex_count, 0..1),
            _ => {}
        }
    }

    /// Scene-space parallax shift for depth 1 this frame (smoothed cursor).
    fn parallax_shift(&mut self, inputs: &FrameInputs) -> [f32; 2] {
        let Some(p) = self.parallax else {
            return [0.0, 0.0];
        };
        let dt = self
            .last_time
            .map_or(f32::INFINITY, |t| (inputs.time - t).max(0.0));
        self.last_time = Some(inputs.time);
        let follow = if p.delay > 0.0 {
            1.0 - (-dt / p.delay).exp()
        } else {
            1.0
        };
        for (s, target) in self.parallax_pointer.iter_mut().zip(inputs.pointer) {
            *s += (target - *s) * follow;
        }
        let k = p.amount * p.mouse_influence;
        let [px, py] = self.parallax_pointer;
        [
            k * self.size[0] as f32 * (0.5 - px),
            k * self.size[1] as f32 * (py - 0.5),
        ]
    }
}

/// IEEE half → single precision (no `half` dependency for one readback).
fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1f);
    let frac = f32::from(h & 0x3ff);
    match exp {
        0 => sign * frac * (2.0f32).powi(-24),
        31 if frac == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        e => sign * (1.0 + frac / 1024.0) * (2.0f32).powi(e - 15),
    }
}

impl SceneRenderer {
    /// Camera shake in scene pixels at `time` (same for every layer).
    fn shake_offset(&self, time: f32) -> [f32; 2] {
        let Some(s) = self.shake else {
            return [0.0, 0.0];
        };
        // ≈1 % of the height peak to peak at amplitude 1 (captures 3347344353,
        // 3195058405); noise in -1..1, so half of that per unit.
        let scale = s.amplitude * 0.005 * self.size[1] as f32;
        let t = time * s.speed * 0.4;
        [
            scale * drift(t, 0x9e37, s.roughness),
            scale * drift(t, 0x85eb, s.roughness),
        ]
    }
}

/// Smooth 1-D value noise in -1..1 with a rougher second octave.
fn drift(t: f32, seed: u32, roughness: f32) -> f32 {
    let octave = |t: f32, seed: u32| {
        let i = t.floor();
        let f = t - i;
        let hash = |n: f32| {
            let mut x =
                (n as i64 as u32).wrapping_mul(0x2c1b_3c6d) ^ seed.wrapping_mul(0x297a_2d39);
            x ^= x >> 15;
            x = x.wrapping_mul(0x8525_7f6b);
            x ^= x >> 13;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        let smooth = f * f * (3.0 - 2.0 * f);
        hash(i) + (hash(i + 1.0) - hash(i)) * smooth
    };
    let rough = roughness.clamp(0.0, 2.0) * 0.3;
    (octave(t, seed) + rough * octave(t * 2.7 + 13.0, seed ^ 0x5bd1)) / (1.0 + rough)
}

fn offset_transform(mut t: Rows, offset: [f32; 2]) -> Rows {
    t[3][0] += offset[0];
    t[3][1] += offset[1];
    t
}

/// Camera axes (world) in a layer's local space, normalised: the directions
/// WE's particle shaders expand billboards along (`g_OrientationRight/Up/
/// Forward`; capture of 3509243656: right = (0.883, 0, 0.470) for a system
/// turned 28° about y).
fn facing(model: Rows, axes: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let inverse = lumen_scene::space::invert_mat4(model).unwrap_or(IDENTITY_ROWS);
    axes.map(|a| {
        let v: [f32; 3] = std::array::from_fn(|c| {
            a[0] * inverse[0][c] + a[1] * inverse[1][c] + a[2] * inverse[2][c]
        });
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if len > 1e-9 { v.map(|x| x / len) } else { a }
    })
}

/// A layer's placement: its 3D world matrix in perspective scenes, else its
/// 2D transform.
fn placement(layer: &ir::ImageLayer) -> Rows {
    layer.world.unwrap_or_else(|| model_rows(layer.transform))
}

/// The 2D part of a placement (`[m0..m5]`, see [`ir::ImageLayer::transform`]).
fn rows_affine(t: Rows) -> [f32; 6] {
    [t[0][0], t[0][1], t[1][0], t[1][1], t[3][0], t[3][1]]
}

/// `g_NormalModelMatrix`: inverse transpose of the model's 3×3 (row-vector),
/// in upload (column) order.
fn normal_matrix(model: Rows) -> [f32; 9] {
    let m = |r: usize, c: usize| model[r][c];
    let cof = |r: usize, c: usize| {
        let (r0, r1) = ((r + 1) % 3, (r + 2) % 3);
        let (c0, c1) = ((c + 1) % 3, (c + 2) % 3);
        m(r0, c0) * m(r1, c1) - m(r0, c1) * m(r1, c0)
    };
    let det = m(0, 0) * cof(0, 0) + m(0, 1) * cof(0, 1) + m(0, 2) * cof(0, 2);
    let inv = if det.abs() < 1e-20 { 0.0 } else { 1.0 / det };
    // (M⁻¹)ᵀ = cofactor matrix / det; upload columns.
    std::array::from_fn(|i| cof(i % 3, i / 3) * inv)
}

/// Raster state of a model draw.
#[derive(Debug, Clone, Copy)]
struct ModelRaster {
    cull: ir::Cull,
    test: bool,
    write: bool,
}

/// Vertices of a model, one per mesh vertex (drawn indexed), attributes
/// filled by name.
fn model_vertices(attributes: &[ir::Attribute], model: &ir::Model) -> Vec<f32> {
    let stride: usize = attributes
        .iter()
        .map(|a| a.components.clamp(1, 4) as usize)
        .sum();
    let mut out = Vec::with_capacity(model.vertices.len() * stride);
    for v in &model.vertices {
        let [x, y, z] = v.position;
        let [nx, ny, nz] = v.normal;
        for a in attributes {
            let n = a.components.clamp(1, 4) as usize;
            let values = match a.name.as_str() {
                "a_Position" => [x, y, z, 1.0],
                "a_Normal" => [nx, ny, nz, 0.0],
                "a_Tangent4" | "a_Tangent" => v.tangent,
                "a_TexCoord" => [v.uv[0], v.uv[1], 0.0, 0.0],
                "a_Color" | "a_Color4u" => [1.0; 4],
                _ => [0.0; 4],
            };
            out.extend_from_slice(&values[..n]);
        }
    }
    out
}

fn scene_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    label: &str,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    })
}

struct BuildContext<'a> {
    gpu: &'a Gpu,
    scene: &'a ir::Scene,
    assets: &'a [AssetTexture],
    placeholder: &'a AssetTexture,
    empty: &'a AssetTexture,
    samplers: &'a [wgpu::Sampler],
    uniforms_layout: &'a wgpu::BindGroupLayout,
}

struct PassPlan<'a> {
    pass: &'a ir::Pass,
    blend: Blend,
    /// Target holding the layer's current image (`previous`).
    previous: Option<usize>,
    fbos: &'a HashMap<String, usize>,
    target: usize,
    geometry: Geometry,
    /// `g_Color4` (object colour + alpha) for albedo passes.
    color: [f32; 4],
    /// Quads replacing the albedo's single quad.
    mesh: Option<&'a [ir::MeshQuad]>,
    /// Skinned mesh replacing the albedo's quad (re-posed every frame).
    puppet: Option<&'a ir::puppet::Puppet>,
    /// 3D mesh replacing the albedo's quad (drawn with depth).
    model: Option<&'a ir::Model>,
    /// The layer's glyph atlas and its resolution (text layers).
    atlas: Option<(&'a wgpu::TextureView, [f32; 4])>,
}

#[derive(Debug, Clone, Copy)]
enum Geometry {
    /// Albedo into the layer's own target: centred quad through a centred
    /// ortho MVP; `model` places it in the scene for lighting
    /// (`g_AltModelMatrix`, WE's PRELIGHTING path).
    LayerTarget { size: [u32; 2], model: Rows },
    /// Albedo placed by `model` and drawn through `camera` (the scene's,
    /// or a composition group's when drawing into its target): centred quad.
    Direct {
        size: [u32; 2],
        model: Rows,
        camera: Rows,
    },
    /// Effect pass: clip-space quad.
    Effect,
    /// Albedo of a fullscreen layer: clip-space quad, no padded-UV crop.
    Fullscreen,
}

/// Triangle list (two triangles per quad) with each attribute filled by
/// name: one quad over the layer / target, or the layer's mesh.
fn pass_vertices(
    attributes: &[ir::Attribute],
    geometry: Geometry,
    uv_max: [f32; 2],
    mesh: Option<&[ir::MeshQuad]>,
) -> Vec<f32> {
    let whole = match geometry {
        Geometry::LayerTarget { size: [w, h], .. } | Geometry::Direct { size: [w, h], .. } => {
            let [w, h] = [w as f32 * 0.5, h as f32 * 0.5];
            ir::MeshQuad {
                rect: [-w, -h, w, h],
                uv: [0.0, 0.0, uv_max[0], uv_max[1]],
            }
        }
        Geometry::Effect | Geometry::Fullscreen => ir::MeshQuad {
            rect: [-1.0, -1.0, 1.0, 1.0],
            uv: [0.0, 0.0, uv_max[0], uv_max[1]],
        },
    };
    let quads = match (mesh, geometry) {
        (Some(m), Geometry::LayerTarget { .. } | Geometry::Direct { .. }) => m,
        _ => std::slice::from_ref(&whole),
    };
    let mut out = Vec::new();
    for q in quads {
        let [x0, y0, x1, y1] = q.rect;
        let [u0, v0, u1, v1] = q.uv;
        // (x, y) in 0..1 from the bottom-left corner; texture v runs top-down.
        for (x, y) in [
            (0.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (0.0, 1.0),
            (1.0, 0.0),
            (1.0, 1.0),
        ] {
            let position = [x0 + (x1 - x0) * x, y0 + (y1 - y0) * y, 0.0, 1.0];
            let uv = [u0 + (u1 - u0) * x, v1 + (v0 - v1) * y, 0.0, 0.0];
            for a in attributes {
                let n = a.components.clamp(1, 4) as usize;
                let values = match a.name.as_str() {
                    "a_Position" => position,
                    "a_TexCoord" => uv,
                    "a_Color" => [1.0; 4],
                    _ => [0.0; 4],
                };
                out.extend_from_slice(&values[..n]);
            }
        }
    }
    out
}

/// Triangle list of a puppet mesh at `positions` (one per puppet vertex),
/// attributes filled by name like [`pass_vertices`].
fn puppet_vertices(
    attributes: &[ir::Attribute],
    puppet: &ir::puppet::Puppet,
    positions: &[[f32; 2]],
    uv_max: [f32; 2],
) -> Vec<f32> {
    let mut out = Vec::with_capacity(puppet.indices.len() * 6);
    for &i in &puppet.indices {
        let i = i as usize;
        let (Some(v), Some(p)) = (puppet.vertices.get(i), positions.get(i)) else {
            continue;
        };
        let position = [p[0], p[1], 0.0, 1.0];
        let uv = [v.uv[0] * uv_max[0], v.uv[1] * uv_max[1], 0.0, 0.0];
        for a in attributes {
            let n = a.components.clamp(1, 4) as usize;
            let values = match a.name.as_str() {
                "a_Position" => position,
                "a_TexCoord" => uv,
                "a_Color" => [1.0; 4],
                _ => [0.0; 4],
            };
            out.extend_from_slice(&values[..n]);
        }
    }
    out
}

/// Layer quad in the scene's clip space: pos.xy + uv per vertex (strip order).
fn composite_quad(size: [u32; 2], t: Rows, camera: Rows) -> [f32; 16] {
    let half = size.map(|v| v as f32 * 0.5);
    let m = mat_mul(t, camera);
    let mut out = [0.0; 16];
    for (i, (x, y)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        let (lx, ly) = (x * half[0], y * half[1]);
        let clip: [f32; 4] = std::array::from_fn(|c| lx * m[0][c] + ly * m[1][c] + m[3][c]);
        // Perspective scenes: project the corner (uv then interpolates
        // affinely across the quad).
        let w = if clip[3].abs() > 1e-9 { clip[3] } else { 1.0 };
        out[i * 4..i * 4 + 4].copy_from_slice(&[
            clip[0] / w,
            clip[1] / w,
            (x + 1.0) * 0.5,
            (1.0 - y) * 0.5,
        ]);
    }
    out
}

/// The GPU layer for IR layer `index`, searching group children too.
fn find_layer(layers: &mut [GpuLayer], index: usize) -> Option<&mut GpuLayer> {
    for layer in layers {
        if layer.index == index {
            return Some(layer);
        }
        if let Some(found) = find_layer(&mut layer.children, index) {
            return Some(found);
        }
    }
    None
}

fn collect_targets(layer: &GpuLayer, out: &mut Vec<usize>) {
    out.extend(&layer.owned);
    for child in &layer.children {
        collect_targets(child, out);
    }
}

/// `scene` without texture pixels (uploaded already) or shader sources.
fn stripped(scene: &ir::Scene) -> ir::Scene {
    ir::Scene {
        width: scene.width,
        height: scene.height,
        clear_color: scene.clear_color,
        scheme_color: scene.scheme_color,
        hdr: scene.hdr,
        ambient_color: scene.ambient_color,
        skylight_color: scene.skylight_color,
        shake: scene.shake,
        parallax: scene.parallax,
        point_lights: scene.point_lights.clone(),
        tube_lights: scene.tube_lights.clone(),
        camera: scene.camera,
        nodes: scene.nodes.clone(),
        layers: scene.layers.clone(),
        textures: Vec::new(),
        fonts: scene.fonts.clone(),
        ..ir::Scene::default()
    }
}

/// Fills a layer target with the frame under the layer: a quad over the whole
/// target sampling the frame snapshot where `composite_quad` would land
/// (strip order, pos.xy + uv).
fn background_quad(size: [u32; 2], t: Rows, camera: Rows) -> [f32; 16] {
    let placed = composite_quad(size, t, camera);
    let mut out = [0.0; 16];
    for (i, (x, y)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        let (cx, cy) = (placed[i * 4], placed[i * 4 + 1]);
        out[i * 4..i * 4 + 4].copy_from_slice(&[x, y, (cx + 1.0) * 0.5, (1.0 - cy) * 0.5]);
    }
    out
}

fn upload(gpu: &Gpu, t: &ir::Texture) -> AssetTexture {
    let max = gpu.device.limits().max_texture_dimension_2d;
    let expected = u64::from(t.width) * u64::from(t.height) * 4;
    if t.width > max || t.height > max || t.rgba.len() as u64 != expected {
        tracing::warn!(texture = %t.name, t.width, t.height, "texture unusable; using placeholder");
        return upload(
            gpu,
            &ir::Texture {
                name: "placeholder".into(),
                width: 1,
                height: 1,
                image_width: 1,
                image_height: 1,
                rgba: vec![255; 4],
                mips: Vec::new(),
                clamp: true,
                nearest: false,
                frames: Vec::new(),
                video: None,
            },
        );
    }
    let size = wgpu::Extent3d {
        width: t.width.max(1),
        height: t.height.max(1),
        depth_or_array_layers: 1,
    };
    // Keep the mip levels whose sizes follow the halving chain exactly.
    let mut data = t.rgba.clone();
    let mut levels = 1;
    let (mut w, mut h) = (u64::from(t.width), u64::from(t.height));
    for mip in &t.mips {
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
        if mip.len() as u64 != w * h * 4 {
            break;
        }
        data.extend_from_slice(mip);
        levels += 1;
    }
    let texture = gpu.device.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor {
            label: Some(&t.name),
            size,
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &data,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    AssetTexture {
        texture,
        view,
        resolution: [
            t.width as f32,
            t.height as f32,
            t.image_width as f32,
            t.image_height as f32,
        ],
        sampler: usize::from(!t.clamp) + 2 * usize::from(t.nearest),
    }
}

fn build_program(
    gpu: &Gpu,
    compiler: &ShaderCompiler,
    p: &ir::ShaderProgram,
    uniforms_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> Result<Program, String> {
    let vs = compiler
        .compile(&p.vertex_hlsl, &p.vertex_entry, Stage::Vertex)
        .map_err(|e| e.to_string())?;
    let fs = compiler
        .compile(&p.fragment_hlsl, &p.fragment_entry, Stage::Fragment)
        .map_err(|e| e.to_string())?;
    // Anything besides our per-stage cbuffer (e.g. DXC's implicit `$Globals`)
    // would fail validation at draw time and void the whole frame.
    for (module, stage, binding) in [(&vs, "vertex", 0), (&fs, "fragment", 1)] {
        for (_, v) in module.global_variables.iter() {
            if v.space == lumen_gpu::shader::naga::AddressSpace::Uniform
                && v.binding
                    .as_ref()
                    .is_none_or(|b| b.group != 0 || b.binding != binding)
            {
                return Err(format!(
                    "{stage} shader has unexpected uniform buffer {:?}",
                    v.name.as_deref().unwrap_or("?")
                ));
            }
        }
    }
    let vs_uniforms = UniformLayout::reflect(&vs, 0, 0).unwrap_or_default();
    let fs_uniforms = UniformLayout::reflect(&fs, 0, 1).unwrap_or_default();
    let texture_count = p.texture_bindings.values().max().map_or(0, |m| m + 1);

    let device = &gpu.device;
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&p.name),
        source: wgpu::ShaderSource::Naga(Cow::Owned(vs)),
    });
    let fs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&p.name),
        source: wgpu::ShaderSource::Naga(Cow::Owned(fs)),
    });
    let entries: Vec<wgpu::BindGroupLayoutEntry> = (0..texture_count)
        .flat_map(|i| {
            [
                wgpu::BindGroupLayoutEntry {
                    binding: 2 * i,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2 * i + 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ]
        })
        .collect();
    let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("pass textures"),
        entries: &entries,
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(&p.name),
        bind_group_layouts: &[Some(uniforms_layout), Some(&textures_layout)],
        immediate_size: 0,
    });

    let mut program = Program {
        name: p.name.clone(),
        vs: vs_module,
        fs: fs_module,
        vs_entry: p.vertex_entry.clone(),
        fs_entry: p.fragment_entry.clone(),
        vs_uniforms,
        fs_uniforms,
        textures_layout,
        pipeline_layout,
        texture_count,
        texture_bindings: p.texture_bindings.clone(),
        attributes: p.attributes.clone(),
        pipelines: HashMap::new(),
    };
    for blend in [Blend::Opaque, Blend::Translucent, Blend::Additive] {
        let pipeline = create_pipeline(device, &program, blend, format, None);
        program.pipelines.insert(blend, pipeline);
    }
    if let Some(e) = pollster::block_on(scope.pop()) {
        return Err(e.to_string());
    }
    Ok(program)
}

fn create_pipeline(
    device: &wgpu::Device,
    p: &Program,
    blend: Blend,
    format: wgpu::TextureFormat,
    model: Option<ModelRaster>,
) -> wgpu::RenderPipeline {
    let mut offset = 0;
    let attributes: Vec<wgpu::VertexAttribute> = p
        .attributes
        .iter()
        .map(|a| {
            let format = match a.components {
                1 => wgpu::VertexFormat::Float32,
                2 => wgpu::VertexFormat::Float32x2,
                3 => wgpu::VertexFormat::Float32x3,
                _ => wgpu::VertexFormat::Float32x4,
            };
            let attr = wgpu::VertexAttribute {
                format,
                offset,
                shader_location: a.location,
            };
            offset += format.size();
            attr
        })
        .collect();
    let buffers = [Some(wgpu::VertexBufferLayout {
        array_stride: offset,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &attributes,
    })];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&p.name),
        layout: Some(&p.pipeline_layout),
        vertex: wgpu::VertexState {
            module: &p.vs,
            entry_point: Some(&p.vs_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: if attributes.is_empty() { &[] } else { &buffers },
        },
        fragment: Some(wgpu::FragmentState {
            module: &p.fs,
            entry_point: Some(&p.fs_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: blend_state(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            // Counter-clockwise fronts: 3509243656's sky is two shells, the
            // inner one (shown by WE) wound CCW towards the camera.
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: match model.map(|m| m.cull) {
                Some(ir::Cull::Back) => Some(wgpu::Face::Back),
                _ => None,
            },
            ..Default::default()
        },
        // Reversed depth (near = 1), cleared to 0.
        depth_stencil: model.map(|m| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(m.write),
            depth_compare: Some(if m.test {
                wgpu::CompareFunction::GreaterEqual
            } else {
                wgpu::CompareFunction::Always
            }),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

struct CompositeShared {
    layout: wgpu::BindGroupLayout,
    pipelines: HashMap<Blend, wgpu::RenderPipeline>,
    /// Copies colour only (alpha stays as cleared): background copies.
    background: wgpu::RenderPipeline,
}

impl CompositeShared {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];
        let make = |blend: Blend, write_mask: wgpu::ColorWrites| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("composite"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: 16,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: blend_state(blend),
                        write_mask,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = [Blend::Opaque, Blend::Translucent, Blend::Additive]
            .into_iter()
            .map(|b| (b, make(b, wgpu::ColorWrites::ALL)))
            .collect();
        let background = make(Blend::Opaque, wgpu::ColorWrites::COLOR);
        Self {
            layout,
            pipelines,
            background,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_is_smooth_and_bounded() {
        let mut prev = drift(0.0, 7, 1.0);
        for i in 1..2000 {
            let v = drift(i as f32 * 0.005, 7, 1.0);
            assert!((-1.0..=1.0).contains(&v));
            assert!((v - prev).abs() < 0.05, "jump at {i}");
            prev = v;
        }
    }

    #[test]
    fn half_floats_decode() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x3800), 0.5);
        assert_eq!(f16_to_f32(0), 0.0);
    }

    #[test]
    fn particles_face_the_camera_in_local_space() {
        // 3509243656: a nebula system turned 28° about y and scaled 1.5,
        // camera unrotated → right = (0.883, 0, 0.470) as captured.
        let model =
            lumen_scene::space::trs_mat4([0.0; 3], [0.0, 28f32.to_radians(), 0.0], [1.5; 3]);
        let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let [right, up, forward] = facing(model, axes);
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
        assert!(close(right, [0.8829, 0.0, 0.4695]), "{right:?}");
        assert!(close(up, [0.0, 1.0, 0.0]));
        assert!(close(forward, [-0.4695, 0.0, 0.8829]), "{forward:?}");
    }

    #[test]
    fn normal_matrix_undoes_non_uniform_scale() {
        // Scale (2, 1, 1): a normal along x stays along x, (1, 1, 0) tilts
        // towards y (inverse transpose), in column upload order.
        let model = lumen_scene::space::trs_mat4([5.0, 0.0, 0.0], [0.0; 3], [2.0, 1.0, 1.0]);
        let n = normal_matrix(model);
        assert_eq!(n, [0.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn matrices_upload_as_columns() {
        let m = upload_matrix(mat_mul(IDENTITY_ROWS, ortho_rows(3840.0, 2160.0)));
        // Column 0 = (2/w, 0, 0, -1): x' = 2x/w - 1.
        assert_eq!(&m[0..4], &[2.0 / 3840.0, 0.0, 0.0, -1.0]);
        assert_eq!(&m[12..16], &[0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn direct_mvp_matches_capture() {
        // Scene 3414847823, layer "static": origin (972, 530) in a 1920×1080 scene.
        let model = model_rows(lumen_scene::affine([972.0, 530.0], 0.0, [1.0, 1.0]));
        let rows = mat_mul(model, ortho_rows(1920.0, 1080.0));
        assert!((rows[0][0] - 0.001042).abs() < 1e-6);
        assert!((rows[3][0] - 0.0125).abs() < 1e-4);
        assert!((rows[3][1] + 0.018519).abs() < 1e-4);
    }

    #[test]
    fn quad_fills_attributes_by_name() {
        let attrs = [
            ir::Attribute {
                name: "a_Position".into(),
                location: 0,
                components: 3,
            },
            ir::Attribute {
                name: "a_TexCoord".into(),
                location: 1,
                components: 2,
            },
        ];
        let geometry = Geometry::LayerTarget {
            size: [100, 50],
            model: IDENTITY_ROWS,
        };
        let q = pass_vertices(&attrs, geometry, [0.5, 1.0], None);
        assert_eq!(q.len(), 6 * 5);
        // Bottom-left vertex samples the bottom of the image (v = 1).
        assert_eq!(&q[0..5], &[-50.0, -25.0, 0.0, 0.0, 1.0]);
        // Top-right: (w/2, h/2), uv (u_max, 0).
        assert_eq!(&q[25..30], &[50.0, 25.0, 0.0, 0.5, 0.0]);

        // A mesh replaces the quad: one glyph, top-left uv (0.25, 0.5).
        let glyph = ir::MeshQuad {
            rect: [1.0, 2.0, 11.0, 22.0],
            uv: [0.25, 0.5, 0.75, 1.0],
        };
        let m = pass_vertices(&attrs, geometry, [1.0, 1.0], Some(&[glyph, glyph]));
        assert_eq!(m.len(), 2 * 6 * 5);
        assert_eq!(&m[0..5], &[1.0, 2.0, 0.0, 0.25, 1.0]);
        assert_eq!(&m[25..30], &[11.0, 22.0, 0.0, 0.75, 0.5]);
    }
}
