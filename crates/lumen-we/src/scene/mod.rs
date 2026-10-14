//! `scene.json` → [`lumen_scene::Scene`] (Phase 2: image layers + effects).
//!
//! Everything WE-specific is resolved here: file lookup order, material and
//! effect JSON, combo selection, uniform defaults from shader annotations,
//! shader translation to HLSL and texture decoding. See
//! `docs/re-notes/renderdoc-captures.md` for the pass structure this mirrors.

pub mod values;
pub mod vfs;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use lumen_core::Limits;
use lumen_scene::{
    Attribute, Blend, Effect, Fbo, ImageLayer, LayerKind, Pass, ProgramId, Scene, ShaderProgram,
    Texture, TextureBinding, TextureId,
};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::shader::{self, Decl, Qualifier};
use crate::tex::{Tex, TexFlags};
use crate::tex_decode::mip_to_rgba8;
use values::{boolean, floats, integer, unwrap_value, vec_n};
pub use vfs::Vfs;

/// Imports `<project_dir>/scene.json` (from `scene.pkg` or loose files).
/// `assets_dir` is `<WE install>/assets`, needed for built-in shaders/effects.
pub fn import(project_dir: &Path, assets_dir: Option<&Path>, limits: &Limits) -> Result<Scene> {
    let vfs = Vfs::open(project_dir, assets_dir, limits)?;
    import_from(&vfs, limits)
}

pub fn import_from(vfs: &Vfs, limits: &Limits) -> Result<Scene> {
    let json = vfs.read_json("scene.json")?;
    let mut importer = Importer {
        vfs,
        limits,
        scene: Scene::default(),
        textures: HashMap::new(),
        programs: HashMap::new(),
        program_uniforms: HashMap::new(),
        engine: shader::EngineContext::default(),
    };
    importer.scene_header(&json);
    let objects: Vec<&Value> = json["objects"].as_array().into_iter().flatten().collect();
    let graph = SceneGraph::new(&objects);
    // Lights first: lit shaders are generated for the scene's light count.
    for object in &objects {
        importer.light(object, &graph);
    }
    importer.engine.point_lights = importer.scene.point_lights.len();
    for object in objects {
        importer.object(object, &graph);
    }
    Ok(importer.scene)
}

/// Object hierarchy: children store origin/scale/angles relative to `parent`.
struct SceneGraph {
    /// id → (local transform, own visibility, parent id).
    nodes: HashMap<i64, ([f32; 6], bool, Option<i64>)>,
}

impl SceneGraph {
    fn new(objects: &[&Value]) -> Self {
        let nodes = objects
            .iter()
            .filter_map(|o| {
                let id = integer(o.get("id"))?;
                Some((
                    id,
                    (
                        local_transform(o),
                        boolean(o.get("visible"), true),
                        integer(o.get("parent")),
                    ),
                ))
            })
            .collect();
        Self { nodes }
    }

    /// World transform and effective visibility (hidden parents hide children).
    fn world(&self, object: &Value) -> ([f32; 6], bool) {
        let mut transform = local_transform(object);
        let mut visible = boolean(object.get("visible"), true);
        let mut parent = integer(object.get("parent"));
        // Depth cap guards against cycles in malformed content.
        for _ in 0..64 {
            let Some((local, vis, next)) = parent.and_then(|p| self.nodes.get(&p)) else {
                break;
            };
            transform = lumen_scene::compose(*local, transform);
            visible &= vis;
            parent = *next;
        }
        (transform, visible)
    }
}

/// Origin · rotation(z) · scale. Rotations of π about x/y (mirroring in an
/// orthographic view) fold into the scale; angles are radians.
fn local_transform(object: &Value) -> [f32; 6] {
    let origin = vec_n(object.get("origin"), [0.0; 3]);
    let scale = vec_n(object.get("scale"), [1.0; 3]);
    let angles = vec_n(object.get("angles"), [0.0; 3]);
    lumen_scene::affine(
        [origin[0], origin[1]],
        angles[2],
        [scale[0] * angles[1].cos(), scale[1] * angles[0].cos()],
    )
}

struct Importer<'a> {
    vfs: &'a Vfs,
    limits: &'a Limits,
    scene: Scene,
    /// Texture name → id (failed loads cached as `None`).
    textures: HashMap<String, Option<TextureId>>,
    /// (shader, combos) → program.
    programs: HashMap<(String, BTreeMap<String, i64>), ProgramId>,
    /// Uniform declarations (with annotations) of each program, both stages.
    program_uniforms: HashMap<ProgramId, Vec<Decl>>,
    engine: shader::EngineContext,
}

/// One material pass after applying scene-level overrides.
struct PassSpec<'v> {
    shader: String,
    blend: Blend,
    combos: BTreeMap<String, i64>,
    /// Slot → texture name (asset or `_rt_*`).
    textures: BTreeMap<u32, String>,
    /// Constant lookups in priority order.
    constants: Vec<&'v serde_json::Map<String, Value>>,
}

impl Importer<'_> {
    fn scene_header(&mut self, json: &Value) {
        let general = &json["general"];
        let ortho = &general["orthogonalprojection"];
        self.scene.width = integer(ortho.get("width"))
            .and_then(|w| u32::try_from(w).ok())
            .unwrap_or(1920);
        self.scene.height = integer(ortho.get("height"))
            .and_then(|h| u32::try_from(h).ok())
            .unwrap_or(1080);
        let [r, g, b] = vec_n(general.get("clearcolor"), [0.0, 0.0, 0.0]);
        self.scene.clear_color = [r, g, b, 1.0];
        self.scene.ambient_color = vec_n(general.get("ambientcolor"), [0.2; 3]);
        self.scene.skylight_color = vec_n(general.get("skylightcolor"), [0.3; 3]);
        for flag in ["bloom", "cameraparallax", "camerashake"] {
            if boolean(general.get(flag), false) {
                self.skip(format!("general.{flag}"));
            }
        }
    }

    fn skip(&mut self, what: String) {
        if !self.scene.skipped.contains(&what) {
            self.scene.skipped.push(what);
        }
    }

    fn light(&mut self, object: &Value, graph: &SceneGraph) {
        let Some(kind) = object.get("light").and_then(Value::as_str) else {
            return;
        };
        let (transform, visible) = graph.world(object);
        if !visible {
            return;
        }
        if kind != "lpoint" {
            self.skip(format!("{kind} lights"));
            return;
        }
        let z = vec_n(object.get("origin"), [0.0; 3])[2];
        let color = vec_n(object.get("color"), [1.0; 3]);
        let intensity = object
            .get("intensity")
            .and_then(floats)
            .and_then(|v| v.first().copied())
            .unwrap_or(1.0);
        self.scene.point_lights.push(lumen_scene::PointLight {
            origin: [transform[4], transform[5], z],
            color: color.map(|c| c * intensity),
            radius: object
                .get("radius")
                .and_then(floats)
                .and_then(|v| v.first().copied())
                .unwrap_or(1000.0),
            exponent: object
                .get("exponent")
                .and_then(floats)
                .and_then(|v| v.first().copied())
                .unwrap_or(1.0),
        });
    }

    fn object(&mut self, object: &Value, graph: &SceneGraph) {
        let name = object["name"].as_str().unwrap_or("").to_owned();
        let kind = ["particle", "text", "model", "sound"]
            .into_iter()
            .find(|k| object.get(*k).is_some_and(|v| !v.is_null()));
        match (object.get("image").and_then(Value::as_str), kind) {
            (Some(model), _) => {
                if let Err(e) = self.image_layer(object, model, &name, graph) {
                    self.skip(format!("image layer {name:?}: {e}"));
                }
            }
            (None, Some(kind)) => self.skip(format!("{kind} objects")),
            (None, None) => {}
        }
    }

    fn image_layer(
        &mut self,
        object: &Value,
        model_path: &str,
        name: &str,
        graph: &SceneGraph,
    ) -> Result<()> {
        let model = self.vfs.read_json(model_path)?;
        // Fullscreen (post-processing) layers cover the scene and start from what is below.
        let fullscreen = boolean(model.get("fullscreen"), false);
        let passthrough = boolean(model.get("passthrough"), false);
        let material_path = model["material"]
            .as_str()
            .ok_or_else(|| unsupported("model without material", model_path))?;
        let material = self.vfs.read_json(material_path)?;

        // Object-level values the albedo material reads by key.
        let mut object_constants = serde_json::Map::new();
        if let Some(v) = object.get("brightness") {
            object_constants.insert("brightness".to_owned(), unwrap_value(v).clone());
        }
        let mut spec = self.pass_spec(&material, None)?;
        spec.constants.push(&object_constants);
        // Lit layers rendered into their own target light themselves with
        // the scene placement (g_AltModelMatrix) instead of the target MVP.
        let has_effects = object["effects"]
            .as_array()
            .is_some_and(|e| e.iter().any(|e| boolean(e.get("visible"), true)));
        if has_effects && !fullscreen {
            spec.combos.insert("PRELIGHTING".to_owned(), 1);
        }
        let composite_blend = spec.blend;
        let albedo = self.pass(&spec, None)?;

        let size = match object.get("size").and_then(floats) {
            _ if fullscreen => [self.scene.width, self.scene.height],
            Some(s) if s.len() >= 2 => [s[0].max(1.0) as u32, s[1].max(1.0) as u32],
            _ => match albedo.textures.get(&0) {
                Some(TextureBinding::Asset(id)) => {
                    let t = &self.scene.textures[id.0];
                    [t.image_width, t.image_height]
                }
                _ => return Err(unsupported("layer without size", name)),
            },
        };
        let dim = self.limits.max_texture_dim;
        if size[0] > dim || size[1] > dim {
            return Err(Error::LimitExceeded {
                what: "layer size",
                value: u64::from(size[0].max(size[1])),
                limit: u64::from(dim),
            });
        }

        let (transform, visible) = if fullscreen {
            let centre = [
                self.scene.width as f32 / 2.0,
                self.scene.height as f32 / 2.0,
            ];
            (
                lumen_scene::affine(centre, 0.0, [1.0, 1.0]),
                graph.world(object).1,
            )
        } else {
            graph.world(object)
        };
        let [r, g, b] = vec_n(object.get("color"), [1.0; 3]);
        let alpha = object
            .get("alpha")
            .and_then(floats)
            .and_then(|a| a.first().copied())
            .unwrap_or(1.0);
        let mut layer = ImageLayer {
            name: name.to_owned(),
            size,
            transform,
            visible,
            kind: if fullscreen {
                LayerKind::Fullscreen
            } else if passthrough {
                LayerKind::Compose
            } else {
                LayerKind::Image
            },
            color: [r, g, b, alpha],
            blend: composite_blend,
            albedo,
            effects: Vec::new(),
        };
        if integer(object.get("colorBlendMode")).unwrap_or(0) != 0 {
            self.skip("layer colorBlendMode".into());
        }

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
        // A passthrough layer without effects redraws what is already there.
        if !(passthrough && layer.effects.is_empty()) {
            self.scene.layers.push(layer);
        }
        Ok(())
    }

    fn effect(&mut self, file: &str, instance: &Value) -> Result<Effect> {
        let json = self.vfs.read_json(file)?;
        let fbos: Vec<Fbo> = json["fbos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| {
                Some(Fbo {
                    name: f["name"].as_str()?.to_owned(),
                    scale_div: integer(f.get("scale"))
                        .and_then(|s| u32::try_from(s).ok())
                        .unwrap_or(1)
                        .clamp(1, 64),
                })
            })
            .collect();
        let overrides = instance["passes"].as_array();
        let mut passes = Vec::new();
        for (i, p) in json["passes"].as_array().into_iter().flatten().enumerate() {
            let material_path = p["material"]
                .as_str()
                .ok_or_else(|| unsupported("effect pass without material", file))?;
            let material = self.vfs.read_json(material_path)?;
            let scene_pass = overrides.and_then(|o| o.get(i));
            let spec = self.pass_spec(&material, scene_pass)?;
            let mut pass = self.pass(&spec, Some(&fbos))?;
            for b in p["bind"].as_array().into_iter().flatten() {
                let (Some(name), Some(index)) = (b["name"].as_str(), integer(b.get("index")))
                else {
                    continue;
                };
                let slot = u32::try_from(index).unwrap_or(0);
                pass.textures.insert(slot, self.rt_binding(name, &fbos));
            }
            // An unset slot 0 in an effect pass is the layer's current image.
            pass.textures.entry(0).or_insert(TextureBinding::Previous);
            pass.target = p["target"].as_str().map(str::to_owned);
            passes.push(pass);
        }
        Ok(Effect {
            name: json["name"].as_str().unwrap_or(file).to_owned(),
            fbos,
            passes,
        })
    }

    /// Material pass 0 merged with the scene's per-pass overrides.
    fn pass_spec<'v>(
        &mut self,
        material: &'v Value,
        scene_pass: Option<&'v Value>,
    ) -> Result<PassSpec<'v>> {
        let m = &material["passes"][0];
        let shader = m["shader"]
            .as_str()
            .ok_or_else(|| unsupported("material pass without shader", ""))?
            .to_owned();
        let blend = match m["blending"].as_str().unwrap_or("normal") {
            "translucent" => Blend::Translucent,
            "additive" => Blend::Additive,
            "normal" | "disabled" => Blend::Opaque,
            other => {
                self.skip(format!("blending {other}"));
                Blend::Opaque
            }
        };
        let mut combos = BTreeMap::new();
        let mut textures = BTreeMap::new();
        let mut constants = Vec::new();
        for source in scene_pass.into_iter().chain([m]).rev() {
            // Material first, scene overrides after (later wins).
            if let Some(map) = source["combos"].as_object() {
                for (k, v) in map {
                    if let Some(v) = integer(Some(v)) {
                        combos.insert(k.to_ascii_uppercase(), v);
                    }
                }
            }
            for (i, t) in source["textures"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                if let (Some(name), Ok(slot)) = (t.as_str(), u32::try_from(i)) {
                    textures.insert(slot, name.to_owned());
                }
            }
        }
        for source in scene_pass.into_iter().chain([m]) {
            if let Some(map) = source["constantshadervalues"].as_object() {
                constants.push(map);
            }
        }
        Ok(PassSpec {
            shader,
            blend,
            combos,
            textures,
            constants,
        })
    }

    fn pass(&mut self, spec: &PassSpec<'_>, fbos: Option<&[Fbo]>) -> Result<Pass> {
        let vertex = self
            .vfs
            .read_string(&format!("shaders/{}.vert", spec.shader))
            .ok_or_else(|| unsupported("missing vertex shader", &spec.shader))?;
        let fragment = self
            .vfs
            .read_string(&format!("shaders/{}.frag", spec.shader))
            .ok_or_else(|| unsupported("missing fragment shader", &spec.shader))?;
        let vfs = self.vfs;
        let resolve = |name: &str| vfs.read_string(&format!("shaders/{name}"));

        // Sampler annotations (all branches) decide default textures and texture combos.
        let mut textures = spec.textures.clone();
        let mut combos = spec.combos.clone();
        let raw = format!(
            "{}\n{}",
            shader::preprocess::expand_includes(&vertex, &resolve)?,
            shader::preprocess::expand_includes(&fragment, &resolve)?
        );
        let samplers: Vec<(u32, Decl)> = raw_decls(&raw)
            .into_iter()
            .filter(Decl::is_texture)
            .filter_map(|d| Some((texture_slot(&d.name)?, d)))
            .collect();
        // A texture the material/scene sets turns on its sampler's combo (e.g. MASK).
        for (slot, d) in &samplers {
            if let Some(combo) = d.annotation.as_ref().and_then(|a| a["combo"].as_str())
                && textures.contains_key(slot)
            {
                combos.entry(combo.to_owned()).or_insert(1);
            }
        }

        let program = self.program(&spec.shader, &vertex, &fragment, &combos)?;

        // Defaults only for samplers the selected variant actually uses.
        for slot in self.scene.programs[program.0].texture_bindings.keys() {
            let default = samplers
                .iter()
                .find(|(s, _)| s == slot)
                .and_then(|(_, d)| d.annotation.as_ref()?["default"].as_str());
            if let Some(default) = default {
                textures.entry(*slot).or_insert_with(|| default.to_owned());
            }
        }

        let mut bindings = BTreeMap::new();
        for (slot, name) in &textures {
            let binding = if name.starts_with("_rt_") {
                self.rt_binding(name, fbos.unwrap_or_default())
            } else {
                match self.texture(name) {
                    Some(id) => TextureBinding::Asset(id),
                    None => continue,
                }
            };
            bindings.insert(*slot, binding);
        }

        let mut constants = BTreeMap::new();
        let uniforms = self
            .program_uniforms
            .get(&program)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for d in uniforms {
            let Some(ann) = d.annotation.as_ref() else {
                continue;
            };
            let Some(key) = ann["material"].as_str() else {
                continue;
            };
            let value = spec
                .constants
                .iter()
                .find_map(|m| lookup(m, key))
                .and_then(floats)
                .or_else(|| ann.get("default").and_then(floats));
            if let Some(v) = value {
                constants.insert(d.name.clone(), v);
            }
        }

        Ok(Pass {
            program,
            textures: bindings,
            target: None,
            constants,
            blend: spec.blend,
        })
    }

    /// `_rt_*` names and bind names: effect FBOs, else the layer's own image.
    fn rt_binding(&mut self, name: &str, fbos: &[Fbo]) -> TextureBinding {
        if fbos.iter().any(|f| f.name == name) {
            return TextureBinding::Fbo(name.to_owned());
        }
        if name == "_rt_FullFrameBuffer" {
            return TextureBinding::FrameBuffer;
        }
        if name == "_rt_MipMappedFrameBuffer" {
            // Same image; WE samples its mips for blur (we have none yet).
            self.skip("render target _rt_MipMappedFrameBuffer (no mips)".into());
            return TextureBinding::FrameBuffer;
        }
        if name != "previous" && !name.starts_with("_rt_imageLayerComposite_") {
            self.skip(format!("render target {name}"));
        }
        TextureBinding::Previous
    }

    fn program(
        &mut self,
        name: &str,
        vertex: &str,
        fragment: &str,
        combos: &BTreeMap<String, i64>,
    ) -> Result<ProgramId> {
        let key = (name.to_owned(), combos.clone());
        if let Some(id) = self.programs.get(&key) {
            return Ok(*id);
        }
        let vfs = self.vfs;
        let resolve = |inc: &str| vfs.read_string(&format!("shaders/{inc}"));
        let prepared = shader::prepare_pair(
            vertex,
            fragment,
            &resolve,
            &shader::engine_defines(),
            combos,
            &self.engine,
        )?;
        for r in &prepared.requires {
            match r.as_str() {
                "LightingV1" => {
                    self.skip("lit materials: ambient only (no scene lights yet)".into())
                }
                other => self.skip(format!("#require {other}")),
            }
        }
        let hlsl = prepared.hlsl()?;

        let mut attributes = Vec::new();
        let mut location = 0;
        for a in &hlsl.attributes {
            let components = match a.ty.as_str() {
                "float" => 1,
                "vec2" => 2,
                "vec3" => 3,
                _ => 4,
            };
            attributes.push(Attribute {
                name: a.name.clone(),
                location,
                components,
            });
            location += if a.ty.starts_with("mat") { 4 } else { 1 };
        }
        let texture_bindings = hlsl
            .textures
            .iter()
            .enumerate()
            .filter_map(|(i, t)| Some((texture_slot(&t.name)?, u32::try_from(i).ok()?)))
            .collect();
        let mut uniforms = hlsl.vertex_uniforms.clone();
        for u in &hlsl.fragment_uniforms {
            if !uniforms.iter().any(|v| v.name == u.name) {
                uniforms.push(u.clone());
            }
        }

        let id = ProgramId(self.scene.programs.len());
        self.scene.programs.push(ShaderProgram {
            name: name.to_owned(),
            vertex_hlsl: hlsl.vertex,
            fragment_hlsl: hlsl.fragment,
            vertex_entry: hlsl.vertex_entry.to_owned(),
            fragment_entry: hlsl.fragment_entry.to_owned(),
            attributes,
            texture_bindings,
        });
        self.program_uniforms.insert(id, uniforms);
        self.programs.insert(key, id);
        Ok(id)
    }

    fn texture(&mut self, name: &str) -> Option<TextureId> {
        if let Some(cached) = self.textures.get(name) {
            return *cached;
        }
        let result = self.load_texture(name);
        let id = match result {
            Ok(t) => {
                let id = TextureId(self.scene.textures.len());
                self.scene.textures.push(t);
                Some(id)
            }
            Err(e) => {
                self.skip(format!("texture {name}: {e}"));
                None
            }
        };
        self.textures.insert(name.to_owned(), id);
        id
    }

    fn load_texture(&self, name: &str) -> Result<Texture> {
        let stem = name.strip_suffix(".tex").unwrap_or(name);
        let path = format!("materials/{stem}.tex");
        let bytes = self
            .vfs
            .read(&path)
            .ok_or_else(|| unsupported("missing texture", &path))?;
        let tex = Tex::parse(&bytes, self.limits)?;
        if tex.is_video {
            return Err(unsupported("video texture", name));
        }
        let mip = tex
            .first_mip()
            .ok_or_else(|| unsupported("texture without mips", name))?;
        let image = mip_to_rgba8(&tex, mip)?;
        let h = &tex.header;
        Ok(Texture {
            name: stem.to_owned(),
            width: image.width,
            height: image.height,
            image_width: h.image_width.min(image.width).max(1),
            image_height: h.image_height.min(image.height).max(1),
            rgba: image.pixels,
            clamp: h.flags.contains(TexFlags::CLAMP_UVS),
            nearest: h.flags.contains(TexFlags::NO_INTERPOLATION),
        })
    }
}

fn unsupported(what: &'static str, value: &str) -> Error {
    Error::Unsupported {
        what,
        value: value.to_owned(),
    }
}

/// `g_Texture7` → 7.
fn texture_slot(name: &str) -> Option<u32> {
    name.strip_prefix("g_Texture")?.parse().ok()
}

/// Every declaration in `source`, including inactive `#if` branches.
fn raw_decls(source: &str) -> Vec<Decl> {
    source
        .lines()
        .filter_map(|l| shader::decl::parse_decl_line(l).ok().flatten())
        .flatten()
        .filter(|d| d.qualifier == Qualifier::Uniform)
        .collect()
}

/// Exact key first, then case-insensitive (WE keys vary in case).
fn lookup<'v>(map: &'v serde_json::Map<String, Value>, key: &str) -> Option<&'v Value> {
    map.get(key).or_else(|| {
        map.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    })
}
