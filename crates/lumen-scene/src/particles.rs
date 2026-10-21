//! Particle systems: a declarative description the runtime simulates
//! (`lumen-particles`) and the renderer draws as camera-facing quads.
//!
//! The model follows Wallpaper Engine's particle editor (emitters spawn,
//! initializers set a new particle's attributes, operators change live
//! particles every frame, one renderer draws them); see
//! `docs/re-notes/particles.md` for the measured semantics. Units are the
//! system's local pixels, seconds and radians; colours are 0..1.

/// A random value between `min` and `max`, skewed by `exponent`
/// (`min + (max − min) · uⁿ`, `u` uniform in 0..1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range<T> {
    pub min: T,
    pub max: T,
    pub exponent: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Emitter {
    /// Random point at a distance between `distance[0]` and `distance[1]`
    /// from `origin`, direction scaled per axis by `directions`.
    Sphere {
        origin: [f32; 3],
        directions: [f32; 3],
        distance: [f32; 2],
        /// Initial speed away from the origin.
        speed: [f32; 2],
        rate: f32,
        /// Particles emitted at once when the emitter starts.
        instantaneous: u32,
        /// Seconds the emitter runs (0 = forever).
        duration: f32,
        /// Emission follows the audio level (WE `audioprocessingmode`).
        audio: bool,
    },
    /// Random point in a box: each axis between `min` and `max` from
    /// `origin`, either side.
    Box {
        origin: [f32; 3],
        min: [f32; 3],
        max: [f32; 3],
        directions: [f32; 3],
        speed: [f32; 2],
        rate: f32,
        instantaneous: u32,
        duration: f32,
        audio: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Initializer {
    Lifetime(Range<f32>),
    Size(Range<f32>),
    /// RGB 0..1, one random mix between the two colours.
    Color(Range<[f32; 3]>),
    Alpha(Range<f32>),
    /// Per-axis random velocity.
    Velocity(Range<[f32; 3]>),
    /// Per-axis random rotation (radians).
    Rotation(Range<[f32; 3]>),
    AngularVelocity(Range<[f32; 3]>),
    /// Velocity along a random direction, `speed` pixels per second.
    TurbulentVelocity {
        speed: [f32; 2],
        scale: f32,
        offset: f32,
        timescale: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operator {
    /// Gravity (pixels/s²) and drag (fraction of velocity lost per second).
    Movement {
        gravity: [f32; 3],
        drag: f32,
    },
    AngularMovement {
        force: [f32; 3],
        drag: f32,
    },
    /// Fade in over the first `fade_in` and out after `fade_out` of the
    /// lifetime (fractions).
    AlphaFade {
        fade_in: f32,
        fade_out: f32,
    },
    /// Multiplier going from `start_value` to `end_value` between the
    /// lifetime fractions `start_time` and `end_time`.
    SizeChange {
        start_time: f32,
        end_time: f32,
        start_value: f32,
        end_value: f32,
    },
    AlphaChange {
        start_time: f32,
        end_time: f32,
        start_value: f32,
        end_value: f32,
    },
    ColorChange {
        start_time: f32,
        end_time: f32,
        start_value: [f32; 3],
        end_value: [f32; 3],
    },
    /// Multiplies alpha by a value oscillating between `scale[0]` and
    /// `scale[1]` at a per-particle frequency (Hz).
    OscillateAlpha {
        frequency: [f32; 2],
        scale: [f32; 2],
        phase: [f32; 2],
    },
    OscillateSize {
        frequency: [f32; 2],
        scale: [f32; 2],
        phase: [f32; 2],
    },
    /// Offsets the position by `mask · amplitude · sin(…)`.
    OscillatePosition {
        frequency: [f32; 2],
        scale: [f32; 2],
        phase: [f32; 2],
        mask: [f32; 3],
    },
    /// Pushes particles along a smooth noise field.
    Turbulence {
        scale: f32,
        timescale: f32,
        speed: [f32; 2],
        mask: [f32; 3],
        phase: f32,
    },
    /// Accelerates particles towards (positive `scale`) or away from a
    /// control point while closer than `threshold`.
    ControlPointAttract {
        control_point: usize,
        origin: [f32; 3],
        scale: f32,
        threshold: f32,
    },
    /// Swirls particles around `center` (control point offset) about `axis`.
    Vortex {
        control_point: usize,
        axis: [f32; 3],
        distance: [f32; 2],
        speed: [f32; 2],
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParticleRenderer {
    Sprite,
    /// Quads stretched along the velocity: length = speed · `length`,
    /// capped at `max_length`.
    SpriteTrail {
        length: f32,
        max_length: f32,
    },
    /// A ribbon through the particle's recent positions: `segments` points
    /// sampled every `length / (segments − 1)` seconds (WE `ropetrail`).
    RopeTrail {
        length: f32,
        segments: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlPoint {
    /// Offset in system-local pixels.
    pub offset: [f32; 3],
    /// Follows the cursor.
    pub follows_pointer: bool,
}

/// How a child system relates to its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildKind {
    /// An independent system placed by the child's origin.
    Static,
    /// Emits at every live parent particle's position.
    Follow,
    /// Emits a burst where a parent particle is born.
    Spawn,
    /// Emits a burst where a parent particle dies.
    Death,
}

#[derive(Debug, Clone)]
pub struct ParticleChild {
    pub kind: ChildKind,
    pub origin: [f32; 3],
    pub scale: [f32; 3],
    pub probability: f32,
    pub system: ParticleSystem,
}

/// Sprite sheet frames for animated particle textures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteSheet {
    pub frames: u32,
    /// Frame size in texture UV.
    pub frame_size: [f32; 2],
    /// Frame height / width in pixels.
    pub aspect: f32,
    /// Random start frame instead of playing through the lifetime.
    pub random_frame: bool,
    /// Plays the sequence this many times per lifetime.
    pub repeat: f32,
}

/// Per-instance multipliers set on the scene object (WE `instanceoverride`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstanceOverride {
    pub rate: f32,
    /// Scales the continuous emission rate (not the maximum count).
    pub count: f32,
    pub size: f32,
    pub lifetime: f32,
    pub speed: f32,
    pub alpha: f32,
    pub color: [f32; 3],
    /// Control point positions overriding the system's (scene-local pixels).
    pub control_points: [Option<[f32; 3]>; 8],
}

impl Default for InstanceOverride {
    fn default() -> Self {
        Self {
            rate: 1.0,
            count: 1.0,
            size: 1.0,
            lifetime: 1.0,
            speed: 1.0,
            alpha: 1.0,
            color: [1.0; 3],
            control_points: [None; 8],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParticleSystem {
    /// Draws this system's particles (material shader, texture, blending).
    pub draw: crate::Pass,
    pub max_count: u32,
    /// Seconds simulated before the first frame (pre-warm).
    pub start_time: f32,
    pub emitters: Vec<Emitter>,
    pub initializers: Vec<Initializer>,
    pub operators: Vec<Operator>,
    pub renderer: ParticleRenderer,
    pub control_points: Vec<ControlPoint>,
    pub children: Vec<ParticleChild>,
    pub sprite_sheet: Option<SpriteSheet>,
    pub instance: InstanceOverride,
    /// Particles have depth: drawn through a perspective view of the scene
    /// (WE `flags & 4`), so nearer particles are larger and further out.
    pub perspective: bool,
    /// Particles live in world space (WE `flags & 1`): as a follow child they
    /// stay where they were emitted. Otherwise they are emitter-local and a
    /// follow child's particles ride along with their parent particle
    /// (captures 289 and 914).
    pub world_space: bool,
}
