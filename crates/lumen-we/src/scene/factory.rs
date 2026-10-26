//! Layers that scripts create while the scene runs (`thisScene.createLayer`).
//!
//! The factory keeps a light copy of the imported scene (texture and program
//! slots without pixels or shader source) so new layers get the same global
//! ids the renderer already uses, and imports each config like a scene
//! object: `"models/x.json"` or `{image: "models/x.json", origin, …}` for an
//! image layer, `{text: "…", font, pointsize, …}` for a text layer.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use lumen_core::Limits;
use lumen_scene::{FontData, ImageLayer, Node, Scene, ShaderProgram, Texture};
use serde_json::Value;

use super::{Importer, Properties, SceneGraph, Vfs, values};
use crate::error::{Error, Result};
use crate::shader;

/// A layer imported at runtime, with the resources it added. Texture,
/// program and font ids inside `layer` continue the scene's own numbering.
#[derive(Debug, Clone)]
pub struct CreatedLayer {
    pub layer: ImageLayer,
    /// New textures, in id order after the existing ones.
    pub textures: Vec<Texture>,
    pub programs: Vec<ShaderProgram>,
    pub fonts: Vec<FontData>,
}

pub struct LayerFactory {
    vfs: Vfs,
    limits: Limits,
    properties: Properties,
    /// The scene as the renderer knows it, without pixels or shader source.
    scene: Scene,
    textures: HashMap<String, Option<lumen_scene::TextureId>>,
    programs: HashMap<(String, BTreeMap<String, i64>), lumen_scene::ProgramId>,
    program_uniforms: HashMap<lumen_scene::ProgramId, Vec<shader::Decl>>,
    fonts: HashMap<String, Option<usize>>,
    texture_formats: HashMap<lumen_scene::TextureId, i64>,
    engine: shader::EngineContext,
    zoom: f32,
}

impl std::fmt::Debug for LayerFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayerFactory")
            .field("textures", &self.scene.textures.len())
            .field("programs", &self.scene.programs.len())
            .finish_non_exhaustive()
    }
}

/// `scene` without texture pixels or shader source (ids and sizes kept).
fn light(scene: &Scene) -> Scene {
    let mut light = scene.clone();
    for t in &mut light.textures {
        t.rgba = Vec::new();
        t.mips = Vec::new();
        t.video = None;
    }
    for p in &mut light.programs {
        p.vertex_hlsl = String::new();
        p.fragment_hlsl = String::new();
    }
    light.scripts = Vec::new();
    light.animations = Vec::new();
    light.sounds = Vec::new();
    light
}

impl LayerFactory {
    /// For the scene imported from `project_dir` (with the same user
    /// property choices) as `scene`.
    pub fn new(
        project_dir: &Path,
        assets_dir: Option<&Path>,
        limits: &Limits,
        choices: &BTreeMap<String, Value>,
        scene: &Scene,
    ) -> Result<Self> {
        let vfs = Vfs::open(project_dir, assets_dir, limits)?;
        let properties = vfs
            .read_json("project.json")
            .map(|p| Properties::from_project(&p))
            .unwrap_or_default()
            .with_overrides(choices);
        let mut json = super::read_scene_json(&vfs)?;
        properties.resolve(&mut json);
        // The scene header gives `general.zoom` (layers scale about their origin).
        let zoom = {
            let mut header = Importer::new(&vfs, limits);
            header.scene_header(&json);
            header.zoom
        };
        let fonts = scene
            .fonts
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.clone(), Some(i)))
            .collect();
        Ok(Self {
            engine: shader::EngineContext {
                point_lights: scene.point_lights.len(),
                tube_lights: scene.tube_lights.len(),
            },
            scene: light(scene),
            vfs,
            limits: limits.clone(),
            properties,
            textures: HashMap::new(),
            programs: HashMap::new(),
            program_uniforms: HashMap::new(),
            fonts,
            texture_formats: HashMap::new(),
            zoom,
        })
    }

    /// Imports `config` as scene node `node` (the next node index; scripts
    /// own the numbering). Fails when the config makes no drawable layer.
    pub fn create(&mut self, config: &Value, node: usize) -> Result<CreatedLayer> {
        let mut object = match config {
            Value::String(path) => serde_json::json!({ "image": path }),
            Value::Object(_) => config.clone(),
            _ => return Err(unsupported("layer config", &config.to_string())),
        };
        self.properties.resolve(&mut object);
        // Workshop scripts name their own assets without `workshop/<id>/`.
        if let Some(path) = object["image"].as_str()
            && self.vfs.read(path).is_none()
            && let Some(found) = self.vfs.workshop_variant(path)
        {
            object["image"] = Value::String(found);
        }
        if let Some(map) = object.as_object_mut() {
            // Objects default to visible; scripts toggle it afterwards.
            map.entry("visible").or_insert(Value::Bool(true));
            map.remove("parent");
        }
        let (textures, programs, fonts) = (
            self.scene.textures.len(),
            self.scene.programs.len(),
            self.scene.fonts.len(),
        );
        let layers = self.scene.layers.len();
        // Keep node numbering in step with the scripts' own.
        while self.scene.nodes.len() < node {
            self.scene.nodes.push(Node {
                name: String::new(),
                id: -1,
                parent: None,
                origin: [0.0; 3],
                scale: [1.0; 3],
                angles: [0.0; 3],
                visible: false,
            });
        }
        let new_node = Node {
            name: object["name"].as_str().unwrap_or("").to_owned(),
            id: -1,
            parent: None,
            origin: values::vec_n(object.get("origin"), [0.0; 3]),
            scale: values::vec_n(object.get("scale"), [1.0; 3]),
            angles: values::vec_n(object.get("angles"), [0.0; 3]),
            visible: values::boolean(object.get("visible"), true),
        };
        if node < self.scene.nodes.len() {
            self.scene.nodes[node] = new_node;
        } else {
            self.scene.nodes.push(new_node);
        }

        let mut importer = Importer::new(&self.vfs, &self.limits);
        importer.properties = self.properties.clone();
        importer.scene = std::mem::take(&mut self.scene);
        importer.textures = std::mem::take(&mut self.textures);
        importer.programs = std::mem::take(&mut self.programs);
        importer.program_uniforms = std::mem::take(&mut self.program_uniforms);
        importer.fonts = std::mem::take(&mut self.fonts);
        importer.texture_formats = std::mem::take(&mut self.texture_formats);
        importer.engine = self.engine;
        importer.zoom = self.zoom;
        let graph = SceneGraph::new(&[&object]);
        importer.object(&object, node, &graph);
        // Perspective scenes place layers in 3D.
        if importer.scene.camera.is_some() {
            let scene = &mut importer.scene;
            for layer in &mut scene.layers[layers..] {
                if let Some(n) = layer.node {
                    layer.world = Some(lumen_scene::mat4_mul(
                        lumen_scene::affine_mat4(layer.local),
                        lumen_scene::node_world_mat4(&scene.nodes, n),
                    ));
                }
            }
        }
        let Importer {
            scene: mut built,
            textures: texture_cache,
            programs: program_cache,
            program_uniforms,
            fonts: font_cache,
            texture_formats,
            ..
        } = importer;
        self.textures = texture_cache;
        self.programs = program_cache;
        self.program_uniforms = program_uniforms;
        self.fonts = font_cache;
        self.texture_formats = texture_formats;

        let created = CreatedLayer {
            layer: built.layers.get(layers).cloned().ok_or_else(|| {
                let reason = built.skipped.last().cloned().unwrap_or_default();
                unsupported("created layer", &reason)
            })?,
            textures: built.textures[textures..].to_vec(),
            programs: built.programs[programs..].to_vec(),
            fonts: built.fonts[fonts..].to_vec(),
        };
        // Keep slots (not data) for what the renderer now owns.
        built.layers.truncate(layers);
        built.layers.push(created.layer.clone());
        built.scripts.clear();
        built.animations.clear();
        built.sounds.clear();
        self.scene = light(&built);
        Ok(created)
    }
}

fn unsupported(what: &'static str, value: &str) -> Error {
    Error::Unsupported {
        what,
        value: value.to_owned(),
    }
}
