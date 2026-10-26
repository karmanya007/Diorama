//! 3D placement for perspective scenes: 4×4 matrices, node world matrices
//! and the perspective camera.
//!
//! Matrices are row-vector, as WE stores and uploads them: `p' = p · M`,
//! translation in row 3. Checked against a RenderDoc capture of 3509243656
//! (model and view-projection matrices of its 3D draws).

use crate::Node;

/// Row-vector 4×4 matrix (`p' = p · M`, translation in row 3).
pub type Mat4 = [[f32; 4]; 4];

pub const IDENTITY_MAT4: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// `a` then `b`: `p · mat4_mul(a, b) == (p · a) · b`.
pub fn mat4_mul(a: Mat4, b: Mat4) -> Mat4 {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..4).map(|k| a[r][k] * b[k][c]).sum()))
}

/// The 2D affine `[m0..m5]` (see [`crate::ImageLayer::transform`]) as a 4×4.
pub fn affine_mat4(t: [f32; 6]) -> Mat4 {
    [
        [t[0], t[1], 0.0, 0.0],
        [t[2], t[3], 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [t[4], t[5], 0.0, 1.0],
    ]
}

/// General inverse (cofactors); `None` when singular.
pub fn invert_mat4(m: Mat4) -> Option<Mat4> {
    let a: Vec<f32> = m.iter().flatten().copied().collect();
    let mut inv = [0.0f32; 16];
    inv[0] = a[5] * a[10] * a[15] - a[5] * a[11] * a[14] - a[9] * a[6] * a[15]
        + a[9] * a[7] * a[14]
        + a[13] * a[6] * a[11]
        - a[13] * a[7] * a[10];
    inv[4] = -a[4] * a[10] * a[15] + a[4] * a[11] * a[14] + a[8] * a[6] * a[15]
        - a[8] * a[7] * a[14]
        - a[12] * a[6] * a[11]
        + a[12] * a[7] * a[10];
    inv[8] = a[4] * a[9] * a[15] - a[4] * a[11] * a[13] - a[8] * a[5] * a[15]
        + a[8] * a[7] * a[13]
        + a[12] * a[5] * a[11]
        - a[12] * a[7] * a[9];
    inv[12] = -a[4] * a[9] * a[14] + a[4] * a[10] * a[13] + a[8] * a[5] * a[14]
        - a[8] * a[6] * a[13]
        - a[12] * a[5] * a[10]
        + a[12] * a[6] * a[9];
    inv[1] = -a[1] * a[10] * a[15] + a[1] * a[11] * a[14] + a[9] * a[2] * a[15]
        - a[9] * a[3] * a[14]
        - a[13] * a[2] * a[11]
        + a[13] * a[3] * a[10];
    inv[5] = a[0] * a[10] * a[15] - a[0] * a[11] * a[14] - a[8] * a[2] * a[15]
        + a[8] * a[3] * a[14]
        + a[12] * a[2] * a[11]
        - a[12] * a[3] * a[10];
    inv[9] = -a[0] * a[9] * a[15] + a[0] * a[11] * a[13] + a[8] * a[1] * a[15]
        - a[8] * a[3] * a[13]
        - a[12] * a[1] * a[11]
        + a[12] * a[3] * a[9];
    inv[13] = a[0] * a[9] * a[14] - a[0] * a[10] * a[13] - a[8] * a[1] * a[14]
        + a[8] * a[2] * a[13]
        + a[12] * a[1] * a[10]
        - a[12] * a[2] * a[9];
    inv[2] = a[1] * a[6] * a[15] - a[1] * a[7] * a[14] - a[5] * a[2] * a[15]
        + a[5] * a[3] * a[14]
        + a[13] * a[2] * a[7]
        - a[13] * a[3] * a[6];
    inv[6] = -a[0] * a[6] * a[15] + a[0] * a[7] * a[14] + a[4] * a[2] * a[15]
        - a[4] * a[3] * a[14]
        - a[12] * a[2] * a[7]
        + a[12] * a[3] * a[6];
    inv[10] = a[0] * a[5] * a[15] - a[0] * a[7] * a[13] - a[4] * a[1] * a[15]
        + a[4] * a[3] * a[13]
        + a[12] * a[1] * a[7]
        - a[12] * a[3] * a[5];
    inv[14] = -a[0] * a[5] * a[14] + a[0] * a[6] * a[13] + a[4] * a[1] * a[14]
        - a[4] * a[2] * a[13]
        - a[12] * a[1] * a[6]
        + a[12] * a[2] * a[5];
    inv[3] = -a[1] * a[6] * a[11] + a[1] * a[7] * a[10] + a[5] * a[2] * a[11]
        - a[5] * a[3] * a[10]
        - a[9] * a[2] * a[7]
        + a[9] * a[3] * a[6];
    inv[7] = a[0] * a[6] * a[11] - a[0] * a[7] * a[10] - a[4] * a[2] * a[11]
        + a[4] * a[3] * a[10]
        + a[8] * a[2] * a[7]
        - a[8] * a[3] * a[6];
    inv[11] = -a[0] * a[5] * a[11] + a[0] * a[7] * a[9] + a[4] * a[1] * a[11]
        - a[4] * a[3] * a[9]
        - a[8] * a[1] * a[7]
        + a[8] * a[3] * a[5];
    inv[15] = a[0] * a[5] * a[10] - a[0] * a[6] * a[9] - a[4] * a[1] * a[10]
        + a[4] * a[2] * a[9]
        + a[8] * a[1] * a[6]
        - a[8] * a[2] * a[5];
    let det = a[0] * inv[0] + a[1] * inv[4] + a[2] * inv[8] + a[3] * inv[12];
    if det.abs() < 1e-20 || !det.is_finite() {
        return None;
    }
    Some(std::array::from_fn(|r| {
        std::array::from_fn(|c| inv[r * 4 + c] / det)
    }))
}

/// Scale, then rotations about x, y and z, then translation (column form
/// `T·Rz·Ry·Rx·S`). The x-then-y order is pinned by the capture of
/// 3509243656 (a model turned about x and y); z last matches the 2D scenes,
/// where a π turn about x/y mirrors in the layer's own frame.
pub fn trs_mat4(origin: [f32; 3], angles: [f32; 3], scale: [f32; 3]) -> Mat4 {
    let (sx, cx) = angles[0].sin_cos();
    let (sy, cy) = angles[1].sin_cos();
    let (sz, cz) = angles[2].sin_cos();
    let s = [
        [scale[0], 0.0, 0.0, 0.0],
        [0.0, scale[1], 0.0, 0.0],
        [0.0, 0.0, scale[2], 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    // Row-vector forms (transposes of the column rotations).
    let rx = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, cx, sx, 0.0],
        [0.0, -sx, cx, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let ry = [
        [cy, 0.0, -sy, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [sy, 0.0, cy, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let rz = [
        [cz, sz, 0.0, 0.0],
        [-sz, cz, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let mut m = mat4_mul(mat4_mul(mat4_mul(s, rx), ry), rz);
    m[3] = [origin[0], origin[1], origin[2], 1.0];
    m
}

impl Node {
    /// Full 3D local transform (see [`trs_mat4`]).
    pub fn local_mat4(&self) -> Mat4 {
        trs_mat4(self.origin, self.angles, self.scale)
    }
}

/// 3D world matrix of `nodes[index]` through its parents (depth-capped).
pub fn node_world_mat4(nodes: &[Node], index: usize) -> Mat4 {
    let Some(node) = nodes.get(index) else {
        return IDENTITY_MAT4;
    };
    let mut m = node.local_mat4();
    let mut parent = node.parent;
    for _ in 0..64 {
        let Some(p) = parent.and_then(|p| nodes.get(p)) else {
            break;
        };
        m = mat4_mul(m, p.local_mat4());
        parent = p.parent;
    }
    m
}

/// The camera of a perspective scene. It looks down its node's −z (WE camera
/// objects) or, without a node, from `eye` towards `center`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Camera object (index into [`crate::Scene::nodes`]).
    pub node: Option<usize>,
    pub eye: [f32; 3],
    pub center: [f32; 3],
    pub up: [f32; 3],
    /// Vertical field of view, degrees.
    pub fov: f32,
    pub near: f32,
    pub far: f32,
}

/// One shot of a camera path (WE `camera.paths`): the camera moves through
/// its keys over `duration` seconds; shots play one after another, looping.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraShot {
    pub duration: f32,
    pub keys: Vec<CameraKey>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraKey {
    /// Seconds into the shot.
    pub time: f32,
    pub eye: [f32; 3],
    pub center: [f32; 3],
    pub up: [f32; 3],
}

/// `camera` placed on its path at `time` (shots in order, looping; linear
/// between keys). `None` without a path.
pub fn camera_on_path(camera: &Camera, shots: &[CameraShot], time: f32) -> Option<Camera> {
    let total: f32 = shots.iter().map(|s| s.duration.max(0.0)).sum();
    if total <= 0.0 {
        return None;
    }
    let mut t = time.rem_euclid(total);
    let shot = shots.iter().find(|s| {
        if t < s.duration {
            return true;
        }
        t -= s.duration.max(0.0);
        false
    })?;
    let keys = &shot.keys;
    let first = keys.first()?;
    let after = keys.iter().position(|k| k.time > t);
    let (eye, center, up) = match after {
        Some(0) | None if keys.len() == 1 => (first.eye, first.center, first.up),
        Some(0) => (first.eye, first.center, first.up),
        None => {
            let last = keys[keys.len() - 1];
            (last.eye, last.center, last.up)
        }
        Some(i) => {
            let (a, b) = (keys[i - 1], keys[i]);
            let f = ((t - a.time) / (b.time - a.time).max(1e-6)).clamp(0.0, 1.0);
            let mix = |x: [f32; 3], y: [f32; 3]| [0, 1, 2].map(|j| x[j] + (y[j] - x[j]) * f);
            (mix(a.eye, b.eye), mix(a.center, b.center), mix(a.up, b.up))
        }
    };
    Some(Camera {
        eye,
        center,
        up,
        ..*camera
    })
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = dot(v, v).sqrt();
    (l > 1e-12).then(|| v.map(|c| c / l))
}

impl Camera {
    /// World → view matrix and the eye position.
    pub fn view(&self, nodes: &[Node]) -> (Mat4, [f32; 3]) {
        if let Some(node) = self.node.filter(|&n| n < nodes.len()) {
            let world = node_world_mat4(nodes, node);
            let eye = [world[3][0], world[3][1], world[3][2]];
            return (invert_mat4(world).unwrap_or(IDENTITY_MAT4), eye);
        }
        let forward = normalize(sub(self.center, self.eye)).unwrap_or([0.0, 0.0, -1.0]);
        let right = normalize(cross(forward, self.up)).unwrap_or([1.0, 0.0, 0.0]);
        let up = cross(right, forward);
        let e = self.eye;
        (
            [
                [right[0], up[0], -forward[0], 0.0],
                [right[1], up[1], -forward[1], 0.0],
                [right[2], up[2], -forward[2], 0.0],
                [-dot(right, e), -dot(up, e), dot(forward, e), 1.0],
            ],
            e,
        )
    }

    /// Perspective projection for a `width / height` aspect with WE's
    /// reversed depth (D3D 0..1, near → 1, far → 0).
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let f = 1.0 / (self.fov.clamp(1.0, 179.0).to_radians() * 0.5).tan();
        let (n, far) = (self.near.max(1e-6), self.far.max(self.near + 1e-3));
        let a = n / (far - n);
        [
            [f / aspect.max(1e-6), 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, a, -1.0],
            [0.0, 0.0, a * far, 0.0],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cameras_follow_their_path_shot_by_shot() {
        let camera = Camera {
            node: None,
            eye: [0.0; 3],
            center: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
            fov: 50.0,
            near: 0.01,
            far: 100.0,
        };
        let key = |time: f32, x: f32| CameraKey {
            time,
            eye: [x, 0.0, 0.0],
            center: [x, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
        };
        let shots = [
            CameraShot {
                duration: 10.0,
                keys: vec![key(0.0, 0.0), key(10.0, 10.0)],
            },
            CameraShot {
                duration: 5.0,
                keys: vec![key(0.0, 100.0), key(5.0, 110.0)],
            },
        ];
        let x = |t: f32| camera_on_path(&camera, &shots, t).unwrap().eye[0];
        assert!((x(5.0) - 5.0).abs() < 1e-4, "halfway through shot 1");
        assert!((x(12.5) - 105.0).abs() < 1e-4, "halfway through shot 2");
        assert!((x(17.5) - 2.5).abs() < 1e-4, "loops");
        assert_eq!(
            camera_on_path(&camera, &shots, 1.0).unwrap().fov,
            50.0,
            "keeps the lens"
        );
        assert!(camera_on_path(&camera, &[], 1.0).is_none());
    }

    fn close(a: Mat4, b: Mat4) -> bool {
        a.iter()
            .flatten()
            .zip(b.iter().flatten())
            .all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn view_projection_matches_capture() {
        // 3509243656: camera object at (0, 0, 6), fov 50, near 0.01, far
        // 10000, 3000×1920 swapchain.
        let node = Node {
            name: "camera".into(),
            id: 443,
            parent: None,
            origin: [0.0, 0.0, 6.0],
            scale: [1.0; 3],
            angles: [0.0; 3],
            visible: true,
        };
        let camera = Camera {
            node: Some(0),
            eye: [0.0; 3],
            center: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
            fov: 50.0,
            near: 0.01,
            far: 10000.0,
        };
        let (view, eye) = camera.view(&[node]);
        assert_eq!(eye, [0.0, 0.0, 6.0]);
        let vp = mat4_mul(view, camera.projection(3000.0 / 1920.0));
        let captured = [
            [1.372484, 0.0, 0.0, 0.0],
            [0.0, 2.144507, 0.0, 0.0],
            [0.0, 0.0, 1e-6, -1.0],
            [0.0, 0.0, 0.009994, 6.0],
        ];
        assert!(close(vp, captured), "{vp:?}");
    }

    #[test]
    fn look_at_without_a_node_faces_the_center() {
        let camera = Camera {
            node: None,
            eye: [0.0, 0.0, 6.0],
            center: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov: 50.0,
            near: 0.01,
            far: 100.0,
        };
        let (view, _) = camera.view(&[]);
        assert!(close(view, trs_mat4([0.0, 0.0, -6.0], [0.0; 3], [1.0; 3])));
    }

    #[test]
    fn rotations_apply_x_then_y_then_z() {
        // Model matrix of the first star in the capture (x/y angles only).
        let (ax, ay) = (217f32.to_radians(), 235f32.to_radians());
        let m = trs_mat4([0.0; 3], [ax, ay, 0.0], [1.0; 3]);
        // Image of the x axis has no y component when only x/y rotate.
        assert!(m[0][1].abs() < 1e-6);
        assert!((m[0][0] - ay.cos()).abs() < 1e-6 && (m[0][2] + ay.sin()).abs() < 1e-6);
        // z alone matches the 2D affine.
        let z = trs_mat4([1.0, 2.0, 0.0], [0.0, 0.0, 0.5], [2.0, 3.0, 1.0]);
        let a = crate::affine([1.0, 2.0], 0.5, [2.0, 3.0]);
        assert!(close(z, affine_mat4(a)));
        let back = invert_mat4(z).map(|i| mat4_mul(z, i));
        assert!(back.is_some_and(|b| close(b, IDENTITY_MAT4)));
    }
}
