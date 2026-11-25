//! Image files as scene textures.

use std::io::Cursor;

use diorama_core::Limits;
use diorama_scene::Texture;
use image::{DynamicImage, ImageDecoder, ImageReader};

/// Decodes an image file into a texture, with mip levels if it is `smooth`.
pub(crate) fn decode(
    name: &str,
    data: &[u8],
    smooth: bool,
    limits: &Limits,
) -> Result<Texture, String> {
    let error = |e: image::ImageError| e.to_string();
    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut decoder = reader.into_decoder().map_err(error)?;
    let mut allowed = image::Limits::default();
    allowed.max_image_width = Some(limits.max_texture_dim);
    allowed.max_image_height = Some(limits.max_texture_dim);
    allowed.max_alloc = Some(limits.max_decompressed_bytes);
    decoder.set_limits(allowed).map_err(error)?;
    let (w, h) = decoder.dimensions();
    if u64::from(w) * u64::from(h) * 4 > limits.max_decompressed_bytes {
        return Err("image too large".into());
    }
    // Photographs may be stored turned.
    let orientation = decoder.orientation().map_err(error)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(error)?;
    image.apply_orientation(orientation);
    let image = image.into_rgba8();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Err("empty image".into());
    }
    let rgba = image.into_raw();
    let mips = if smooth {
        mips(&rgba, width, height)
    } else {
        Vec::new()
    };
    Ok(Texture {
        name: name.to_owned(),
        width,
        height,
        image_width: width,
        image_height: height,
        rgba,
        mips,
        clamp: true,
        nearest: !smooth,
        frames: Vec::new(),
        video: None,
    })
}

/// Levels below the first, each half the size of the one before.
fn mips(rgba: &[u8], width: u32, height: u32) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let (mut w, mut h) = (width, height);
    while w > 1 || h > 1 {
        let next = halve(out.last().map_or(rgba, Vec::as_slice), w, h);
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
        out.push(next);
    }
    out
}

/// Averages 2×2 blocks; colours weigh by alpha, so what is transparent does
/// not bleed into the edges.
fn halve(src: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = Vec::with_capacity(nw as usize * nh as usize * 4);
    for y in 0..nh {
        for x in 0..nw {
            let (mut color, mut alpha) = ([0u32; 3], 0u32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = ((2 * x + dx).min(w - 1), (2 * y + dy).min(h - 1));
                let i = (sy as usize * w as usize + sx as usize) * 4;
                let a = u32::from(src[i + 3]);
                for (c, v) in color.iter_mut().zip(&src[i..i + 3]) {
                    *c += u32::from(*v) * a;
                }
                alpha += a;
            }
            for c in color {
                out.push(((c + alpha / 2) / alpha.max(1)) as u8);
            }
            out.push(((alpha + 2) / 4) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::write_buffer_with_format(
            &mut out,
            rgba,
            width,
            height,
            image::ExtendedColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .unwrap();
        out.into_inner()
    }

    #[test]
    fn images_decode_with_mips() {
        let data = png(3, 2, &[255, 0, 0, 255].repeat(6));
        let t = decode("a.png", &data, true, &Limits::default()).unwrap();
        assert_eq!((t.width, t.height, t.rgba.len()), (3, 2, 24));
        assert_eq!(t.mips, [vec![255, 0, 0, 255]]);
        let t = decode("a.png", &data, false, &Limits::default()).unwrap();
        assert!(t.nearest && t.mips.is_empty());
    }

    #[test]
    fn transparent_pixels_do_not_darken_edges() {
        let src = [200, 100, 50, 255, 0, 0, 0, 0];
        assert_eq!(halve(&src, 2, 1), [200, 100, 50, 128]);
        assert_eq!(halve(&[9, 9, 9, 0].repeat(4), 2, 2), [0, 0, 0, 0]);
    }

    #[test]
    fn large_and_broken_images_are_refused() {
        let limits = Limits {
            max_texture_dim: 2,
            ..Limits::default()
        };
        let data = png(3, 2, &[255; 24]);
        assert!(decode("a.png", &data, true, &limits).is_err());
        let limits = Limits {
            max_decompressed_bytes: 20,
            ..Limits::default()
        };
        assert!(decode("a.png", &data, true, &limits).is_err());
        assert!(decode("a.png", &data[..20], true, &Limits::default()).is_err());
        assert!(decode("a.png", b"not an image", true, &Limits::default()).is_err());
    }
}
