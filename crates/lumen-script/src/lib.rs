//! SceneScript runtime: runs a scene's scripted values in QuickJS and turns
//! what they change into per-layer updates.
//!
//! Every scripted value is an ES module (`docs/re-notes/scenescript.md`). The
//! host loads WE's helper classes and modules (from the user's install, via
//! the scene's [`ScriptLibrary`](lumen_scene::ScriptLibrary)), then Lumen's
//! own `prelude.js` (`thisScene`, layers, `engine`, `input`, …), then each
//! script with `thisLayer` bound to its object. A frame sets the inputs and
//! runs timers, calls each `update()` separately (each with its own time
//! budget, so a runaway script only stops itself), and collects the changes
//! as JSON.
//!
//! Untrusted content: the runtime has a memory limit, every call has a time
//! budget enforced by QuickJS's interrupt handler, and scripts get no file,
//! network or process APIs (QuickJS has none unless the host adds them).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lumen_scene::{Scene, ScriptTarget, node_depth, node_world};
use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::{CatchResultExt, Context, Function, Module, Object, Persistent, Runtime};
use serde_json::{Value, json};

mod media;

pub use media::{ThumbnailColors, thumbnail_colors};

const PRELUDE: &str = include_str!("prelude.js");
const MEMORY_LIMIT: usize = 128 << 20;
/// Budget for loading every script and running `init`.
const START_BUDGET: Duration = Duration::from_secs(2);
/// Budget for one script's `update()`, and for a frame's timers.
const CALL_BUDGET: Duration = Duration::from_millis(20);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("JavaScript runtime: {0}")]
    Runtime(String),
}

/// Inputs of one script frame.
#[derive(Debug, Clone, Copy)]
pub struct Inputs {
    /// Wall clock for `Date` in milliseconds since the Unix epoch.
    pub clock_ms: f64,
    /// Seconds since the wallpaper started (`engine.runtime`).
    pub runtime: f64,
    /// Seconds since the previous frame (`engine.frametime`).
    pub frametime: f64,
    /// Cursor in scene pixels, y up (`input.cursorWorldPosition`).
    pub cursor: [f32; 2],
    /// Fraction of the local day, 0..1 (`engine.timeOfDay`).
    pub daytime: f32,
    /// The cursor is over the desktop (layers under it get cursor events).
    pub pointer_active: bool,
    /// Left mouse button held on the desktop.
    pub left_down: bool,
    /// Audio spectrum, 64 bands per channel (left, right), 0..1.
    pub audio: [[f32; 64]; 2],
}

/// What changed about one layer this frame. `None` / empty = unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerChange {
    pub transform: Option<[f32; 6]>,
    /// 3D placement (perspective scenes only).
    pub world: Option<lumen_scene::Mat4>,
    pub color: Option<[f32; 4]>,
    pub visible: Option<bool>,
    pub text: Option<String>,
    /// (effect, visible).
    pub effects: Vec<(usize, bool)>,
    /// (effect, pass, uniform, value).
    pub constants: Vec<(usize, usize, String, Vec<f32>)>,
    /// Animated texture frame set through `getTextureAnimation()`.
    pub texture_frame: Option<u32>,
}

/// A layer a script created (`thisScene.createLayer`): the host must import
/// `config` as layer `layer` placed by node `node` and add it to the renderer.
#[derive(Debug, Clone, PartialEq)]
pub struct CreatedLayer {
    pub layer: usize,
    pub node: usize,
    /// A model path or an object config, as the script gave it.
    pub config: Value,
}

/// What a script asked of a sound object (index in `Scene::sounds`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SoundCommand {
    Play(usize),
    Pause(usize),
    Stop(usize),
    /// 0..1.
    Volume(usize, f32),
}

/// What the host knows about one layer: placement inputs and what it last
/// reported.
struct LayerSlot {
    node: Option<usize>,
    local: [f32; 6],
    color: [f32; 4],
    text: Option<String>,
    shown: LayerShown,
    size: [u32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct LayerShown {
    transform: [f32; 6],
    world: Option<lumen_scene::Mat4>,
    visible: bool,
}

pub struct ScriptHost {
    // Dropped in declaration order: persistent handles before their runtime.
    calls: Calls,
    /// Scripts with an `update()`.
    updatable: Vec<usize>,
    context: Context,
    _runtime: Runtime,
    deadline: Arc<Deadline>,
    nodes: Vec<lumen_scene::Node>,
    layers: Vec<LayerSlot>,
    /// Component events for the next frame: (handler, event object).
    events: Vec<(String, Value)>,
    /// Whether each sound plays (from the player), for `isPlaying()`.
    sound_states: Option<Vec<bool>>,
    /// Sound commands from the last frame.
    sound_commands: Vec<SoundCommand>,
    /// Point-light intensities scripts set in the last frame.
    light_changes: Vec<(usize, f32)>,
    /// `localStorage` as JSON after scripts changed it (to persist).
    storage: Option<String>,
    /// Layers created and reordered in the last frame.
    created: Vec<CreatedLayer>,
    /// (layer, drawn just below this layer; `None` = on top).
    sorts: Vec<(usize, Option<usize>)>,
}

impl std::fmt::Debug for ScriptHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptHost")
            .field("layers", &self.layers.len())
            .finish_non_exhaustive()
    }
}

struct Calls {
    begin: Persistent<Function<'static>>,
    run: Persistent<Function<'static>>,
    disable: Persistent<Function<'static>>,
    end: Persistent<Function<'static>>,
}

/// Time budget shared with QuickJS's interrupt handler.
struct Deadline {
    start: Instant,
    /// Nanoseconds after `start`.
    at: AtomicU64,
}

impl Deadline {
    fn set(&self, budget: Duration) {
        let at = self.start.elapsed() + budget;
        self.at.store(
            u64::try_from(at.as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    fn passed(&self) -> bool {
        u64::try_from(self.start.elapsed().as_nanos()).unwrap_or(u64::MAX)
            > self.at.load(Ordering::Relaxed)
    }
}

fn js_err(e: impl std::fmt::Display) -> Error {
    Error::Runtime(e.to_string())
}

fn target_json(target: &ScriptTarget) -> Value {
    let t = |kind: &str, a: usize| json!({"kind": kind, "a": a});
    match target {
        ScriptTarget::Origin(n) => t("origin", *n),
        ScriptTarget::Scale(n) => t("scale", *n),
        ScriptTarget::Angles(n) => t("angles", *n),
        ScriptTarget::Visible(n) => t("visible", *n),
        ScriptTarget::Alpha(l) => t("alpha", *l),
        ScriptTarget::Color(l) => t("color", *l),
        ScriptTarget::Text(l) => t("text", *l),
        ScriptTarget::LightIntensity(i) => t("light", *i),
        ScriptTarget::EffectVisible { layer, effect } => {
            json!({"kind": "effectVisible", "a": layer, "b": effect})
        }
        ScriptTarget::Constant {
            layer,
            effect,
            pass,
            uniform,
        } => json!({"kind": "constant", "a": layer, "b": effect, "c": pass, "uniform": uniform}),
        ScriptTarget::Other(what) => json!({"kind": "other", "what": what}),
    }
}

/// A text layer's style as `ITextLayer` exposes it (WE units: points).
fn text_style(scene: &Scene, t: &lumen_scene::TextSpec) -> Value {
    let align = |a: lumen_scene::TextAlign, start: &str, end: &str| match a {
        lumen_scene::TextAlign::Start => start.to_owned(),
        lumen_scene::TextAlign::Center => "center".to_owned(),
        lumen_scene::TextAlign::End => end.to_owned(),
    };
    json!({
        "pointsize": t.pixel_size * 72.0 / 300.0,
        "font": scene.fonts.get(t.font).map(|f| f.name.as_str()),
        "padding": t.padding[0],
        "horizontalalign": align(t.halign, "left", "right"),
        "verticalalign": align(t.valign, "top", "bottom"),
        "maxwidth": t.max_width,
        "maxrows": t.max_rows,
    })
}

/// The scene as the prelude sees it (`__lumen.init`).
fn state_json(scene: &Scene, canvas: [u32; 2]) -> String {
    let layer_of_node: BTreeMap<usize, usize> = scene
        .layers
        .iter()
        .enumerate()
        .filter_map(|(i, l)| Some((l.node?, i)))
        .collect();
    let nodes: Vec<Value> = scene
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            json!({
                "name": n.name, "id": n.id, "parent": n.parent,
                "origin": n.origin, "scale": n.scale, "angles": n.angles,
                "visible": n.visible, "layer": layer_of_node.get(&i),
            })
        })
        .collect();
    let layers: Vec<Value> = scene
        .layers
        .iter()
        .map(|l| {
            json!({
                "alpha": l.color[3],
                "color": &l.color[..3],
                "text": l.text.as_ref().map(|t| &t.text),
                "style": l.text.as_ref().map(|t| text_style(scene, t)),
                "size": l.size,
                "frames": texture_frames(scene, l),
                // Per pass, the material keys scripts may set (getMaterial).
                "effects": l.effects.iter()
                    .map(|e| json!({
                        "name": e.name,
                        "visible": e.visible,
                        "materials": e.passes.iter()
                            .map(|p| scene.programs.get(p.program.0).map(|g| &g.material_keys))
                            .collect::<Vec<_>>(),
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    let animations: Vec<Value> = scene
        .animations
        .iter()
        .map(|a| {
            let mut v = target_json(&a.target);
            v["node"] = json!(a.node);
            v["keyframes"] = serde_json::from_str(&a.keyframes).unwrap_or(Value::Null);
            v["initial"] = serde_json::from_str(&a.initial).unwrap_or(Value::Null);
            v
        })
        .collect();
    let scripts: Vec<Value> = scene
        .scripts
        .iter()
        .map(|s| {
            let mut v = target_json(&s.target);
            v["node"] = json!(s.node);
            v["initial"] = serde_json::from_str(&s.initial).unwrap_or(Value::Null);
            v["properties"] = json!(s.properties);
            v
        })
        .collect();
    let (parallax, shake) = (scene.parallax, scene.shake);
    let general = json!({
        "cameraparallax": parallax.is_some(),
        "cameraparallaxamount": parallax.map_or(0.0, |p| p.amount),
        "cameraparallaxdelay": parallax.map_or(0.0, |p| p.delay),
        "cameraparallaxmouseinfluence": parallax.map_or(0.0, |p| p.mouse_influence),
        "camerashake": shake.is_some(),
        "camerashakeamplitude": shake.map_or(0.0, |s| s.amplitude),
        "camerashakespeed": shake.map_or(0.0, |s| s.speed),
        "camerashakeroughness": shake.map_or(0.0, |s| s.roughness),
        "camerafade": false,
        "clearenabled": true,
        "clearcolor": &scene.clear_color[..3],
        "ambientcolor": scene.ambient_color,
        "skylightcolor": scene.skylight_color,
        "bloom": scene.hdr,
        "fov": 50.0,
        "nearz": 0.01,
        "farz": 10000.0,
        "bloomstrength": 1.0,
        "bloomthreshold": 0.65,
    });
    json!({
        "nodes": nodes, "layers": layers, "scripts": scripts, "general": general,
        "animations": animations,
        "sounds": scene.sounds.iter().map(|s| json!({
            "node": s.node, "volume": s.volume, "playing": !s.start_silent,
        })).collect::<Vec<_>>(),
        "userProperties": scene.user_properties, "canvas": canvas,
    })
    .to_string()
}

impl ScriptHost {
    /// Loads every script of `scene`; `None` when it has none.
    pub fn new(scene: &Scene, canvas: [u32; 2], clock_ms: f64) -> Result<Option<Self>, Error> {
        Self::with_storage(scene, canvas, clock_ms, None)
    }

    /// [`ScriptHost::new`] with `localStorage` restored from a previous run
    /// (a JSON object, see [`ScriptHost::take_storage`]).
    pub fn with_storage(
        scene: &Scene,
        canvas: [u32; 2],
        clock_ms: f64,
        storage: Option<&str>,
    ) -> Result<Option<Self>, Error> {
        if scene.scripts.is_empty() && scene.animations.is_empty() {
            return Ok(None);
        }
        let library = scene.script_library.clone().unwrap_or_default();
        let runtime = Runtime::new().map_err(js_err)?;
        runtime.set_memory_limit(MEMORY_LIMIT);
        let deadline = Arc::new(Deadline {
            start: Instant::now(),
            at: AtomicU64::new(0),
        });
        deadline.set(START_BUDGET);
        let watch = deadline.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || watch.passed())));
        let mut resolver = BuiltinResolver::default();
        let mut loader = BuiltinLoader::default();
        for (name, source) in &library.modules {
            resolver.add_module(name.clone());
            loader.add_module(name.clone(), source.clone());
        }
        runtime.set_loader(resolver, loader);
        let context = Context::full(&runtime).map_err(js_err)?;

        let (calls, updatable) = context.with(|ctx| -> Result<_, Error> {
            ctx.globals()
                .set("__lumen", Object::new(ctx.clone()).map_err(js_err)?)
                .map_err(js_err)?;
            let lumen: Object = ctx.globals().get("__lumen").map_err(js_err)?;
            lumen.set("clock", clock_ms).map_err(js_err)?;
            if let Some(storage) = storage {
                lumen.set("storage", storage).map_err(js_err)?;
            }
            ctx.eval::<(), _>(library.classes.as_str())
                .catch(&ctx)
                .map_err(js_err)?;
            ctx.eval::<(), _>(PRELUDE).catch(&ctx).map_err(js_err)?;
            let init: Function = lumen.get("init").map_err(js_err)?;
            init.call::<_, ()>((state_json(scene, canvas),))
                .catch(&ctx)
                .map_err(js_err)?;
            let attach: Function = lumen.get("attach").map_err(js_err)?;
            for (i, script) in scene.scripts.iter().enumerate() {
                let this = script.node.map_or("null".to_owned(), |n| n.to_string());
                // Scripts on an effect's values see that effect as thisObject.
                let object = match &script.target {
                    ScriptTarget::EffectVisible { layer, effect }
                    | ScriptTarget::Constant { layer, effect, .. } => {
                        format!("__lumen.effectFor({layer}, {effect})")
                    }
                    _ => "thisLayer".to_owned(),
                };
                // Same line as the source's first line: error lines stay right.
                let source = format!(
                    "const thisLayer = __lumen.layerFor({this}); const thisObject = {object}; {}",
                    script.source
                );
                let loaded = Module::declare(ctx.clone(), format!("script{i}"), source)
                    .and_then(|m| {
                        let (module, promise) = m.eval()?;
                        promise.finish::<()>()?;
                        module.namespace()
                    })
                    .catch(&ctx);
                match loaded {
                    Ok(ns) => attach.call::<_, ()>((i, ns)).map_err(js_err)?,
                    Err(e) => tracing::warn!(script = i, "script failed to load: {e}"),
                }
            }
            let start: Function = lumen.get("start").map_err(js_err)?;
            if let Err(e) = start.call::<_, ()>(()).catch(&ctx) {
                tracing::warn!("script start: {e}");
            }
            let updatable: Function = lumen.get("updatable").map_err(js_err)?;
            let updatable: Vec<usize> = updatable.call(()).map_err(js_err)?;
            let get = |name: &str| -> Result<_, Error> {
                let f: Function = lumen.get(name).map_err(js_err)?;
                Ok(Persistent::save(&ctx, f))
            };
            let calls = Calls {
                begin: get("begin")?,
                run: get("run")?,
                disable: get("disable")?,
                end: get("end")?,
            };
            Ok((calls, updatable))
        })?;

        let layers = scene
            .layers
            .iter()
            .map(|l| LayerSlot {
                size: l.size,
                node: l.node,
                local: l.local,
                color: l.color,
                text: l.text.as_ref().map(|t| t.text.clone()),
                shown: LayerShown {
                    transform: l.transform,
                    world: l.world,
                    visible: l.visible,
                },
            })
            .collect();
        Ok(Some(Self {
            calls,
            updatable,
            context,
            _runtime: runtime,
            deadline,
            nodes: scene.nodes.clone(),
            layers,
            events: Vec::new(),
            sound_states: None,
            sound_commands: Vec::new(),
            light_changes: Vec::new(),
            created: Vec::new(),
            sorts: Vec::new(),
            storage: None,
        }))
    }

    /// Runs one frame of scripts; changed layers by IR index.
    pub fn update(&mut self, inputs: &Inputs) -> BTreeMap<usize, LayerChange> {
        let deadline = &self.deadline;
        let calls = &self.calls;
        let mut stopped = Vec::new();
        let events = std::mem::take(&mut self.events);
        let out = self.context.with(|ctx| {
            let restore = |f: &Persistent<Function<'static>>| f.clone().restore(&ctx).ok();
            let (begin, run, disable, end) = (
                restore(&calls.begin)?,
                restore(&calls.run)?,
                restore(&calls.disable)?,
                restore(&calls.end)?,
            );
            deadline.set(CALL_BUDGET);
            let pointer = json!({
                "down": inputs.left_down,
                "hits": self.hits(inputs),
                "events": events,
                "audio": [inputs.audio[0].to_vec(), inputs.audio[1].to_vec()],
                "sounds": self.sound_states,
            })
            .to_string();
            let args = (
                inputs.clock_ms,
                inputs.runtime,
                inputs.frametime,
                inputs.cursor[0],
                inputs.cursor[1],
                inputs.daytime,
                pointer,
            );
            if let Err(e) = begin.call::<_, ()>(args).catch(&ctx) {
                tracing::warn!("script timers: {e}");
            }
            for &i in &self.updatable {
                deadline.set(CALL_BUDGET);
                if let Err(e) = run.call::<_, ()>((i,)).catch(&ctx) {
                    // Uncatchable inside JS: the interrupt or out of memory.
                    tracing::warn!(script = i, "script stopped: {e}");
                    stopped.push(i);
                    deadline.set(CALL_BUDGET);
                    let _ = disable.call::<_, ()>((i,));
                }
            }
            deadline.set(CALL_BUDGET);
            end.call::<_, String>(()).catch(&ctx).ok()
        });
        self.updatable.retain(|i| !stopped.contains(i));
        let Some(out) = out.and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
            return BTreeMap::new();
        };
        self.apply(&out)
    }

    /// Tells scripts which sounds play (what `isPlaying()` returns); without
    /// this they see what they asked for.
    pub fn set_sound_states(&mut self, playing: Vec<bool>) {
        self.sound_states = Some(playing);
    }

    /// Sound commands scripts gave during the last [`ScriptHost::update`].
    pub fn take_sound_commands(&mut self) -> Vec<SoundCommand> {
        std::mem::take(&mut self.sound_commands)
    }

    /// (point light, intensity) set by scripts during the last update.
    pub fn take_light_changes(&mut self) -> Vec<(usize, f32)> {
        std::mem::take(&mut self.light_changes)
    }

    /// `localStorage` (a JSON object) if scripts changed it since the last
    /// call: persist it and hand it to [`ScriptHost::with_storage`] next run.
    pub fn take_storage(&mut self) -> Option<String> {
        self.storage.take()
    }

    /// Layers scripts created in the last frame; add them to the renderer
    /// (then [`ScriptHost::register_layer`]) before applying that frame's
    /// changes.
    pub fn take_created(&mut self) -> Vec<CreatedLayer> {
        std::mem::take(&mut self.created)
    }

    /// Layer moves of the last frame: (layer, drawn just below this one).
    pub fn take_sorts(&mut self) -> Vec<(usize, Option<usize>)> {
        std::mem::take(&mut self.sorts)
    }

    /// Tells the host how a created layer was imported (its size and local
    /// placement), for hit testing and transforms from the next frame.
    pub fn register_layer(&mut self, index: usize, layer: &lumen_scene::ImageLayer) {
        if let Some(slot) = self.layers.get_mut(index) {
            slot.size = layer.size;
            slot.local = layer.local;
            slot.shown.world = layer.world;
            // Recompute the placement with the real local transform.
            slot.shown.transform = [f32::NAN; 6];
        }
    }

    /// Delivers a component event (e.g. `mediaPlaybackChanged`) to every
    /// script exporting `handler`, at the start of the next frame. Vectors
    /// in `event` given as `{"vec3": [r, g, b]}` become `Vec3`.
    pub fn send(&mut self, handler: &str, event: Value) {
        tracing::debug!(handler, %event, "script event");
        if self.events.len() < 64 {
            self.events.push((handler.to_owned(), event));
        }
    }

    /// Layers under the cursor as `[[node, local x, local y], …]` (local
    /// pixels around the layer centre, y up), back to front.
    fn hits(&self, inputs: &Inputs) -> Value {
        if !inputs.pointer_active {
            return json!([]);
        }
        let [cx, cy] = inputs.cursor;
        let hits: Vec<Value> = self
            .layers
            .iter()
            .filter(|s| s.shown.visible && s.size[0] > 1 && s.size[1] > 1)
            .filter_map(|s| {
                let node = s.node?;
                let [a, b, c, d, e, f] = s.shown.transform;
                let det = a * d - b * c;
                if det.abs() < 1e-9 {
                    return None;
                }
                let (x, y) = (cx - e, cy - f);
                let lx = (d * x - c * y) / det;
                let ly = (-b * x + a * y) / det;
                let [w, h] = s.size.map(|v| v as f32 * 0.5);
                (lx.abs() <= w && ly.abs() <= h).then(|| json!([node, lx, ly]))
            })
            .collect();
        Value::Array(hits)
    }

    fn apply(&mut self, out: &Value) -> BTreeMap<usize, LayerChange> {
        for line in out["logs"].as_array().into_iter().flatten() {
            tracing::debug!(target: "scenescript", "{}", line.as_str().unwrap_or(""));
        }
        let mut changes: BTreeMap<usize, LayerChange> = BTreeMap::new();

        let mut moved = false;
        for c in out["created"].as_array().into_iter().flatten() {
            let (Some(layer), Some(node)) = (index(&c[0]), index(&c[1])) else {
                continue;
            };
            // Scripts number new layers and nodes after the existing ones.
            if layer != self.layers.len() || node != self.nodes.len() {
                tracing::warn!(layer, node, "created layer out of step; ignored");
                continue;
            }
            let config = c[2].clone();
            let vec = |key: &str, default: [f32; 3]| {
                let mut v = default;
                read3(&config[key], &mut v);
                v
            };
            self.nodes.push(lumen_scene::Node {
                name: config["name"].as_str().unwrap_or("").to_owned(),
                id: -1,
                parent: None,
                origin: vec("origin", [0.0; 3]),
                scale: vec("scale", [1.0; 3]),
                angles: vec("angles", [0.0; 3]),
                visible: config["visible"].as_bool().unwrap_or(true),
            });
            let rgb = vec("color", [1.0; 3]);
            self.layers.push(LayerSlot {
                node: Some(node),
                local: lumen_scene::IDENTITY_AFFINE,
                color: [
                    rgb[0],
                    rgb[1],
                    rgb[2],
                    config["alpha"].as_f64().unwrap_or(1.0) as f32,
                ],
                text: config["text"].as_str().map(str::to_owned),
                shown: LayerShown {
                    transform: [f32::NAN; 6],
                    world: None,
                    visible: true,
                },
                size: [0, 0],
            });
            self.created.push(CreatedLayer {
                layer,
                node,
                config,
            });
            moved = true;
        }
        if let Some(storage) = out["storage"].as_str() {
            self.storage = Some(storage.to_owned());
        }
        for s in out["sorts"].as_array().into_iter().flatten() {
            if let Some(layer) = index(&s[0]) {
                self.sorts.push((layer, index(&s[1])));
            }
        }
        // New or newly registered layers need their placement computed.
        moved |= self.layers.iter().any(|s| s.shown.transform[0].is_nan());
        for (key, n) in out["nodes"].as_object().into_iter().flatten() {
            let Some(node) = key
                .parse::<usize>()
                .ok()
                .and_then(|i| self.nodes.get_mut(i))
            else {
                continue;
            };
            read3(&n["origin"], &mut node.origin);
            read3(&n["scale"], &mut node.scale);
            read3(&n["angles"], &mut node.angles);
            if let Some(v) = n["visible"].as_bool() {
                node.visible = v;
            }
            moved = true;
        }
        if moved {
            for (i, slot) in self.layers.iter_mut().enumerate() {
                let Some(node) = slot.node else {
                    continue;
                };
                let (world, mut visible) = node_world(&self.nodes, node);
                // Orthographic layers past the camera's depth range vanish.
                if slot.shown.world.is_none() {
                    visible &= node_depth(&self.nodes, node).abs() <= lumen_scene::ORTHO_DEPTH;
                }
                let now = LayerShown {
                    transform: lumen_scene::compose(world, slot.local),
                    world: slot.shown.world.map(|_| {
                        lumen_scene::mat4_mul(
                            lumen_scene::affine_mat4(slot.local),
                            lumen_scene::node_world_mat4(&self.nodes, node),
                        )
                    }),
                    visible,
                };
                let shown = &mut slot.shown;
                if now != *shown {
                    let c = changes.entry(i).or_default();
                    if now.transform != shown.transform {
                        c.transform = Some(now.transform);
                    }
                    if now.world != shown.world {
                        c.world = now.world;
                    }
                    if now.visible != shown.visible {
                        c.visible = Some(now.visible);
                    }
                    *shown = now;
                }
            }
        }

        for (key, l) in out["layers"].as_object().into_iter().flatten() {
            let Some(i) = key.parse::<usize>().ok().filter(|&i| i < self.layers.len()) else {
                continue;
            };
            let slot = &mut self.layers[i];
            let color = &mut slot.color;
            let mut rgb = [color[0], color[1], color[2]];
            read3(&l["color"], &mut rgb);
            let alpha = l["alpha"].as_f64().map_or(color[3], |a| a as f32);
            let new = [rgb[0], rgb[1], rgb[2], alpha];
            let c = changes.entry(i).or_default();
            if new != *color {
                *color = new;
                c.color = Some(new);
            }
            if let Some(text) = l["text"].as_str()
                && slot.text.as_deref() != Some(text)
            {
                slot.text = Some(text.to_owned());
                c.text = Some(text.to_owned());
            }
        }
        for e in out["effects"].as_array().into_iter().flatten() {
            if let (Some(l), Some(effect), Some(v)) = (index(&e[0]), index(&e[1]), e[2].as_bool()) {
                changes.entry(l).or_default().effects.push((effect, v));
            }
        }
        for c in out["constants"].as_array().into_iter().flatten() {
            let (Some(l), Some(effect), Some(pass), Some(uniform)) =
                (index(&c[0]), index(&c[1]), index(&c[2]), c[3].as_str())
            else {
                continue;
            };
            let value: Vec<f32> = c[4]
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect();
            changes
                .entry(l)
                .or_default()
                .constants
                .push((effect, pass, uniform.to_owned(), value));
        }
        for t in out["textureFrames"].as_array().into_iter().flatten() {
            if let (Some(l), Some(frame)) = (index(&t[0]), t[1].as_u64()) {
                changes.entry(l).or_default().texture_frame = u32::try_from(frame).ok();
            }
        }
        for c in out["lights"].as_array().into_iter().flatten() {
            if let (Some(i), Some(v)) = (index(&c[0]), c[1].as_f64()) {
                self.light_changes.push((i, v as f32));
            }
        }
        for c in out["sounds"].as_array().into_iter().flatten() {
            let Some(i) = index(&c[0]) else { continue };
            self.sound_commands.push(match c[1].as_str() {
                Some("play") => SoundCommand::Play(i),
                Some("pause") => SoundCommand::Pause(i),
                Some("stop") => SoundCommand::Stop(i),
                Some("volume") => SoundCommand::Volume(i, c[2].as_f64().unwrap_or(1.0) as f32),
                _ => continue,
            });
        }
        changes.retain(|_, c| *c != LayerChange::default());
        changes
    }
}

/// Frame durations (seconds) of the layer's animated texture (its material's
/// first texture); empty for still images.
fn texture_frames(scene: &Scene, layer: &lumen_scene::ImageLayer) -> Vec<f32> {
    match layer.albedo.textures.get(&0) {
        Some(lumen_scene::TextureBinding::Asset(id)) => scene
            .textures
            .get(id.0)
            .map(|t| t.frames.iter().map(|f| f.duration).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn index(v: &Value) -> Option<usize> {
    v.as_u64().and_then(|i| usize::try_from(i).ok())
}

/// Overwrites `out` with a JSON number array, keeping entries that are
/// missing or not finite numbers.
fn read3(v: &Value, out: &mut [f32]) {
    for (o, x) in out.iter_mut().zip(v.as_array().into_iter().flatten()) {
        if let Some(x) = x.as_f64().filter(|x| x.is_finite()) {
            *o = x as f32;
        }
    }
}

#[cfg(test)]
mod tests;
