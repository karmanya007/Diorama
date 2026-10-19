#![allow(clippy::unwrap_used, reason = "test")]

use lumen_scene::{
    Blend, ImageLayer, LayerKind, Node, Pass, ProgramId, Scene, Script, ScriptLibrary,
    ScriptTarget, TextAlign, TextSpec,
};

use super::*;

/// Just enough of WE's helper classes for the tests (CI has no WE install).
const CLASSES: &str = r"
class Vec2 { constructor(x, y) { if (typeof x === 'string') { x = x.split(' '); this.x = +x[0]; this.y = +x[1]; } else { this.x = x ?? 0; this.y = y ?? this.x; } } }
class Vec3 { constructor(x, y, z) { if (typeof x === 'string') { x = x.split(' '); this.x = +x[0]; this.y = +x[1]; this.z = +x[2]; } else if (x instanceof Vec3) { this.x = x.x; this.y = x.y; this.z = x.z; } else { this.x = x ?? 0; this.y = y ?? this.x; this.z = z ?? this.x; } } }
class Vec4 { constructor(x, y, z, w) { this.x = x; this.y = y; this.z = z; this.w = w; } }
this._Internal = {
  updateScriptProperties(script, vars) { vars = JSON.parse(vars); for (const k in vars) if (k in script.scriptProperties) script.scriptProperties[k] = vars[k]; },
  convertUserProperties(p) { p = JSON.parse(p); const r = {}; for (const k in p) r[k] = p[k].value; return r; },
};
this.createScriptProperties = function () {
  const vars = {};
  const obj = { addText(o) { vars[o.name] = o.value; return obj; }, addCheckbox(o) { vars[o.name] = o.value; return obj; }, finish() { return vars; } };
  return obj;
};
";

fn node(name: &str, x: f32) -> Node {
    Node {
        name: name.into(),
        id: 0,
        parent: None,
        origin: [x, 0.0, 0.0],
        scale: [1.0; 3],
        angles: [0.0; 3],
        visible: true,
    }
}

fn layer(node: usize, text: Option<&str>) -> ImageLayer {
    ImageLayer {
        name: format!("layer{node}"),
        size: [10, 10],
        node: Some(node),
        local: lumen_scene::IDENTITY_AFFINE,
        text: text.map(|t| TextSpec {
            font: 0,
            text: t.into(),
            pixel_size: 10.0,
            halign: TextAlign::Center,
            valign: TextAlign::Center,
            max_width: None,
            max_rows: None,
            ellipsis: false,
            padding: [0.0; 2],
        }),
        particles: None,
        puppet: None,
        model: None,
        world: None,
        transform: lumen_scene::affine([0.0, 0.0], 0.0, [1.0, 1.0]),
        visible: true,
        parallax_depth: [1.0; 2],
        group: None,
        kind: LayerKind::Image,
        color: [1.0; 4],
        blend: Blend::Translucent,
        albedo: Pass {
            program: ProgramId(0),
            textures: Default::default(),
            target: None,
            constants: Default::default(),
            blend: Blend::Translucent,
        },
        mesh: None,
        copy_background: false,
        composite: None,
        effects: Vec::new(),
    }
}

fn script(source: &str, node: usize, target: ScriptTarget, properties: &str) -> Script {
    Script {
        source: source.into(),
        node: Some(node),
        target,
        initial: "null".into(),
        properties: properties.into(),
    }
}

fn scene(scripts: Vec<Script>) -> Scene {
    let mut nodes = vec![node("clock", 0.0), node("mover", 5.0)];
    nodes[1].origin = [5.0, 0.0, 0.0];
    Scene {
        width: 100,
        height: 100,
        nodes,
        layers: vec![layer(0, Some("12:34")), layer(1, None)],
        scripts,
        script_library: Some(ScriptLibrary {
            classes: CLASSES.into(),
            modules: vec![(
                "WEMath".into(),
                "export function mix(a, b, v) { return a + (b - a) * v; }".into(),
            )],
        }),
        user_properties: r#"{"speed": {"type": "slider", "value": 3}}"#.into(),
        ..Scene::default()
    }
}

/// 2026-10-15 16:29:21 UTC.
const CLOCK: f64 = 1_791_995_361_000.0;

fn inputs(runtime: f64) -> Inputs {
    Inputs {
        clock_ms: CLOCK,
        runtime,
        frametime: 1.0 / 30.0,
        cursor: [0.0, 0.0],
        daytime: 0.5,
        pointer_active: false,
        left_down: false,
        audio: [[0.0; 64]; 2],
    }
}

#[test]
fn clock_text_uses_the_host_clock_and_script_properties() {
    let clock = script(
        "'use strict';
export var scriptProperties = createScriptProperties().addText({ name: 'delimiter', value: ':' }).finish();
export function update(value) {
  const t = new Date();
  return ('0' + t.getUTCHours()).slice(-2) + scriptProperties.delimiter + ('0' + t.getUTCMinutes()).slice(-2);
}",
        0,
        ScriptTarget::Text(0),
        r#"{"delimiter": "-"}"#,
    );
    let mut host = ScriptHost::new(&scene(vec![clock]), [100, 100], CLOCK)
        .unwrap()
        .unwrap();
    let changes = host.update(&inputs(0.0));
    assert_eq!(changes[&0].text.as_deref(), Some("16-29"));
}

#[test]
fn scripts_move_their_layer_and_others() {
    let mover = script(
        "import * as WEMath from 'WEMath';
export function update(value) {
  value.x = WEMath.mix(0, 100, engine.runtime) * engine.userProperties.speed;
  thisScene.getLayer('clock').alpha = 0.25;
  return value;
}",
        1,
        ScriptTarget::Origin(1),
        "{}",
    );
    let mut host = ScriptHost::new(&scene(vec![mover]), [100, 100], CLOCK)
        .unwrap()
        .unwrap();
    let changes = host.update(&inputs(0.5));
    // 50 × speed 3 = 150.
    assert_eq!(changes[&1].transform.map(|t| t[4]), Some(150.0));
    assert_eq!(changes[&0].color, Some([1.0, 1.0, 1.0, 0.25]));
    // Nothing changes on a repeat frame with the same inputs.
    let again = host.update(&inputs(0.5));
    assert!(again.is_empty(), "{again:?}");
}

#[test]
fn a_runaway_script_only_stops_itself() {
    let spin = script(
        "export function update(value) { for (;;) {} }",
        1,
        ScriptTarget::Visible(1),
        "{}",
    );
    let fine = script(
        "export function update(value) { return 'ok'; }",
        0,
        ScriptTarget::Text(0),
        "{}",
    );
    let mut host = ScriptHost::new(&scene(vec![spin, fine]), [100, 100], CLOCK)
        .unwrap()
        .unwrap();
    let changes = host.update(&inputs(0.0));
    assert_eq!(changes[&0].text.as_deref(), Some("ok"));
    assert_eq!(host.updatable, vec![1]);
}

#[test]
fn broken_scripts_do_not_stop_loading() {
    let broken = script("export function update( {", 0, ScriptTarget::Text(0), "{}");
    let thrower = script(
        "export function update() { throw new Error('nope'); }",
        1,
        ScriptTarget::Visible(1),
        "{}",
    );
    let mut host = ScriptHost::new(&scene(vec![broken, thrower]), [100, 100], CLOCK)
        .unwrap()
        .unwrap();
    for _ in 0..5 {
        assert!(host.update(&inputs(0.0)).is_empty());
    }
}

fn animation(keyframes: &str, target: ScriptTarget) -> lumen_scene::Animation {
    lumen_scene::Animation {
        node: Some(1),
        target,
        keyframes: keyframes.into(),
        initial: r#""5 0 0""#.into(),
    }
}

/// Origin x from 0 to 10 over frames 0–10 at 10 fps, straight (no handles).
const RAMP: &str = r#"{
  "c0": [{"frame": 0, "value": 0, "front": {"enabled": false}},
         {"frame": 10, "value": 10, "back": {"enabled": false}}],
  "c1": [{"frame": 0, "value": 0}], "c2": [{"frame": 0, "value": 0}],
  "options": {"fps": 10, "length": 10, "mode": "MODE"}, "relative": true
}"#;

#[test]
fn keyframes_play_relative_loop_and_mirror() {
    let x_at = |mode: &str, t: f64| {
        let mut s = scene(Vec::new());
        s.animations = vec![animation(
            &RAMP.replace("MODE", mode),
            ScriptTarget::Origin(1),
        )];
        let mut host = ScriptHost::new(&s, [100, 100], CLOCK).unwrap().unwrap();
        host.update(&inputs(t))[&1].transform.unwrap()[4]
    };
    // Relative to the original x = 5.
    assert!((x_at("loop", 0.5) - 10.0).abs() < 1e-3);
    // Loop wraps after 10 frames; mirror plays back; single holds the end.
    assert!((x_at("loop", 1.3) - 8.0).abs() < 1e-3);
    assert!((x_at("mirror", 1.3) - 12.0).abs() < 1e-3);
    assert!((x_at("single", 1.3) - 15.0).abs() < 1e-3);
}

#[test]
fn scripts_start_paused_animations() {
    let keyframes = RAMP.replace("MODE", "single").replace(
        r#""mode": "single""#,
        r#""mode": "single", "startpaused": true"#,
    );
    let starter = script(
        "export function init() { thisLayer.getAnimation().play(); }",
        1,
        ScriptTarget::Other("x".into()),
        "{}",
    );
    let mut paused = scene(Vec::new());
    paused.animations = vec![animation(&keyframes, ScriptTarget::Origin(1))];
    let mut started = paused.clone();
    started.scripts = vec![starter];
    let x = |s: &Scene| {
        let mut host = ScriptHost::new(s, [100, 100], CLOCK).unwrap().unwrap();
        host.update(&inputs(0.5))
            .get(&1)
            .and_then(|c| c.transform)
            .map(|t| t[4])
    };
    // Paused at frame 0: relative 0 + the original 5.
    assert!(x(&paused).is_none_or(|x| (x - 5.0).abs() < 1e-3));
    assert!((x(&started).unwrap() - 10.0).abs() < 1e-3);
}

#[test]
fn clicks_reach_the_layer_under_the_cursor() {
    let button = script(
        "export function cursorClick(event) { thisScene.getLayer('clock').text = 'clicked ' + event.localPosition.x; }",
        1,
        ScriptTarget::Other("click".into()),
        "{}",
    );
    let mut host = ScriptHost::new(&scene(vec![button]), [100, 100], CLOCK)
        .unwrap()
        .unwrap();
    let at = |down: bool, x: f32| Inputs {
        cursor: [x, 2.0],
        pointer_active: true,
        left_down: down,
        ..inputs(0.0)
    };
    assert!(host.update(&at(false, 2.0)).is_empty());
    assert!(host.update(&at(true, 2.0)).is_empty());
    let changes = host.update(&at(false, 2.0));
    assert_eq!(changes[&0].text.as_deref(), Some("clicked 2"));
    // Pressed on the layer, released outside it (10 × 10 around 0): no click.
    host.update(&at(true, 2.0));
    assert!(host.update(&at(false, 40.0)).is_empty());
}

#[test]
fn events_reach_every_script_with_a_handler() {
    let player = script(
        "export function mediaThumbnailChanged(e) { thisScene.getLayer('clock').text = e.hasThumbnail + ' ' + (e.primaryColor instanceof Vec3) + ' ' + e.primaryColor.y; }",
        1,
        ScriptTarget::Other("media".into()),
        "{}",
    );
    let mut host = ScriptHost::new(&scene(vec![player]), [100, 100], CLOCK)
        .unwrap()
        .unwrap();
    host.send(
        "mediaThumbnailChanged",
        json!({"hasThumbnail": true, "primaryColor": {"vec3": [0.0, 0.5, 1.0]}}),
    );
    let changes = host.update(&inputs(0.0));
    assert_eq!(changes[&0].text.as_deref(), Some("true true 0.5"));
}

#[test]
fn audio_buffers_follow_the_spectrum() {
    let meter = script(
        "let audio = engine.registerAudioBuffers(16);
export function update(value) { return audio.left[0].toFixed(2) + ' ' + audio.right[15].toFixed(2) + ' ' + audio.average[0].toFixed(2); }",
        0,
        ScriptTarget::Text(0),
        "{}",
    );
    let mut host = ScriptHost::new(&scene(vec![meter]), [100, 100], CLOCK)
        .unwrap()
        .unwrap();
    let mut left = [0.0; 64];
    left[..4].copy_from_slice(&[1.0, 1.0, 0.0, 0.0]);
    let mut right = [0.0; 64];
    right[60..].copy_from_slice(&[0.2; 4]);
    let changes = host.update(&Inputs {
        audio: [left, right],
        ..inputs(0.0)
    });
    // 64 → 16 bands: averages of four.
    assert_eq!(changes[&0].text.as_deref(), Some("0.50 0.20 0.25"));
}

#[test]
fn scripts_drive_texture_animations() {
    let player = script(
        "export function update(value) {
  const a = thisLayer.getTextureAnimation();
  if (engine.runtime === 0) { a.rate = 0; a.setFrame(2); }
  else a.rate = 4.5;
  return value;
}",
        1,
        ScriptTarget::Origin(1),
        "{}",
    );
    let mut s = scene(vec![player]);
    let frame = lumen_scene::TextureFrame {
        origin: [0.0; 2],
        x_axis: [0.25, 0.0],
        y_axis: [0.0, 1.0],
        duration: 0.1,
    };
    s.textures.push(lumen_scene::Texture {
        name: "sheet".into(),
        width: 4,
        height: 1,
        image_width: 4,
        image_height: 1,
        rgba: vec![0; 16],
        mips: Vec::new(),
        clamp: true,
        nearest: true,
        frames: vec![frame; 4],
        video: None,
    });
    s.layers[1].albedo.textures.insert(
        0,
        lumen_scene::TextureBinding::Asset(lumen_scene::TextureId(0)),
    );
    let mut host = ScriptHost::new(&s, [100, 100], CLOCK).unwrap().unwrap();
    assert_eq!(host.update(&inputs(0.0))[&1].texture_frame, Some(2));
    // Rate 0: frame holds; the new rate applies from the next frame.
    assert!(host.update(&inputs(1.0 / 30.0)).is_empty());
    // 1/30 s × 4.5 = 0.15 s on from frame 2 (0.2 s) → frame 3.
    assert_eq!(host.update(&inputs(2.0 / 30.0))[&1].texture_frame, Some(3));
}

#[test]
fn scripts_control_sounds() {
    let dj = script(
        "export function update(value) {
  const s = thisLayer;
  if (engine.runtime === 0) { s.volume = 0.5; s.volume = 0.5; s.play(); }
  else if (!s.isPlaying()) thisScene.getLayer('clock').alpha = 0.5;
  else s.stop();
  return value;
}",
        1,
        ScriptTarget::Origin(1),
        "{}",
    );
    let mut s = scene(vec![dj]);
    s.sounds.push(lumen_scene::Sound {
        name: "song.mp3".into(),
        node: Some(1),
        tracks: Vec::new(),
        volume: 1.0,
        mode: lumen_scene::SoundMode::Loop,
        start_silent: true,
    });
    let mut host = ScriptHost::new(&s, [100, 100], CLOCK).unwrap().unwrap();
    host.update(&inputs(0.0));
    // The repeated volume is sent once.
    assert_eq!(
        host.take_sound_commands(),
        [SoundCommand::Volume(0, 0.5), SoundCommand::Play(0)]
    );
    // The player reports it plays: the script stops it.
    host.set_sound_states(vec![true]);
    host.update(&inputs(0.1));
    assert_eq!(host.take_sound_commands(), [SoundCommand::Stop(0)]);
    // It reports it stopped: the script sees that.
    host.set_sound_states(vec![false]);
    let changes = host.update(&inputs(0.2));
    assert_eq!(changes[&0].color.map(|c| c[3]), Some(0.5));
}
