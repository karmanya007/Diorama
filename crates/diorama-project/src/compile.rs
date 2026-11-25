//! Documents → scene IR.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};

use diorama_core::Limits;
use diorama_scene::{
    Attribute, Blend, FontData, IDENTITY_AFFINE, ImageLayer, LayerKind, MeshQuad, Node, Parallax,
    Pass, ProgramId, Scene, ShaderLanguage, ShaderProgram, TextAlign, TextSpec, Texture,
    TextureBinding, TextureId, VideoData,
};
use serde_json::Value;

use crate::effects::{Frame, List};
use crate::fields::Fields;
use crate::{Assets, Error, Host, Result, Values, picture};

/// The newest document version read here.
pub const VERSION: u64 = 1;

const MAX_LAYERS: usize = 1000;
const MAX_IMAGE: u64 = 256 << 20;
const MAX_FONT: u64 = 64 << 20;
/// Longest text of a layer, in characters.
const MAX_TEXT: usize = 10_000;
/// Positions and sizes stay sane for 32-bit floats.
const FAR: f32 = 100_000.0;

pub(crate) const VERTEX: &str = include_str!("shaders/vertex.wgsl");
const PLAIN: &str = include_str!("shaders/plain.wgsl");
const BLENDED: &str = include_str!("shaders/blend.wgsl");
const IMAGE: &str = include_str!("shaders/image.wgsl");
const TEXT: &str = include_str!("shaders/text.wgsl");
const FINISHED: &str = include_str!("shaders/finished.wgsl");

/// How a layer mixes with what is below it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mix {
    /// Blended by the GPU.
    Gpu(Blend),
    /// Computed in the shader (`mode` in blend.wgsl).
    Shader(f32),
}

const MIXES: [(&str, Mix); 10] = [
    ("normal", Mix::Gpu(Blend::Translucent)),
    ("add", Mix::Gpu(Blend::Additive)),
    ("multiply", Mix::Shader(1.0)),
    ("screen", Mix::Shader(2.0)),
    ("overlay", Mix::Shader(3.0)),
    ("darken", Mix::Shader(4.0)),
    ("lighten", Mix::Shader(5.0)),
    ("hard-light", Mix::Shader(6.0)),
    ("soft-light", Mix::Shader(7.0)),
    ("difference", Mix::Shader(8.0)),
];

const HALIGN: [(&str, TextAlign); 3] = [
    ("left", TextAlign::Start),
    ("center", TextAlign::Center),
    ("right", TextAlign::End),
];
const VALIGN: [(&str, TextAlign); 3] = [
    ("top", TextAlign::Start),
    ("middle", TextAlign::Center),
    ("bottom", TextAlign::End),
];

pub(crate) fn compile(
    document: &Value,
    assets: &dyn Assets,
    values: &Values,
    host: &dyn Host,
    limits: &Limits,
) -> Result<Scene> {
    let root = document
        .as_object()
        .ok_or_else(|| Error::Document("not an object".into()))?;
    let version = root
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Document("no version".into()))?;
    if version > VERSION {
        return Err(Error::Newer(version));
    }
    let problems = RefCell::new(Vec::new());
    let fields = Fields {
        map: root,
        values,
        at: "wallpaper".into(),
        problems: &problems,
    };
    let side = limits.max_texture_dim as f32;
    let [width, height] = fields
        .optional_pair("size", [1.0, side])
        .ok_or_else(|| Error::Document("no size".into()))?
        .map(|s| s.round() as u32);
    let background = fields.color("background", [0.0, 0.0, 0.0, 1.0]);
    let [r, g, b, _] = background;
    let mut compiler = Compiler {
        assets,
        values,
        host,
        limits,
        problems: &problems,
        scene: Scene {
            width,
            height,
            clear_color: [r, g, b, 1.0],
            scheme_color: [r, g, b],
            parallax: parallax(&fields),
            ..Scene::default()
        },
        textures: HashMap::new(),
        fonts: HashMap::new(),
        programs: HashMap::new(),
        effect_programs: HashMap::new(),
        bytes: 0,
    };
    match root.get("layers") {
        Some(Value::Array(layers)) => compiler.layers(layers),
        Some(_) => fields.problem("layers is not a list"),
        None => {}
    }
    let mut scene = compiler.scene;
    scene.skipped = problems.into_inner();
    Ok(scene)
}

/// `{"amount", "smoothing"}`: a layer of depth 1 moves by `amount` of the
/// wallpaper's size while the cursor crosses the screen.
fn parallax(fields: &Fields<'_>) -> Option<Parallax> {
    let map = fields.map.get("parallax")?.as_object()?;
    let f = Fields {
        map,
        at: "parallax".into(),
        ..*fields
    };
    let amount = f.number("amount", 0.05, [0.0, 1.0]);
    (amount > 0.0).then(|| Parallax {
        amount,
        mouse_influence: 1.0,
        delay: f.number("smoothing", 0.15, [0.0, 5.0]),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Source {
    Image,
    Text,
    /// A layer after its effects.
    Finished,
}

pub(crate) struct Compiler<'a> {
    assets: &'a dyn Assets,
    values: &'a Values,
    host: &'a dyn Host,
    limits: &'a Limits,
    problems: &'a RefCell<Vec<String>>,
    pub(crate) scene: Scene,
    /// By path and whether it is smooth; the empty path is plain white.
    textures: HashMap<(String, bool), TextureId>,
    /// By path; the empty path is the host's font.
    fonts: HashMap<String, usize>,
    programs: HashMap<(Source, bool), ProgramId>,
    /// By effect, entry point and whether it moves.
    pub(crate) effect_programs: HashMap<(&'static str, &'static str, bool), ProgramId>,
    /// Decoded pixels so far.
    bytes: u64,
}

impl<'a> Compiler<'a> {
    fn layers(&mut self, layers: &'a [Value]) {
        if layers.len() > MAX_LAYERS {
            self.problems.borrow_mut().push(format!(
                "more than {MAX_LAYERS} layers: the rest is left out"
            ));
        }
        let mut ids = HashSet::new();
        for (i, layer) in layers.iter().take(MAX_LAYERS).enumerate() {
            let Some(map) = layer.as_object() else {
                self.problems
                    .borrow_mut()
                    .push(format!("layer {} is not an object", i + 1));
                continue;
            };
            let mut fields = Fields {
                map,
                values: self.values,
                at: format!("layer {}", i + 1),
                problems: self.problems,
            };
            let id = fields.text("id");
            let name = fields
                .text("name")
                .or_else(|| id.clone())
                .unwrap_or_else(|| fields.at.clone());
            if name != fields.at {
                fields.at = format!("layer {name}");
            }
            if let Some(id) = id
                && !ids.insert(id)
            {
                fields.problem("another layer has its id");
            }
            if let Err(message) = self.layer(&fields, name) {
                fields.problem(&format!("{message}; left out"));
            }
        }
    }

    fn layer(&mut self, f: &Fields<'_>, name: String) -> Result<(), String> {
        let kind = f.text("type").ok_or("no type")?;
        let mix = f.choice("blend", &MIXES, MIXES[0].1);
        let [w, h] = [self.scene.width as f32, self.scene.height as f32];
        let far = [-FAR, FAR];
        let side = [1.0, self.limits.max_texture_dim as f32];
        let mut tint = [1.0; 4];
        let mut text = None;
        let mut size = [1, 1];
        let mut layer_kind = LayerKind::Image;
        let list = List::read(f);
        let (source, binding) = match kind.as_str() {
            "image" | "video" => {
                let path = f.text("source").ok_or("no source")?;
                let id = if kind == "image" {
                    self.image(&path, f.flag("smooth", true))?
                } else {
                    self.video(&path)?
                };
                tint = f.color("tint", tint);
                let t = &self.scene.textures[id.0];
                size = [t.image_width, t.image_height];
                if let Some(s) = f.optional_pair("size", side) {
                    size = s.map(|v| v.round() as u32);
                }
                (Source::Image, TextureBinding::Asset(id))
            }
            "color" => {
                tint = f.color("color", tint);
                let full = f.pair("size", [w, h], side);
                size = full.map(|v| v.round() as u32);
                (Source::Image, TextureBinding::Asset(self.white()))
            }
            "text" => {
                tint = f.color("color", tint);
                let mut content = f.text("text").unwrap_or_default();
                if let Some((end, _)) = content.char_indices().nth(MAX_TEXT) {
                    content.truncate(end);
                    f.problem("text is too long: the rest is left out");
                }
                let pixel_size = f.number("font-size", 64.0, [1.0, 2000.0]);
                text = Some(TextSpec {
                    font: self.font(f.text("font"))?,
                    text: content,
                    pixel_size,
                    halign: f.choice("align", &HALIGN, TextAlign::Center),
                    valign: f.choice("vertical-align", &VALIGN, TextAlign::Center),
                    max_width: f.optional_number("max-width", [1.0, FAR]),
                    max_rows: f
                        .optional_number("max-lines", [1.0, 1000.0])
                        .map(|n| n as usize),
                    ellipsis: f.flag("ellipsis", true),
                    padding: [pixel_size / 4.0; 2],
                });
                (Source::Text, TextureBinding::TextAtlas)
            }
            "adjustment" => {
                if list.is_empty() {
                    return Err("no effects".into());
                }
                size = [self.scene.width, self.scene.height];
                layer_kind = LayerKind::Fullscreen;
                (Source::Image, TextureBinding::FrameBuffer)
            }
            other => return Err(format!("unknown type {other}")),
        };
        let opacity = f.number("opacity", 1.0, [0.0, 1.0]);
        let mut effects = Vec::new();
        let mut mesh = None;
        if !list.is_empty() {
            let spread = if layer_kind == LayerKind::Fullscreen {
                0.0
            } else {
                list.spread
            };
            let mut frame = Frame {
                size: [0.0, 0.0],
                spread,
            };
            if let Some(spec) = &mut text {
                let TextSpec { padding, .. } = spec;
                *padding = padding.map(|p| p + spread);
            } else {
                let [w, h] = size.map(|s| s as f32);
                let room = side[1];
                frame.spread = spread.min((room - w) / 2.0).min((room - h) / 2.0).floor();
                let grown = [w + 2.0 * frame.spread, h + 2.0 * frame.spread];
                frame.size = grown;
                size = grown.map(|s| s as u32);
                if layer_kind == LayerKind::Image {
                    // Drawn as a mesh, so that blending over nothing leaves
                    // what effects expect: colours premultiplied by alpha.
                    mesh = Some(vec![MeshQuad {
                        rect: [-w / 2.0, -h / 2.0, w / 2.0, h / 2.0],
                        uv: [0.0, 0.0, 1.0, 1.0],
                    }]);
                }
            }
            effects = self.effects(&list, frame);
        }
        // Effects come before opacity and blending, which the composite does.
        let (albedo, composite) = if effects.is_empty() {
            tint[3] *= opacity;
            (self.albedo(source, binding, mix), None)
        } else {
            let mut composite = self.albedo(Source::Finished, TextureBinding::Previous, mix);
            composite
                .constants
                .insert("opacity".to_owned(), vec![opacity]);
            let plain = Mix::Gpu(Blend::Translucent);
            (self.albedo(source, binding, plain), Some(composite))
        };

        let placed = layer_kind == LayerKind::Image;
        let [x, y] = f.pair("position", [0.0, 0.0], far);
        let [sx, sy] = f.pair("scale", [1.0, 1.0], [-1000.0, 1000.0]);
        // Documents count y down from the middle and turn clockwise; scenes
        // count it up from the bottom left and turn the other way.
        let rotation = f.number("rotation", 0.0, far).rem_euclid(360.0);
        let (x, y, sx, sy, rotation) = if placed {
            (x, y, sx, sy, rotation)
        } else {
            (0.0, 0.0, 1.0, 1.0, 0.0)
        };
        let node = Node {
            name: name.clone(),
            id: self.scene.nodes.len() as i64,
            parent: None,
            origin: [w / 2.0 + x, h / 2.0 - y, 0.0],
            scale: [sx, sy, 1.0],
            angles: [0.0, 0.0, -rotation.to_radians()],
            visible: f.flag("visible", true),
        };
        let transform = node.local();
        let visible = node.visible;
        let depth = if placed {
            f.number("depth", 0.0, [-10.0, 10.0])
        } else {
            0.0
        };
        self.scene.nodes.push(node);
        self.scene.layers.push(ImageLayer {
            name,
            size,
            node: Some(self.scene.nodes.len() - 1),
            local: IDENTITY_AFFINE,
            text,
            particles: None,
            puppet: None,
            model: None,
            world: None,
            transform,
            visible,
            kind: layer_kind,
            group: None,
            parallax_depth: [depth, depth],
            color: tint,
            blend: match mix {
                Mix::Gpu(blend) => blend,
                Mix::Shader(_) => Blend::Translucent,
            },
            albedo,
            mesh,
            copy_background: false,
            composite,
            effects,
        });
        Ok(())
    }

    fn albedo(&mut self, source: Source, binding: TextureBinding, mix: Mix) -> Pass {
        let mut textures = BTreeMap::from([(0, binding)]);
        let mut constants = BTreeMap::new();
        if let Mix::Shader(mode) = mix {
            textures.insert(1, TextureBinding::FrameBuffer);
            constants.insert("mode".to_owned(), vec![mode]);
        }
        Pass {
            program: self.program(source, matches!(mix, Mix::Shader(_))),
            textures,
            target: None,
            constants,
            blend: Blend::Opaque,
        }
    }

    fn program(&mut self, source: Source, blended: bool) -> ProgramId {
        if let Some(id) = self.programs.get(&(source, blended)) {
            return *id;
        }
        let (name, color) = match source {
            Source::Image => ("image", IMAGE),
            Source::Text => ("text", TEXT),
            Source::Finished => ("finished", FINISHED),
        };
        let stage = if blended { BLENDED } else { PLAIN };
        let bindings: &[(u32, u32)] = if blended {
            &[(0, 0), (1, 1)]
        } else {
            &[(0, 0)]
        };
        let id = ProgramId(self.scene.programs.len());
        self.scene.programs.push(program(
            &format!("{name}{}", if blended { " blended" } else { "" }),
            VERTEX,
            format!("{stage}{color}"),
            bindings,
        ));
        self.programs.insert((source, blended), id);
        id
    }

    fn push_texture(&mut self, key: (String, bool), texture: Texture) -> TextureId {
        let id = TextureId(self.scene.textures.len());
        self.scene.textures.push(texture);
        self.textures.insert(key, id);
        id
    }

    /// Counts the pixels of a texture about to be held.
    fn hold(&mut self, width: u32, height: u32) -> Result<(), String> {
        let bytes = self.bytes + u64::from(width) * u64::from(height) * 4;
        if bytes > self.limits.max_decompressed_bytes {
            return Err("the wallpaper's images are too large together".into());
        }
        self.bytes = bytes;
        Ok(())
    }

    fn white(&mut self) -> TextureId {
        let key = (String::new(), true);
        match self.textures.get(&key) {
            Some(id) => *id,
            None => self.push_texture(key, solid("white", [1, 1], [255; 4])),
        }
    }

    pub(crate) fn image(&mut self, path: &str, smooth: bool) -> Result<TextureId, String> {
        let key = (path.to_owned(), smooth);
        if let Some(id) = self.textures.get(&key) {
            return Ok(*id);
        }
        let data = self
            .assets
            .read(path, MAX_IMAGE)
            .map_err(|e| e.to_string())?;
        let texture = picture::decode(path, &data, smooth, self.limits)
            .map_err(|e| format!("{path}: {e}"))?;
        self.hold(texture.width, texture.height)?;
        Ok(self.push_texture(key, texture))
    }

    fn video(&mut self, path: &str) -> Result<TextureId, String> {
        let key = (path.to_owned(), true);
        if let Some(id) = self.textures.get(&key) {
            return Ok(*id);
        }
        let video = self.assets.video(path).map_err(|e| e.to_string())?;
        let size = match &video {
            VideoData::File(file) => self.host.video_size(file),
            VideoData::Memory(_) => None,
        };
        let [width, height] = size.ok_or_else(|| format!("{path}: cannot play this video"))?;
        let max = self.limits.max_texture_dim;
        if width == 0 || height == 0 || width > max || height > max {
            return Err(format!("{path}: {width}×{height} is not a usable size"));
        }
        self.hold(width, height)?;
        let mut texture = solid(path, [width, height], [0, 0, 0, 255]);
        texture.video = Some(video);
        Ok(self.push_texture(key, texture))
    }

    /// The font at `path`, or the host's.
    fn font(&mut self, path: Option<String>) -> Result<usize, String> {
        let key = path.unwrap_or_default();
        if let Some(index) = self.fonts.get(&key) {
            return Ok(*index);
        }
        let data = if key.is_empty() {
            self.host.default_font().ok_or("no font")?
        } else {
            let data = self
                .assets
                .read(&key, MAX_FONT)
                .map_err(|e| e.to_string())?;
            data.into()
        };
        diorama_text::Font::new(data.clone(), 0).map_err(|e| format!("{key}: {e}"))?;
        let index = self.scene.fonts.len();
        self.scene.fonts.push(FontData {
            name: key.clone(),
            data,
        });
        self.fonts.insert(key, index);
        Ok(index)
    }
}

fn solid(name: &str, [width, height]: [u32; 2], rgba: [u8; 4]) -> Texture {
    Texture {
        name: name.to_owned(),
        width,
        height,
        image_width: width,
        image_height: height,
        rgba: rgba.repeat(width as usize * height as usize),
        mips: Vec::new(),
        clamp: true,
        nearest: false,
        frames: Vec::new(),
        video: None,
    }
}

/// A WGSL program over quads; `bindings`: texture slots and where they are bound.
pub(crate) fn program(
    name: &str,
    vertex: &str,
    fragment: String,
    bindings: &[(u32, u32)],
) -> ShaderProgram {
    let attribute = |name: &str, location, components| Attribute {
        name: name.to_owned(),
        location,
        components,
    };
    ShaderProgram {
        name: name.to_owned(),
        language: ShaderLanguage::Wgsl,
        vertex_source: vertex.to_owned(),
        fragment_source: fragment,
        vertex_entry: "vs".to_owned(),
        fragment_entry: "fs".to_owned(),
        attributes: vec![attribute("a_Position", 0, 3), attribute("a_TexCoord", 1, 2)],
        texture_bindings: bindings.iter().copied().collect(),
        material_keys: BTreeMap::new(),
    }
}
