//! `wpdump survey`: aggregate feature usage across a Workshop folder.
//!
//! Output is counts only (no titles, no content) so the report can live in the
//! repo and drive implementation priorities.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use diorama_core::Limits;
use diorama_we::Pkg;
use serde_json::Value;

#[derive(Debug, Default)]
struct Counter(BTreeMap<String, usize>);

impl Counter {
    fn add(&mut self, key: impl Into<String>) {
        *self.0.entry(key.into()).or_default() += 1;
    }

    fn table(&self, out: &mut String, heading: &str, limit: usize) {
        let _ = writeln!(out, "\n### {heading}\n\n| count | item |\n|---:|---|");
        let mut rows: Vec<_> = self.0.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (key, count) in rows.iter().take(limit) {
            let _ = writeln!(out, "| {count} | `{key}` |");
        }
        if rows.len() > limit {
            let _ = writeln!(out, "| … | {} more |", rows.len() - limit);
        }
    }
}

#[derive(Debug, Default)]
struct Survey {
    items: usize,
    project_types: Counter,
    user_property_types: Counter,
    scenes: usize,
    scene_features: Counter,
    object_kinds: Counter,
    unknown_object_keys: Counter,
    effects: Counter,
    effect_origin: Counter,
    combos: Counter,
    dynamic_values: Counter,
    script_scenes: usize,
    particle_parts: Counter,
    image_model_features: Counter,
    custom_shaders: usize,
    errors: Vec<String>,
}

pub fn run(corpus: &Path, install: &Path) -> Result<String> {
    let limits = Limits::default();
    let mut s = Survey::default();

    let mut dirs: Vec<_> = fs::read_dir(corpus)
        .with_context(|| format!("reading {}", corpus.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    for dir in dirs {
        let Ok(project_raw) = fs::read(dir.join("project.json")) else {
            continue;
        };
        s.items += 1;
        let project: Value = match serde_json::from_slice(&project_raw) {
            Ok(v) => v,
            Err(e) => {
                s.errors.push(format!("project.json: {e}"));
                continue;
            }
        };
        let kind = project["type"]
            .as_str()
            .unwrap_or("unknown")
            .to_ascii_lowercase();
        s.project_types.add(kind.as_str());
        if let Some(props) = project["general"]["properties"].as_object() {
            for prop in props.values() {
                s.user_property_types
                    .add(prop["type"].as_str().unwrap_or("?"));
            }
        }

        let pkg_path = dir.join("scene.pkg");
        if kind == "scene" && pkg_path.is_file() {
            let bytes = fs::read(&pkg_path)?;
            match Pkg::parse(&bytes, &limits) {
                Ok(pkg) => survey_scene(&mut s, &pkg, install),
                Err(e) => s.errors.push(format!("pkg: {e}")),
            }
        }
    }

    Ok(render(&s))
}

/// Reads a path from the package, falling back to the WE install's built-in assets.
fn load_json(pkg: &Pkg<'_>, install: &Path, path: &str) -> Option<Value> {
    if let Some(bytes) = pkg.get(path) {
        return serde_json::from_slice(bytes).ok();
    }
    let bytes = fs::read(install.join("assets").join(path)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn survey_scene(s: &mut Survey, pkg: &Pkg<'_>, install: &Path) {
    let Some(scene) = pkg
        .get("scene.json")
        .and_then(|b| serde_json::from_slice::<Value>(b).ok())
    else {
        s.errors.push("scene.json unreadable".to_owned());
        return;
    };
    s.scenes += 1;

    s.custom_shaders += pkg
        .entries()
        .iter()
        .filter(|e| e.path.starts_with("shaders/") && !e.path.starts_with("shaders/effects/"))
        .count();

    let general = &scene["general"];
    if general["orthogonalprojection"].is_null() {
        s.scene_features.add("perspective (3D) camera");
    } else {
        s.scene_features.add("orthographic (2D) camera");
    }
    for flag in [
        "bloom",
        "hdr",
        "cameraparallax",
        "camerashake",
        "camerafade",
        "clearenabled",
    ] {
        if truthy(&general[flag]) {
            s.scene_features.add(flag);
        }
    }

    let mut has_script = false;
    walk_dynamic_values(&scene, s, &mut has_script);
    if has_script {
        s.script_scenes += 1;
    }

    for object in scene["objects"].as_array().into_iter().flatten() {
        survey_object(s, pkg, install, object);
    }
}

fn survey_object(s: &mut Survey, pkg: &Pkg<'_>, install: &Path, object: &Value) {
    let kind = ["image", "particle", "model", "sound", "text", "light"]
        .into_iter()
        .find(|k| !object[*k].is_null() && object[*k] != Value::Array(vec![]));
    match kind {
        Some(kind) => s.object_kinds.add(kind),
        None => {
            s.object_kinds.add("other");
            let keys: BTreeSet<_> = object
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            s.unknown_object_keys
                .add(keys.into_iter().collect::<Vec<_>>().join(","));
        }
    }

    if kind == Some("image")
        && let Some(model) = object["image"]
            .as_str()
            .and_then(|p| load_json(pkg, install, p))
    {
        for key in [
            "puppet",
            "fullscreen",
            "passthrough",
            "autosize",
            "nopadding",
        ] {
            if truthy(&model[key]) {
                s.image_model_features.add(key);
            }
        }
    }

    if kind == Some("particle")
        && let Some(particle) = object["particle"]
            .as_str()
            .and_then(|p| load_json(pkg, install, p))
    {
        for section in ["emitter", "initializer", "operator", "renderer"] {
            for part in particle[section].as_array().into_iter().flatten() {
                let name = part["name"].as_str().unwrap_or("?");
                s.particle_parts.add(format!("{section}: {name}"));
            }
        }
        if !particle["children"].is_null() {
            s.particle_parts.add("children (sub-systems)");
        }
    }

    for effect in object["effects"].as_array().into_iter().flatten() {
        let file = effect["file"].as_str().unwrap_or("?");
        // "effects/<name>/effect.json" or "effects/workshop/<id>/<name>/effect.json"
        let name = file
            .strip_prefix("effects/")
            .map(|f| f.trim_end_matches("/effect.json"))
            .unwrap_or(file);
        s.effects.add(name);
        s.effect_origin.add(if pkg.get(file).is_some() {
            "embedded in pkg"
        } else {
            "built-in (WE install)"
        });
        for pass in effect["passes"].as_array().into_iter().flatten() {
            for (combo, value) in pass["combos"].as_object().into_iter().flatten() {
                s.combos.add(format!("{name}: {combo}={value}"));
            }
        }
    }
}

/// Counts values that are bound to user properties, scripted, or animated.
fn walk_dynamic_values(v: &Value, s: &mut Survey, has_script: &mut bool) {
    match v {
        Value::Object(map) => {
            if map.contains_key("script") {
                s.dynamic_values.add("script");
                *has_script = true;
            }
            if map.contains_key("user") {
                s.dynamic_values.add("user property binding");
            }
            if map.contains_key("animation") {
                s.dynamic_values.add("keyframe animation");
            }
            for child in map.values() {
                walk_dynamic_values(child, s, has_script);
            }
        }
        Value::Array(items) => {
            for child in items {
                walk_dynamic_values(child, s, has_script);
            }
        }
        _ => {}
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Object(o) => o.get("value").is_some_and(truthy),
        Value::Array(a) => !a.is_empty(),
        Value::Null => false,
    }
}

fn render(s: &Survey) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Corpus survey\n\nGenerated by `wpdump survey`. Counts only.\n\n\
         - Workshop items: {}\n- Scenes surveyed: {}\n- Scenes using SceneScript: {}\n\
         - Non-effect custom shader files in pkgs: {}\n- Errors: {}",
        s.items,
        s.scenes,
        s.script_scenes,
        s.custom_shaders,
        s.errors.len()
    );
    s.project_types.table(&mut out, "Wallpaper types", 20);
    s.user_property_types
        .table(&mut out, "User property types (project.json)", 20);
    s.scene_features.table(&mut out, "Scene features", 20);
    s.object_kinds.table(&mut out, "Object kinds", 20);
    s.unknown_object_keys
        .table(&mut out, "Keys of unclassified objects", 15);
    s.image_model_features
        .table(&mut out, "Image model features", 20);
    s.effects.table(&mut out, "Effects (by use)", 80);
    s.effect_origin.table(&mut out, "Effect origin", 5);
    s.combos.table(&mut out, "Effect combos set in scenes", 40);
    s.dynamic_values.table(
        &mut out,
        "Dynamic values (bindings / scripts / animation)",
        10,
    );
    s.particle_parts
        .table(&mut out, "Particle system components", 80);
    if !s.errors.is_empty() {
        let _ = writeln!(out, "\n### Errors\n");
        for e in &s.errors {
            let _ = writeln!(out, "- {e}");
        }
    }
    out
}
