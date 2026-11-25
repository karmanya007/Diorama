//! What effects do: exact colours at known pixels. Skipped without any GPU
//! adapter; needs no DXC.

#![allow(clippy::unwrap_used, reason = "test")]

mod support;

use serde_json::{Value, json};
use support::{Host, MIDDLE, Picture, System, draw, draw_at};

/// A white 16×16 square in the middle of black, with `effect` over it all.
fn over_square(effect: Value, time: f32) -> Option<Picture> {
    let layers = json!([
        {"type": "color", "size": 16},
        {"type": "adjustment", "effects": [effect]},
    ]);
    draw_at("#000", layers, MIDDLE, time)
}

/// The whole picture in `color` with `effect`.
fn filled(color: &str, effect: Value, time: f32) -> Option<Picture> {
    let layers = json!([{"type": "color", "color": color, "effects": [effect]}]);
    draw_at("#000", layers, MIDDLE, time)
}

/// Every effect at settings that do nothing.
fn neutral() -> Vec<Value> {
    let effects = json!([
        {"type": "blur", "radius": 0},
        {"type": "glow", "strength": 0},
        {"type": "tint", "amount": 0},
        {"type": "adjust"},
        {"type": "vignette", "amount": 0},
        {"type": "grain", "amount": 0},
        {"type": "shift", "amount": 0},
        {"type": "pixelate", "size": 1},
        {"type": "drift", "speed": 0},
        {"type": "sway", "amount": 0},
        {"type": "ripple", "amount": 0},
        {"type": "shake", "amount": 0},
        {"type": "pulse", "amount": 0},
        {"type": "mask", "image": "assets/swatch.png", "amount": 0},
        {"type": "fade", "start": 1, "end": 1},
    ]);
    effects.as_array().unwrap().clone()
}

#[test]
fn effects_that_do_nothing_change_nothing() {
    let named: Vec<String> = neutral()
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_owned())
        .collect();
    for e in diorama_project::effects() {
        assert!(named.contains(&e.name), "{} is not tested", e.name);
    }
    let layers = |effects: &[Value], blend: &str| {
        json!([
            {"type": "color", "color": "#246", "size": [32, 32], "position": [-16, 0]},
            {"type": "image", "source": "assets/swatch.png", "smooth": false, "scale": 8,
             "opacity": 0.8, "blend": blend, "effects": effects},
        ])
    };
    for blend in ["normal", "add", "multiply", "overlay"] {
        let Some(without) = draw("#642", layers(&[], blend), MIDDLE) else {
            return;
        };
        let all = draw("#642", layers(&neutral(), blend), MIDDLE).unwrap();
        // Fifteen passes of 8-bit rounding, divided by alpha.
        let d = all.difference(&without);
        assert!(d <= 4, "all, {blend}: {d}");
        for effect in neutral() {
            let with = draw("#642", layers(std::slice::from_ref(&effect), blend), MIDDLE).unwrap();
            let d = with.difference(&without);
            assert!(d <= 3, "{effect}, {blend}: {d}");
        }
    }
}

#[test]
fn hidden_effects_do_nothing() {
    let hidden = json!({"type": "tint", "color": "#00f", "amount": 1, "visible": false});
    let Some(p) = filled("#f00", hidden, 0.0) else {
        return;
    };
    p.assert(32, 16, [255, 0, 0], "hidden tint");
}

#[test]
fn blur_spreads_softly_or_keeps_the_outline() {
    let square = |edges: &str| {
        let layers = json!([{"type": "color", "size": 16, "effects": [
            {"type": "blur", "radius": 6, "edges": edges}]}]);
        draw("#000", layers, MIDDLE)
    };
    let Some(soft) = square("soft") else {
        return;
    };
    let gray = |p: &Picture, x: usize| p.at(x, 16)[0];
    assert!(gray(&soft, 32) > 245, "middle {}", gray(&soft, 32));
    // Half way at the outline (between pixels 39 and 40), falling outwards.
    let edge = (u32::from(gray(&soft, 39)) + u32::from(gray(&soft, 40))) / 2;
    assert!((115..=140).contains(&edge), "edge {edge}");
    assert!(gray(&soft, 42) > 20 && gray(&soft, 42) < 110);
    assert!(gray(&soft, 48) < 4);
    assert_eq!(soft.at(0, 0), [0, 0, 0]);
    // Both axes alike.
    assert!(soft.at(32, 24)[0].abs_diff(soft.at(40, 16)[0]) <= 3);
    let hard = square("hard").unwrap();
    hard.assert(38, 16, [255, 255, 255], "inside");
    hard.assert(41, 16, [0, 0, 0], "outside");
}

#[test]
fn wide_blurs_work_in_small_copies() {
    // Sigma 20: a quarter of the size, or less.
    let layers =
        json!([{"type": "color", "size": [16, 32], "effects": [{"type": "blur", "radius": 40}]}]);
    let Some(p) = draw("#000", layers, MIDDLE) else {
        return;
    };
    let at = |x: usize| p.at(x, 16)[0];
    assert!(at(32) > at(44) && at(44) > at(56) && at(56) > 0);
    assert!(at(20).abs_diff(at(43)) <= 6, "{} {}", at(20), at(43));
}

#[test]
fn glow_shines_from_bright_parts_only() {
    let square = |color: &str| {
        let layers = json!([{"type": "color", "color": color, "size": 12, "effects": [
            {"type": "glow", "radius": 8, "strength": 1, "threshold": 0.5, "color": "#f80"}]}]);
        draw("#000", layers, MIDDLE)
    };
    let Some(bright) = square("#fff") else {
        return;
    };
    bright.assert(32, 16, [255, 255, 255], "the square itself");
    let [r, g, b] = bright.at(40, 16);
    assert!(r > 30 && g > 10 && b < 3 && r > g, "{:?}", [r, g, b]);
    let dark = square("#444").unwrap();
    dark.assert(32, 16, [68, 68, 68], "a dark square");
    dark.assert(40, 16, [0, 0, 0], "beside it");
}

#[test]
fn colours() {
    let tint = json!({"type": "tint", "color": "#0000ff", "amount": 1});
    let Some(p) = filled("#f00", tint, 0.0) else {
        return;
    };
    p.assert(32, 16, [0, 0, 54], "red's luma in blue");
    let adjust = |color: &str, settings: Value| {
        let mut e = settings;
        e["type"] = json!("adjust");
        filled(color, e, 0.0).unwrap().at(32, 16)
    };
    let near = |got: [u8; 3], rgb: [u8; 3], what: &str| {
        let near = got.iter().zip(&rgb).all(|(a, b)| a.abs_diff(*b) <= 2);
        assert!(near, "{what}: {got:?}, not {rgb:?}");
    };
    near(
        adjust("#804020", json!({"brightness": 0.5})),
        [192, 96, 48],
        "brightness",
    );
    near(
        adjust("#40c080", json!({"contrast": 1})),
        [0, 255, 128],
        "contrast",
    );
    near(
        adjust("#ff0000", json!({"saturation": -1})),
        [54, 54, 54],
        "grey",
    );
    near(adjust("#ff8000", json!({"hue": 120})), [0, 255, 128], "hue");
    // At half alpha over black: settings act on the colour, not its shade.
    near(
        adjust("#40c08080", json!({"contrast": 1})),
        [0, 128, 64],
        "contrast, half transparent",
    );
}

#[test]
fn vignette_keeps_the_middle() {
    let effect =
        json!({"type": "vignette", "color": "#00f", "amount": 1, "size": 0.5, "softness": 0.4});
    let Some(p) = filled("#fff", effect, 0.0) else {
        return;
    };
    p.assert(32, 16, [255, 255, 255], "middle");
    p.assert(0, 0, [0, 0, 255], "corner");
    p.assert(63, 31, [0, 0, 255], "other corner");
}

#[test]
fn grain_is_even_and_moves_when_asked() {
    let grain = |speed: f32, time: f32| {
        let effect = json!({"type": "grain", "amount": 0.2, "size": 1, "speed": speed});
        filled("#808080", effect, time)
    };
    let Some(still) = grain(0.0, 0.0) else {
        return;
    };
    let values: Vec<f32> = (0..32)
        .flat_map(|y| (0..64).map(move |x| (x, y)))
        .map(|(x, y)| f32::from(still.at(x, y)[0]))
        .collect();
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let spread = values.iter().map(|v| (v - mean).abs()).sum::<f32>() / values.len() as f32;
    assert!((mean - 128.0).abs() < 3.0, "mean {mean}");
    assert!((18.0..34.0).contains(&spread), "spread {spread}");
    assert!(still == grain(0.0, 5.0).unwrap());
    assert!(grain(10.0, 0.0).unwrap() != grain(10.0, 5.0).unwrap());
}

#[test]
fn shift_parts_red_and_blue() {
    let effect = json!({"type": "shift", "amount": 4, "direction": 0});
    let Some(p) = over_square(effect, 0.0) else {
        return;
    };
    p.assert(32, 16, [255, 255, 255], "middle");
    p.assert(42, 16, [255, 0, 0], "red to the right");
    p.assert(21, 16, [0, 0, 255], "blue to the left");
    p.assert(46, 16, [0, 0, 0], "beyond");
    let down = json!({"type": "shift", "amount": 4, "direction": 90});
    over_square(down, 0.0)
        .unwrap()
        .assert(32, 26, [255, 0, 0], "red below");
}

#[test]
fn pixelate_makes_squares() {
    let effect = json!({"type": "pixelate", "size": 16});
    let Some(p) = over_square(effect, 0.0) else {
        return;
    };
    // Cells of 16: their middles are at 24 and 40 (the square is 24–40) across.
    for x in [17, 24, 31] {
        assert_eq!(p.at(x, 12), p.at(24, 8), "cell at {x}");
    }
    p.assert(8, 8, [0, 0, 0], "outside");
}

#[test]
fn drift_moves_and_starts_over() {
    let drift = |direction: f32, time: f32| {
        over_square(
            json!({"type": "drift", "speed": 10, "direction": direction}),
            time,
        )
    };
    let Some(right) = drift(0.0, 1.0) else {
        return;
    };
    // 24–40 is now 34–50.
    right.assert(28, 16, [0, 0, 0], "left behind");
    right.assert(45, 16, [255, 255, 255], "moved right");
    drift(90.0, 1.0)
        .unwrap()
        .assert(32, 30, [255, 255, 255], "moved down");
    let wrapped = drift(0.0, 3.2).unwrap();
    wrapped.assert(60, 16, [255, 255, 255], "right edge");
    wrapped.assert(4, 16, [255, 255, 255], "comes back at the left");
    assert!(
        drift(0.0, 6.4)
            .unwrap()
            .difference(&drift(0.0, 0.0).unwrap())
            <= 2
    );
}

#[test]
fn sway_bends_what_is_not_held() {
    // A quarter of a beat: the far end is out by the whole amount.
    let sway = |anchor: &str| {
        let effect =
            json!({"type": "sway", "amount": 8, "speed": 0.25, "length": 5000, "anchor": anchor});
        let layers = json!([
            {"type": "color", "size": [8, 32]},
            {"type": "adjustment", "effects": [effect]},
        ]);
        draw_at("#000", layers, MIDDLE, 1.0)
    };
    let Some(p) = sway("bottom") else {
        return;
    };
    p.assert(30, 31, [255, 255, 255], "held at the bottom");
    p.assert(30, 0, [0, 0, 0], "gone from the top");
    p.assert(42, 0, [255, 255, 255], "bent to the right");
    let p = sway("top").unwrap();
    p.assert(30, 0, [255, 255, 255], "held at the top");
    p.assert(42, 31, [255, 255, 255], "bent at the bottom");
    let p = sway("none").unwrap();
    p.assert(42, 16, [255, 255, 255], "all of it moves");
    p.assert(30, 16, [0, 0, 0], "all of it moves");
}

#[test]
fn ripple_and_shake_stay_near() {
    for effect in [
        json!({"type": "ripple", "amount": 3, "length": 12, "speed": 0.5}),
        json!({"type": "shake", "amount": 3, "speed": 2}),
    ] {
        let Some(a) = over_square(effect.clone(), 0.4) else {
            return;
        };
        let b = over_square(effect.clone(), 1.3).unwrap();
        assert!(a != b, "{effect} does not move");
        for p in [&a, &b] {
            p.assert(32, 16, [255, 255, 255], "middle");
            p.assert(20, 16, [0, 0, 0], "left of it");
            p.assert(44, 16, [0, 0, 0], "right of it");
            p.assert(32, 3, [0, 0, 0], "above it");
        }
    }
}

#[test]
fn pulse_beats() {
    // Half a beat: at its peak.
    let pulse = |what: &str, color: &str, time: f32| {
        let effect = json!({"type": "pulse", "what": what, "amount": 0.5, "speed": 0.5});
        filled(color, effect, time)
    };
    let Some(p) = pulse("brightness", "#404040", 1.0) else {
        return;
    };
    p.assert(32, 16, [96, 96, 96], "brighter");
    pulse("brightness", "#404040", 2.0)
        .unwrap()
        .assert(32, 16, [64, 64, 64], "after a beat");
    pulse("opacity", "#ffffff", 1.0)
        .unwrap()
        .assert(32, 16, [128, 128, 128], "fainter");
    let zoom = json!({"type": "pulse", "what": "zoom", "amount": 0.5, "speed": 0.5});
    let p = over_square(zoom, 1.0).unwrap();
    // 8 pixels from the middle become 12.
    p.assert(42, 16, [255, 255, 255], "grown");
    p.assert(46, 16, [0, 0, 0], "outside");
}

#[test]
fn masks_follow_brightness_and_alpha() {
    let mask = |invert: bool| {
        let effect = json!({"type": "mask", "image": "assets/swatch.png", "invert": invert});
        filled("#fff", effect, 0.0)
    };
    let Some(p) = mask(false) else {
        return;
    };
    // The swatch spans the layer: columns of 16, rows of 16; smooth between.
    // Pixel middles are half a pixel past the swatch's.
    p.assert(8, 4, [58, 58, 58], "red");
    p.assert(24, 4, [177, 177, 177], "green");
    p.assert(63, 0, [255, 255, 255], "white");
    p.assert(63, 31, [128, 128, 128], "half transparent white");
    mask(true).unwrap().assert(63, 0, [0, 0, 0], "inverted");
}

#[test]
fn fade_goes_towards_its_side() {
    let fade = |direction: f32| {
        filled(
            "#fff",
            json!({"type": "fade", "direction": direction, "start": 0, "end": 1}),
            0.0,
        )
    };
    let Some(p) = fade(90.0) else {
        return;
    };
    assert!(p.at(32, 0)[0] > 250 && p.at(32, 31)[0] < 5);
    assert!(p.at(32, 16)[0].abs_diff(128) < 8);
    let p = fade(180.0).unwrap();
    assert!(p.at(0, 16)[0] < 5 && p.at(63, 16)[0] > 250);
}

#[test]
fn room_for_one_effect_is_not_part_of_the_picture_for_the_next() {
    // Blur adds room on each side; the fade still spans the square alone.
    let layers = json!([{"type": "color", "size": 16, "effects": [
        {"type": "blur", "radius": 4},
        {"type": "fade", "direction": 0, "start": 0.5, "end": 0.5},
    ]}]);
    let Some(p) = draw("#000", layers, MIDDLE) else {
        return;
    };
    assert!(p.at(29, 16)[0] > 250, "left half");
    p.assert(34, 16, [0, 0, 0], "right half");
}

#[test]
fn adjustments_change_what_is_below_them_only() {
    let layers = json!([
        {"type": "color", "color": "#f00", "size": [32, 32], "position": [-16, 0]},
        {"type": "adjustment", "opacity": 0.5, "effects": [{"type": "adjust", "saturation": -1}]},
        {"type": "color", "color": "#f00", "size": [32, 32], "position": [16, 0]},
    ]);
    let Some(p) = draw("#000", layers, MIDDLE) else {
        return;
    };
    p.assert(8, 16, [155, 27, 27], "half grey");
    p.assert(56, 16, [255, 0, 0], "above the adjustment");
}

#[test]
fn text_takes_effects() {
    if System.default_font().is_none() {
        eprintln!("no font; skipping");
        return;
    }
    let text = |effects: Value| {
        let layers = json!([{"type": "text", "text": "HH", "font-size": 20, "effects": effects}]);
        draw("#000", layers, MIDDLE)
    };
    let Some(plain) = text(json!([])) else {
        return;
    };
    let same = text(json!([{"type": "tint", "amount": 0}])).unwrap();
    // Drawn through a target: edges may land a little differently.
    let sum = |p: &Picture| p.0.iter().map(|v| u32::from(*v)).sum::<u32>();
    assert!(sum(&plain).abs_diff(sum(&same)) < sum(&plain) / 20);
    let glowing = text(json!([{"type": "glow", "radius": 12, "strength": 3}])).unwrap();
    assert!(sum(&glowing) > sum(&plain) * 11 / 10);
}

#[test]
fn what_does_not_change_is_not_drawn_again() {
    let layer = |depth: f32, effects: Value| json!([{"type": "color", "color": "#f00", "size": 16, "depth": depth, "effects": effects}]);
    let still = layer(
        0.0,
        json!([{"type": "blur"}, {"type": "grain", "speed": 0}, {"type": "sway", "speed": 0}, {"type": "ripple", "amount": 0}]),
    );
    let (first, later) = ((1.0, MIDDLE), (9.0, MIDDLE));
    let Some(idle) = support::idle(&still, first, later) else {
        return;
    };
    assert!(idle, "nothing moves");
    let moving = layer(0.0, json!([{"type": "sway"}]));
    assert_eq!(support::idle(&moving, first, later), Some(false), "sway");
    // Layers with depth follow the cursor, and rest with it.
    let deep = layer(1.0, json!([]));
    assert_eq!(support::idle(&deep, first, later), Some(true));
    assert_eq!(
        support::idle(&deep, first, (9.0, [0.9, 0.5])),
        Some(false),
        "cursor moved"
    );
}
