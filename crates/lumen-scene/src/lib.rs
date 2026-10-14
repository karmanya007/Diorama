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
    pub clear_color: [f32; 4],
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
    /// Back to front.
    pub layers: Vec<ImageLayer>,
    pub textures: Vec<Texture>,
    pub programs: Vec<ShaderProgram>,
    /// Things the importer could not represent yet (shown in logs/reports).
    pub skipped: Vec<String>,
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
    /// Tightly packed RGBA8, gamma-encoded.
    pub rgba: Vec<u8>,
    pub clamp: bool,
    pub nearest: bool,
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
    /// Layer-local pixels (centred, y up) → scene pixels (origin bottom-left,
    /// y up), parents already applied: `x' = m0·x + m2·y + m4`,
    /// `y' = m1·x + m3·y + m5`.
    pub transform: [f32; 6],
    pub visible: bool,
    pub kind: LayerKind,
    /// `parallaxDepth`: how strongly the layer follows the camera parallax.
    pub parallax_depth: [f32; 2],
    /// Object colour and alpha (`g_Color4`; `g_Color`/`g_Alpha` for solid layers).
    pub color: [f32; 4],
    /// How the finished layer is composited onto the scene.
    pub blend: Blend,
    /// Draws the layer's own image into its first target.
    pub albedo: Pass,
    /// Custom composite for layers with effects (e.g. a colour blend mode):
    /// placed like a direct layer, sampling the finished layer image as
    /// `Previous`. `None` = plain blended copy.
    pub composite: Option<Pass>,
    pub effects: Vec<Effect>,
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
}
