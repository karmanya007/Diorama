//! Text layers: lays out a layer's [`ir::TextSpec`] with `diorama-text` and
//! keeps its glyph atlas on the GPU (`R8Unorm`, sampled as `(r, 0, 0, 1)`
//! like WE's font textures).

use diorama_gpu::{Gpu, wgpu};
use diorama_scene as ir;
use diorama_text::{Font, HAlign, Style, TextEngine, VAlign};

/// A text layer's text box: target size, where its centre sits relative to
/// the layer's anchor, and the glyph quads around that centre.
pub(crate) struct LaidOut {
    pub size: [u32; 2],
    pub centre: [f32; 2],
    pub mesh: Vec<ir::MeshQuad>,
}

struct GpuAtlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: u32,
    generation: u64,
}

pub(crate) struct TextState {
    /// Centre of the last laid-out text box relative to the anchor.
    pub centre: [f32; 2],
    font: Font,
    engine: TextEngine,
    atlas: Option<GpuAtlas>,
}

fn halign(a: ir::TextAlign) -> HAlign {
    match a {
        ir::TextAlign::Start => HAlign::Left,
        ir::TextAlign::Center => HAlign::Center,
        ir::TextAlign::End => HAlign::Right,
    }
}

fn valign(a: ir::TextAlign) -> VAlign {
    match a {
        ir::TextAlign::Start => VAlign::Top,
        ir::TextAlign::Center => VAlign::Center,
        ir::TextAlign::End => VAlign::Bottom,
    }
}

impl TextState {
    pub fn new(font: &ir::FontData) -> Option<Self> {
        Some(Self {
            font: Font::new(font.data.clone(), 0).ok()?,
            engine: TextEngine::new(),
            atlas: None,
            centre: [0.0, 0.0],
        })
    }

    /// Lays out `text` and brings the GPU atlas up to date.
    pub fn layout(&mut self, gpu: &Gpu, spec: &ir::TextSpec, text: &str) -> LaidOut {
        let style = Style {
            pixel_size: spec.pixel_size,
            halign: halign(spec.halign),
            valign: valign(spec.valign),
            max_width: spec.max_width,
            max_rows: spec.max_rows,
            ellipsis: spec.ellipsis,
        };
        let layout = self.engine.layout(&self.font, text, &style);
        self.upload(gpu);

        let max = gpu.device.limits().max_texture_dimension_2d;
        let [pad_x, pad_y] = spec.padding.map(|p| p.max(0.0));
        let size = [
            ((layout.width().ceil() + 2.0 * pad_x) as u32).clamp(1, max),
            ((layout.height().ceil() + 2.0 * pad_y) as u32).clamp(1, max),
        ];
        let b = layout.bounds;
        let centre = [(b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5];
        self.centre = centre;
        let s = self.engine.atlas().size() as f32;
        let mesh = layout
            .quads
            .iter()
            .map(|q| ir::MeshQuad {
                rect: [
                    q.rect[0] - centre[0],
                    q.rect[1] - centre[1],
                    q.rect[2] - centre[0],
                    q.rect[3] - centre[1],
                ],
                uv: [
                    q.atlas[0] as f32 / s,
                    q.atlas[1] as f32 / s,
                    (q.atlas[0] + q.atlas[2]) as f32 / s,
                    (q.atlas[1] + q.atlas[3]) as f32 / s,
                ],
            })
            .collect();
        LaidOut { size, centre, mesh }
    }

    pub fn view(&self) -> Option<&wgpu::TextureView> {
        self.atlas.as_ref().map(|a| &a.view)
    }

    /// `g_Texture0Resolution` of the atlas.
    pub fn resolution(&self) -> [f32; 4] {
        let s = self.atlas.as_ref().map_or(1.0, |a| a.size as f32);
        [s, s, s, s]
    }

    fn upload(&mut self, gpu: &Gpu) {
        let atlas = self.engine.atlas();
        if self
            .atlas
            .as_ref()
            .is_some_and(|a| a.generation == atlas.generation() && a.size == atlas.size())
        {
            return;
        }
        let size = atlas.size();
        let extent = wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        };
        if self.atlas.as_ref().is_none_or(|a| a.size != size) {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("glyph atlas"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.atlas = Some(GpuAtlas {
                texture,
                view,
                size,
                generation: 0,
            });
        }
        if let Some(a) = &mut self.atlas {
            gpu.queue.write_texture(
                a.texture.as_image_copy(),
                atlas.pixels(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size),
                    rows_per_image: Some(size),
                },
                extent,
            );
            a.generation = atlas.generation();
        }
    }
}
