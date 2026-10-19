//! Particle layers: a simulation plus one dynamic-vertex draw per system of
//! the tree. Vertices follow WE's `genericparticle` layout without geometry
//! shaders: six per particle (two triangles), each carrying the particle's
//! position and the quad corner the shader expands it to.

use lumen_particles::{Particle, Simulation};
use lumen_scene::particles::{ParticleRenderer, ParticleSystem};
use lumen_scene::{self as ir, Attribute};

/// Quad corners (u, v), v down, as two triangles.
const CORNERS: [(f32, f32); 6] = [
    (0.0, 0.0),
    (1.0, 0.0),
    (0.0, 1.0),
    (0.0, 1.0),
    (1.0, 0.0),
    (1.0, 1.0),
];

pub(crate) struct SystemDraw {
    /// Index of the system in the simulation.
    pub system: usize,
    /// Index of the draw in the layer's passes.
    pub pass: usize,
    /// Vertex capacity of its buffer.
    pub capacity: u32,
}

pub(crate) struct ParticleGpu {
    pub sim: Simulation,
    pub draws: Vec<SystemDraw>,
    /// Reused vertex scratch.
    pub scratch: Vec<f32>,
}

/// Systems of the tree in simulation order: root, then children depth-first.
pub(crate) fn systems(root: &ParticleSystem) -> Vec<&ParticleSystem> {
    fn walk<'a>(s: &'a ParticleSystem, out: &mut Vec<&'a ParticleSystem>) {
        out.push(s);
        for c in &s.children {
            walk(&c.system, out);
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

/// `g_RenderVar0` (trail length scale, max length, min length) and
/// `g_RenderVar1` (sprite frame width/height in UV, frame count, frame aspect).
pub(crate) fn render_vars(system: &ParticleSystem) -> ([f32; 4], [f32; 4]) {
    let var0 = match system.renderer {
        ParticleRenderer::SpriteTrail { length, max_length } => [length, max_length, 0.0, 0.0],
        ParticleRenderer::RopeTrail { length, .. } => [length, 1000.0, 0.0, 0.0],
        ParticleRenderer::Sprite => [0.0; 4],
    };
    let var1 = system.sprite_sheet.map_or([1.0, 1.0, 1.0, 1.0], |s| {
        [s.frame_size[0], s.frame_size[1], s.frames as f32, s.aspect]
    });
    (var0, var1)
}

/// Sprite-sheet position 0..1 the shader turns into a frame.
fn frame_phase(system: &ParticleSystem, p: &Particle) -> f32 {
    match system.sprite_sheet {
        Some(s) if s.random_frame => {
            // Stable per particle: derived from its lifetime draw.
            (p.lifetime * 7919.0).fract()
        }
        Some(s) => (p.life() * s.repeat.max(0.01)).fract(),
        None => p.life(),
    }
}

/// Appends the vertices of `particles` for `attributes` to `out`; returns
/// how many vertices were written (at most `max_vertices`).
/// Where a perspective system's particles are seen from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct View {
    /// The layer's transform (local → scene pixels).
    pub transform: [f32; 6],
    /// Scene centre (the eye looks at it).
    pub center: [f32; 2],
    /// Eye distance from the z = 0 plane, in scene pixels.
    pub distance: f32,
}

/// Captured WE perspective particles (2583499659, 2468489223): the z = 0
/// plane matches the 2D view, seen from 0.9163 × half the scene height.
pub(crate) fn view(transform: [f32; 6], scene: [u32; 2]) -> View {
    let [w, h] = scene.map(|v| v as f32);
    View {
        transform,
        center: [w * 0.5, h * 0.5],
        distance: 0.9163 * h * 0.5,
    }
}

/// Position and size scale of a particle at local `pos` under perspective;
/// `None` behind the eye.
fn project(view: &View, pos: [f32; 3]) -> Option<([f32; 3], f32)> {
    let [a, b, c, d, e, f] = view.transform;
    let world = [a * pos[0] + c * pos[1] + e, b * pos[0] + d * pos[1] + f];
    let depth = view.distance - pos[2];
    if depth <= view.distance * 0.05 {
        return None;
    }
    let k = view.distance / depth;
    let seen = [
        view.center[0] + (world[0] - view.center[0]) * k,
        view.center[1] + (world[1] - view.center[1]) * k,
    ];
    // Back to local pixels.
    let det = a * d - b * c;
    if det.abs() < 1e-9 {
        return None;
    }
    let (x, y) = (seen[0] - e, seen[1] - f);
    Some(([(d * x - c * y) / det, (-b * x + a * y) / det, 0.0], k))
}

pub(crate) fn fill(
    attributes: &[Attribute],
    system: &ParticleSystem,
    particles: &[Particle],
    max_vertices: u32,
    view: Option<&View>,
    out: &mut Vec<f32>,
) -> u32 {
    out.clear();
    let mut written = 0u32;
    for p in particles {
        if written + CORNERS.len() as u32 > max_vertices {
            break;
        }
        let (pos, size_scale) = match view.filter(|_| system.perspective) {
            Some(v) => match project(v, p.draw_position()) {
                Some(seen) => seen,
                None => continue,
            },
            None => (p.draw_position(), 1.0),
        };
        let size = p.size * size_scale;
        let phase = frame_phase(system, p);
        for (u, v) in CORNERS {
            for a in attributes {
                let n = a.components.clamp(1, 4) as usize;
                let values = match a.name.as_str() {
                    "a_Position" => [pos[0], pos[1], pos[2], 1.0],
                    "a_TexCoordVec4" => [u, v, p.rotation[2], size],
                    "a_TexCoordC2" => [p.rotation[0], p.rotation[1], 0.0, 0.0],
                    "a_Color" => [p.color[0], p.color[1], p.color[2], p.alpha],
                    "a_TexCoordVec4C1" => [
                        p.velocity[0] * size_scale,
                        p.velocity[1] * size_scale,
                        p.velocity[2] * size_scale,
                        phase,
                    ],
                    _ => [0.0; 4],
                };
                out.extend_from_slice(&values[..n]);
            }
            written += 1;
        }
    }
    written
}

/// Simulation seed for a layer (stable across runs).
pub(crate) fn seed(index: usize, name: &str) -> u64 {
    name.bytes()
        .fold(index as u64 ^ 0xA076_1D64_78BD_642F, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x1000_0000_01B3)
        })
}

/// The IR layer's particle tree, if any.
pub(crate) fn tree(layer: &ir::ImageLayer) -> Option<&ParticleSystem> {
    layer.particles.as_deref()
}
