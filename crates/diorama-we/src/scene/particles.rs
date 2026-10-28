//! WE particle objects → [`diorama_scene::ParticleSystem`] layers.
//!
//! Component names and keys are those of WE's particle editor
//! (`particles/*.json`); where a key is absent the editor's default is used
//! (see `docs/re-notes/particles.md`). Systems draw with WE's
//! `genericparticle` shader through its geometry-shader-free path.

use std::collections::BTreeMap;

use diorama_scene::particles::{
    ChildKind, ControlPoint, Emitter, Initializer, InstanceOverride, Operator, ParticleChild,
    ParticleRenderer, ParticleSystem, Range, SpriteSheet,
};
use diorama_scene::{ImageLayer, LayerKind, TextureBinding};
use serde_json::Value;

use super::values::{boolean, floats, integer, unwrap_value, vec_n};
use super::{Importer, SceneGraph, unsupported};
use crate::error::Result;
use crate::tex::Tex;

/// Children deeper than this are ignored (cycles in malformed content).
const MAX_DEPTH: usize = 4;

fn num(v: &Value, key: &str, default: f32) -> f32 {
    v.get(key)
        .and_then(floats)
        .and_then(|f| f.first().copied())
        .unwrap_or(default)
}

fn vec3(v: &Value, key: &str, default: [f32; 3]) -> [f32; 3] {
    match v.get(key).map(unwrap_value) {
        // A single number means the same value on every axis.
        Some(Value::Number(n)) => [n.as_f64().unwrap_or(0.0) as f32; 3],
        other => vec_n(other, default),
    }
}

fn range(v: &Value, min: f32, max: f32) -> Range<f32> {
    Range {
        min: num(v, "min", min),
        max: num(v, "max", max),
        exponent: num(v, "exponent", 1.0),
    }
}

fn range3(v: &Value, min: [f32; 3], max: [f32; 3]) -> Range<[f32; 3]> {
    Range {
        min: vec3(v, "min", min),
        max: vec3(v, "max", max),
        exponent: num(v, "exponent", 1.0),
    }
}

/// Rotation values: a single number is the z (screen) angle.
fn rotation3(v: &Value, key: &str) -> [f32; 3] {
    match v.get(key).map(unwrap_value) {
        Some(Value::Number(n)) => [0.0, 0.0, n.as_f64().unwrap_or(0.0) as f32],
        other => vec_n(other, [0.0; 3]),
    }
}

fn pair(v: &Value, min: &str, max: &str, defaults: [f32; 2]) -> [f32; 2] {
    [num(v, min, defaults[0]), num(v, max, defaults[1])]
}

fn emitter(e: &Value) -> Option<Emitter> {
    let rate = num(e, "rate", 5.0);
    let instantaneous = integer(e.get("instantaneous"))
        .unwrap_or(0)
        .clamp(0, 100_000) as u32;
    let duration = num(e, "duration", 0.0);
    let origin = vec3(e, "origin", [0.0; 3]);
    let speed = pair(e, "speedmin", "speedmax", [0.0, 0.0]);
    // Audio-driven emitters stay silent without sound (capture of
    // 2176097362: such systems draw nothing).
    let audio = integer(e.get("audioprocessingmode")).unwrap_or(0) != 0;
    match e["name"].as_str()? {
        "sphererandom" => Some(Emitter::Sphere {
            origin,
            directions: vec3(e, "directions", [1.0, 1.0, 0.0]),
            distance: pair(e, "distancemin", "distancemax", [0.0, 256.0]),
            speed,
            rate,
            instantaneous,
            duration,
            audio,
        }),
        "boxrandom" => Some(Emitter::Box {
            origin,
            min: vec3(e, "distancemin", [0.0; 3]),
            max: vec3(e, "distancemax", [256.0, 256.0, 0.0]),
            directions: vec3(e, "directions", [1.0, 1.0, 0.0]),
            speed,
            rate,
            instantaneous,
            duration,
            audio,
        }),
        _ => None,
    }
}

fn initializer(i: &Value) -> Option<Initializer> {
    Some(match i["name"].as_str()? {
        "lifetimerandom" => Initializer::Lifetime(range(i, 1.0, 1.0)),
        // WE draws particles at half the editor's size (capture 289:
        // sizerandom 70–90 → 33–43 px quads, 500–1000 → 285–433).
        "sizerandom" => {
            let r = range(i, 20.0, 20.0);
            Initializer::Size(Range {
                min: r.min * 0.5,
                max: r.max * 0.5,
                exponent: r.exponent,
            })
        }
        "alpharandom" => Initializer::Alpha(range(i, 1.0, 1.0)),
        "colorrandom" => {
            let c = range3(i, [255.0; 3], [255.0; 3]);
            Initializer::Color(Range {
                min: c.min.map(|v| v / 255.0),
                max: c.max.map(|v| v / 255.0),
                exponent: c.exponent,
            })
        }
        "velocityrandom" => Initializer::Velocity(range3(i, [0.0; 3], [0.0; 3])),
        "rotationrandom" => Initializer::Rotation(Range {
            min: rotation3(i, "min"),
            max: rotation3(i, "max"),
            exponent: num(i, "exponent", 1.0),
        }),
        "angularvelocityrandom" => Initializer::AngularVelocity(Range {
            min: rotation3(i, "min"),
            max: rotation3(i, "max"),
            exponent: num(i, "exponent", 1.0),
        }),
        "turbulentvelocityrandom" => Initializer::TurbulentVelocity {
            speed: pair(i, "speedmin", "speedmax", [100.0, 250.0]),
            scale: num(i, "scale", 1.0),
            offset: num(i, "offset", 0.0),
            timescale: num(i, "timescale", 1.0),
            forward: vec3(i, "forward", [0.0, 1.0, 0.0]),
        },
        _ => return None,
    })
}

fn operator(o: &Value) -> Option<Operator> {
    let phase = pair(o, "phasemin", "phasemax", [0.0, 0.0]);
    Some(match o["name"].as_str()? {
        "movement" => Operator::Movement {
            gravity: vec3(o, "gravity", [0.0; 3]),
            drag: num(o, "drag", 0.0),
        },
        "angularmovement" => Operator::AngularMovement {
            force: rotation3(o, "force"),
            drag: num(o, "drag", 0.0),
        },
        "alphafade" => Operator::AlphaFade {
            fade_in: num(o, "fadeintime", 0.5),
            fade_out: num(o, "fadeouttime", 0.5),
        },
        "sizechange" => Operator::SizeChange {
            start_time: num(o, "starttime", 0.0),
            end_time: num(o, "endtime", 1.0),
            start_value: num(o, "startvalue", 1.0),
            end_value: num(o, "endvalue", 0.0),
        },
        "alphachange" => Operator::AlphaChange {
            start_time: num(o, "starttime", 0.0),
            end_time: num(o, "endtime", 1.0),
            start_value: num(o, "startvalue", 1.0),
            end_value: num(o, "endvalue", 0.0),
        },
        "colorchange" => Operator::ColorChange {
            start_time: num(o, "starttime", 0.0),
            end_time: num(o, "endtime", 1.0),
            start_value: vec3(o, "startvalue", [1.0; 3]),
            end_value: vec3(o, "endvalue", [1.0; 3]),
        },
        "oscillatealpha" => Operator::OscillateAlpha {
            frequency: pair(o, "frequencymin", "frequencymax", [0.0, 10.0]),
            scale: pair(o, "scalemin", "scalemax", [0.0, 1.0]),
            phase,
        },
        "oscillatesize" => Operator::OscillateSize {
            frequency: pair(o, "frequencymin", "frequencymax", [0.0, 10.0]),
            scale: pair(o, "scalemin", "scalemax", [0.8, 1.2]),
            phase,
        },
        "oscillateposition" => Operator::OscillatePosition {
            frequency: pair(o, "frequencymin", "frequencymax", [0.0, 10.0]),
            scale: pair(o, "scalemin", "scalemax", [0.0, 10.0]),
            phase,
            mask: vec3(o, "mask", [1.0, 1.0, 0.0]),
        },
        "turbulence" => Operator::Turbulence {
            scale: num(o, "scale", 0.005),
            timescale: num(o, "timescale", 1.0),
            speed: pair(o, "speedmin", "speedmax", [500.0, 1000.0]),
            mask: vec3(o, "mask", [1.0, 1.0, 0.0]),
            phase: num(o, "phasemax", 0.0),
        },
        "controlpointattract" => Operator::ControlPointAttract {
            control_point: integer(o.get("controlpoint")).unwrap_or(0).clamp(0, 7) as usize,
            origin: vec3(o, "origin", [0.0; 3]),
            scale: num(o, "scale", 100.0),
            threshold: num(o, "threshold", 64.0),
        },
        "vortex" => Operator::Vortex {
            control_point: integer(o.get("controlpoint")).unwrap_or(0).clamp(0, 7) as usize,
            axis: vec3(o, "axis", [0.0, 0.0, 1.0]),
            distance: pair(o, "distanceinner", "distanceouter", [0.0, 500.0]),
            speed: pair(o, "speedinner", "speedouter", [0.0, 100.0]),
        },
        _ => return None,
    })
}

fn renderer(r: Option<&Value>) -> ParticleRenderer {
    let Some(r) = r else {
        return ParticleRenderer::Sprite;
    };
    match r["name"].as_str().unwrap_or("sprite") {
        "spritetrail" => ParticleRenderer::SpriteTrail {
            length: num(r, "length", 0.05),
            max_length: num(r, "maxlength", 10.0),
        },
        "rope" => ParticleRenderer::Rope {
            subdivision: integer(r.get("subdivision")).unwrap_or(0).clamp(0, 16) as u32,
        },
        "ropetrail" => ParticleRenderer::RopeTrail {
            length: num(r, "length", 0.1),
            // Captured trails without `segments` hold 4 points (914, 285).
            segments: integer(r.get("segments")).unwrap_or(4).clamp(2, 64) as u32,
        },
        _ => ParticleRenderer::Sprite,
    }
}

/// `instanceoverride` of a scene object, with control points converted from
/// scene pixels into the system's local space by `to_local`.
fn instance(v: Option<&Value>, to_local: impl Fn([f32; 3]) -> [f32; 3]) -> InstanceOverride {
    let mut o = InstanceOverride::default();
    let Some(v) = v.filter(|v| v.is_object()) else {
        return o;
    };
    // `rate` is not applied: live counts in three captures match the
    // un-overridden emission (ash 10/s × 0.5 → 85 alive over 7–10 s,
    // raindrops 25/s × 2 → 97 over 3–5 s, 3509243656's nebula 20/s × 0.5 →
    // 79/83 over 3–5 s).
    o.count = num(v, "count", 1.0);
    o.size = num(v, "size", 1.0);
    o.lifetime = num(v, "lifetime", 1.0);
    o.speed = num(v, "speed", 1.0);
    o.alpha = num(v, "alpha", 1.0);
    let brightness = num(v, "brightness", 1.0);
    o.color = vec3(v, "colorn", [1.0; 3]).map(|c| c * brightness);
    for (i, cp) in o.control_points.iter_mut().enumerate() {
        if let Some(p) = v.get(format!("controlpoint{i}")).and_then(floats) {
            *cp = Some(to_local([
                p.first().copied().unwrap_or(0.0),
                p.get(1).copied().unwrap_or(0.0),
                p.get(2).copied().unwrap_or(0.0),
            ]));
        }
    }
    o
}

impl Importer<'_> {
    pub(super) fn particle_layer(
        &mut self,
        object: &Value,
        name: &str,
        node: usize,
        graph: &SceneGraph,
    ) -> Result<()> {
        let path = object["particle"]
            .as_str()
            .ok_or_else(|| unsupported("particle object without a system", name))?;
        let (world, visible) = graph.world(object);
        let local = diorama_scene::affine([0.0, 0.0], 0.0, [self.zoom, self.zoom]);
        let transform = diorama_scene::compose(world, local);
        let to_local = |p: [f32; 3]| {
            let [a, b, c, d, e, f] = transform;
            let det = a * d - b * c;
            if det.abs() < 1e-9 {
                return p;
            }
            let (x, y) = (p[0] - e, p[1] - f);
            [(d * x - c * y) / det, (-b * x + a * y) / det, p[2]]
        };
        let overrides = instance(object.get("instanceoverride"), to_local);
        let system = self.particle_system(path, overrides, 0)?;
        self.scene.layers.push(ImageLayer {
            name: name.to_owned(),
            size: [1, 1],
            node: Some(node),
            local,
            text: None,
            particles: None,
            puppet: None,
            model: None,
            world: None,
            transform,
            visible,
            parallax_depth: vec_n(object.get("parallaxDepth"), [1.0, 1.0]),
            group: self.group_of(object, graph),
            kind: LayerKind::Image,
            color: [1.0; 4],
            blend: system.draw.blend,
            albedo: system.draw.clone(),
            mesh: None,
            copy_background: false,
            composite: None,
            effects: Vec::new(),
        });
        if let Some(layer) = self.scene.layers.last_mut() {
            layer.particles = Some(Box::new(system));
        }
        Ok(())
    }

    fn particle_system(
        &mut self,
        path: &str,
        instance: InstanceOverride,
        depth: usize,
    ) -> Result<ParticleSystem> {
        let json = self.vfs.read_json(path)?;
        let renderer = renderer(json["renderer"].as_array().and_then(|r| r.first()));
        let material_path = json["material"]
            .as_str()
            .ok_or_else(|| unsupported("particle system without material", path))?;
        let material = self.read_material(material_path)?;
        let mut spec = self.pass_spec(&material, None)?;
        let sprite_sheet = spec
            .textures
            .get(&0)
            .cloned()
            .and_then(|t| self.sprite_sheet(&t, &json));
        // A rope through the particles is no trail: its texture runs along
        // the particles (capture 287 of 2553418404: UVs 1 − index / (n − 1)).
        let trail = matches!(
            renderer,
            ParticleRenderer::SpriteTrail { .. } | ParticleRenderer::RopeTrail { .. }
        );
        let rope = matches!(
            renderer,
            ParticleRenderer::RopeTrail { .. } | ParticleRenderer::Rope { .. }
        );
        let mut combos = BTreeMap::new();
        if sprite_sheet.is_some() {
            combos.insert("SPRITESHEET".to_owned(), 1);
        }
        if trail {
            combos.insert("TRAILRENDERER".to_owned(), 1);
        }
        if rope {
            // WE draws rope trails with its rope shader whatever the
            // material names (capture 914: segment draws, TRAILRENDERER
            // UVs, thin vertex format).
            if spec.shader == "genericparticle" {
                spec.shader = "genericropeparticle".into();
            }
        } else if trail || sprite_sheet.is_some() {
            combos.insert("THICKFORMAT".to_owned(), 1);
        }
        // A rope's ends differ in size and colour (WE's capture carries both).
        if matches!(renderer, ParticleRenderer::Rope { .. }) {
            combos.insert("THICKFORMAT".to_owned(), 1);
        }
        spec.combos.extend(combos);
        let draw = self.pass(&spec, None)?;
        // Particles draw straight into the frame, as in WE, which samples a
        // single-level TEX without mips: tiny sprites stay crisp points
        // (capture 1225 of 1769376307: 64×64 star halos, 1 level on the GPU).
        for binding in draw.textures.values() {
            if let TextureBinding::Asset(id) = binding
                && self.generated_mips.contains(id)
                && let Some(t) = self.scene.textures.get_mut(id.0)
            {
                t.mips.clear();
            }
        }

        let control_points = json["controlpoint"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| ControlPoint {
                offset: vec3(c, "offset", [0.0; 3]),
                follows_pointer: integer(c.get("flags")).unwrap_or(0) & 1 != 0
                    || boolean(c.get("locktopointer"), false),
            })
            .collect();
        let list = |key: &str| json[key].as_array().cloned().unwrap_or_default();
        let mut system = ParticleSystem {
            draw,
            max_count: integer(json.get("maxcount"))
                .unwrap_or(100)
                .clamp(0, 100_000) as u32,
            start_time: num(&json, "starttime", 0.0).clamp(0.0, 600.0),
            emitters: list("emitter").iter().filter_map(emitter).collect(),
            initializers: list("initializer").iter().filter_map(initializer).collect(),
            operators: list("operator").iter().filter_map(operator).collect(),
            renderer,
            control_points,
            children: Vec::new(),
            sprite_sheet,
            instance,
            perspective: integer(json.get("flags")).unwrap_or(0) & 4 != 0,
            world_space: integer(json.get("flags")).unwrap_or(0) & 1 != 0,
        };
        if depth < MAX_DEPTH {
            for c in list("children") {
                let Some(child_path) = c["name"].as_str() else {
                    continue;
                };
                let kind = match c["type"].as_str().unwrap_or("static") {
                    "eventfollow" => ChildKind::Follow,
                    "eventspawn" => ChildKind::Spawn,
                    "eventdeath" => ChildKind::Death,
                    _ => ChildKind::Static,
                };
                // Children inherit the instance multipliers (not control points).
                let inherited = InstanceOverride {
                    control_points: [None; 8],
                    ..instance
                };
                match self.particle_system(child_path, inherited, depth + 1) {
                    Ok(mut child) => {
                        if let Some(max) = integer(c.get("maxcount")) {
                            child.max_count = max.clamp(0, 100_000) as u32;
                        }
                        system.children.push(ParticleChild {
                            kind,
                            origin: vec3(&c, "origin", [0.0; 3]),
                            scale: vec3(&c, "scale", [1.0; 3]),
                            probability: num(&c, "probability", 1.0),
                            system: child,
                        });
                    }
                    Err(e) => self.skip(format!("particle child {child_path}: {e}")),
                }
            }
        }
        Ok(system)
    }

    /// Frames of a sprite-sheet texture (TEX `TEXS` trailer), if it is one.
    fn sprite_sheet(&self, texture: &str, system: &Value) -> Option<SpriteSheet> {
        let path = format!("materials/{texture}.tex");
        let bytes = self.vfs.read(&path)?;
        let tex = Tex::parse(&bytes, self.limits).ok()?;
        let sheet = tex.sprite_sheet?;
        let frame = sheet.frames.first()?;
        let (tw, th) = (
            tex.header.texture_width.max(1) as f32,
            tex.header.texture_height.max(1) as f32,
        );
        Some(SpriteSheet {
            frames: u32::try_from(sheet.frames.len()).unwrap_or(1),
            frame_size: [frame.width / tw, frame.height / th],
            aspect: frame.height / frame.width.max(1.0),
            random_frame: system["animationmode"].as_str() == Some("randomframe"),
            repeat: num(system, "sequencemultiplier", 1.0),
        })
    }
}
