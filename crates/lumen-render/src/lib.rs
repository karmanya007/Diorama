//! Rendering (L3): image/video blits over each output (Phase 1) and the
//! scene IR renderer (Phase 2, [`scene`]).

pub mod scene;
mod video;

use lumen_gpu::{Gpu, wgpu};

pub use video::{Nv12Planes, VideoBlit};

/// How an image is mapped onto an output whose aspect ratio differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FitMode {
    /// Cover the whole output, cropping the image (WE's behaviour for scenes).
    #[default]
    Fill,
    /// Show the whole image, letterboxing with the clear colour.
    Fit,
    /// Ignore aspect ratio.
    Stretch,
}

/// Target-UV → image-UV scale around the centre for `mode`.
pub fn uv_scale(mode: FitMode, image: (u32, u32), target: (u32, u32)) -> [f32; 2] {
    let ai = image.0 as f32 / image.1.max(1) as f32;
    let at = target.0 as f32 / target.1.max(1) as f32;
    match mode {
        FitMode::Stretch => [1.0, 1.0],
        FitMode::Fill if at > ai => [1.0, ai / at],
        FitMode::Fill => [at / ai, 1.0],
        FitMode::Fit if at > ai => [at / ai, 1.0],
        FitMode::Fit => [1.0, ai / at],
    }
}

/// Rectangle in virtual-desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn size(self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Smallest rectangle containing all `rects` (the span canvas), or `None` if empty.
    pub fn bounding(rects: impl IntoIterator<Item = Rect>) -> Option<Rect> {
        let mut it = rects.into_iter();
        let first = it.next()?;
        let (mut x0, mut y0) = (i64::from(first.x), i64::from(first.y));
        let (mut x1, mut y1) = (x0 + i64::from(first.width), y0 + i64::from(first.height));
        for r in it {
            x0 = x0.min(i64::from(r.x));
            y0 = y0.min(i64::from(r.y));
            x1 = x1.max(i64::from(r.x) + i64::from(r.width));
            y1 = y1.max(i64::from(r.y) + i64::from(r.height));
        }
        Some(Rect {
            x: x0 as i32,
            y: y0 as i32,
            width: (x1 - x0) as u32,
            height: (y1 - y0) as u32,
        })
    }
}

/// Maps a target's UVs to image UVs (`image_uv = uv * scale + offset`) when
/// the image is fitted to `canvas` with `mode` and `target` shows part of it.
/// Per-output wallpapers use `canvas == target`; span mode uses the bounding
/// box of all outputs as the canvas so one image continues across monitors.
pub fn uv_transform(
    mode: FitMode,
    image: (u32, u32),
    canvas: Rect,
    target: Rect,
) -> ([f32; 2], [f32; 2]) {
    let s = uv_scale(mode, image, canvas.size());
    let cw = canvas.width.max(1) as f32;
    let ch = canvas.height.max(1) as f32;
    let rel = [
        (target.x - canvas.x) as f32 / cw,
        (target.y - canvas.y) as f32 / ch,
    ];
    let frac = [target.width as f32 / cw, target.height as f32 / ch];
    (
        [s[0] * frac[0], s[1] * frac[1]],
        [(rel[0] - 0.5) * s[0] + 0.5, (rel[1] - 0.5) * s[1] + 0.5],
    )
}

/// Largest on-screen size the image is ever drawn at across `targets` under
/// `mode`, so it can be downscaled once on the CPU before upload (sharp
/// sampling without mipmaps, and less VRAM — P12).
pub fn display_size(mode: FitMode, image: (u32, u32), targets: &[(u32, u32)]) -> (u32, u32) {
    scaled(image, fit_factor(mode, image, targets).min(1.0))
}

/// Size to render a scene at so that it covers its largest on-screen use
/// exactly (WE renders straight into its swapchain): may be larger than
/// the scene's own size, never larger than `max_dim` on either side.
pub fn render_size(
    mode: FitMode,
    scene: (u32, u32),
    targets: &[(u32, u32)],
    max_dim: u32,
) -> (u32, u32) {
    let (w, h) = (scene.0.max(1) as f32, scene.1.max(1) as f32);
    let limit = max_dim as f32 / w.max(h);
    scaled(scene, fit_factor(mode, scene, targets).min(limit))
}

fn fit_factor(mode: FitMode, image: (u32, u32), targets: &[(u32, u32)]) -> f32 {
    let (iw, ih) = (image.0.max(1) as f32, image.1.max(1) as f32);
    targets
        .iter()
        .map(|&(tw, th)| {
            let (sx, sy) = (tw as f32 / iw, th as f32 / ih);
            match mode {
                FitMode::Fill => sx.max(sy),
                FitMode::Fit => sx.min(sy),
                FitMode::Stretch => sx.max(sy),
            }
        })
        .fold(0.0_f32, f32::max)
}

fn scaled(image: (u32, u32), factor: f32) -> (u32, u32) {
    if factor <= 0.0 {
        return image;
    }
    (
        ((image.0 as f32 * factor).round() as u32).max(1),
        ((image.1 as f32 * factor).round() as u32).max(1),
    )
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Params {
    scale: [f32; 2],
    offset: [f32; 2],
    clear: [f32; 4],
}

/// GPU resources for drawing one image onto any number of outputs.
#[derive(Debug)]
pub struct ImageBlit {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    params: wgpu::Buffer,
    image_size: (u32, u32),
}

impl ImageBlit {
    /// `rgba` is tightly packed 8-bit RGBA, gamma-encoded (uploaded as UNORM,
    /// not sRGB, to match WE's gamma-space rendering).
    pub fn new(
        gpu: &Gpu,
        target_format: wgpu::TextureFormat,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> Self {
        let device = &gpu.device;
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wallpaper image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self::from_view(gpu, target_format, &view, width, height)
    }

    /// Blits an existing texture (e.g. a rendered scene). Its format must be
    /// filterable float; views keep their texture alive.
    pub fn from_view(
        gpu: &Gpu,
        target_format: wgpu::TextureFormat,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Self {
        let device = &gpu.device;
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("wallpaper sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blit params"),
            size: size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params.as_entire_binding(),
                },
            ],
        });

        let pipeline = fullscreen_pipeline(
            device,
            "blit",
            include_str!("blit.wgsl"),
            &layout,
            target_format,
        );

        Self {
            pipeline,
            bind_group,
            params,
            image_size: (width, height),
        }
    }

    /// Draws the image onto `target`, which shows the `target_rect` part of
    /// `canvas` (see [`uv_transform`]), and submits.
    pub fn draw(
        &self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        canvas: Rect,
        target_rect: Rect,
        mode: FitMode,
        clear: [f32; 4],
    ) {
        let (scale, offset) = uv_transform(mode, self.image_size, canvas, target_rect);
        let params = Params {
            scale,
            offset,
            clear,
        };
        gpu.queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        draw_fullscreen(gpu, &self.pipeline, &self.bind_group, target, clear);
    }
}

/// Pipeline drawing a fullscreen triangle (`vs_main`/`fs_main` in `source`) with one bind group.
pub(crate) fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    source: &str,
    layout: &wgpu::BindGroupLayout,
    target_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// Clears `target` and draws one fullscreen triangle, then submits.
pub(crate) fn draw_fullscreen(
    gpu: &Gpu,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    target: &wgpu::TextureView,
    clear: [f32; 4],
) {
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fullscreen"),
        });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fullscreen"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(clear[0]),
                        g: f64::from(clear[1]),
                        b: f64::from(clear[2]),
                        a: f64::from(clear[3]),
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    gpu.queue.submit([encoder.finish()]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_matches_we_cover_fit() {
        // RenderDoc: WE drew a 3840×2160 scene on a 3000×1920 desktop with x-scale 2/3375,
        // i.e. 3375 of 3840 image pixels visible horizontally, full height.
        let s = uv_scale(FitMode::Fill, (3840, 2160), (3000, 1920));
        assert!((s[0] - 3375.0 / 3840.0).abs() < 1e-4);
        assert!((s[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn fit_letterboxes_and_stretch_ignores_aspect() {
        let s = uv_scale(FitMode::Fit, (1000, 1000), (2000, 1000));
        assert_eq!(s, [2.0, 1.0]);
        assert_eq!(uv_scale(FitMode::Stretch, (1, 9), (9, 1)), [1.0, 1.0]);
    }

    const MAIN: Rect = Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    const PORTRAIT: Rect = Rect {
        x: -1080,
        y: -546,
        width: 1080,
        height: 1920,
    };

    #[test]
    fn bounding_box_of_real_layout() {
        let canvas = Rect::bounding([MAIN, PORTRAIT]).unwrap();
        assert_eq!(
            canvas,
            Rect {
                x: -1080,
                y: -546,
                width: 3000,
                height: 1920
            }
        );
        assert_eq!(Rect::bounding([]), None);
    }

    #[test]
    fn per_output_transform_equals_centered_scale() {
        let (scale, offset) = uv_transform(FitMode::Fill, (3840, 2160), MAIN, MAIN);
        let s = uv_scale(FitMode::Fill, (3840, 2160), MAIN.size());
        assert_eq!(scale, s);
        assert_eq!(offset, [0.5 - 0.5 * s[0], 0.5 - 0.5 * s[1]]);
    }

    #[test]
    fn span_slices_are_continuous() {
        let image = (3840, 2160);
        let canvas = Rect::bounding([MAIN, PORTRAIT]).unwrap();
        let (sp, op) = uv_transform(FitMode::Fill, image, canvas, PORTRAIT);
        let (sm, om) = uv_transform(FitMode::Fill, image, canvas, MAIN);
        // Right edge of the portrait monitor meets the left edge of the main one (x = 0).
        let portrait_right = 1.0 * sp[0] + op[0];
        let main_left = om[0];
        assert!((portrait_right - main_left).abs() < 1e-6);
        // Same image pixels per screen pixel on both monitors.
        assert!((sp[0] / 1080.0 - sm[0] / 1920.0).abs() < 1e-9);
        // Cover fit over 3000×1920 crops width only: the whole canvas spans 3375/3840 of the image.
        let canvas_left = op[0];
        let canvas_right = om[0] + sm[0];
        assert!(((canvas_right - canvas_left) - 3375.0 / 3840.0).abs() < 1e-4);
    }

    #[test]
    fn render_size_follows_outputs_both_ways() {
        // 4K scene on this machine's 1080p + portrait monitors (per output):
        // the portrait one needs 1920 px of height → 3413×1920.
        let outputs = [(1920, 1080), (1080, 1920)];
        assert_eq!(
            render_size(FitMode::Fill, (3840, 2160), &outputs, 8192),
            (3413, 1920)
        );
        // 1080p scene on a 4K monitor is rendered at 4K, capped by the device.
        assert_eq!(
            render_size(FitMode::Fill, (1920, 1080), &[(3840, 2160)], 8192),
            (3840, 2160)
        );
        assert_eq!(
            render_size(FitMode::Fill, (1920, 1080), &[(3840, 2160)], 2048),
            (2048, 1152)
        );
    }

    #[test]
    fn display_size_downscales_to_largest_need() {
        // 4K image on 1080p + portrait 1080×1920 outputs, cover: portrait needs scale 1920/2160.
        let size = display_size(FitMode::Fill, (3840, 2160), &[(1920, 1080), (1080, 1920)]);
        assert_eq!(size, (3413, 1920));
        // Never upscales.
        assert_eq!(
            display_size(FitMode::Fill, (640, 480), &[(3840, 2160)]),
            (640, 480)
        );
    }
}
