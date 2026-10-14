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

use lumen_gpu::shader::{ShaderCompiler, ShaderError, Stage, UniformLayout};
use lumen_gpu::{Gpu, wgpu};
use lumen_scene::{self as ir, Blend, TextureBinding};
use wgpu::util::DeviceExt as _;

/// Every scene target uses WE's gamma-space RGBA8 (no sRGB conversion).
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

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
}

impl Default for FrameInputs {
    fn default() -> Self {
        Self {
            time: 0.0,
            daytime: 0.5,
            pointer: [0.5, 0.5],
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
    _texture: wgpu::Texture,
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
    uniforms: wgpu::BindGroup,
    textures: wgpu::BindGroup,
    vs_buffer: wgpu::Buffer,
    fs_buffer: wgpu::Buffer,
    vs_data: Vec<u8>,
    fs_data: Vec<u8>,
    target: usize,
}

struct GpuLayer {
    passes: Vec<GpuPass>,
    composite: wgpu::BindGroup,
    composite_vertices: wgpu::Buffer,
    blend: Blend,
}

pub struct SceneRenderer {
    size: [u32; 2],
    clear: wgpu::Color,
    targets: Vec<Target>,
    scene_target: Target,
    programs: Vec<Option<Program>>,
    layers: Vec<GpuLayer>,
    composite_pipelines: HashMap<Blend, wgpu::RenderPipeline>,
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

/// Layer-local ortho: `0..w × 0..h` → clip space (as captured).
fn layer_mvp(w: f32, h: f32) -> [f32; 16] {
    upload_matrix([
        [2.0 / w, 0.0, 0.0, 0.0],
        [0.0, 2.0 / h, 0.0, 0.0],
        [0.0, 0.0, 0.0005, 0.0],
        [-1.0, -1.0, 0.5, 1.0],
    ])
}

/// Effect passes draw a clip-space quad through this near-identity MVP.
fn effect_mvp() -> [f32; 16] {
    upload_matrix([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0005, 0.0],
        [0.0, 0.0, 0.5, 1.0],
    ])
}

const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
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

fn create_target(device: &wgpu::Device, label: &str, size: [u32; 2]) -> Target {
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
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
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
    Placeholder,
}

impl SceneRenderer {
    pub fn new(
        gpu: &Gpu,
        scene: &ir::Scene,
        compiler: &ShaderCompiler,
    ) -> Result<Self, SceneError> {
        if scene.width == 0 || scene.height == 0 {
            return Err(SceneError::Empty);
        }
        let device = &gpu.device;
        let samplers: Vec<wgpu::Sampler> = SAMPLERS
            .iter()
            .map(|(address, filter)| {
                device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("scene sampler"),
                    address_mode_u: *address,
                    address_mode_v: *address,
                    mag_filter: *filter,
                    min_filter: *filter,
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
                clamp: true,
                nearest: false,
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
            match build_program(gpu, compiler, p, &uniforms_layout) {
                Ok(program) => programs.push(Some(program)),
                Err(e) => {
                    tracing::warn!(program = %p.name, "shader skipped: {e}");
                    programs.push(None);
                }
            }
        }

        let composite = CompositeShared::new(device);
        let mut renderer = Self {
            size: [scene.width, scene.height],
            clear: wgpu::Color {
                r: f64::from(scene.clear_color[0]),
                g: f64::from(scene.clear_color[1]),
                b: f64::from(scene.clear_color[2]),
                a: f64::from(scene.clear_color[3]),
            },
            targets: Vec::new(),
            scene_target: create_target(device, "scene", [scene.width, scene.height]),
            programs,
            layers: Vec::new(),
            composite_pipelines: composite.pipelines,
        };

        let ctx = BuildContext {
            gpu,
            scene,
            assets: &assets,
            placeholder: &placeholder,
            samplers: &samplers,
            uniforms_layout: &uniforms_layout,
        };
        for layer in scene.layers.iter().filter(|l| l.visible) {
            if let Some(gpu_layer) = renderer.build_layer(&ctx, layer, &composite.layout) {
                renderer.layers.push(gpu_layer);
            }
        }
        // Asset textures stay alive through their views/bind groups.
        drop(assets);
        Ok(renderer)
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// The composited scene (scene-sized, [`FORMAT`]); sample it to display.
    pub fn output(&self) -> &wgpu::TextureView {
        &self.scene_target.view
    }

    fn build_layer(
        &mut self,
        ctx: &BuildContext<'_>,
        layer: &ir::ImageLayer,
        composite_layout: &wgpu::BindGroupLayout,
    ) -> Option<GpuLayer> {
        let device = &ctx.gpu.device;
        let ping = self.targets.len();
        self.targets.push(create_target(
            device,
            &format!("{} a", layer.name),
            layer.size,
        ));
        self.targets.push(create_target(
            device,
            &format!("{} b", layer.name),
            layer.size,
        ));
        let mut current = ping;

        let mut passes = Vec::new();
        // Albedo: no blending inside the layer target (blend applies at composite).
        let albedo = PassPlan {
            pass: &layer.albedo,
            blend: Blend::Opaque,
            previous: None,
            fbos: &HashMap::new(),
            target: ping,
            albedo_size: Some(layer.size),
        };
        passes.push(self.build_pass(ctx, &albedo)?);

        for effect in &layer.effects {
            let mut fbos = HashMap::new();
            for fbo in &effect.fbos {
                let size = layer.size.map(|s| (s / fbo.scale_div).max(1));
                fbos.insert(fbo.name.clone(), self.targets.len());
                self.targets.push(create_target(
                    device,
                    &format!("{} {}", layer.name, fbo.name),
                    size,
                ));
            }
            for pass in &effect.passes {
                let target = match &pass.target {
                    Some(name) => match fbos.get(name) {
                        Some(t) => *t,
                        None => {
                            tracing::warn!(effect = %effect.name, "unknown target {name}");
                            continue;
                        }
                    },
                    None => ping + 1 - (current - ping),
                };
                let plan = PassPlan {
                    pass,
                    blend: pass.blend,
                    previous: Some(current),
                    fbos: &fbos,
                    target,
                    albedo_size: None,
                };
                if let Some(p) = self.build_pass(ctx, &plan) {
                    passes.push(p);
                    if pass.target.is_none() {
                        current = target;
                    }
                }
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
            contents: bytemuck::cast_slice(&composite_quad(layer, self.size)),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Some(GpuLayer {
            passes,
            composite,
            composite_vertices,
            blend: layer.blend,
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
                    plan.previous.map_or(Source::Placeholder, Source::Target)
                }
                Some(TextureBinding::Fbo(name)) => plan
                    .fbos
                    .get(name)
                    .map_or(Source::Placeholder, |t| Source::Target(*t)),
                None => Source::Placeholder,
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
        let mvp = match plan.albedo_size {
            Some([w, h]) => layer_mvp(w as f32, h as f32),
            None => effect_mvp(),
        };
        let [sw, sh] = self.size.map(|v| v as f32);
        let [tw, th] = target_size.map(|v| v as f32);
        for (layout, buf) in [&program.vs_uniforms, &program.fs_uniforms]
            .into_iter()
            .zip(data.iter_mut())
        {
            for m in &layout.members {
                if m.rows == 4 && m.columns == 4 {
                    layout.write(buf, &m.name, &IDENTITY);
                }
            }
            for (name, value) in &plan.pass.constants {
                layout.write(buf, name, value);
            }
            layout.write(buf, "g_ModelViewProjectionMatrix", &mvp);
            for (slot, res) in &resolutions {
                layout.write(buf, &format!("g_Texture{slot}Resolution"), res);
            }
            layout.write(buf, "g_Color4", &[1.0; 4]);
            layout.write(buf, "g_Color", &[1.0; 3]);
            layout.write(buf, "g_Screen", &[sw, sh, sw / sh]);
            layout.write(buf, "g_TexelSize", &[1.0 / tw, 1.0 / th]);
            layout.write(buf, "g_TexelSizeHalf", &[0.5 / tw, 0.5 / th]);
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

        // Albedo UVs stop at the image inside a padded texture.
        let uv_max = match (plan.albedo_size, plan.pass.textures.get(&0)) {
            (Some(_), Some(TextureBinding::Asset(id))) => {
                ctx.assets.get(id.0).map_or([1.0, 1.0], |a| {
                    [
                        a.resolution[2] / a.resolution[0],
                        a.resolution[3] / a.resolution[1],
                    ]
                })
            }
            _ => [1.0, 1.0],
        };
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("pass quad"),
            contents: bytemuck::cast_slice(&pass_quad(
                &program.attributes,
                plan.albedo_size,
                uv_max,
            )),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Some(GpuPass {
            program: index,
            pipeline,
            vertices,
            uniforms,
            textures,
            vs_buffer,
            fs_buffer,
            vs_data,
            fs_data,
            target: plan.target,
        })
    }

    /// Blocking readback of [`SceneRenderer::output`] as tightly packed RGBA8
    /// (headless capture and golden tests; not for the frame loop).
    pub fn read_output(&self, gpu: &Gpu) -> Option<Vec<u8>> {
        read_target(gpu, &self.scene_target)
    }

    /// Every intermediate target as (label, size, RGBA8), for debugging passes.
    pub fn read_targets(&self, gpu: &Gpu) -> Vec<(String, [u32; 2], Vec<u8>)> {
        self.targets
            .iter()
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
        let row = 4 * w;
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
            out.extend_from_slice(&r[..row as usize]);
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
        for layer in &mut self.layers {
            for pass in &mut layer.passes {
                let Some(Some(program)) = self.programs.get(pass.program) else {
                    continue;
                };
                for (layout, buf, gpu_buf) in [
                    (&program.vs_uniforms, &mut pass.vs_data, &pass.vs_buffer),
                    (&program.fs_uniforms, &mut pass.fs_data, &pass.fs_buffer),
                ] {
                    let mut dirty = layout.write(buf, "g_Time", &[inputs.time]);
                    dirty |= layout.write(buf, "g_Daytime", &[inputs.daytime]);
                    dirty |= layout.write(buf, "g_PointerPosition", &inputs.pointer);
                    dirty |= layout.write(buf, "g_PointerPositionLast", &inputs.pointer);
                    if dirty {
                        gpu.queue.write_buffer(gpu_buf, 0, buf);
                    }
                }
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(&program.name),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.targets[pass.target].view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                rp.set_pipeline(&pass.pipeline);
                rp.set_bind_group(0, &pass.uniforms, &[]);
                rp.set_bind_group(1, &pass.textures, &[]);
                rp.set_vertex_buffer(0, pass.vertices.slice(..));
                rp.draw(0..4, 0..1);
            }
        }

        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.scene_target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            for layer in &self.layers {
                let Some(pipeline) = self.composite_pipelines.get(&layer.blend) else {
                    continue;
                };
                rp.set_pipeline(pipeline);
                rp.set_bind_group(0, &layer.composite, &[]);
                rp.set_vertex_buffer(0, layer.composite_vertices.slice(..));
                rp.draw(0..4, 0..1);
            }
        }
        gpu.queue.submit([encoder.finish()]);
    }
}

struct BuildContext<'a> {
    gpu: &'a Gpu,
    #[allow(dead_code, reason = "kept for upcoming per-scene engine uniforms")]
    scene: &'a ir::Scene,
    assets: &'a [AssetTexture],
    placeholder: &'a AssetTexture,
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
    /// `Some(layer size)` for the albedo pass (layer-local quad and MVP).
    albedo_size: Option<[u32; 2]>,
}

/// Triangle-strip quad (BL, BR, TL, TR) with each attribute filled by name.
fn pass_quad(attributes: &[ir::Attribute], albedo: Option<[u32; 2]>, uv_max: [f32; 2]) -> Vec<f32> {
    let mut out = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let position = match albedo {
            Some([w, h]) => [x * w as f32, y * h as f32, 0.0, 1.0],
            None => [x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0],
        };
        // Texture v runs top-down; clip y runs bottom-up.
        let uv = [x * uv_max[0], (1.0 - y) * uv_max[1], 0.0, 0.0];
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
fn composite_quad(layer: &ir::ImageLayer, scene: [u32; 2]) -> [f32; 16] {
    let [sw, sh] = scene.map(|v| v as f32);
    let half = [
        layer.size[0] as f32 * layer.scale[0] * 0.5,
        layer.size[1] as f32 * layer.scale[1] * 0.5,
    ];
    let (sin, cos) = layer.angles[2].sin_cos();
    let mut out = [0.0; 16];
    for (i, (x, y)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        let (lx, ly) = (x * half[0], y * half[1]);
        let px = layer.origin[0] + lx * cos - ly * sin;
        let py = layer.origin[1] + lx * sin + ly * cos;
        out[i * 4..i * 4 + 4].copy_from_slice(&[
            px / sw * 2.0 - 1.0,
            py / sh * 2.0 - 1.0,
            (x + 1.0) * 0.5,
            (1.0 - y) * 0.5,
        ]);
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
                clamp: true,
                nearest: false,
            },
        );
    }
    let size = wgpu::Extent3d {
        width: t.width.max(1),
        height: t.height.max(1),
        depth_or_array_layers: 1,
    };
    let texture = gpu.device.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor {
            label: Some(&t.name),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &t.rgba,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    AssetTexture {
        _texture: texture,
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
) -> Result<Program, String> {
    let vs = compiler
        .compile(&p.vertex_hlsl, &p.vertex_entry, Stage::Vertex)
        .map_err(|e| e.to_string())?;
    let fs = compiler
        .compile(&p.fragment_hlsl, &p.fragment_entry, Stage::Fragment)
        .map_err(|e| e.to_string())?;
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
        let pipeline = create_pipeline(device, &program, blend);
        program.pipelines.insert(blend, pipeline);
    }
    if let Some(e) = pollster::block_on(scope.pop()) {
        return Err(e.to_string());
    }
    Ok(program)
}

fn create_pipeline(device: &wgpu::Device, p: &Program, blend: Blend) -> wgpu::RenderPipeline {
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
                format: FORMAT,
                blend: blend_state(blend),
                write_mask: wgpu::ColorWrites::ALL,
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
}

struct CompositeShared {
    layout: wgpu::BindGroupLayout,
    pipelines: HashMap<Blend, wgpu::RenderPipeline>,
}

impl CompositeShared {
    fn new(device: &wgpu::Device) -> Self {
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
        let mut pipelines = HashMap::new();
        for blend in [Blend::Opaque, Blend::Translucent, Blend::Additive] {
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                        format: FORMAT,
                        blend: blend_state(blend),
                        write_mask: wgpu::ColorWrites::ALL,
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
            });
            pipelines.insert(blend, pipeline);
        }
        Self { layout, pipelines }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrices_upload_as_columns() {
        let m = layer_mvp(3840.0, 2160.0);
        // Column 0 = (2/w, 0, 0, -1): x' = 2x/w - 1.
        assert_eq!(&m[0..4], &[2.0 / 3840.0, 0.0, 0.0, -1.0]);
        assert_eq!(&m[12..16], &[0.0, 0.0, 0.0, 1.0]);
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
        let q = pass_quad(&attrs, Some([100, 50]), [0.5, 1.0]);
        assert_eq!(q.len(), 4 * 5);
        // Bottom-left vertex samples the bottom of the image (v = 1).
        assert_eq!(&q[0..5], &[0.0, 0.0, 0.0, 0.0, 1.0]);
        // Top-right: (w, h), uv (u_max, 0).
        assert_eq!(&q[15..20], &[100.0, 50.0, 0.0, 0.5, 0.0]);
    }
}
