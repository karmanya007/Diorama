//! Engine-neutral scene IR (L1, ADR-002).
//!
//! Importers (e.g. `lumen-we`) resolve everything format-specific — file
//! lookups, shader dialect translation, texture decoding, default values —
//! and hand the renderer this fully resolved description. The renderer never
//! reads importer formats.
//!
//! Conventions match what the captured reference frames show:
//! - image layers render into their own target, then effect passes ping-pong
//!   over it, then the result is composited onto the screen;
//! - colours are gamma-encoded RGBA8 throughout (no sRGB conversion).

use std::collections::BTreeMap;
use std::sync::Arc;

pub mod particles;
pub mod puppet;
pub mod space;
pub use particles::ParticleSystem;
pub use space::{Camera, IDENTITY_MAT4, Mat4, affine_mat4, mat4_mul, node_world_mat4};

/// Index into [`Scene::textures`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextureId(pub usize);

/// Index into [`Scene::programs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProgramId(pub usize);

#[derive(Debug, Clone, Default)]
pub struct Scene {
    /// Size of the scene's orthographic projection in scene pixels.
    pub width: u32,
    pub height: u32,
    /// Perspective scenes: the camera (layers are then placed by
    /// [`ImageLayer::world`] in 3D world units).
    pub camera: Option<Camera>,
    pub clear_color: [f32; 4],
    /// The project's scheme colour: WE fills monitor areas the scene does not
    /// cover with it (fit/custom alignment); also the theme colour.
    pub scheme_color: [f32; 3],
    /// Render in floating point (WE's HDR bloom path: values above 1 bloom).
    pub hdr: bool,
    /// `g_LightAmbientColor` / `g_LightSkylightColor` for lit materials.
    pub ambient_color: [f32; 3],
    pub skylight_color: [f32; 3],
    /// Camera shake (`general.camerashake`), if enabled.
    pub shake: Option<Shake>,
    /// Mouse parallax (`general.cameraparallax`), if enabled.
    pub parallax: Option<Parallax>,
    /// Dynamic lights, in the order shaders index them (`g_LPoint_*[i]`).
    pub point_lights: Vec<PointLight>,
    /// Tube lights (`g_LTube_*[i]`): light from the nearest point of a segment.
    pub tube_lights: Vec<TubeLight>,
    /// Back to front.
    pub layers: Vec<ImageLayer>,
    pub textures: Vec<Texture>,
    pub programs: Vec<ShaderProgram>,
    /// Every scene object's own placement (layers and invisible helpers),
    /// so scripts can move objects at runtime.
    pub nodes: Vec<Node>,
    /// Font files used by text layers.
    pub fonts: Vec<FontData>,
    /// Scripted values, in scene order.
    pub scripts: Vec<Script>,
    /// Keyframe-animated values, in scene order.
    pub animations: Vec<Animation>,
    /// Helper JavaScript the scripts expect (`None` without scripts).
    pub script_library: Option<ScriptLibrary>,
    /// User properties as `{"name": {"type", "value"}, …}` JSON (what
    /// `engine.userProperties` exposes).
    pub user_properties: String,
    /// Sound objects (played on the system's audio output).
    pub sounds: Vec<Sound>,
    /// Things the importer could not represent yet (shown in logs/reports).
    pub skipped: Vec<String>,
}

/// A scene object's placement relative to its parent.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub name: String,
    /// The object's id in its source format.
    pub id: i64,
    /// Index into [`Scene::nodes`].
    pub parent: Option<usize>,
    pub origin: [f32; 3],
    pub scale: [f32; 3],
    /// Radians.
    pub angles: [f32; 3],
    pub visible: bool,
}

impl Node {
    /// Origin · rotation(z) · scale. Rotations of π about x/y (mirroring in
    /// an orthographic view) fold into the scale.
    pub fn local(&self) -> [f32; 6] {
        affine(
            [self.origin[0], self.origin[1]],
            self.angles[2],
            [
                self.scale[0] * self.angles[1].cos(),
                self.scale[1] * self.angles[0].cos(),
            ],
        )
    }
}

/// World transform and effective visibility of `nodes[index]` (hidden
/// parents hide children). Depth-capped against cycles.
pub fn node_world(nodes: &[Node], index: usize) -> ([f32; 6], bool) {
    let Some(node) = nodes.get(index) else {
        return (IDENTITY_AFFINE, true);
    };
    let mut transform = node.local();
    let mut visible = node.visible;
    let mut parent = node.parent;
    for _ in 0..64 {
        let Some(p) = parent.and_then(|p| nodes.get(p)) else {
            break;
        };
        transform = compose(p.local(), transform);
        visible &= p.visible;
        parent = p.parent;
    }
    (transform, visible)
}

#[derive(Debug, Clone)]
pub struct FontData {
    pub name: String,
    pub data: Arc<[u8]>,
}

/// A scripted value: an ES module whose `update(value)` result becomes the
/// property each frame (WE SceneScript).
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    pub source: String,
    /// Object the script belongs to (`thisLayer`), index into [`Scene::nodes`].
    pub node: Option<usize>,
    pub target: ScriptTarget,
    /// The property's value before scripts run, as JSON.
    pub initial: String,
    /// Values of the script's own settings (`scriptProperties`), as a JSON object.
    pub properties: String,
}

/// A keyframe-animated value (WE `{"animation": {c0.., options}, "value"}`).
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    /// Object the value belongs to, index into [`Scene::nodes`].
    pub node: Option<usize>,
    pub target: ScriptTarget,
    /// The animation object (channels `c0`…, `options`, `relative`) as JSON.
    pub keyframes: String,
    /// The value before animation, as JSON.
    pub initial: String,
}

/// What a script's result is written to.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptTarget {
    /// Node properties (index into [`Scene::nodes`]).
    Origin(usize),
    Scale(usize),
    Angles(usize),
    Visible(usize),
    /// Layer properties (index into [`Scene::layers`]).
    Alpha(usize),
    Color(usize),
    Text(usize),
    EffectVisible {
        layer: usize,
        effect: usize,
    },
    /// A uniform of one effect pass.
    Constant {
        layer: usize,
        effect: usize,
        pass: usize,
        uniform: String,
    },
    /// Not represented yet; the script still runs for its side effects.
    Other(String),
}

/// JavaScript the scripts build on: global classes (`Vec3`, …) and named
/// modules they import (`WEMath`, …).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScriptLibrary {
    pub classes: String,
    pub modules: Vec<(String, String)>,
}

/// A text layer's content and style; the renderer lays it out (and again
/// whenever scripts change the text).
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpec {
    /// Index into [`Scene::fonts`].
    pub font: usize,
    pub text: String,
    /// Em size in layer pixels.
    pub pixel_size: f32,
    pub halign: TextAlign,
    pub valign: TextAlign,
    pub max_width: Option<f32>,
    pub max_rows: Option<usize>,
    pub ellipsis: bool,
    /// Space around the text box in the layer target (x, y).
    pub padding: [f32; 2],
}

/// Horizontal (start = left) or vertical (start = top) text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    Start,
    #[default]
    Center,
    End,
}

/// Layers shift away from the cursor: offset = depth · amount · influence ·
/// (W·(0.5 − px), H·(py − 0.5)), cursor (px, py) normalised over the output,
/// y down. Measured on RenderDoc captures with the cursor at known points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parallax {
    pub amount: f32,
    pub mouse_influence: f32,
    /// Smoothing time constant in seconds.
    pub delay: f32,
}

/// Smooth random camera drift. Multi-frame captures show a slow,
/// camera-level offset (same for every layer) of about 1 % of the screen
/// height peak to peak at amplitude 1; WE's exact noise is not recovered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shake {
    pub amplitude: f32,
    pub speed: f32,
    pub roughness: f32,
}

/// A point light in scene space (`g_LPoint_Origin` / `g_LPoint_Color`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointLight {
    pub origin: [f32; 3],
    /// Colour already multiplied by intensity.
    pub color: [f32; 3],
    pub radius: f32,
    /// Falloff exponent.
    pub exponent: f32,
}

/// A tube light in scene space (`g_LTube_OriginA/OriginB/Color`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TubeLight {
    /// Segment ends.
    pub a: [f32; 3],
    pub b: [f32; 3],
    /// Colour already multiplied by intensity.
    pub color: [f32; 3],
    pub radius: f32,
    /// Falloff exponent.
    pub exponent: f32,
}

/// A sound object: a list of tracks and how they play.
#[derive(Debug, Clone)]
pub struct Sound {
    pub name: String,
    /// Scene node (scripts reach the sound through it).
    pub node: Option<usize>,
    /// (file name, encoded file) per track, in order.
    pub tracks: Vec<(String, Arc<[u8]>)>,
    /// 0..1.
    pub volume: f32,
    pub mode: SoundMode,
    /// Silent until a script starts it.
    pub start_silent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SoundMode {
    /// Tracks in order, forever.
    Loop,
    /// Tracks in order, once.
    Once,
    /// A random track, then `min_gap..max_gap` seconds of silence, forever.
    Random { min_gap: f32, max_gap: f32 },
}

/// A decoded texture (mip 0 only for now).
#[derive(Debug, Clone)]
pub struct Texture {
    pub name: String,
    /// Allocated size of `rgba` (may be padded beyond the image).
    pub width: u32,
    pub height: u32,
    /// Meaningful image inside the allocation.
    pub image_width: u32,
    pub image_height: u32,
    /// Tightly packed RGBA8, gamma-encoded (mip 0).
    pub rgba: Vec<u8>,
    /// Further mip levels, each half the previous size (at least 1×1).
    pub mips: Vec<Vec<u8>>,
    pub clamp: bool,
    pub nearest: bool,
    /// Animation frames (sprite sheets, animated GIFs); empty = still image.
    pub frames: Vec<TextureFrame>,
    /// A looping MP4 video shown in this texture; `rgba` is then only the
    /// (black) content until the first decoded frame.
    pub video: Option<Arc<[u8]>>,
}

/// One frame of an animated texture: the frame's parallelogram in UV
/// (`uv = origin + u · x_axis + v · y_axis`) and how long it shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureFrame {
    pub origin: [f32; 2],
    pub x_axis: [f32; 2],
    pub y_axis: [f32; 2],
    /// Seconds.
    pub duration: f32,
}

impl TextureFrame {
    /// The frame showing `time` seconds into a looping animation.
    pub fn at(frames: &[TextureFrame], time: f32) -> Option<&TextureFrame> {
        let total: f32 = frames.iter().map(|f| f.duration.max(0.0)).sum();
        if total <= 0.0 {
            return frames.first();
        }
        let mut t = time.rem_euclid(total);
        for f in frames {
            if t < f.duration {
                return Some(f);
            }
            t -= f.duration.max(0.0);
        }
        frames.last()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Blend {
    /// Replace destination.
    #[default]
    Opaque,
    /// `src * a + dst * (1 - a)`.
    Translucent,
    /// `src * a + dst`.
    Additive,
}

#[derive(Debug, Clone)]
pub struct ImageLayer {
    pub name: String,
    /// Unscaled size in scene pixels; also the size of its render targets.
    pub size: [u32; 2],
    /// The scene object placing this layer (index into [`Scene::nodes`]):
    /// `transform = node_world(node) ∘ local`.
    pub node: Option<usize>,
    /// Layer offset/scale relative to its node (anchor alignment, zoom).
    pub local: [f32; 6],
    /// Text content; the renderer builds the glyph mesh and atlas from it
    /// (and centres the text box on `transform`).
    pub text: Option<TextSpec>,
    /// A particle system drawn at `transform` instead of the albedo quad
    /// (each system of the tree with its own `draw` pass).
    pub particles: Option<Box<ParticleSystem>>,
    /// Puppet warp: the albedo is this skinned mesh instead of the quad.
    pub puppet: Option<Box<puppet::Puppet>>,
    /// A 3D mesh drawn by the albedo (model space → `world`) instead of the
    /// quad; perspective scenes only.
    pub model: Option<Arc<Model>>,
    /// Perspective scenes: layer-local pixels (or model units) → world,
    /// `node_world ∘ local` in 3D. `None` in orthographic scenes.
    pub world: Option<Mat4>,
    /// Layer-local pixels (centred, y up) → scene pixels (origin bottom-left,
    /// y up), parents already applied: `x' = m0·x + m2·y + m4`,
    /// `y' = m1·x + m3·y + m5`.
    pub transform: [f32; 6],
    pub visible: bool,
    pub kind: LayerKind,
    /// Index (in [`Scene::layers`]) of the composition layer this layer is
    /// drawn into instead of the scene; it renders right after that layer's
    /// albedo and before its effects.
    pub group: Option<usize>,
    /// `parallaxDepth`: how strongly the layer follows the camera parallax.
    pub parallax_depth: [f32; 2],
    /// Object colour and alpha (`g_Color4`; `g_Color`/`g_Alpha` for solid layers).
    pub color: [f32; 4],
    /// How the finished layer is composited onto the scene.
    pub blend: Blend,
    /// Draws the layer's own image into its first target.
    pub albedo: Pass,
    /// Quads the albedo pass draws instead of one quad over the layer (the
    /// glyphs of a text layer). An albedo with a mesh blends into the layer
    /// target with the layer's blending, like WE's font draws.
    pub mesh: Option<Vec<MeshQuad>>,
    /// The layer target starts as a copy of the frame below the layer
    /// (WE `copybackground`); forces a layer target even without effects.
    pub copy_background: bool,
    /// Custom composite for layers with effects (e.g. a colour blend mode):
    /// placed like a direct layer, sampling the finished layer image as
    /// `Previous`. `None` = plain blended copy.
    pub composite: Option<Pass>,
    pub effects: Vec<Effect>,
}

/// A static triangle mesh with its material's raster state.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub vertices: Vec<ModelVertex>,
    /// Triangle list into `vertices`.
    pub indices: Vec<u32>,
    pub cull: Cull,
    pub depth_test: bool,
    pub depth_write: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// xyz + handedness.
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
}

/// Which faces a model hides (WE `cullmode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Cull {
    /// Back faces (`normal`).
    #[default]
    Back,
    /// None (`nocull`).
    None,
}

/// One textured quad of a [`ImageLayer::mesh`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshQuad {
    /// `[x0, y0, x1, y1]` in layer-local pixels (centred on the layer, y up).
    pub rect: [f32; 4],
    /// `[u0, v0, u1, v1]` texture coordinates of the rect's top-left and
    /// bottom-right corners (v down).
    pub uv: [f32; 4],
}

/// How a layer's albedo pass is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayerKind {
    /// A textured quad placed by `transform`.
    #[default]
    Image,
    /// Covers the scene; the albedo is a clip-space quad over what is below
    /// (WE fullscreen/post-processing layers).
    Fullscreen,
    /// Fills its own target from texcoords while sampling the frame buffer
    /// under its scene placement (WE `composelayer`).
    Compose,
}

#[derive(Debug, Clone)]
pub struct Effect {
    pub name: String,
    /// Hidden effects are skipped (scripts may show them later).
    pub visible: bool,
    /// Extra render targets this effect uses, sized `layer size / scale_div`.
    pub fbos: Vec<Fbo>,
    pub passes: Vec<Pass>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fbo {
    pub name: String,
    pub scale_div: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureBinding {
    Asset(TextureId),
    /// The layer's current image (its running ping-pong target).
    Previous,
    /// A named effect FBO.
    Fbo(String),
    /// Everything composited below this layer so far (`_rt_FullFrameBuffer`).
    FrameBuffer,
    /// The glyph atlas of the layer's text (built by the renderer).
    TextAtlas,
    /// An image the host provides and updates at runtime, by name
    /// (`mediaThumbnail`, `mediaPreviousThumbnail`: WE's `$`-prefixed
    /// system user textures). White until the host sets it.
    System(String),
}

#[derive(Debug, Clone)]
pub struct Pass {
    pub program: ProgramId,
    /// By texture slot (`g_TextureN` ↔ index N).
    pub textures: BTreeMap<u32, TextureBinding>,
    /// Write into this FBO instead of the layer's next ping-pong target.
    pub target: Option<String>,
    /// Uniform values by uniform name (material constants and defaults,
    /// already resolved). Engine uniforms (time, matrices, resolutions) are
    /// supplied by the renderer and override nothing here.
    pub constants: BTreeMap<String, Vec<f32>>,
    pub blend: Blend,
}

/// A translated vertex + fragment shader with the metadata the renderer
/// needs to feed it.
#[derive(Debug, Clone)]
pub struct ShaderProgram {
    /// Human-readable origin (e.g. `effects/waterripple`).
    pub name: String,
    pub vertex_hlsl: String,
    pub fragment_hlsl: String,
    pub vertex_entry: String,
    pub fragment_entry: String,
    /// Vertex inputs in location order.
    pub attributes: Vec<Attribute>,
    /// Texture slot → binding index `i` (texture at `2i`, sampler at `2i + 1`, group 1).
    pub texture_bindings: BTreeMap<u32, u32>,
}

/// `[m0..m5]` for translate · rotate(z) · scale, see [`ImageLayer::transform`].
pub fn affine(origin: [f32; 2], angle: f32, scale: [f32; 2]) -> [f32; 6] {
    let (sin, cos) = angle.sin_cos();
    [
        cos * scale[0],
        sin * scale[0],
        -sin * scale[1],
        cos * scale[1],
        origin[0],
        origin[1],
    ]
}

/// `parent ∘ child`.
pub fn compose(parent: [f32; 6], child: [f32; 6]) -> [f32; 6] {
    let [a, b, c, d, e, f] = parent;
    let [p, q, r, s, t, u] = child;
    [
        a * p + c * q,
        b * p + d * q,
        a * r + c * s,
        b * r + d * s,
        a * t + c * u + e,
        b * t + d * u + f,
    ]
}

pub const IDENTITY_AFFINE: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub name: String,
    pub location: u32,
    /// Number of float components (2, 3 or 4).
    pub components: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_offsets_follow_parent_rotation() {
        let parent = affine([100.0, 50.0], std::f32::consts::FRAC_PI_2, [2.0, 2.0]);
        let child = affine([10.0, 0.0], 0.0, [1.0, 1.0]);
        let world = compose(parent, child);
        // (10, 0) rotated 90° and doubled → (0, 20), plus parent origin.
        assert!((world[4] - 100.0).abs() < 1e-4 && (world[5] - 70.0).abs() < 1e-4);
        assert_eq!(compose(IDENTITY_AFFINE, child), child);
    }

    #[test]
    fn texture_frames_loop_by_duration() {
        let f = |d| TextureFrame {
            origin: [d, 0.0],
            x_axis: [0.5, 0.0],
            y_axis: [0.0, 1.0],
            duration: d,
        };
        let frames = [f(0.1), f(0.3)];
        assert_eq!(
            TextureFrame::at(&frames, 0.05).map(|f| f.duration),
            Some(0.1)
        );
        assert_eq!(
            TextureFrame::at(&frames, 0.2).map(|f| f.duration),
            Some(0.3)
        );
        assert_eq!(
            TextureFrame::at(&frames, 0.45).map(|f| f.duration),
            Some(0.1)
        );
        assert!(TextureFrame::at(&[], 1.0).is_none());
    }
}
