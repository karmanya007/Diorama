//! WE `.mdl` puppet meshes (format notes: `docs/formats/mdl.md`).
//!
//! ```text
//! "MDLV00NN\0"  u32 vertex format flags  u32 ?  u32 ?  material path\0
//! u32 0  u32 vertex bytes  vertices…  u32 index bytes  u16 indices…
//! "MDLS000N\0"  u32 end offset  u32 bone count
//!   per bone: name\0  u32 flags  i32 parent  u32 matrix bytes (64)  f32[16]  string\0
//! "MDLA000N\0"  u32 end offset  u32 animation count
//!   per animation: u32 id  u32 ?  name\0  mode\0  f32 fps  u32 frames  u32 ?  u32 tracks
//!     per track: u32 ?  u32 bytes  (frames + 1) × f32[9] (position, euler angles, scale)
//! ```
//! Files are zero-padded (WE writes 1 MiB). Only the skinned 2D puppet
//! layout is read; 3D models (other vertex formats) are rejected.

use crate::error::{Error, Result};
use crate::reader::ByteReader;

/// Vertex format of puppet meshes: position, 4 bone indices, 4 weights, uv.
const PUPPET_FORMAT: u32 = 0x0180_0009;
const PUPPET_VERTEX_BYTES: usize = 52;
const MAX_BONES: u32 = 1024;
const MAX_ANIMATIONS: u32 = 1024;
const MAX_NAME: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub struct Mdl {
    pub material: String,
    pub vertices: Vec<MdlVertex>,
    pub indices: Vec<u16>,
    pub bones: Vec<MdlBone>,
    pub animations: Vec<MdlAnimation>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MdlVertex {
    pub position: [f32; 3],
    pub bones: [u32; 4],
    pub weights: [f32; 4],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct MdlBone {
    pub name: String,
    pub parent: Option<usize>,
    /// Column-major local bind matrix.
    pub matrix: [f32; 16],
}

#[derive(Debug, Clone, PartialEq)]
pub struct MdlAnimation {
    pub id: u32,
    pub name: String,
    /// `loop`, `mirror` or `single`.
    pub mode: String,
    pub fps: f32,
    pub frames: u32,
    /// Per bone: `frames + 1` samples of (position, angles, scale).
    pub tracks: Vec<Vec<[f32; 9]>>,
}

fn magic(r: &mut ByteReader<'_>, prefix: &'static str) -> Result<()> {
    let tag = r.cstr(16, "mdl tag")?;
    if tag.len() == 8 && tag.starts_with(prefix) {
        Ok(())
    } else {
        Err(Error::BadMagic {
            expected: prefix,
            found: tag.to_owned(),
        })
    }
}

fn count(value: u32, limit: u32, what: &'static str) -> Result<usize> {
    if value > limit {
        return Err(Error::LimitExceeded {
            what,
            value: u64::from(value),
            limit: u64::from(limit),
        });
    }
    Ok(value as usize)
}

impl Mdl {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(data);
        magic(&mut r, "MDLV")?;
        let format = r.u32()?;
        if format != PUPPET_FORMAT {
            return Err(Error::Unsupported {
                what: "mdl vertex format",
                value: format!("{format:#x}"),
            });
        }
        r.u32()?;
        r.u32()?;
        let material = r.cstr(MAX_NAME, "mdl material")?.to_owned();
        r.u32()?;
        let vertex_bytes = r.u32()? as usize;
        let vertex_data = r.bytes(vertex_bytes)?;
        let vertices = vertex_data
            .as_chunks::<PUPPET_VERTEX_BYTES>()
            .0
            .iter()
            .map(|c| {
                let mut v = ByteReader::new(c);
                let position = [v.f32()?, v.f32()?, v.f32()?];
                let bones = [v.u32()?, v.u32()?, v.u32()?, v.u32()?];
                let weights = [v.f32()?, v.f32()?, v.f32()?, v.f32()?];
                let uv = [v.f32()?, v.f32()?];
                Ok(MdlVertex {
                    position,
                    bones,
                    weights,
                    uv,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let index_bytes = r.u32()? as usize;
        let indices: Vec<u16> = r
            .bytes(index_bytes)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        if indices.iter().any(|&i| usize::from(i) >= vertices.len()) {
            return Err(Error::Unsupported {
                what: "mdl index",
                value: "out of range".into(),
            });
        }

        magic(&mut r, "MDLS")?;
        r.u32()?;
        let bone_count = count(r.u32()?, MAX_BONES, "mdl bones")?;
        let mut bones = Vec::with_capacity(bone_count);
        for _ in 0..bone_count {
            let name = r.cstr(MAX_NAME, "bone name")?.to_owned();
            r.u32()?;
            let parent = usize::try_from(r.i32()?).ok().filter(|&p| p < bone_count);
            let matrix_bytes = r.u32()? as usize;
            let raw = r.bytes(matrix_bytes)?;
            let mut matrix = [0.0; 16];
            for (m, c) in matrix.iter_mut().zip(raw.as_chunks::<4>().0) {
                *m = f32::from_le_bytes(*c);
            }
            r.cstr(MAX_NAME * 16, "bone data")?;
            bones.push(MdlBone {
                name,
                parent,
                matrix,
            });
        }

        let mut animations = Vec::new();
        // Puppets without animations end after the skeleton.
        if r.pos() < data.len() && data.get(r.pos()) != Some(&0) {
            magic(&mut r, "MDLA")?;
            r.u32()?;
            let n = count(r.u32()?, MAX_ANIMATIONS, "mdl animations")?;
            for _ in 0..n {
                let id = r.u32()?;
                r.u32()?;
                let name = r.cstr(MAX_NAME, "animation name")?.to_owned();
                let mode = r.cstr(64, "animation mode")?.to_owned();
                let fps = r.f32()?;
                let frames = r.u32()?;
                r.u32()?;
                let track_count = count(r.u32()?, MAX_BONES, "animation tracks")?;
                let mut tracks = Vec::with_capacity(track_count);
                for _ in 0..track_count {
                    r.u32()?;
                    let bytes = r.u32()? as usize;
                    let samples = r
                        .bytes(bytes)?
                        .as_chunks::<36>()
                        .0
                        .iter()
                        .map(|c| {
                            let mut s = [0.0; 9];
                            for (v, b) in s.iter_mut().zip(c.as_chunks::<4>().0) {
                                *v = f32::from_le_bytes(*b);
                            }
                            s
                        })
                        .collect();
                    tracks.push(samples);
                }
                animations.push(MdlAnimation {
                    id,
                    name,
                    mode,
                    fps,
                    frames,
                    tracks,
                });
            }
        }
        Ok(Self {
            material,
            vertices,
            indices,
            bones,
            animations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-triangle, two-bone puppet with one two-frame animation.
    fn sample() -> Vec<u8> {
        let mut b = Vec::new();
        let u = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
        let f = |b: &mut Vec<u8>, v: f32| b.extend_from_slice(&v.to_le_bytes());
        b.extend_from_slice(b"MDLV0013\0");
        u(&mut b, PUPPET_FORMAT);
        u(&mut b, 1);
        u(&mut b, 1);
        b.extend_from_slice(b"materials/x.json\0");
        u(&mut b, 0);
        u(&mut b, 3 * 52);
        for i in 0..3 {
            for p in [i as f32, 2.0, 0.0] {
                f(&mut b, p);
            }
            for bone in [1, 0, 0, 0] {
                u(&mut b, bone);
            }
            for w in [1.0, 0.0, 0.0, 0.0] {
                f(&mut b, w);
            }
            f(&mut b, 0.5);
            f(&mut b, 0.25);
        }
        u(&mut b, 6);
        for i in [0u16, 1, 2] {
            b.extend_from_slice(&i.to_le_bytes());
        }
        b.extend_from_slice(b"MDLS0001\0");
        u(&mut b, 0);
        u(&mut b, 2);
        for parent in [-1i32, 0] {
            b.push(0);
            u(&mut b, 1);
            b.extend_from_slice(&parent.to_le_bytes());
            u(&mut b, 64);
            for i in 0..16 {
                f(&mut b, if i % 5 == 0 { 1.0 } else { 0.0 });
            }
            b.push(0);
        }
        b.extend_from_slice(b"MDLA0001\0");
        u(&mut b, 0);
        u(&mut b, 1);
        u(&mut b, 7);
        u(&mut b, 0);
        b.extend_from_slice(b"walk\0mirror\0");
        f(&mut b, 10.0);
        u(&mut b, 1);
        u(&mut b, 0);
        u(&mut b, 2);
        for _ in 0..2 {
            u(&mut b, 0);
            u(&mut b, 2 * 36);
            for _ in 0..2 {
                for v in [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0] {
                    f(&mut b, v);
                }
            }
        }
        b.resize(b.len() + 64, 0);
        b
    }

    #[test]
    fn parses_a_puppet() {
        let m = Mdl::parse(&sample()).unwrap();
        assert_eq!(m.material, "materials/x.json");
        assert_eq!(m.vertices.len(), 3);
        assert_eq!(m.vertices[2].position, [2.0, 2.0, 0.0]);
        assert_eq!(m.vertices[0].bones[0], 1);
        assert_eq!(m.indices, [0, 1, 2]);
        assert_eq!(m.bones[1].parent, Some(0));
        assert_eq!(m.bones[0].parent, None);
        assert_eq!(m.animations[0].mode, "mirror");
        assert_eq!(m.animations[0].tracks[1][1][5], 0.5);
    }

    #[test]
    fn truncated_or_foreign_files_are_errors() {
        let data = sample();
        for len in [0, 8, 20, 100, 300, data.len() - 80] {
            assert!(Mdl::parse(&data[..len]).is_err(), "{len}");
        }
        let mut other = data.clone();
        other[9] = 0x0f;
        assert!(matches!(Mdl::parse(&other), Err(Error::Unsupported { .. })));
    }
}
