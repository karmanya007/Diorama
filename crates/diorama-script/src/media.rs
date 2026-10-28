//! Media integration: the colours WE derives from the now-playing thumbnail
//! (`MediaThumbnailEvent`). WE's exact palette algorithm is unknown; this
//! takes the most common colours of a coarse histogram, keeps them apart,
//! and picks a readable text colour by WCAG contrast.

/// Colours of a thumbnail, normalised RGB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThumbnailColors {
    pub primary: [f32; 3],
    pub secondary: [f32; 3],
    pub tertiary: [f32; 3],
    /// Secondary or tertiary when readable on primary, else `high_contrast`.
    pub text: [f32; 3],
    /// Black or white, whichever contrasts more with primary.
    pub high_contrast: [f32; 3],
}

/// Minimum distance (RGB, 0..1 per channel) between palette colours.
const SEPARATION: f32 = 0.2;

fn luminance(c: [f32; 3]) -> f32 {
    let lin = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

fn contrast(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Palette of tightly packed RGBA pixels (transparent pixels ignored).
pub fn thumbnail_colors(rgba: &[u8]) -> ThumbnailColors {
    // 4 bits per channel: average colour and count per bucket.
    let mut buckets = vec![([0u64; 3], 0u64); 4096];
    for p in rgba.as_chunks::<4>().0.iter().filter(|p| p[3] >= 128) {
        let i =
            (usize::from(p[0] >> 4) << 8) | (usize::from(p[1] >> 4) << 4) | usize::from(p[2] >> 4);
        let b = &mut buckets[i];
        for (sum, v) in b.0.iter_mut().zip(p) {
            *sum += u64::from(*v);
        }
        b.1 += 1;
    }
    let mut ranked: Vec<([f32; 3], u64)> = buckets
        .into_iter()
        .filter(|b| b.1 > 0)
        .map(|(sum, n)| (sum.map(|s| s as f32 / n as f32 / 255.0), n))
        .collect();
    ranked.sort_by_key(|b| std::cmp::Reverse(b.1));
    let mut palette: Vec<[f32; 3]> = Vec::new();
    for (c, _) in &ranked {
        if palette.iter().all(|p| distance(*p, *c) >= SEPARATION) {
            palette.push(*c);
            if palette.len() == 3 {
                break;
            }
        }
    }
    let primary = palette.first().copied().unwrap_or([0.0; 3]);
    let secondary = palette.get(1).copied().unwrap_or(primary);
    let tertiary = palette.get(2).copied().unwrap_or(secondary);
    let high_contrast = if contrast(primary, [1.0; 3]) >= contrast(primary, [0.0; 3]) {
        [1.0; 3]
    } else {
        [0.0; 3]
    };
    let text = [secondary, tertiary]
        .into_iter()
        .find(|c| contrast(primary, *c) >= 4.5)
        .unwrap_or(high_contrast);
    ThumbnailColors {
        primary,
        secondary,
        tertiary,
        text,
        high_contrast,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_ranks_by_area_and_picks_readable_text() {
        // 60 % dark blue, 30 % white, 10 % red.
        let mut img = Vec::new();
        for i in 0..100 {
            let px = match i {
                0..60 => [10, 20, 90, 255],
                60..90 => [250, 250, 250, 255],
                _ => [220, 30, 30, 255],
            };
            img.extend_from_slice(&px);
        }
        let c = thumbnail_colors(&img);
        assert!(c.primary[2] > 0.3 && c.primary[0] < 0.1, "{c:?}");
        assert!(c.secondary.iter().all(|v| *v > 0.9), "{c:?}");
        assert!(c.tertiary[0] > 0.8 && c.tertiary[1] < 0.2, "{c:?}");
        assert_eq!(c.text, c.secondary);
        assert_eq!(c.high_contrast, [1.0; 3]);
    }

    #[test]
    fn empty_thumbnail_is_black() {
        let c = thumbnail_colors(&[]);
        assert_eq!(c.primary, [0.0; 3]);
        assert_eq!(c.high_contrast, [1.0; 3]);
    }
}
