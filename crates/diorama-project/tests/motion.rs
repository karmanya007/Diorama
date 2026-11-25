//! Values that move: keys, sound, and that nothing is drawn while they rest.
//! Skipped without any GPU adapter; needs no DXC.

#![allow(clippy::unwrap_used, reason = "test")]

mod support;

use serde_json::{Value, json};
use support::{MIDDLE, draw_at, draw_loud, idle};

fn keys(list: Value) -> Value {
    json!({ "keys": list, "repeat": "once" })
}

/// From 0 s to 2 s, evenly.
fn line(from: Value, to: Value) -> Value {
    keys(json!([{"t": 0, "v": from, "ease": "linear"}, {"t": 2, "v": to}]))
}

#[test]
fn place_moves() {
    let layers = json!([
        {"type": "color", "color": "#f00", "size": 8, "position": line(json!([-16, 0]), json!([16, 8]))},
    ]);
    let Some(p) = draw_at("#000", layers.clone(), MIDDLE, 0.0) else {
        return;
    };
    p.assert(16, 16, [255, 0, 0], "at the start");
    let p = draw_at("#000", layers.clone(), MIDDLE, 1.0).unwrap();
    p.assert(16, 16, [0, 0, 0], "left");
    p.assert(32, 20, [255, 0, 0], "half way, down by four");
    p.assert(32, 14, [0, 0, 0], "above");
    let p = draw_at("#000", layers, MIDDLE, 50.0).unwrap();
    p.assert(48, 24, [255, 0, 0], "the end");

    let layers = json!([
        {"type": "color", "color": "#0f0", "size": [20, 4], "rotation": line(json!(0), json!(90)),
         "position": [-16, 0]},
        {"type": "color", "color": "#00f", "size": [2, 2], "scale": line(json!(1), json!([10, 5])),
         "position": [16, 0]},
    ]);
    let p = draw_at("#000", layers.clone(), MIDDLE, 0.0).unwrap();
    p.assert(8, 16, [0, 255, 0], "lies");
    p.assert(52, 16, [0, 0, 0], "small");
    let p = draw_at("#000", layers.clone(), MIDDLE, 1.0).unwrap();
    // Clockwise: the right end goes down.
    p.assert(22, 22, [0, 255, 0], "turned by 45 degrees");
    p.assert(22, 10, [0, 0, 0], "not the other way");
    let p = draw_at("#000", layers, MIDDLE, 2.0).unwrap();
    p.assert(16, 24, [0, 255, 0], "stands");
    p.assert(8, 16, [0, 0, 0], "no longer lies");
    p.assert(56, 16, [0, 0, 255], "ten times as wide");
    p.assert(48, 22, [0, 0, 0], "five times as high");
}

#[test]
fn colour_and_opacity_move_with_and_without_effects() {
    let moving = |effects: Value| {
        json!([
            {"type": "color", "color": line(json!("#f00"), json!("#00f")), "opacity": 0.5, "size": [32, 32],
             "position": [-16, 0], "effects": effects},
            {"type": "image", "source": "assets/swatch.png", "smooth": false, "size": [32, 32],
             "position": [16, 0], "opacity": line(json!(0), json!(1)), "effects": effects},
        ])
    };
    // An effect that changes nothing.
    for effects in [json!([]), json!([{"type": "adjust"}])] {
        let Some(p) = draw_at("#000", moving(effects.clone()), MIDDLE, 0.0) else {
            return;
        };
        p.assert(16, 16, [128, 0, 0], "red");
        p.assert(34, 4, [0, 0, 0], "nothing yet");
        let p = draw_at("#000", moving(effects.clone()), MIDDLE, 1.0).unwrap();
        p.assert(16, 16, [64, 0, 64], "between");
        p.assert(34, 4, [128, 0, 0], "half there");
        let p = draw_at("#000", moving(effects), MIDDLE, 2.0).unwrap();
        p.assert(16, 16, [0, 0, 128], "blue");
        p.assert(34, 4, [255, 0, 0], "there");
    }
}

#[test]
fn settings_of_effects_move() {
    let layers = json!([
        {"type": "color", "color": "#808080", "effects": [
            {"type": "vignette", "visible": false},
            {"type": "adjust", "brightness": line(json!(0), json!(0.5))},
            {"type": "tint", "color": line(json!("#fff"), json!("#f00")), "amount": 1},
        ]},
    ]);
    let Some(p) = draw_at("#000", layers.clone(), MIDDLE, 0.0) else {
        return;
    };
    let [r, g, b] = p.at(32, 16);
    assert!(
        r.abs_diff(g) < 3 && r.abs_diff(b) < 3 && r < 140,
        "grey: {r} {g} {b}"
    );
    let p = draw_at("#000", layers, MIDDLE, 2.0).unwrap();
    let [r, g, b] = p.at(32, 16);
    assert!(
        r > 180 && g < 100 && b < 100,
        "brighter and red: {r} {g} {b}"
    );

    // A blur that starts with nothing has its passes, but shows the layer.
    let layers = |radius: Value| {
        json!([{"type": "image", "source": "assets/swatch.png", "smooth": false, "size": [32, 16],
                "effects": [{"type": "blur", "radius": radius, "edges": "hard"}]}])
    };
    let sharp = draw_at("#000", layers(json!(0)), MIDDLE, 0.0).unwrap();
    let start = draw_at("#000", layers(line(json!(0), json!(60))), MIDDLE, 0.0).unwrap();
    assert!(
        start.difference(&sharp) <= 4,
        "{}",
        start.difference(&sharp)
    );
    let still = draw_at("#000", layers(json!(60)), MIDDLE, 0.0).unwrap();
    let end = draw_at("#000", layers(line(json!(0), json!(60))), MIDDLE, 2.0).unwrap();
    assert!(end.difference(&still) <= 2, "{}", end.difference(&still));
    let between = draw_at("#000", layers(line(json!(0), json!(60))), MIDDLE, 0.1).unwrap();
    assert!(between.difference(&sharp) > 4 && between.difference(&still) > 4);

    // The speed of an effect does not move: it stays what it starts with.
    let speed = |speed: Value| {
        json!([{"type": "image", "source": "assets/swatch.png", "smooth": false, "size": [32, 16],
                "effects": [{"type": "drift", "speed": speed}]}])
    };
    let a = draw_at("#000", speed(json!(10)), MIDDLE, 1.5).unwrap();
    let b = draw_at("#000", speed(line(json!(10), json!(90))), MIDDLE, 1.5).unwrap();
    assert_eq!(a.difference(&b), 0);
}

#[test]
fn values_follow_the_sound() {
    let layers = json!([
        {"type": "color", "color": "#fff", "size": [32, 32], "position": [-16, 0],
         "opacity": {"audio": "bass", "from": 0.2, "to": 1, "threshold": 0}},
        {"type": "color", "color": "#fff", "size": [32, 32], "position": [16, 0],
         "opacity": {"audio": "treble", "from": 0, "to": 1}},
    ]);
    // Bass at half.
    let Some(p) = draw_loud(
        "#000",
        layers.clone(),
        |band| if band < 19 { 0.5 } else { 0.0 },
    ) else {
        return;
    };
    p.assert(16, 16, [153, 153, 153], "between 0.2 and 1");
    p.assert(48, 16, [0, 0, 0], "no treble");
    let p = draw_loud("#000", layers.clone(), |_| 0.0).unwrap();
    p.assert(16, 16, [51, 51, 51], "silence");
    let p = draw_loud("#000", layers, |_| 1.0).unwrap();
    p.assert(48, 16, [255, 255, 255], "loud");
}

#[test]
fn nothing_is_drawn_while_values_rest() {
    let keyed = |list: Value, repeat: &str| json!([{"type": "color", "size": 8, "position": {"keys": list, "repeat": repeat}}]);
    let there = json!([{"t": 0, "v": [0, 0]}, {"t": 2, "v": [9, 0]}]);
    let rest = (9.0, MIDDLE);
    let Some(once) = idle(&keyed(there.clone(), "once"), (5.0, MIDDLE), rest) else {
        return;
    };
    assert!(once, "over");
    let moving = idle(&keyed(there.clone(), "once"), (0.5, MIDDLE), (0.6, MIDDLE));
    assert_eq!(moving, Some(false));
    assert_eq!(
        idle(&keyed(there, "loop"), (5.5, MIDDLE), rest),
        Some(false)
    );
    let held = json!([{"t": 0, "v": [0, 0], "ease": "hold"}, {"t": 10, "v": [9, 0]}]);
    let held = keyed(held, "loop");
    assert_eq!(idle(&held, (1.0, MIDDLE), (8.0, MIDDLE)), Some(true));
    assert_eq!(
        idle(&held, (8.0, MIDDLE), (10.0, MIDDLE)),
        Some(true),
        "at 0 s again"
    );
    let quiet = json!([{"type": "color", "opacity": {"audio": "bass", "from": 1, "to": 0}}]);
    assert_eq!(idle(&quiet, (1.0, MIDDLE), rest), Some(true), "silence");
}
