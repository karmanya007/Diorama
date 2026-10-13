//! NV12 video frames → screen. Planes are uploaded each frame (`Rg8`/`R8`
//! textures); colour conversion happens in the shader.
//!
//! Upload goes through CPU memory for now. Zero-copy from the decoder's
//! D3D11 textures is a later optimisation (plan, architecture issue #9).

use lumen_gpu::{Gpu, wgpu};

use crate::{FitMode, Params, Rect, draw_fullscreen, fullscreen_pipeline, uv_transform};

/// One NV12 frame borrowed from the decoder.
#[derive(Debug, Clone, Copy)]
pub struct Nv12Planes<'a> {
    pub width: u32,
    pub height: u32,
    /// Bytes per row for both planes.
    pub stride: u32,
    pub luma: &'a [u8],
    pub chroma: &'a [u8],
}

#[derive(Debug)]
pub struct VideoBlit {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    params: wgpu::Buffer,
    luma: wgpu::Texture,
    chroma: wgpu::Texture,
    size: (u32, u32),
}

impl VideoBlit {
    pub fn new(gpu: &Gpu, target_format: wgpu::TextureFormat, width: u32, height: u32) -> Self {
        let device = &gpu.device;
        let plane = |label, format, w: u32, h: u32| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w.max(1),
                    height: h.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let luma = plane("video luma", wgpu::TextureFormat::R8Unorm, width, height);
        let chroma = plane(
            "video chroma",
            wgpu::TextureFormat::Rg8Unorm,
            width.div_ceil(2),
            height.div_ceil(2),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("video sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("video params"),
            size: size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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
        let luma_view = luma.create_view(&wgpu::TextureViewDescriptor::default());
        let chroma_view = chroma.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&luma_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&chroma_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        let pipeline = fullscreen_pipeline(
            device,
            "video",
            include_str!("video.wgsl"),
            &layout,
            target_format,
        );
        Self {
            pipeline,
            bind_group,
            params,
            luma,
            chroma,
            size: (width, height),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Uploads a decoded frame. Frames of a different size are ignored (the
    /// caller recreates the blit when the stream size changes).
    pub fn upload(&self, gpu: &Gpu, frame: Nv12Planes<'_>) {
        if (frame.width, frame.height) != self.size {
            return;
        }
        let write = |texture: &wgpu::Texture, data: &[u8], w: u32, h: u32| {
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(frame.stride),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        };
        let (w, h) = self.size;
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let luma_len = (frame.stride * h) as usize;
        let chroma_len = (frame.stride * ch) as usize;
        if frame.luma.len() >= luma_len && frame.chroma.len() >= chroma_len {
            write(&self.luma, &frame.luma[..luma_len], w, h);
            write(&self.chroma, &frame.chroma[..chroma_len], cw, ch);
        }
    }

    /// Draws the last uploaded frame; same placement rules as [`crate::ImageBlit::draw`].
    pub fn draw(
        &self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        canvas: Rect,
        target_rect: Rect,
        mode: FitMode,
        clear: [f32; 4],
    ) {
        let (scale, offset) = uv_transform(mode, self.size, canvas, target_rect);
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
