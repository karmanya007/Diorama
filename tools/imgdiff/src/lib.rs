//! Image comparison for golden tests: SSIM on luma (8×8 windows, stride 4)
//! and PSNR, plus the cover-fit crop WE applies when presenting a scene.

use image::{RgbaImage, imageops};

/// Scales `img` to cover `width × height` and crops the centre, like WE's
/// scene presentation and Diorama's `FitMode::Fill`.
pub fn cover(img: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    let scale = f64::max(
        f64::from(width) / f64::from(img.width()),
        f64::from(height) / f64::from(img.height()),
    );
    let sw = ((f64::from(img.width()) * scale).round() as u32).max(width);
    let sh = ((f64::from(img.height()) * scale).round() as u32).max(height);
    let scaled = imageops::resize(img, sw, sh, imageops::FilterType::Triangle);
    imageops::crop_imm(&scaled, (sw - width) / 2, (sh - height) / 2, width, height).to_image()
}

fn luma(img: &RgbaImage) -> Vec<f64> {
    img.pixels()
        .map(|p| 0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]))
        .collect()
}

/// Mean SSIM over 8×8 windows; images must have equal size.
pub fn ssim(a: &RgbaImage, b: &RgbaImage) -> f64 {
    const WIN: u32 = 8;
    const STEP: u32 = 4;
    let (c1, c2) = ((0.01f64 * 255.0).powi(2), (0.03f64 * 255.0).powi(2));
    let (w, h) = a.dimensions();
    if (w, h) != b.dimensions() || w < WIN || h < WIN {
        return 0.0;
    }
    let (la, lb) = (luma(a), luma(b));
    let n = f64::from(WIN * WIN);
    let (mut total, mut count) = (0.0, 0u64);
    for y in (0..=h - WIN).step_by(STEP as usize) {
        for x in (0..=w - WIN).step_by(STEP as usize) {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for yy in y..y + WIN {
                let row = (yy * w) as usize;
                for xx in x..x + WIN {
                    let (va, vb) = (la[row + xx as usize], lb[row + xx as usize]);
                    sa += va;
                    sb += vb;
                    saa += va * va;
                    sbb += vb * vb;
                    sab += va * vb;
                }
            }
            let (ma, mb) = (sa / n, sb / n);
            let (va, vb) = (saa / n - ma * ma, sbb / n - mb * mb);
            let cov = sab / n - ma * mb;
            total += ((2.0 * ma * mb + c1) * (2.0 * cov + c2))
                / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            count += 1;
        }
    }
    total / count.max(1) as f64
}

/// PSNR over RGB in dB (`inf` for identical images).
pub fn psnr(a: &RgbaImage, b: &RgbaImage) -> f64 {
    let mut se = 0.0;
    let mut n = 0u64;
    for (p, q) in a.pixels().zip(b.pixels()) {
        for c in 0..3 {
            let d = f64::from(p[c]) - f64::from(q[c]);
            se += d * d;
            n += 1;
        }
    }
    let mse = se / n.max(1) as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

/// Absolute difference ×4, for eyeballing where images disagree.
pub fn diff_image(a: &RgbaImage, b: &RgbaImage) -> RgbaImage {
    RgbaImage::from_fn(a.width(), a.height(), |x, y| {
        let (p, q) = (a.get_pixel(x, y), b.get_pixel(x, y));
        let d = |c: usize| {
            (i16::from(p[c]) - i16::from(q[c]))
                .unsigned_abs()
                .saturating_mul(4)
                .min(255) as u8
        };
        image::Rgba([d(0), d(1), d(2), 255])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_images_score_perfectly() {
        let img = RgbaImage::from_fn(32, 32, |x, y| {
            image::Rgba([(x * 8) as u8, (y * 8) as u8, 0, 255])
        });
        assert!((ssim(&img, &img) - 1.0).abs() < 1e-9);
        assert!(psnr(&img, &img).is_infinite());
        let mut other = img.clone();
        other.put_pixel(3, 3, image::Rgba([255, 255, 255, 255]));
        assert!(ssim(&img, &other) < 1.0);
    }

    #[test]
    fn cover_crops_to_target() {
        let img = RgbaImage::new(3840, 2160);
        assert_eq!(cover(&img, 3000, 1920).dimensions(), (3000, 1920));
    }
}
