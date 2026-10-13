//! Rendering (L3). Phase 1: a single image drawn over each output.
//! Scene rendering (layers, effect chains) arrives in Phase 2.

use lumen_gpu::{Gpu, wgpu};

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

/// Largest on-screen size the image is ever drawn at across `targets` under
/// `mode`, so it can be downscaled once on the CPU before upload (sharp
/// sampling without mipmaps, and less VRAM — P12).
pub fn display_size(mode: FitMode, image: (u32, u32), targets: &[(u32, u32)]) -> (u32, u32) {
    let (iw, ih) = (image.0.max(1) as f32, image.1.max(1) as f32);
    let factor = targets
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
        .min(1.0);
    if factor <= 0.0 {
        return image;
    }
    (
        ((iw * factor).round() as u32).max(1),
        ((ih * factor).round() as u32).max(1),
    )
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    scale: [f32; 2],
    _pad: [f32; 2],
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
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
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

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
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
        });

        Self {
            pipeline,
            bind_group,
            params,
            image_size: (width, height),
        }
    }

    /// Draws the image onto `target` (sized `target_size`) and submits.
    pub fn draw(
        &self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        target_size: (u32, u32),
        mode: FitMode,
        clear: [f32; 4],
    ) {
        let params = Params {
            scale: uv_scale(mode, self.image_size, target_size),
            _pad: [0.0; 2],
            clear,
        };
        gpu.queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("blit"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blit"),
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
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        gpu.queue.submit([encoder.finish()]);
    }
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
