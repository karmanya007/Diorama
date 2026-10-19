//! WE text objects → text layers (`TextSpec`; the renderer lays them out).
//!
//! Layout rules and the render-target shape come from RenderDoc captures
//! (`docs/re-notes/text.md`): glyphs are drawn with WE's `font` shader; a text
//! layer without effects draws its glyphs straight into the scene, otherwise
//! into a target of the text box plus `padding` on each side, which starts as
//! a copy of the frame below (unless `copybackground` is false).

use std::path::PathBuf;
use std::sync::Arc;

use lumen_scene::{Blend, FontData, ImageLayer, LayerKind, TextAlign, TextSpec, TextureBinding};
use serde_json::Value;

use super::values::{boolean, floats, integer, unwrap_value, vec_n};
use super::{Importer, PassSpec, SceneGraph, unsupported};
use crate::error::Result;

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

fn align(v: &Value) -> TextAlign {
    match v.as_str().unwrap_or("center") {
        "left" | "top" => TextAlign::Start,
        "right" | "bottom" => TextAlign::End,
        _ => TextAlign::Center,
    }
}

impl Importer<'_> {
    pub(super) fn text_layer(
        &mut self,
        object: &Value,
        name: &str,
        node: usize,
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
        let padding = match object.get("padding").and_then(floats).as_deref() {
            Some([x, y, ..]) => [*x, *y],
            Some([p]) => [*p, *p],
            _ => [32.0, 32.0],
        };
        let spec = TextSpec {
            font,
            text,
            pixel_size: number(object.get("pointsize")).unwrap_or(32.0) * DPI / 72.0,
            halign: align(&object["horizontalalign"]),
            valign: align(&object["verticalalign"]),
            max_width: boolean(object.get("limitwidth"), false)
                .then(|| number(object.get("maxwidth")))
                .flatten(),
            max_rows: boolean(object.get("limitrows"), false)
                .then(|| integer(object.get("maxrows")))
                .flatten()
                .map(|r| r.max(1) as usize),
            ellipsis: boolean(object.get("limituseellipsis"), false),
            padding,
        };

        let pass = PassSpec {
            shader: "font".into(),
            blend: Blend::Translucent,
            combos: Default::default(),
            textures: Default::default(),
            constants: Vec::new(),
        };
        let mut albedo = self.pass(&pass, None)?;
        albedo.textures.insert(0, TextureBinding::TextAtlas);

        let (world, visible) = graph.world(object);
        // The renderer centres the laid-out text box on top of this.
        let local = lumen_scene::affine([0.0, 0.0], 0.0, [self.zoom, self.zoom]);
        let [r, g, b] = vec_n(object.get("color"), [1.0; 3]);
        let alpha = number(object.get("alpha")).unwrap_or(1.0);
        let mut layer = ImageLayer {
            name: name.to_owned(),
            size: [1, 1],
            node: Some(node),
            local,
            text: Some(spec),
            particles: None,
            puppet: None,
            transform: lumen_scene::compose(world, local),
            visible,
            parallax_depth: vec_n(object.get("parallaxDepth"), [1.0, 1.0]),
            group: self.group_of(object, graph),
            kind: LayerKind::Image,
            color: [r, g, b, alpha],
            blend: Blend::Translucent,
            albedo,
            mesh: None,
            // Text targets start from the frame below unless switched off
            // (captures 914, 924: no `copybackground` key, copy still made).
            // Without effects there is no target: glyphs draw straight in.
            copy_background: boolean(
                object.get("copybackground"),
                object["effects"].as_array().is_some_and(|e| !e.is_empty()),
            ),
            composite: None,
            effects: Vec::new(),
        };
        self.attach_effects(object, &mut layer);
        self.scene.layers.push(layer);
        Ok(())
    }

    /// Loads (once) a font from the project / WE assets, or a Windows system
    /// font for `systemfont_*` names; index into `Scene::fonts`.
    fn font(&mut self, name: &str) -> Option<usize> {
        if let Some(cached) = self.fonts.get(name) {
            return *cached;
        }
        let bytes = match name.strip_prefix("systemfont_") {
            Some(system) => system_font(system),
            None => self.vfs.read(name),
        };
        let index = bytes
            .map(Arc::<[u8]>::from)
            .filter(|data| lumen_text::Font::new(data.clone(), 0).is_ok())
            .map(|data| {
                self.scene.fonts.push(FontData {
                    name: name.to_owned(),
                    data,
                });
                self.scene.fonts.len() - 1
            });
        self.fonts.insert(name.to_owned(), index);
        index
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
