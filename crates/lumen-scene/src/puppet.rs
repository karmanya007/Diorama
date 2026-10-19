//! Puppet warp: an image layer drawn as a mesh deformed by a 2D skeleton
//! (linear blend skinning), posed by keyframed bone animations.

use crate::{IDENTITY_AFFINE, affine, compose};

#[derive(Debug, Clone, PartialEq)]
pub struct Puppet {
    pub vertices: Vec<PuppetVertex>,
    /// Triangle list into `vertices`.
    pub indices: Vec<u32>,
    pub bones: Vec<PuppetBone>,
    /// Animations playing on the layer, blended by weight.
    pub animations: Vec<PlayingAnimation>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PuppetVertex {
    /// Layer-local pixels, centred on the image, y up (bind pose).
    pub position: [f32; 2],
    /// 0..1 over the image (not the padded texture), v down.
    pub uv: [f32; 2],
    pub bones: [u32; 4],
    pub weights: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PuppetBone {
    pub parent: Option<usize>,
    /// Bind pose relative to the parent.
    pub bind: [f32; 6],
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayingAnimation {
    pub animation: PuppetAnimation,
    /// Speed factor.
    pub rate: f32,
    pub weight: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PuppetAnimation {
    pub name: String,
    pub fps: f32,
    /// Length in frames; each track holds `frames + 1` samples.
    pub frames: u32,
    pub mode: AnimationMode,
    /// Samples per bone (indexed like `Puppet::bones`).
    pub tracks: Vec<Vec<BonePose>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationMode {
    Loop,
    /// Forwards then backwards.
    Mirror,
    /// Once, then holds the last frame.
    Single,
}

/// A bone's transform relative to its parent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BonePose {
    pub position: [f32; 2],
    /// Radians, counter-clockwise (y up).
    pub angle: f32,
    pub scale: [f32; 2],
}

impl BonePose {
    fn lerp(self, other: Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Self {
            position: [
                mix(self.position[0], other.position[0]),
                mix(self.position[1], other.position[1]),
            ],
            angle: mix(self.angle, other.angle),
            scale: [
                mix(self.scale[0], other.scale[0]),
                mix(self.scale[1], other.scale[1]),
            ],
        }
    }

    fn affine(self) -> [f32; 6] {
        affine(self.position, self.angle, self.scale)
    }
}

impl PuppetAnimation {
    /// Fractional frame at `time` seconds (after `rate`).
    pub fn frame_at(&self, time: f32) -> f32 {
        let n = self.frames as f32;
        if n <= 0.0 {
            return 0.0;
        }
        let f = (time * self.fps).max(0.0);
        match self.mode {
            AnimationMode::Loop => f.rem_euclid(n),
            AnimationMode::Mirror => {
                let p = f.rem_euclid(2.0 * n);
                if p <= n { p } else { 2.0 * n - p }
            }
            AnimationMode::Single => f.min(n),
        }
    }

    /// Bone `bone`'s pose at fractional `frame`, if the track exists.
    fn sample(&self, bone: usize, frame: f32) -> Option<BonePose> {
        let track = self.tracks.get(bone)?;
        let last = track.len().checked_sub(1)?;
        let i = (frame.floor().max(0.0) as usize).min(last);
        let j = (i + 1).min(last);
        Some(track[i].lerp(track[j], frame - i as f32))
    }
}

/// Inverse of an invertible affine; identity otherwise.
fn invert(m: [f32; 6]) -> [f32; 6] {
    let [a, b, c, d, e, f] = m;
    let det = a * d - b * c;
    if det.abs() < 1e-12 {
        return IDENTITY_AFFINE;
    }
    let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
    [ia, ib, ic, id, -(ia * e + ic * f), -(ib * e + id * f)]
}

fn apply(m: [f32; 6], p: [f32; 2]) -> [f32; 2] {
    [
        m[0] * p[0] + m[2] * p[1] + m[4],
        m[1] * p[0] + m[3] * p[1] + m[5],
    ]
}

impl Puppet {
    /// World (puppet-space) transform of every bone from local ones.
    fn world(&self, local: &[[f32; 6]]) -> Vec<[f32; 6]> {
        let mut world: Vec<Option<[f32; 6]>> = vec![None; self.bones.len()];
        // Parents may come after children; resolve iteratively, cycles → root.
        for _ in 0..self.bones.len() {
            for (i, bone) in self.bones.iter().enumerate() {
                if world[i].is_some() {
                    continue;
                }
                world[i] = match bone.parent.filter(|&p| p < self.bones.len() && p != i) {
                    None => Some(local[i]),
                    Some(p) => world[p].map(|w| compose(w, local[i])),
                };
            }
        }
        world
            .into_iter()
            .zip(local)
            .map(|(w, l)| w.unwrap_or(*l))
            .collect()
    }

    /// Vertex positions at `time` seconds.
    pub fn pose(&self, time: f32) -> Vec<[f32; 2]> {
        let bind_local: Vec<[f32; 6]> = self.bones.iter().map(|b| b.bind).collect();
        let total: f32 = self.animations.iter().map(|a| a.weight.max(0.0)).sum();
        let local: Vec<[f32; 6]> = if total <= 0.0 {
            bind_local.clone()
        } else {
            (0..self.bones.len())
                .map(|b| {
                    // Weighted mix of the playing animations' poses.
                    let mut pose: Option<(BonePose, f32)> = None;
                    for a in &self.animations {
                        let w = a.weight.max(0.0);
                        let frame = a.animation.frame_at(time * a.rate);
                        let Some(p) = a.animation.sample(b, frame) else {
                            continue;
                        };
                        pose = Some(match pose {
                            None => (p, w),
                            Some((acc, acc_w)) if acc_w + w > 0.0 => {
                                (acc.lerp(p, w / (acc_w + w)), acc_w + w)
                            }
                            Some(acc) => acc,
                        });
                    }
                    pose.map_or(bind_local[b], |(p, _)| p.affine())
                })
                .collect()
        };
        let bind = self.world(&bind_local);
        let posed = self.world(&local);
        let skin: Vec<[f32; 6]> = posed
            .iter()
            .zip(&bind)
            .map(|(p, b)| compose(*p, invert(*b)))
            .collect();
        self.vertices
            .iter()
            .map(|v| {
                let mut out = [0.0; 2];
                let mut sum = 0.0;
                for (&bone, &w) in v.bones.iter().zip(&v.weights) {
                    if w <= 0.0 {
                        continue;
                    }
                    if let Some(m) = skin.get(bone as usize) {
                        let p = apply(*m, v.position);
                        out[0] += p[0] * w;
                        out[1] += p[1] * w;
                        sum += w;
                    }
                }
                if sum > 0.0 {
                    [out[0] / sum, out[1] / sum]
                } else {
                    v.position
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(x: f32, angle: f32) -> BonePose {
        BonePose {
            position: [x, 0.0],
            angle,
            scale: [1.0, 1.0],
        }
    }

    /// Two bones in a chain along x; the tip vertex follows the second.
    fn arm(angles: [f32; 2]) -> Puppet {
        let v = |x: f32, bone: u32| PuppetVertex {
            position: [x, 0.0],
            uv: [0.0, 0.0],
            bones: [bone, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        };
        Puppet {
            vertices: vec![v(0.0, 0), v(20.0, 1)],
            indices: Vec::new(),
            bones: vec![
                PuppetBone {
                    parent: None,
                    bind: IDENTITY_AFFINE,
                },
                PuppetBone {
                    parent: Some(0),
                    bind: affine([10.0, 0.0], 0.0, [1.0, 1.0]),
                },
            ],
            animations: vec![PlayingAnimation {
                animation: PuppetAnimation {
                    name: "a".into(),
                    fps: 1.0,
                    frames: 2,
                    mode: AnimationMode::Mirror,
                    tracks: vec![
                        vec![pose(0.0, 0.0), pose(0.0, angles[0]), pose(0.0, 0.0)],
                        vec![pose(10.0, 0.0), pose(10.0, angles[1]), pose(10.0, 0.0)],
                    ],
                },
                rate: 1.0,
                weight: 1.0,
            }],
        }
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn bind_pose_keeps_vertices() {
        let p = arm([0.0, 0.0]);
        assert!(close(p.pose(0.0)[1], [20.0, 0.0]));
    }

    #[test]
    fn child_bones_rotate_about_their_joint() {
        // Frame 1: the second bone turns 90° about (10, 0): the tip (20, 0) → (10, 10).
        let p = arm([0.0, std::f32::consts::FRAC_PI_2]);
        assert!(close(p.pose(1.0)[1], [10.0, 10.0]), "{:?}", p.pose(1.0));
        // Root turning carries the child: tip → (0, 20).
        let p = arm([std::f32::consts::FRAC_PI_2, 0.0]);
        assert!(close(p.pose(1.0)[1], [0.0, 20.0]), "{:?}", p.pose(1.0));
    }

    #[test]
    fn modes_map_time_to_frames() {
        let mut a = arm([0.0, 0.0]).animations[0].animation.clone();
        assert_eq!(a.frame_at(3.0), 1.0); // mirror: 0 1 2 1 0
        a.mode = AnimationMode::Loop;
        assert_eq!(a.frame_at(3.0), 1.0);
        assert_eq!(a.frame_at(2.5), 0.5);
        a.mode = AnimationMode::Single;
        assert_eq!(a.frame_at(9.0), 2.0);
    }
}
