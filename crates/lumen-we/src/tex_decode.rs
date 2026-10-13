//! CPU conversion of `.tex` mips to RGBA8.
//!
//! The GPU path uploads BC formats directly; this module serves tools
//! (`wpdump tex2png`), tests, and fallback on hardware without BC support.

use crate::error::{Error, Result};
use crate::tex::{Tex, TexFormat, TexMip};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba8Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// FreeImage format ids seen in `TEXB0003+` containers.
pub fn freeimage_format_name(id: i32) -> &'static str {
    match id {
        0 => "bmp",
        2 => "jpeg",
        13 => "png",
        17 => "tga",
        25 => "gif",
        35 => "webp",
        _ => "unknown",
    }
}

pub fn mip_to_rgba8(tex: &Tex, mip: &TexMip) -> Result<Rgba8Image> {
    if tex.is_video {
        return Err(Error::Unsupported {
            what: "texture kind",
            value: "mp4 video".to_owned(),
        });
    }
    if tex.image_format.is_some() {
        let img = image::load_from_memory(&mip.data)
            .map_err(|e| Error::Image(e.to_string()))?
            .into_rgba8();
        return Ok(Rgba8Image {
            width: img.width(),
            height: img.height(),
            pixels: img.into_raw(),
        });
    }

    let format = tex.header.format;
    let (w, h) = (mip.width, mip.height);
    let needed = format.byte_size(w, h).ok_or_else(|| Error::Unsupported {
        what: "tex format",
        value: format!("{format:?}"),
    })?;
    let data = mip.data.get(..needed as usize).ok_or(Error::SizeMismatch {
        what: "mip data",
        expected: needed,
        actual: mip.data.len() as u64,
    })?;

    let pixels = match format {
        TexFormat::Rgba8888 => data.to_vec(),
        TexFormat::R8 => data.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        TexFormat::Rg88 => data
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[l, a]| [l, l, l, a])
            .collect(),
        TexFormat::Dxt1 => decode_bc(data, w, h, BcKind::Bc1),
        TexFormat::Dxt3 => decode_bc(data, w, h, BcKind::Bc2),
        TexFormat::Dxt5 => decode_bc(data, w, h, BcKind::Bc3),
        TexFormat::Unknown(_) => unreachable_format(format)?,
    };
    Ok(Rgba8Image {
        width: w,
        height: h,
        pixels,
    })
}

fn unreachable_format(format: TexFormat) -> Result<Vec<u8>> {
    Err(Error::Unsupported {
        what: "tex format",
        value: format!("{format:?}"),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BcKind {
    Bc1,
    Bc2,
    Bc3,
}

/// Decodes BC1/BC2/BC3 (DXT1/3/5). `data` must hold every block (checked by caller).
fn decode_bc(data: &[u8], width: u32, height: u32, kind: BcKind) -> Vec<u8> {
    let block_bytes = if kind == BcKind::Bc1 { 8 } else { 16 };
    let (w, h) = (width as usize, height as usize);
    let blocks_x = w.div_ceil(4);
    let mut out = vec![0u8; w * h * 4];

    for (i, block) in data.chunks_exact(block_bytes).enumerate() {
        let (bx, by) = (i % blocks_x, i / blocks_x);
        if by * 4 >= h {
            break;
        }
        let texels = decode_block(block, kind);
        for (t, texel) in texels.iter().enumerate() {
            let (x, y) = (bx * 4 + t % 4, by * 4 + t / 4);
            if x < w && y < h {
                let o = (y * w + x) * 4;
                out[o..o + 4].copy_from_slice(texel);
            }
        }
    }
    out
}

fn decode_block(block: &[u8], kind: BcKind) -> [[u8; 4]; 16] {
    let (alpha, color) = match kind {
        BcKind::Bc1 => (None, block),
        BcKind::Bc2 => (Some(bc2_alpha(&block[..8])), &block[8..]),
        BcKind::Bc3 => (Some(bc3_alpha(&block[..8])), &block[8..]),
    };
    let mut texels = bc1_color(color, kind == BcKind::Bc1);
    if let Some(alpha) = alpha {
        for (texel, a) in texels.iter_mut().zip(alpha) {
            texel[3] = a;
        }
    }
    texels
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 0x1f) as u8;
    let g = ((c >> 5) & 0x3f) as u8;
    let b = (c & 0x1f) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}

fn mix(a: [u8; 3], b: [u8; 3], wa: u16, wb: u16) -> [u8; 4] {
    let ch = |i: usize| ((u16::from(a[i]) * wa + u16::from(b[i]) * wb) / (wa + wb)) as u8;
    [ch(0), ch(1), ch(2), 255]
}

fn bc1_color(block: &[u8], punch_through: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    let palette = if c0 > c1 || !punch_through {
        [
            [p0[0], p0[1], p0[2], 255],
            [p1[0], p1[1], p1[2], 255],
            mix(p0, p1, 2, 1),
            mix(p0, p1, 1, 2),
        ]
    } else {
        [
            [p0[0], p0[1], p0[2], 255],
            [p1[0], p1[1], p1[2], 255],
            mix(p0, p1, 1, 1),
            [0, 0, 0, 0],
        ]
    };
    let indices = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    std::array::from_fn(|i| palette[((indices >> (2 * i)) & 3) as usize])
}

fn bc2_alpha(block: &[u8]) -> [u8; 16] {
    let bits = u64::from_le_bytes(block[..8].try_into().unwrap_or_default());
    std::array::from_fn(|i| ((bits >> (4 * i)) & 0xf) as u8 * 17)
}

fn bc3_alpha(block: &[u8]) -> [u8; 16] {
    let (a0, a1) = (u16::from(block[0]), u16::from(block[1]));
    let table: [u8; 8] = if a0 > a1 {
        std::array::from_fn(|i| match i {
            0 => a0 as u8,
            1 => a1 as u8,
            _ => {
                let i = i as u16 - 1;
                (((7 - i) * a0 + i * a1) / 7) as u8
            }
        })
    } else {
        std::array::from_fn(|i| match i {
            0 => a0 as u8,
            1 => a1 as u8,
            6 => 0,
            7 => 255,
            _ => {
                let i = i as u16 - 1;
                (((5 - i) * a0 + i * a1) / 5) as u8
            }
        })
    };
    let mut bits = [0u8; 8];
    bits[..6].copy_from_slice(&block[2..8]);
    let bits = u64::from_le_bytes(bits);
    std::array::from_fn(|i| table[((bits >> (3 * i)) & 7) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_expands_to_full_range() {
        assert_eq!(rgb565(0xffff), [255, 255, 255]);
        assert_eq!(rgb565(0x0000), [0, 0, 0]);
        assert_eq!(rgb565(0xf800), [255, 0, 0]);
    }

    #[test]
    fn bc1_solid_block() {
        // c0 = red, c1 = black, all indices 0 → every texel red.
        let block = [0x00, 0xf8, 0x00, 0x00, 0, 0, 0, 0];
        let out = decode_bc(&block, 4, 4, BcKind::Bc1);
        assert!(
            out.as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [255, 0, 0, 255])
        );
    }

    #[test]
    fn bc1_punch_through_alpha() {
        // c0 <= c1 and index 3 → transparent black.
        let block = [0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        let out = decode_bc(&block, 4, 4, BcKind::Bc1);
        assert!(out.as_chunks::<4>().0.iter().all(|p| *p == [0, 0, 0, 0]));
    }

    #[test]
    fn bc3_alpha_endpoints_and_interpolation() {
        // a0 = 255, a1 = 0, first texel index 0, second index 1, third index 2.
        let mut block = [0u8; 8];
        block[0] = 255;
        block[1] = 0;
        // indices: t0=0, t1=1, t2=2 → bits 000 001 010 = 0b010_001_000
        block[2] = 0b1000_1000;
        let alpha = bc3_alpha(&block);
        assert_eq!(alpha[0], 255);
        assert_eq!(alpha[1], 0);
        assert_eq!(alpha[2], (6 * 255 / 7) as u8);
    }

    #[test]
    fn bc2_explicit_alpha() {
        let block = [0xf0, 0, 0, 0, 0, 0, 0, 0];
        let alpha = bc2_alpha(&block);
        assert_eq!(alpha[0], 0);
        assert_eq!(alpha[1], 255);
    }

    #[test]
    fn partial_blocks_are_cropped() {
        let block = [0xff, 0xff, 0x00, 0x00, 0, 0, 0, 0];
        let out = decode_bc(&block, 2, 3, BcKind::Bc1);
        assert_eq!(out.len(), 2 * 3 * 4);
        assert!(
            out.as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [255, 255, 255, 255])
        );
    }
}
