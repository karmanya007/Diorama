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
        // Rope trails: segment count (unscaled), -, history phase, segment
        // count; the phase changes every frame (see `rope_vars`).
        ParticleRenderer::RopeTrail { segments, .. } => {
            [segments as f32, 0.0, 0.0, segments as f32]
        }
        ParticleRenderer::Sprite | ParticleRenderer::Rope { .. } => [0.0; 4],
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

/// Vertices a system's buffer needs per particle.
pub(crate) fn vertices_per_particle(system: &ParticleSystem) -> u32 {
    match system.renderer {
        ParticleRenderer::RopeTrail { segments, .. } => 6 * segments.max(1),
        ParticleRenderer::Rope { subdivision } => 6 * (subdivision + 1),
        _ => CORNERS.len() as u32,
    }
}

/// `g_RenderVar0` of a rope trail with history `phase` (0..1 of a sample
/// interval since the last sample).
pub(crate) fn rope_vars(system: &ParticleSystem, phase: f32) -> Option<[f32; 4]> {
    match system.renderer {
        ParticleRenderer::RopeTrail { segments, .. } => {
            let n = segments as f32;
            Some([n, 0.0, phase, n])
        }
        _ => None,
    }
}

/// Rope-trail vertices for WE's `genericropeparticle` without a geometry
/// shader: one quad per segment between consecutive trail points (the head
/// first, then the history, newest first), six vertices each. Segments are
/// straight (control points on the ends). Returns the vertex count.
pub(crate) fn fill_rope(
    attributes: &[Attribute],
    particles: &[Particle],
    max_vertices: u32,
    out: &mut Vec<f32>,
) -> u32 {
    out.clear();
    let mut written = 0u32;
    for p in particles {
        let head = p.draw_position();
        let points: Vec<[f32; 3]> = std::iter::once(head)
            .chain(p.trail.iter().copied())
            .collect();
        let history = p.trail.len() as f32;
        for (k, pair) in points.windows(2).enumerate() {
            if written + CORNERS.len() as u32 > max_vertices {
                return written;
            }
            let (start, end) = (pair[0], pair[1]);
            for (u, v) in CORNERS {
                for a in attributes {
                    let n = a.components.clamp(1, 4) as usize;
                    let values = match a.name.as_str() {
                        "a_PositionVec4" => [start[0], start[1], start[2], p.size],
                        "a_TexCoordVec4" => [end[0], end[1], end[2], history],
                        "a_TexCoordVec4C1" => [start[0], start[1], start[2], k as f32],
                        "a_TexCoordVec3C2" => [end[0], end[1], end[2], 0.0],
                        "a_TexCoordC3" => [u, v, 0.0, 0.0],
                        "a_Color" => [p.color[0], p.color[1], p.color[2], p.alpha],
                        _ => [0.0; 4],
                    };
                    out.extend_from_slice(&values[..n]);
                }
                written += 1;
            }
        }
    }
    written
}

/// Vertices for WE's `genericropeparticle` (thick format) without a
/// geometry shader, `rope` renderer: a ribbon through the particles, oldest
/// first (WE keeps new particles at the end of the array). Each span is cut
/// into `pieces` along a Catmull-Rom spline, one quad (six vertices) each;
/// the control points carry the spline's tangents so the ribbon's sides
/// stay continuous.
///
/// WE's geometry shader spreads a rope ± its size around the line, at the
/// editor's size (capture 287 of 2553418404: 57–75 for `sizerandom` 90–120
/// × 2.5 × 0.2), twice the sprite size the simulation keeps. The shader
/// path without one spreads to one side only (`right · u · 2 − 1`), so the
/// ends move back by that `right` (2D: forward is +z, `right` =
/// normalize(−ty, tx) · size) to centre it. Returns the vertex count.
pub(crate) fn fill_rope_chain(
    attributes: &[Attribute],
    particles: &[Particle],
    pieces: u32,
    max_vertices: u32,
    out: &mut Vec<f32>,
) -> u32 {
    out.clear();
    let mut order: Vec<&Particle> = particles.iter().collect();
    order.sort_by(|a, b| b.age.total_cmp(&a.age));
    let points: Vec<[f32; 3]> = order.iter().map(|p| p.draw_position()).collect();
    let n = points.len();
    if n < 2 {
        return 0;
    }
    let pieces = pieces.max(1);
    // The shader spreads the texture over `length - 1` steps of one quad.
    let length = ((n - 1) as u32 * pieces + 1) as f32;
    let at = |i: isize| points[i.clamp(0, n as isize - 1) as usize];
    // Position and tangent (per unit of t) on span k at t ∈ [0, 1].
    let spline = |k: usize, t: f32| -> ([f32; 3], [f32; 3]) {
        let k = k as isize;
        let (p0, p1, p2, p3) = (at(k - 1), at(k), at(k + 1), at(k + 2));
        let (t2, t3) = (t * t, t * t * t);
        let mut pos = [0.0; 3];
        let mut tan = [0.0; 3];
        for c in 0..3 {
            pos[c] = 0.5
                * (2.0 * p1[c]
                    + (p2[c] - p0[c]) * t
                    + (2.0 * p0[c] - 5.0 * p1[c] + 4.0 * p2[c] - p3[c]) * t2
                    + (-p0[c] + 3.0 * p1[c] - 3.0 * p2[c] + p3[c]) * t3);
            tan[c] = 0.5
                * ((p2[c] - p0[c])
                    + 2.0 * (2.0 * p0[c] - 5.0 * p1[c] + 4.0 * p2[c] - p3[c]) * t
                    + 3.0 * (-p0[c] + 3.0 * p1[c] - 3.0 * p2[c] + p3[c]) * t2);
        }
        (pos, tan)
    };
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let mut written = 0u32;
    for k in 0..n - 1 {
        let (a, b) = (order[k], order[k + 1]);
        for j in 0..pieces {
            if written + CORNERS.len() as u32 > max_vertices {
                return written;
            }
            let (t0, t1) = (j as f32 / pieces as f32, (j + 1) as f32 / pieces as f32);
            let ((start, tan0), (end, tan1)) = (spline(k, t0), spline(k, t1));
            let step = 1.0 / pieces as f32;
            let size = |t: f32| 2.0 * mix(a.size, b.size, t);
            let back = |p: [f32; 3], tan: [f32; 3], s: f32| {
                let len = (tan[0] * tan[0] + tan[1] * tan[1]).sqrt().max(1e-6);
                [p[0] + tan[1] / len * s, p[1] - tan[0] / len * s, p[2]]
            };
            let (start, end) = (back(start, tan0, size(t0)), back(end, tan1, size(t1)));
            let delta = [0, 1, 2].map(|c| end[c] - start[c]);
            // trailRightStart = cross(eye, delta + start - cp0) along the
            // tangent; trailRightEnd = cross(eye, delta - (end - cp1)).
            let cp0 = [0, 1, 2].map(|c| start[c] - (tan0[c] * step - delta[c]));
            let cp1 = [0, 1, 2].map(|c| end[c] - (delta[c] - tan1[c] * step));
            let colour = |t: f32| {
                [
                    mix(a.color[0], b.color[0], t),
                    mix(a.color[1], b.color[1], t),
                    mix(a.color[2], b.color[2], t),
                    mix(a.alpha, b.alpha, t),
                ]
            };
            let position = (k as u32 * pieces + j) as f32;
            for (u, v) in CORNERS {
                for attr in attributes {
                    let m = attr.components.clamp(1, 4) as usize;
                    let values = match attr.name.as_str() {
                        "a_PositionVec4" => [start[0], start[1], start[2], size(t0)],
                        "a_TexCoordVec4" => [end[0], end[1], end[2], length],
                        "a_TexCoordVec4C1" => [cp0[0], cp0[1], cp0[2], position],
                        "a_TexCoordVec4C2" => [cp1[0], cp1[1], cp1[2], size(t1)],
                        "a_TexCoordVec4C3" => colour(t1),
                        "a_TexCoordC4" => [u, v, 0.0, 0.0],
                        "a_Color" => colour(t0),
                        _ => [0.0; 4],
                    };
                    out.extend_from_slice(&values[..m]);
                }
                written += 1;
            }
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
