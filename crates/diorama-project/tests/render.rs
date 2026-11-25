//! What documents look like: exact colours at known pixels. Skipped without
//! any GPU adapter; needs no DXC.

#![allow(clippy::unwrap_used, reason = "test")]

mod support;

use std::path::Path;

use diorama_core::Limits;
use diorama_project::Values;
use serde_json::json;
use support::{Host, MIDDLE, System, draw};

#[test]
fn images_are_upright_placed_and_turned() {
    let layers = json!([
        {"type": "image", "source": "assets/swatch.png", "smooth": false, "position": [-16, 0], "scale": 4},
        // Clockwise: what was left is now on top, so the opaque row is on the right.
        {"type": "image", "source": "assets/swatch.png", "smooth": false, "position": [16, 0], "scale": 4,
         "rotation": 90},
    ]);
    let Some(p) = draw("#000", layers, MIDDLE) else {
        return;
    };
    // 16×8 pixels around (16, 16): red top left, white bottom right at half.
    p.assert(10, 14, [255, 0, 0], "top left");
    p.assert(14, 14, [0, 255, 0], "second");
    p.assert(22, 14, [255, 255, 255], "top right");
    p.assert(10, 18, [128, 0, 0], "bottom left, half transparent");
    p.assert(22, 18, [128, 128, 128], "bottom right, half transparent");
    p.assert(5, 16, [0, 0, 0], "outside");
    p.assert(16, 10, [0, 0, 0], "above");
    // 8×16 around (48, 16).
    p.assert(50, 10, [255, 0, 0], "turned: red on top, opaque right");
    p.assert(46, 10, [128, 0, 0], "turned: half transparent left");
    p.assert(50, 22, [255, 255, 255], "turned: white at the bottom");
}

#[test]
fn opacity_tint_and_size() {
    let layers = json!([
        {"type": "color", "color": "#ffffff", "opacity": 0.2, "size": [32, 32], "position": [-16, 0]},
        {"type": "color", "color": "#ff000080", "size": [32, 32], "position": [16, 0]},
        {"type": "color", "color": "#00ff00", "size": [4, 4], "position": [30, -14], "visible": false},
        {"type": "image", "source": "assets/swatch.png", "smooth": false, "size": [16, 8], "tint": "#333",
         "position": [24, 12]},
    ]);
    let Some(p) = draw("#000", layers, MIDDLE) else {
        return;
    };
    p.assert(8, 16, [51, 51, 51], "white at a fifth");
    p.assert(40, 8, [128, 0, 0], "red at half");
    p.assert(62, 2, [128, 0, 0], "hidden layer");
    p.assert(62, 26, [51, 51, 51], "tinted white, opaque row");
}

#[test]
fn blend_modes_follow_their_formulas() {
    // Below: 102, 204, 51 (0.4, 0.8, 0.2). Layer: 0.5, 0.25, 0.75.
    let mix = |mode: &str, opacity: f32| {
        let layers = json!([
            {"type": "color", "color": [0.5, 0.25, 0.75], "blend": mode, "opacity": opacity},
        ]);
        draw("#66cc33", layers, MIDDLE).map(|p| p.at(32, 16))
    };
    let (a, b) = ([0.4f32, 0.8, 0.2], [0.5f32, 0.25, 0.75]);
    let over = |a: f32, b: f32| {
        if a < 0.5 {
            2.0 * a * b
        } else {
            1.0 - 2.0 * (1.0 - a) * (1.0 - b)
        }
    };
    let soft = |a: f32, b: f32| {
        if b <= 0.5 {
            return a - (1.0 - 2.0 * b) * a * (1.0 - a);
        }
        let d = if a <= 0.25 {
            ((16.0 * a - 12.0) * a + 4.0) * a
        } else {
            a.sqrt()
        };
        a + (2.0 * b - 1.0) * (d - a)
    };
    type Formula<'f> = &'f dyn Fn(f32, f32) -> f32;
    let modes: [(&str, Formula<'_>); 10] = [
        ("normal", &|_, b| b),
        ("add", &|a, b| (a + b).min(1.0)),
        ("multiply", &|a, b| a * b),
        ("screen", &|a, b| a + b - a * b),
        ("overlay", &over),
        ("darken", &f32::min),
        ("lighten", &f32::max),
        ("hard-light", &|a, b| over(b, a)),
        ("soft-light", &soft),
        ("difference", &|a, b| (a - b).abs()),
    ];
    for (name, f) in modes {
        for opacity in [1.0f32, 0.5] {
            let Some(got) = mix(name, opacity) else {
                return;
            };
            let expected: [u8; 3] = std::array::from_fn(|i| {
                let full = f(a[i], b[i]);
                let v = if name == "add" {
                    (a[i] + b[i] * opacity).min(1.0)
                } else {
                    a[i] + (full - a[i]) * opacity
                };
                (v * 255.0).round() as u8
            });
            let near = got.iter().zip(&expected).all(|(a, b)| a.abs_diff(*b) <= 2);
            assert!(near, "{name} at {opacity}: {got:?}, not {expected:?}");
        }
    }
}

#[test]
fn blended_layers_see_what_is_right_below_them() {
    let layers = json!([
        {"type": "color", "color": "#f00", "size": [32, 32], "position": [-16, 0]},
        {"type": "color", "color": "#0f0", "size": [32, 32], "position": [16, 0]},
        {"type": "color", "color": "#fff", "blend": "difference", "size": [64, 16], "position": [0, -8]},
    ]);
    let Some(p) = draw("#000", layers, MIDDLE) else {
        return;
    };
    p.assert(8, 4, [0, 255, 255], "red inverted");
    p.assert(56, 4, [255, 0, 255], "green inverted");
    p.assert(8, 28, [255, 0, 0], "below the blended layer");
}

#[test]
fn depth_moves_layers_with_the_cursor() {
    let layers = json!([
        {"type": "color", "color": "#f00", "size": [8, 8], "position": [0, -8], "depth": 1},
        {"type": "color", "color": "#0f0", "size": [8, 8], "position": [0, 8]},
        {"type": "color", "color": "#00f", "size": [4, 4], "position": [26, 0], "depth": 1},
    ]);
    let rest = draw("#000", layers.clone(), MIDDLE);
    let Some(p) = draw("#000", layers, [1.0, 0.5]) else {
        return;
    };
    // Half of 0.25 × 64, away from the cursor; what has no depth stays.
    p.assert(24, 8, [255, 0, 0], "moved left");
    p.assert(32, 8, [0, 0, 0], "where it was");
    p.assert(32, 24, [0, 255, 0], "no depth");
    // With the cursor in the middle everything is where the document says.
    let rest = rest.unwrap();
    rest.assert(57, 15, [0, 0, 255], "right of the middle");
    rest.assert(60, 15, [0, 0, 0], "and no further");
}

#[test]
fn text_is_drawn_where_it_is_anchored() {
    if System.default_font().is_none() {
        eprintln!("no font; skipping");
        return;
    }
    let layers = json!([
        {"type": "text", "text": "█", "color": "#ff0", "font-size": 24, "opacity": 0.5},
        {"type": "text", "text": "HHHH", "color": "#0ff", "font-size": 16, "align": "left",
         "vertical-align": "top", "position": [-32, -16]},
    ]);
    let Some(p) = draw("#000", layers, MIDDLE) else {
        return;
    };
    let lit = |x: std::ops::Range<usize>, y: std::ops::Range<usize>, channel: usize| {
        y.flat_map(|y| x.clone().map(move |x| (x, y)))
            .map(|(x, y)| u32::from(p.at(x, y)[channel]))
            .sum::<u32>()
    };
    assert!(lit(0..30, 0..16, 2) > 2000, "letters at the top left");
    assert_eq!(lit(0..64, 22..32, 2), 0, "and nowhere else");
    assert!(lit(20..44, 4..28, 0) > 1000, "the block in the middle");
}

#[test]
fn the_examples_use_all_their_layers() {
    if System.default_font().is_none() {
        eprintln!("no font; skipping");
        return;
    }
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/wallpapers");
    for (name, layers, moving) in [("ridges", 9, 0), ("lanterns", 19, 15)] {
        let dir = examples.join(name);
        assert!(diorama_project::is_project(&dir));
        let scene =
            diorama_project::load(&dir, &Values::new(), &System, &Limits::default()).unwrap();
        assert!(scene.skipped.is_empty(), "{name}: {:?}", scene.skipped);
        let counts = (scene.layers.len(), scene.motions.len());
        assert_eq!(counts, (layers, moving), "{name}");
    }
}

#[test]
fn installed_fonts_are_used_or_stood_in_for() {
    if System.default_font().is_none() {
        eprintln!("no font; skipping");
        return;
    }
    let skipped = |font: &str| {
        let mut files = diorama_project::Memory::default();
        let document = json!({"version": 1, "size": [64, 32], "layers": [
            {"type": "text", "text": "a", "font": font, "bold": true}]});
        files.0.insert(
            "wallpaper.json".into(),
            serde_json::to_vec(&document).unwrap(),
        );
        let scene =
            diorama_project::compile(&files, &Values::new(), &System, &Limits::default()).unwrap();
        assert_eq!(scene.layers.len(), 1, "{font}");
        scene.skipped
    };
    assert!(skipped("system:Known").is_empty());
    assert!(skipped("system:Gone")[0].contains("Gone is not installed"));
    assert!(skipped("assets/gone.ttf")[0].contains("stands in"));
}
