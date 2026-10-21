//! CPU particle simulation for [`lumen_scene::ParticleSystem`] trees.
//!
//! Deterministic: a seed and the sequence of time steps fully decide the
//! result, so headless frames are reproducible. Large time jumps are
//! simulated in fixed steps (capped) so a frame at scene time `t` looks like
//! the system has been running.

use lumen_scene::particles::{
    ChildKind, Emitter, Initializer, Operator, ParticleRenderer, ParticleSystem, Range,
};

/// Longest single simulation step (seconds).
const MAX_STEP: f32 = 1.0 / 30.0;
/// At most this much time is simulated to catch up (pre-warm or a jump).
const MAX_CATCH_UP: f32 = 30.0;

/// One live particle (system-local pixels, seconds, radians, colours 0..1).
#[derive(Debug, Clone, PartialEq)]
pub struct Particle {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub rotation: [f32; 3],
    pub angular_velocity: [f32; 3],
    /// Current (operator-adjusted) size, colour and alpha.
    pub size: f32,
    pub color: [f32; 3],
    pub alpha: f32,
    /// Offset from oscillating operators (not integrated).
    pub offset: [f32; 3],
    /// Unique within the simulation (follow children find their parent by it).
    pub id: u64,
    /// Parent particle of an emitter-local follow child: `position` is then
    /// relative to `anchor`, the parent's current position.
    parent: Option<u64>,
    anchor: [f32; 3],
    /// Rope trails: earlier draw positions, newest first.
    pub trail: Vec<[f32; 3]>,
    pub age: f32,
    pub lifetime: f32,
    base_size: f32,
    base_color: [f32; 3],
    base_alpha: f32,
    /// Per-particle random numbers for operators.
    seed: u32,
}

impl Particle {
    /// Fraction of the lifetime elapsed, 0..1.
    pub fn life(&self) -> f32 {
        (self.age / self.lifetime.max(1e-4)).clamp(0.0, 1.0)
    }

    /// Where the particle is drawn.
    pub fn draw_position(&self) -> [f32; 3] {
        add(add(self.anchor, self.position), self.offset)
    }
}

/// One system of the tree (root first, then children depth-first).
#[derive(Debug, Clone)]
struct Node {
    system: ParticleSystem,
    parent: Option<(usize, ChildKind)>,
    /// Child placement relative to the parent (static children).
    origin: [f32; 3],
    particles: Vec<Particle>,
    /// Fractional particles owed per emitter.
    owed: Vec<f32>,
    emitted_instant: bool,
    /// Parent particles born / died this step (for spawn/death children).
    born: Vec<[f32; 3]>,
    died: Vec<[f32; 3]>,
    /// Ids of the particles born this step (follow children's bursts).
    born_ids: Vec<u64>,
    /// Rope trails: time since the last history sample.
    trail_timer: f32,
}

#[derive(Debug, Clone)]
pub struct Simulation {
    nodes: Vec<Node>,
    time: f32,
    rng: u64,
    /// Where pointer-following control points are (system-local pixels).
    pointer: Option<[f32; 3]>,
    /// Audio level 0..1 for audio-driven emitters (0 until audio capture).
    audio_level: f32,
    next_id: u64,
}

fn flatten(
    system: &ParticleSystem,
    parent: Option<(usize, ChildKind)>,
    origin: [f32; 3],
    out: &mut Vec<Node>,
) {
    let index = out.len();
    out.push(Node {
        system: system.clone(),
        parent,
        origin,
        particles: Vec::new(),
        owed: vec![0.0; system.emitters.len()],
        emitted_instant: false,
        born: Vec::new(),
        died: Vec::new(),
        born_ids: Vec::new(),
        trail_timer: 0.0,
    });
    for child in &system.children {
        flatten(&child.system, Some((index, child.kind)), child.origin, out);
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn length(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Hash → 0..1 (per-particle stable randoms).
fn hash01(seed: u32, salt: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// Smooth pseudo-random field in −1..1 per axis (sum of sines).
fn noise3(p: [f32; 3], t: f32) -> [f32; 3] {
    let n = |a: f32, b: f32, c: f32, k: f32| {
        ((a * 1.7 + t * 0.9 + k).sin()
            + (b * 2.3 - t * 1.1 + k * 1.3).sin() * 0.7
            + (c * 1.3 + a * 0.6 + t * 0.7 + k * 2.1).sin() * 0.5)
            / 2.2
    };
    [
        n(p[1], p[2], p[0], 0.3),
        n(p[2], p[0], p[1], 4.1),
        n(p[0], p[1], p[2], 8.7),
    ]
}

impl Simulation {
    pub fn new(system: &ParticleSystem, seed: u64) -> Self {
        let mut nodes = Vec::new();
        flatten(system, None, [0.0; 3], &mut nodes);
        Self {
            nodes,
            time: 0.0,
            rng: seed ^ 0x9E37_79B9_7F4A_7C15,
            pointer: None,
            audio_level: 0.0,
            next_id: 0,
        }
    }

    /// Rope trails: how far the next history sample is, 0..1 of a sample
    /// interval (WE's `g_RenderVar0.z`, shared by all particles).
    pub fn trail_phase(&self, index: usize) -> f32 {
        let Some(node) = self.nodes.get(index) else {
            return 0.0;
        };
        match trail_interval(&node.system) {
            Some(dt) => (node.trail_timer / dt).clamp(0.0, 1.0),
            None => 0.0,
        }
    }

    /// Number of systems in the tree (root first, children depth-first: the
    /// order the importer created their draw passes in).
    pub fn system_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn system(&self, index: usize) -> Option<&ParticleSystem> {
        self.nodes.get(index).map(|n| &n.system)
    }

    pub fn particles(&self, index: usize) -> &[Particle] {
        self.nodes.get(index).map_or(&[], |n| &n.particles)
    }

    /// The cursor in system-local pixels, for pointer-following control points.
    pub fn set_pointer(&mut self, pointer: Option<[f32; 3]>) {
        self.pointer = pointer;
    }

    /// Audio level 0..1 driving audio-processed emitters.
    pub fn set_audio_level(&mut self, level: f32) {
        self.audio_level = level.clamp(0.0, 1.0);
    }

    /// Simulates up to scene time `time` (seconds since the wallpaper
    /// started), including the root's pre-warm on the first call.
    pub fn advance_to(&mut self, time: f32) {
        let target = time + self.nodes[0].system.start_time;
        // Skip what is too old to matter (pre-warm or a long pause).
        self.time = self.time.max(target - MAX_CATCH_UP);
        while self.time < target {
            self.step((target - self.time).min(MAX_STEP));
        }
    }

    fn random(&mut self) -> f32 {
        // SplitMix64.
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }

    fn pick(&mut self, r: Range<f32>) -> f32 {
        let u = self.random().powf(r.exponent.max(0.0));
        lerp(r.min, r.max, u)
    }

    fn pick3(&mut self, r: Range<[f32; 3]>) -> [f32; 3] {
        let mut out = [0.0; 3];
        for (i, o) in out.iter_mut().enumerate() {
            let u = self.random().powf(r.exponent.max(0.0));
            *o = lerp(r.min[i], r.max[i], u);
        }
        out
    }

    /// Advances every system by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        for i in 0..self.nodes.len() {
            self.nodes[i].born.clear();
            self.nodes[i].died.clear();
            self.nodes[i].born_ids.clear();
            self.emit(i, dt);
            self.attach(i);
            self.update(i, dt);
            self.record_trails(i, dt);
        }
        self.time += dt;
    }

    /// Most particles system `i` can hold. A child that follows its parent's
    /// particles runs one instance per parent particle (capture 289: one
    /// trail draw per firefly), so it holds that many times its own maximum.
    pub fn capacity(&self, i: usize) -> usize {
        let s = &self.nodes[i].system;
        let own = s.max_count as usize;
        match self.nodes[i].parent {
            Some((p, ChildKind::Follow)) => own.saturating_mul(self.capacity(p).max(1)),
            _ => own,
        }
    }

    /// Emitter-local follow children: refresh the parent positions they ride
    /// on; children of parents that died go with them.
    fn attach(&mut self, i: usize) {
        let Some((p, ChildKind::Follow)) = self.nodes[i].parent else {
            return;
        };
        if self.nodes[i].system.world_space {
            return;
        }
        let parents: std::collections::HashMap<u64, [f32; 3]> = self.nodes[p]
            .particles
            .iter()
            .map(|q| (q.id, q.draw_position()))
            .collect();
        self.nodes[i]
            .particles
            .retain_mut(|c| match c.parent.and_then(|id| parents.get(&id)) {
                Some(at) => {
                    c.anchor = *at;
                    true
                }
                None => c.parent.is_none(),
            });
    }

    /// Rope trails: every sample interval, each particle remembers where it is.
    fn record_trails(&mut self, i: usize, dt: f32) {
        let node = &mut self.nodes[i];
        let (Some(interval), ParticleRenderer::RopeTrail { segments, .. }) =
            (trail_interval(&node.system), node.system.renderer)
        else {
            return;
        };
        node.trail_timer += dt;
        while node.trail_timer >= interval {
            node.trail_timer -= interval;
            for p in &mut node.particles {
                let at = p.draw_position();
                p.trail.insert(0, at);
                p.trail.truncate(segments as usize);
            }
        }
    }

    fn emit(&mut self, i: usize, dt: f32) {
        let parent = self.nodes[i].parent;
        let local = !self.nodes[i].system.world_space;
        // Where emission happens: the system origin, or parent particles
        // (with the parent's id for follow children).
        let anchors: Vec<([f32; 3], Option<u64>)> = match parent {
            None => vec![([0.0; 3], None)],
            Some((_, ChildKind::Static)) => vec![(self.nodes[i].origin, None)],
            Some((p, ChildKind::Follow)) => self.nodes[p]
                .particles
                .iter()
                .map(|q| (q.draw_position(), Some(q.id)))
                .collect(),
            Some((p, ChildKind::Spawn)) => self.nodes[p].born.iter().map(|b| (*b, None)).collect(),
            Some((p, ChildKind::Death)) => self.nodes[p].died.iter().map(|d| (*d, None)).collect(),
        };
        if anchors.is_empty() {
            return;
        }
        let follow = matches!(parent, Some((_, ChildKind::Follow)));
        // A follow child runs one instance per parent particle: its
        // instantaneous burst fires when that parent is born (capture 914:
        // one glow per shooting star).
        let fresh: Vec<([f32; 3], Option<u64>)> = match parent {
            Some((p, ChildKind::Follow)) => {
                let born = &self.nodes[p].born_ids;
                anchors
                    .iter()
                    .filter(|(_, id)| id.is_some_and(|id| born.contains(&id)))
                    .copied()
                    .collect()
            }
            _ => Vec::new(),
        };
        let burst = matches!(parent, Some((_, ChildKind::Spawn | ChildKind::Death)));
        let emitters = self.nodes[i].system.emitters.clone();
        // The `count` override scales emission, not the maximum (capture 2478
        // of 2208593521: rain at 400/s × 0.49 s holds 409 with count 2 and
        // 78 with count 0.38; 1769376307's stars, count 2, stop at their
        // maxcount 1000).
        let rate_scale = self.nodes[i].system.instance.rate * self.nodes[i].system.instance.count;
        let first = !self.nodes[i].emitted_instant;
        self.nodes[i].emitted_instant = true;
        for (e, emitter) in emitters.iter().enumerate() {
            let (rate, instantaneous, duration, audio) = match emitter {
                Emitter::Sphere {
                    rate,
                    instantaneous,
                    duration,
                    audio,
                    ..
                }
                | Emitter::Box {
                    rate,
                    instantaneous,
                    duration,
                    audio,
                    ..
                } => (*rate, *instantaneous, *duration, *audio),
            };
            let rate = if audio { rate * self.audio_level } else { rate };
            let mut count = 0usize;
            let mut at: Vec<([f32; 3], Option<u64>)> = Vec::new();
            if burst {
                count = (instantaneous.max(1) as usize) * anchors.len();
            } else {
                if follow {
                    for a in &fresh {
                        at.extend(std::iter::repeat_n(*a, instantaneous as usize));
                    }
                } else if first {
                    count += instantaneous as usize;
                }
                let running = duration <= 0.0 || self.time < duration;
                if running {
                    let owed = &mut self.nodes[i].owed[e];
                    *owed += rate * rate_scale * dt * anchors.len() as f32;
                    let whole = owed.floor();
                    *owed -= whole;
                    count += whole as usize;
                }
            }
            at.extend((0..count).map(|k| anchors[k % anchors.len()]));
            let own = self.nodes[i].system.max_count as usize;
            for (anchor, parent_id) in at {
                if self.nodes[i].particles.len() >= self.capacity(i) {
                    break;
                }
                // Each parent's instance holds at most the child's own maximum.
                if let Some(id) = parent_id.filter(|_| follow && local)
                    && self.nodes[i]
                        .particles
                        .iter()
                        .filter(|c| c.parent == Some(id))
                        .count()
                        >= own
                {
                    continue;
                }
                let mut p = if follow && local {
                    let mut p = self.spawn(i, emitter, [0.0; 3]);
                    p.parent = parent_id;
                    p.anchor = anchor;
                    p
                } else {
                    self.spawn(i, emitter, anchor)
                };
                p.id = self.next_id;
                self.next_id += 1;
                self.nodes[i].born.push(p.draw_position());
                self.nodes[i].born_ids.push(p.id);
                self.nodes[i].particles.push(p);
            }
        }
    }

    fn spawn(&mut self, i: usize, emitter: &Emitter, anchor: [f32; 3]) -> Particle {
        let (position, velocity) = match *emitter {
            Emitter::Sphere {
                origin,
                directions,
                distance,
                speed,
                ..
            } => {
                // Random direction (uniform on the sphere), flattened by `directions`.
                let z = self.random() * 2.0 - 1.0;
                let a = self.random() * std::f32::consts::TAU;
                let r = (1.0 - z * z).max(0.0).sqrt();
                let mut dir = [
                    r * a.cos() * directions[0],
                    r * a.sin() * directions[1],
                    z * directions[2],
                ];
                let len = length(dir);
                if len > 1e-6 {
                    dir = scale(dir, 1.0 / len);
                }
                let d = lerp(distance[0], distance[1], self.random());
                let s = lerp(speed[0], speed[1], self.random());
                (add(add(anchor, origin), scale(dir, d)), scale(dir, s))
            }
            Emitter::Box {
                origin,
                min,
                max,
                directions,
                speed,
                ..
            } => {
                let mut p = [0.0; 3];
                for (axis, v) in p.iter_mut().enumerate() {
                    let side = if self.random() < 0.5 { -1.0 } else { 1.0 };
                    *v = side * lerp(min[axis], max[axis], self.random());
                }
                let mut dir = [
                    (self.random() * 2.0 - 1.0) * directions[0],
                    (self.random() * 2.0 - 1.0) * directions[1],
                    (self.random() * 2.0 - 1.0) * directions[2],
                ];
                let len = length(dir);
                if len > 1e-6 {
                    dir = scale(dir, 1.0 / len);
                }
                let s = lerp(speed[0], speed[1], self.random());
                (add(add(anchor, origin), p), scale(dir, s))
            }
        };
        let instance = self.nodes[i].system.instance;
        let mut p = Particle {
            position,
            velocity,
            rotation: [0.0; 3],
            angular_velocity: [0.0; 3],
            size: 20.0,
            color: [1.0; 3],
            alpha: 1.0,
            offset: [0.0; 3],
            id: 0,
            parent: None,
            anchor: [0.0; 3],
            trail: Vec::new(),
            age: 0.0,
            lifetime: 1.0,
            base_size: 20.0,
            base_color: [1.0; 3],
            base_alpha: 1.0,
            seed: (self.random() * 16_777_216.0) as u32,
        };
        let initializers = self.nodes[i].system.initializers.clone();
        for init in &initializers {
            match *init {
                Initializer::Lifetime(r) => p.lifetime = self.pick(r),
                Initializer::Size(r) => p.base_size = self.pick(r),
                Initializer::Color(r) => {
                    let u = self.random().powf(r.exponent.max(0.0));
                    p.base_color = [
                        lerp(r.min[0], r.max[0], u),
                        lerp(r.min[1], r.max[1], u),
                        lerp(r.min[2], r.max[2], u),
                    ];
                }
                Initializer::Alpha(r) => p.base_alpha = self.pick(r),
                Initializer::Velocity(r) => p.velocity = add(p.velocity, self.pick3(r)),
                Initializer::Rotation(r) => p.rotation = self.pick3(r),
                Initializer::AngularVelocity(r) => p.angular_velocity = self.pick3(r),
                // WE samples a noise field at position × scale + time ×
                // timescale; with both 0 that is a lattice point where the
                // noise is 0, so particles get no velocity (capture 287 of
                // 1537391845: all 11 torch particles at the emitter, a round
                // glow). Otherwise a random direction stands in for the field.
                Initializer::TurbulentVelocity {
                    scale: 0.0,
                    timescale: 0.0,
                    ..
                } => {}
                Initializer::TurbulentVelocity { speed, .. } => {
                    let a = self.random() * std::f32::consts::TAU;
                    let s = lerp(speed[0], speed[1], self.random());
                    p.velocity = add(p.velocity, [a.cos() * s, a.sin() * s, 0.0]);
                }
            }
        }
        p.lifetime = (p.lifetime * instance.lifetime).max(0.01);
        p.base_size *= instance.size;
        p.base_alpha *= instance.alpha;
        p.velocity = scale(p.velocity, instance.speed);
        for c in 0..3 {
            p.base_color[c] *= instance.color[c];
        }
        p.size = p.base_size;
        p.color = p.base_color;
        p.alpha = p.base_alpha;
        p
    }

    fn control_point(&self, i: usize, index: usize) -> [f32; 3] {
        let system = &self.nodes[i].system;
        if let Some(Some(p)) = system.instance.control_points.get(index) {
            return *p;
        }
        match system.control_points.get(index) {
            Some(cp) if cp.follows_pointer => self.pointer.unwrap_or(cp.offset),
            Some(cp) => cp.offset,
            None => [0.0; 3],
        }
    }

    fn update(&mut self, i: usize, dt: f32) {
        let operators = self.nodes[i].system.operators.clone();
        let time = self.time;
        let moves = operators
            .iter()
            .any(|o| matches!(o, Operator::Movement { .. }));
        let spins = operators
            .iter()
            .any(|o| matches!(o, Operator::AngularMovement { .. }));
        let cps: Vec<[f32; 3]> = (0..8).map(|c| self.control_point(i, c)).collect();
        let node = &mut self.nodes[i];
        let mut died = Vec::new();
        node.particles.retain_mut(|p| {
            p.age += dt;
            if p.age >= p.lifetime {
                died.push(p.draw_position());
                return false;
            }
            let life = p.life();
            let mut size = p.base_size;
            let mut alpha = p.base_alpha;
            let mut color = p.base_color;
            p.offset = [0.0; 3];
            for (k, op) in operators.iter().enumerate() {
                let r = |salt: u32| hash01(p.seed, (k as u32) * 16 + salt);
                match *op {
                    Operator::Movement { gravity, drag } => {
                        p.velocity = add(p.velocity, scale(gravity, dt));
                        p.velocity = scale(p.velocity, (1.0 - drag * dt).max(0.0));
                    }
                    Operator::AngularMovement { force, drag } => {
                        p.angular_velocity = add(p.angular_velocity, scale(force, dt));
                        p.angular_velocity = scale(p.angular_velocity, (1.0 - drag * dt).max(0.0));
                    }
                    Operator::AlphaFade { fade_in, fade_out } => {
                        if fade_in > 0.0 && life < fade_in {
                            alpha *= life / fade_in;
                        }
                        if life > fade_out && fade_out < 1.0 {
                            alpha *= ((1.0 - life) / (1.0 - fade_out)).max(0.0);
                        }
                    }
                    Operator::SizeChange {
                        start_time,
                        end_time,
                        start_value,
                        end_value,
                    } => size *= ramp(life, start_time, end_time, start_value, end_value),
                    Operator::AlphaChange {
                        start_time,
                        end_time,
                        start_value,
                        end_value,
                    } => alpha *= ramp(life, start_time, end_time, start_value, end_value),
                    Operator::ColorChange {
                        start_time,
                        end_time,
                        start_value,
                        end_value,
                    } => {
                        for c in 0..3 {
                            color[c] *=
                                ramp(life, start_time, end_time, start_value[c], end_value[c]);
                        }
                    }
                    Operator::OscillateAlpha {
                        frequency,
                        scale: s,
                        phase,
                    } => alpha *= oscillate(p.age, frequency, s, phase, &r),
                    Operator::OscillateSize {
                        frequency,
                        scale: s,
                        phase,
                    } => size *= oscillate(p.age, frequency, s, phase, &r),
                    Operator::OscillatePosition {
                        frequency,
                        scale: s,
                        phase,
                        mask,
                    } => {
                        for (axis, m) in mask.iter().enumerate() {
                            let f = lerp(frequency[0], frequency[1], r(axis as u32));
                            let ph = lerp(phase[0], phase[1], r(4 + axis as u32));
                            let amp = lerp(s[0], s[1], r(8 + axis as u32));
                            let w = std::f32::consts::TAU * (f * p.age + ph);
                            p.offset[axis] += m * amp * w.sin();
                        }
                    }
                    Operator::Turbulence {
                        scale: s,
                        timescale,
                        speed,
                        mask,
                        phase,
                    } => {
                        let n =
                            noise3(scale(p.position, s), time * timescale + phase + r(0) * 10.0);
                        let force = lerp(speed[0], speed[1], r(1));
                        for axis in 0..3 {
                            p.velocity[axis] += n[axis] * mask[axis] * force * dt;
                        }
                    }
                    Operator::ControlPointAttract {
                        control_point,
                        origin,
                        scale: s,
                        threshold,
                    } => {
                        let cp = add(cps.get(control_point).copied().unwrap_or([0.0; 3]), origin);
                        let d = [
                            cp[0] - p.position[0],
                            cp[1] - p.position[1],
                            cp[2] - p.position[2],
                        ];
                        let dist = length(d);
                        if dist > 1e-3 && dist < threshold {
                            p.velocity = add(p.velocity, scale(d, s * dt / dist));
                        }
                    }
                    Operator::Vortex {
                        control_point,
                        axis,
                        distance,
                        speed,
                    } => {
                        let c = cps.get(control_point).copied().unwrap_or([0.0; 3]);
                        let d = [p.position[0] - c[0], p.position[1] - c[1]];
                        let dist = (d[0] * d[0] + d[1] * d[1]).sqrt();
                        if dist > 1e-3 {
                            let t = ((dist - distance[0]) / (distance[1] - distance[0]).max(1e-3))
                                .clamp(0.0, 1.0);
                            let v = lerp(speed[0], speed[1], t);
                            let sign = if axis[2] < 0.0 { -1.0 } else { 1.0 };
                            p.velocity[0] += -d[1] / dist * v * sign * dt;
                            p.velocity[1] += d[0] / dist * v * sign * dt;
                        }
                    }
                }
            }
            if moves {
                p.position = add(p.position, scale(p.velocity, dt));
            }
            if spins {
                p.rotation = add(p.rotation, scale(p.angular_velocity, dt));
            }
            p.size = size;
            p.alpha = alpha.clamp(0.0, 1.0);
            p.color = color;
            true
        });
        node.died = died;
    }
}

/// `start_value` before `start_time`, `end_value` after `end_time`, linear between.
fn ramp(life: f32, start_time: f32, end_time: f32, start_value: f32, end_value: f32) -> f32 {
    if end_time <= start_time {
        return if life < start_time {
            start_value
        } else {
            end_value
        };
    }
    let t = ((life - start_time) / (end_time - start_time)).clamp(0.0, 1.0);
    lerp(start_value, end_value, t)
}

fn oscillate(
    age: f32,
    frequency: [f32; 2],
    scale: [f32; 2],
    phase: [f32; 2],
    r: &impl Fn(u32) -> f32,
) -> f32 {
    let f = lerp(frequency[0], frequency[1], r(12));
    let ph = lerp(phase[0], phase[1], r(13));
    let w = (std::f32::consts::TAU * (f * age + ph)).sin() * 0.5 + 0.5;
    lerp(scale[0], scale[1], w)
}

/// Seconds between rope-trail history samples: the trail spans `length`
/// seconds over `segments` points (capture 914: 0.4 s, 4 points, 0.133 s
/// apart at the stars' terminal speed).
fn trail_interval(system: &ParticleSystem) -> Option<f32> {
    match system.renderer {
        ParticleRenderer::RopeTrail { length, segments } if segments > 1 && length > 0.0 => {
            Some((length / (segments - 1) as f32).max(1e-3))
        }
        _ => None,
    }
}

/// Renderer settings the vertex builder needs for one system.
pub fn renderer(system: &ParticleSystem) -> ParticleRenderer {
    system.renderer
}

#[cfg(test)]
mod tests;
