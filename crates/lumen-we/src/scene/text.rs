//! WE text objects → layers drawing a glyph mesh over a font atlas.
//!
//! Layout rules and the render-target shape come from RenderDoc captures
//! (`docs/re-notes/text.md`): glyphs are drawn with WE's `font` shader; a text
//! layer without effects draws its glyphs straight into the scene, otherwise
//! into a target of the text box plus `padding` on each side, which starts as
//! a copy of the frame below when `copybackground` is set.

use std::path::PathBuf;
use std::sync::Arc;

use lumen_scene::{Blend, ImageLayer, LayerKind, MeshQuad, Texture, TextureBinding, TextureId};
use lumen_text::{Font, HAlign, Style, TextEngine, VAlign};
use serde_json::Value;

use super::values::{boolean, floats, integer, unwrap_value, vec_n};
use super::{Importer, PassSpec, SceneGraph, unsupported};
use crate::error::{Error, Result};

/// WE rasterizes point sizes at 300 dpi (Atami 32 pt → 133.3 px per em).
const DPI: f32 = 300.0;

/// `systemfont_*` names → Windows font files (table in wallpaper64.exe).
const SYSTEM_FONTS: &[(&str, &str)] = &[
    ("arial", "arial.ttf"),
    ("calibri", "calibri.ttf"),
    ("cambria", "cambria.ttc"),
    ("comicsans", "comic.ttf"),
    ("consolas", "consola.ttf"),
    ("sansserif", "micross.ttf"),
    ("segoe", "segoeui.ttf"),
    ("verdana", "verdana.ttf"),
];

/// Used when a font cannot be found (e.g. a Windows system font elsewhere).
const FALLBACK_FONT: &str = "fonts/NotoSans-Regular.ttf";

fn number(v: Option<&Value>) -> Option<f32> {
    v.and_then(floats).and_then(|f| f.first().copied())
}

impl Importer<'_> {
    pub(super) fn text_layer(
        &mut self,
        object: &Value,
        name: &str,
        graph: &SceneGraph,
    ) -> Result<()> {
        let font_name = object["font"].as_str().unwrap_or("systemfont_arial");
        let font = self
            .font(font_name)
            .or_else(|| self.font(FALLBACK_FONT))
            .ok_or_else(|| unsupported("font", font_name))?;
        let text = match unwrap_value(&object["text"]) {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            v => v.to_string(),
        };
        let align = |key: &str| object[key].as_str().unwrap_or("center").to_owned();
        let style = Style {
            pixel_size: number(object.get("pointsize")).unwrap_or(32.0) * DPI / 72.0,
            halign: match align("horizontalalign").as_str() {
                "left" => HAlign::Left,
                "right" => HAlign::Right,
                _ => HAlign::Center,
            },
            valign: match align("verticalalign").as_str() {
                "top" => VAlign::Top,
                "bottom" => VAlign::Bottom,
                _ => VAlign::Center,
            },
            max_width: boolean(object.get("limitwidth"), false)
                .then(|| number(object.get("maxwidth")))
                .flatten(),
            max_rows: boolean(object.get("limitrows"), false)
                .then(|| integer(object.get("maxrows")))
                .flatten()
                .map(|r| r.max(1) as usize),
            ellipsis: boolean(object.get("limituseellipsis"), false),
        };
        let mut engine = TextEngine::new();
        let layout = engine.layout(&font, &text, &style);

        let [pad_x, pad_y] = match object.get("padding").and_then(floats).as_deref() {
            Some([x, y, ..]) => [*x, *y],
            Some([p]) => [*p, *p],
            _ => [32.0, 32.0],
        };
        let size = [
            (layout.width().ceil() + 2.0 * pad_x).max(1.0) as u32,
            (layout.height().ceil() + 2.0 * pad_y).max(1.0) as u32,
        ];
        let dim = self.limits.max_texture_dim;
        if size[0] > dim || size[1] > dim {
            return Err(Error::LimitExceeded {
                what: "text layer size",
                value: u64::from(size[0].max(size[1])),
                limit: u64::from(dim),
            });
        }
        let b = layout.bounds;
        let centre = [(b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5];

        // Atlas cropped to the glyphs this text uses, as (r, 0, 0, 1) like
        // WE's R8 textures sample.
        let atlas = engine.atlas();
        let used = layout.quads.iter().fold([1u32, 1u32], |acc, q| {
            [
                acc[0].max(q.atlas[0] + q.atlas[2]),
                acc[1].max(q.atlas[1] + q.atlas[3]),
            ]
        });
        let mut rgba = Vec::with_capacity((used[0] * used[1] * 4) as usize);
        for y in 0..used[1] {
            let row = (y * atlas.size()) as usize;
            for x in 0..used[0] as usize {
                rgba.extend_from_slice(&[atlas.pixels()[row + x], 0, 0, 255]);
            }
        }
        let atlas_id = TextureId(self.scene.textures.len());
        self.scene.textures.push(Texture {
            name: format!("text atlas {name}"),
            width: used[0],
            height: used[1],
            image_width: used[0],
            image_height: used[1],
            rgba,
            mips: Vec::new(),
            clamp: true,
            nearest: false,
        });
        let [aw, ah] = used.map(|v| v as f32);
        let mesh = layout
            .quads
            .iter()
            .map(|q| MeshQuad {
                rect: [
                    q.rect[0] - centre[0],
                    q.rect[1] - centre[1],
                    q.rect[2] - centre[0],
                    q.rect[3] - centre[1],
                ],
                uv: [
                    q.atlas[0] as f32 / aw,
                    q.atlas[1] as f32 / ah,
                    (q.atlas[0] + q.atlas[2]) as f32 / aw,
                    (q.atlas[1] + q.atlas[3]) as f32 / ah,
                ],
            })
            .collect();

        let spec = PassSpec {
            shader: "font".into(),
            blend: Blend::Translucent,
            combos: Default::default(),
            textures: Default::default(),
            constants: Vec::new(),
        };
        let mut albedo = self.pass(&spec, None)?;
        albedo.textures.insert(0, TextureBinding::Asset(atlas_id));

        let (world, visible) = graph.world(object);
        let zoom = self.zoom;
        let transform = lumen_scene::compose(
            world,
            lumen_scene::affine([centre[0] * zoom, centre[1] * zoom], 0.0, [zoom, zoom]),
        );
        let [r, g, bl] = vec_n(object.get("color"), [1.0; 3]);
        let alpha = number(object.get("alpha")).unwrap_or(1.0);
        let mut layer = ImageLayer {
            name: name.to_owned(),
            size,
            transform,
            visible,
            parallax_depth: vec_n(object.get("parallaxDepth"), [1.0, 1.0]),
            group: self.group_of(object, graph),
            kind: LayerKind::Image,
            color: [r, g, bl, alpha],
            blend: Blend::Translucent,
            albedo,
            mesh: Some(mesh),
            copy_background: boolean(object.get("copybackground"), false),
            composite: None,
            effects: Vec::new(),
        };
        for effect in object["effects"].as_array().into_iter().flatten() {
            if !boolean(effect.get("visible"), true) {
                continue;
            }
            let file = effect["file"].as_str().unwrap_or("");
            match self.effect(file, effect) {
                Ok(e) => layer.effects.push(e),
                Err(e) => self.skip(format!("effect {file}: {e}")),
            }
        }
        self.scene.layers.push(layer);
        Ok(())
    }

    /// Loads (once) a font from the project / WE assets, or a Windows system
    /// font for `systemfont_*` names.
    fn font(&mut self, name: &str) -> Option<Font> {
        if let Some(cached) = self.fonts.get(name) {
            return cached.clone();
        }
        let bytes = match name.strip_prefix("systemfont_") {
            Some(system) => system_font(system),
            None => self.vfs.read(name),
        };
        let font = bytes.and_then(|b| Font::new(Arc::from(b), 0).ok());
        self.fonts.insert(name.to_owned(), font.clone());
        font
    }
}

fn system_font(name: &str) -> Option<Vec<u8>> {
    let file = SYSTEM_FONTS
        .iter()
        .find(|(n, _)| *n == name)
        .map_or("arial.ttf", |(_, f)| f);
    let dir = PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts");
    std::fs::read(dir.join(file)).ok()
}
