//! Reader for Wallpaper Engine `.tex` textures. See `docs/formats/tex.md`.
//!
//! ```text
//! "TEXV0005\0"  "TEXI0001\0"
//! i32 format, u32 flags, u32 texture_w, texture_h, image_w, image_h
//! [u32 extra]                       only when flags & 0x40 (3D LUTs)
//! u32 unknown
//! "TEXB000N\0"                      N = 1..=4
//! u32 image_count
//! [i32 image_format]                N >= 3, FreeImage format id, -1 = raw pixels
//! [u32 is_video_mp4]                N == 4
//! image_count × {
//!     u32 mip_count
//!     mip_count × {
//!         u32 width, height
//!         [u32 depth]               flags & 0x40
//!         [u32 lz4, u32 decompressed_size]   N >= 2
//!         u32 size, size bytes
//!     }
//! }
//! ["TEXS000N\0" sprite frames]     only when flags & 0x4
//! ```

use lumen_core::Limits;

use crate::error::{Error, Result};
use crate::reader::ByteReader;

const TAG_MAX: usize = 16;
const LZ4_MAX_RATIO: u64 = 256;

/// Pixel format of raw (non-encoded) mip data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexFormat {
    Rgba8888,
    Dxt5,
    Dxt3,
    Dxt1,
    /// Two 8-bit channels. Used for particle sprites; treated as luminance + alpha.
    Rg88,
    R8,
    Unknown(i32),
}

impl TexFormat {
    fn from_raw(v: i32) -> Self {
        match v {
            0 => Self::Rgba8888,
            4 => Self::Dxt5,
            6 => Self::Dxt3,
            7 => Self::Dxt1,
            8 => Self::Rg88,
            9 => Self::R8,
            other => Self::Unknown(other),
        }
    }

    /// Bytes needed for a `width × height` image, or `None` for unknown formats.
    pub fn byte_size(self, width: u32, height: u32) -> Option<u64> {
        let (w, h) = (u64::from(width), u64::from(height));
        let blocks = w.div_ceil(4) * h.div_ceil(4);
        Some(match self {
            Self::Rgba8888 => w * h * 4,
            Self::Rg88 => w * h * 2,
            Self::R8 => w * h,
            Self::Dxt1 => blocks * 8,
            Self::Dxt3 | Self::Dxt5 => blocks * 16,
            Self::Unknown(_) => return None,
        })
    }
}

/// Raw `TEXI` flag bits. Meanings of bits not listed here are not known yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexFlags(pub u32);

impl TexFlags {
    pub const NO_INTERPOLATION: u32 = 0x1;
    pub const CLAMP_UVS: u32 = 0x2;
    pub const SPRITE_SHEET: u32 = 0x4;
    /// Mip payload is an MP4 file, in any container version (verified on corpus).
    pub const VIDEO: u32 = 0x20;
    /// Header and every mip carry one extra `u32`. Seen on 32×32×32 color-grading LUTs.
    pub const EXTRA_DEPTH: u32 = 0x40;

    pub fn contains(self, bit: u32) -> bool {
        self.0 & bit == bit
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TexHeader {
    pub format: TexFormat,
    pub flags: TexFlags,
    /// Allocated (often power-of-two padded) size.
    pub texture_width: u32,
    pub texture_height: u32,
    /// Size of the meaningful image inside the texture.
    pub image_width: u32,
    pub image_height: u32,
    pub extra: Option<u32>,
    pub unknown: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TexMip {
    pub width: u32,
    pub height: u32,
    pub depth: Option<u32>,
    /// Decompressed bytes: raw pixels in [`TexHeader::format`], or an encoded file
    /// (PNG, JPEG, …) when [`Tex::image_format`] is set, or MP4 when [`Tex::is_video`].
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TexImage {
    pub mips: Vec<TexMip>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpriteFrame {
    pub image_id: u32,
    /// Seconds this frame is shown.
    pub frametime: f32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub width_y: f32,
    pub height_x: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpriteSheet {
    pub version: u8,
    /// Present from `TEXS0003`.
    pub size: Option<(u32, u32)>,
    pub frames: Vec<SpriteFrame>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tex {
    pub header: TexHeader,
    pub container_version: u8,
    /// FreeImage format id of encoded mip data; `None` means raw pixels.
    pub image_format: Option<i32>,
    pub is_video: bool,
    pub images: Vec<TexImage>,
    pub sprite_sheet: Option<SpriteSheet>,
}

impl Tex {
    pub fn parse(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        expect_tag(&mut r, "TEXV0005")?;
        expect_tag(&mut r, "TEXI0001")?;
        let header = read_header(&mut r, limits)?;

        let container = r.cstr(TAG_MAX, "TEXB tag")?;
        let container_version = tag_version(container, "TEXB")?;
        if !(1..=4).contains(&container_version) {
            return Err(Error::Unsupported {
                what: "TEXB version",
                value: container.to_owned(),
            });
        }

        let image_count = r.u32()?;
        check_limit("tex image count", image_count, 4096)?;
        let image_format = if container_version >= 3 {
            Some(r.i32()?).filter(|&f| f != -1)
        } else {
            None
        };
        let video_field = container_version >= 4 && r.u32()? != 0;
        let is_video = video_field || header.flags.contains(TexFlags::VIDEO);

        let mut images = Vec::with_capacity(image_count as usize);
        for _ in 0..image_count {
            let mip_count = r.u32()?;
            check_limit("tex mip count", mip_count, 32)?;
            let mut mips = Vec::with_capacity(mip_count as usize);
            for _ in 0..mip_count {
                mips.push(read_mip(&mut r, &header, container_version, limits)?);
            }
            images.push(TexImage { mips });
        }

        let sprite_sheet = if header.flags.contains(TexFlags::SPRITE_SHEET) {
            Some(read_sprite_sheet(&mut r)?)
        } else {
            None
        };

        Ok(Self {
            header,
            container_version,
            image_format,
            is_video,
            images,
            sprite_sheet,
        })
    }

    /// Largest mip of the first image.
    pub fn first_mip(&self) -> Option<&TexMip> {
        self.images.first()?.mips.first()
    }
}

fn expect_tag(r: &mut ByteReader<'_>, expected: &'static str) -> Result<()> {
    let found = r.cstr(TAG_MAX, "tex tag")?;
    if found == expected {
        Ok(())
    } else {
        Err(Error::BadMagic {
            expected,
            found: found.to_owned(),
        })
    }
}

/// `"TEXB0003"` → 3.
fn tag_version(tag: &str, prefix: &'static str) -> Result<u8> {
    tag.strip_prefix(prefix)
        .filter(|digits| digits.len() == 4)
        .and_then(|digits| digits.parse().ok())
        .ok_or_else(|| Error::BadMagic {
            expected: prefix,
            found: tag.to_owned(),
        })
}

fn check_limit(what: &'static str, value: u32, limit: u32) -> Result<()> {
    if value > limit {
        return Err(Error::LimitExceeded {
            what,
            value: u64::from(value),
            limit: u64::from(limit),
        });
    }
    Ok(())
}

fn read_header(r: &mut ByteReader<'_>, limits: &Limits) -> Result<TexHeader> {
    let format = TexFormat::from_raw(r.i32()?);
    let flags = TexFlags(r.u32()?);
    let texture_width = r.u32()?;
    let texture_height = r.u32()?;
    let image_width = r.u32()?;
    let image_height = r.u32()?;
    for dim in [texture_width, texture_height, image_width, image_height] {
        check_limit("texture dimension", dim, limits.max_texture_dim)?;
    }
    let extra = if flags.contains(TexFlags::EXTRA_DEPTH) {
        Some(r.u32()?)
    } else {
        None
    };
    let unknown = r.u32()?;
    Ok(TexHeader {
        format,
        flags,
        texture_width,
        texture_height,
        image_width,
        image_height,
        extra,
        unknown,
    })
}

fn read_mip(
    r: &mut ByteReader<'_>,
    header: &TexHeader,
    container_version: u8,
    limits: &Limits,
) -> Result<TexMip> {
    let width = r.u32()?;
    let height = r.u32()?;
    check_limit("mip dimension", width, limits.max_texture_dim)?;
    check_limit("mip dimension", height, limits.max_texture_dim)?;
    let depth = if header.flags.contains(TexFlags::EXTRA_DEPTH) {
        Some(r.u32()?)
    } else {
        None
    };
    let (lz4, decompressed_size) = if container_version >= 2 {
        (r.u32()? != 0, r.u32()?)
    } else {
        (false, 0)
    };
    let size = r.u32()?;
    let raw = r.bytes(size as usize)?;

    let data = if lz4 {
        let expected = u64::from(decompressed_size);
        // An LZ4 block cannot expand more than ~255×; reject impossible claims
        // before allocating the output buffer.
        let max_plausible = u64::from(size)
            .saturating_mul(LZ4_MAX_RATIO)
            .saturating_add(64)
            .min(limits.max_decompressed_bytes);
        if expected > max_plausible {
            return Err(Error::LimitExceeded {
                what: "decompressed mip size",
                value: expected,
                limit: max_plausible,
            });
        }
        let out = lz4_flex::block::decompress(raw, decompressed_size as usize)
            .map_err(|e| Error::Lz4(e.to_string()))?;
        if out.len() as u64 != expected {
            return Err(Error::SizeMismatch {
                what: "decompressed mip",
                expected,
                actual: out.len() as u64,
            });
        }
        out
    } else {
        raw.to_vec()
    };

    Ok(TexMip {
        width,
        height,
        depth,
        data,
    })
}

fn read_sprite_sheet(r: &mut ByteReader<'_>) -> Result<SpriteSheet> {
    let tag = r.cstr(TAG_MAX, "TEXS tag")?;
    let version = tag_version(tag, "TEXS")?;
    if !(1..=3).contains(&version) {
        return Err(Error::Unsupported {
            what: "TEXS version",
            value: tag.to_owned(),
        });
    }
    let frame_count = r.u32()?;
    check_limit("sprite frame count", frame_count, 65_536)?;
    let size = if version >= 3 {
        Some((r.u32()?, r.u32()?))
    } else {
        None
    };

    let mut frames = Vec::with_capacity(frame_count as usize);
    for _ in 0..frame_count {
        let image_id = r.u32()?;
        let frametime = r.f32()?;
        // TEXS0001 stores the rectangle as integers (unverified: not present in local corpus).
        let mut coord = || -> Result<f32> {
            if version == 1 {
                Ok(r.i32()? as f32)
            } else {
                r.f32()
            }
        };
        let (x, y, width, width_y, height_x, height) =
            (coord()?, coord()?, coord()?, coord()?, coord()?, coord()?);
        frames.push(SpriteFrame {
            image_id,
            frametime,
            x,
            y,
            width,
            width_y,
            height_x,
            height,
        });
    }
    Ok(SpriteSheet {
        version,
        size,
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Builder(Vec<u8>);

    impl Builder {
        fn new() -> Self {
            Self(Vec::new())
        }
        fn tag(mut self, s: &str) -> Self {
            self.0.extend_from_slice(s.as_bytes());
            self.0.push(0);
            self
        }
        fn u32(mut self, v: u32) -> Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn i32(self, v: i32) -> Self {
            self.u32(v.cast_unsigned())
        }
        fn f32(self, v: f32) -> Self {
            self.u32(v.to_bits())
        }
        fn bytes(mut self, b: &[u8]) -> Self {
            self.0.extend_from_slice(b);
            self
        }
        fn header(self, format: i32, flags: u32, w: u32, h: u32) -> Self {
            self.tag("TEXV0005")
                .tag("TEXI0001")
                .i32(format)
                .u32(flags)
                .u32(w)
                .u32(h)
                .u32(w)
                .u32(h)
                .u32(0xFF00_FF00)
        }
    }

    #[test]
    fn parses_texb1_raw_rgba() {
        let pixels = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let bytes = Builder::new()
            .header(0, 2, 2, 1)
            .tag("TEXB0001")
            .u32(1) // images
            .u32(1) // mips
            .u32(2)
            .u32(1)
            .u32(8)
            .bytes(&pixels)
            .0;
        let tex = Tex::parse(&bytes, &Limits::default()).unwrap();
        assert_eq!(tex.header.format, TexFormat::Rgba8888);
        assert!(tex.header.flags.contains(TexFlags::CLAMP_UVS));
        assert_eq!(tex.container_version, 1);
        assert_eq!(tex.image_format, None);
        assert_eq!(tex.first_mip().unwrap().data, pixels);
    }

    #[test]
    fn parses_texb4_lz4_mip() {
        let pixels = vec![7u8; 64];
        let compressed = lz4_flex::block::compress(&pixels);
        let bytes = Builder::new()
            .header(9, 0, 8, 8)
            .tag("TEXB0004")
            .u32(1)
            .i32(-1) // raw
            .u32(0) // not video
            .u32(1)
            .u32(8)
            .u32(8)
            .u32(1) // lz4
            .u32(64)
            .u32(compressed.len() as u32)
            .bytes(&compressed)
            .0;
        let tex = Tex::parse(&bytes, &Limits::default()).unwrap();
        assert_eq!(tex.header.format, TexFormat::R8);
        assert!(!tex.is_video);
        assert_eq!(tex.first_mip().unwrap().data, pixels);
    }

    #[test]
    fn parses_extra_depth_and_encoded_format() {
        let png = b"\x89PNG fake";
        let bytes = Builder::new()
            .tag("TEXV0005")
            .tag("TEXI0001")
            .i32(0)
            .u32(0x42)
            .u32(32)
            .u32(32)
            .u32(1024)
            .u32(32)
            .u32(32) // extra
            .u32(0)
            .tag("TEXB0004")
            .u32(1)
            .i32(13) // FreeImage PNG
            .u32(0)
            .u32(1)
            .u32(32)
            .u32(32)
            .u32(32) // depth
            .u32(0)
            .u32(0)
            .u32(png.len() as u32)
            .bytes(png)
            .0;
        let tex = Tex::parse(&bytes, &Limits::default()).unwrap();
        assert_eq!(tex.header.extra, Some(32));
        assert_eq!(tex.image_format, Some(13));
        let mip = tex.first_mip().unwrap();
        assert_eq!(mip.depth, Some(32));
        assert_eq!(mip.data, png);
    }

    #[test]
    fn video_flag_marks_mp4_payload_in_texb3() {
        let mp4 = b"\0\0\0\x20ftypisom";
        let bytes = Builder::new()
            .header(0, 0x23, 16, 16)
            .tag("TEXB0003")
            .u32(1)
            .i32(-1)
            .u32(1)
            .u32(16)
            .u32(16)
            .u32(0)
            .u32(0)
            .u32(mp4.len() as u32)
            .bytes(mp4)
            .0;
        let tex = Tex::parse(&bytes, &Limits::default()).unwrap();
        assert!(tex.is_video);
        assert_eq!(tex.image_format, None);
        assert_eq!(tex.first_mip().unwrap().data, mp4);
    }

    #[test]
    fn parses_sprite_sheet_v3() {
        let bytes = Builder::new()
            .header(0, 4, 4, 4)
            .tag("TEXB0003")
            .u32(1)
            .i32(-1)
            .u32(1)
            .u32(1)
            .u32(1)
            .u32(0)
            .u32(4)
            .u32(4)
            .bytes(&[0; 4])
            .tag("TEXS0003")
            .u32(1)
            .u32(256)
            .u32(256)
            .u32(0)
            .f32(0.0625)
            .f32(0.0)
            .f32(0.0)
            .f32(256.0)
            .f32(0.0)
            .f32(0.0)
            .f32(256.0)
            .0;
        let tex = Tex::parse(&bytes, &Limits::default()).unwrap();
        let sheet = tex.sprite_sheet.unwrap();
        assert_eq!(sheet.version, 3);
        assert_eq!(sheet.size, Some((256, 256)));
        assert_eq!(sheet.frames.len(), 1);
        assert_eq!(sheet.frames[0].frametime, 0.0625);
        assert_eq!(sheet.frames[0].height, 256.0);
    }

    #[test]
    fn rejects_lz4_bomb() {
        let bytes = Builder::new()
            .header(0, 0, 4, 4)
            .tag("TEXB0002")
            .u32(1)
            .u32(1)
            .u32(4)
            .u32(4)
            .u32(1)
            .u32(u32::MAX)
            .u32(1)
            .bytes(&[0])
            .0;
        assert!(matches!(
            Tex::parse(&bytes, &Limits::default()),
            Err(Error::LimitExceeded { .. })
        ));
    }

    #[test]
    fn rejects_implausible_lz4_expansion() {
        // 10 compressed bytes claiming 1 MiB: within max_decompressed_bytes but
        // beyond what LZ4 can produce, so it must fail before allocating.
        let bytes = Builder::new()
            .header(0, 0, 512, 512)
            .tag("TEXB0002")
            .u32(1)
            .u32(1)
            .u32(512)
            .u32(512)
            .u32(1)
            .u32(1 << 20)
            .u32(10)
            .bytes(&[0; 10])
            .0;
        assert!(matches!(
            Tex::parse(&bytes, &Limits::default()),
            Err(Error::LimitExceeded {
                what: "decompressed mip size",
                ..
            })
        ));
    }

    #[test]
    fn rejects_huge_dimensions() {
        let bytes = Builder::new().header(0, 0, 1 << 20, 4).0;
        assert!(matches!(
            Tex::parse(&bytes, &Limits::default()),
            Err(Error::LimitExceeded { .. })
        ));
    }

    #[test]
    fn rejects_unknown_container() {
        let bytes = Builder::new().header(0, 0, 4, 4).tag("TEXB0009").0;
        assert!(matches!(
            Tex::parse(&bytes, &Limits::default()),
            Err(Error::Unsupported { .. })
        ));
    }

    #[test]
    fn byte_sizes_match_corpus_observations() {
        // Values cross-checked against decompressed sizes seen in WE assets.
        assert_eq!(TexFormat::Dxt5.byte_size(128, 128), Some(16_384));
        assert_eq!(TexFormat::Rg88.byte_size(256, 256), Some(131_072));
        assert_eq!(TexFormat::R8.byte_size(1024, 1024), Some(1_048_576));
        assert_eq!(TexFormat::Rgba8888.byte_size(256, 256), Some(262_144));
        assert_eq!(TexFormat::Dxt1.byte_size(6, 6), Some(32));
    }
}
