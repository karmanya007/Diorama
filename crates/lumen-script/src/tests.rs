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
