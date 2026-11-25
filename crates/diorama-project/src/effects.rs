//! Effects: what there is (`effects/effects.json`, for the editor too) and
//! the passes a document's effects become. Each is a WGSL file that starts
//! with a `Settings` struct made from the manifest and `prelude.wgsl`.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::OnceLock;

use diorama_scene::{
    Blend, Effect as SceneEffect, Fbo, Pass, ProgramId, TextureBinding, TextureId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::compile::{Compiler, VERTEX, program};
use crate::fields::Fields;

/// Effects per layer.
const MAX_EFFECTS: usize = 32;
/// Most room effects may add on each side of a layer, in pixels.
const MAX_SPREAD: f32 = 1000.0;

const PRELUDE: &str = include_str!("effects/prelude.wgsl");
const BLUR_PASSES: &str = include_str!("effects/blur_passes.wgsl");

/// An effect and its settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    /// What documents call it (`type`).
    pub name: String,
    pub label: String,
    pub group: String,
    pub about: String,
    /// The setting that is 0 when it stands still; absent if it always does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animated: Option<String>,
    #[serde(default, skip_serializing)]
    build: Build,
    #[serde(default, skip_serializing)]
    spread: Option<Spread>,
    pub settings: Vec<Setting>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Setting {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Kind {
    Number {
        default: f32,
        min: f32,
        max: f32,
        step: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    /// A value and its label each.
    Choice {
        default: String,
        choices: Vec<(String, String)>,
    },
    Color {
        default: String,
    },
    Flag {
        default: bool,
    },
    /// An image asset.
    Image,
}

/// How an effect's passes are made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Build {
    /// One pass, `fs`.
    #[default]
    Single,
    /// The passes of `blur_passes.wgsl`, then `finish`.
    Blur,
}

/// How far an effect reaches outside its layer: a setting times a factor.
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct Spread {
    setting: String,
    times: f32,
    /// Not when this choice has this value.
    #[serde(default)]
    unless: Option<(String, String)>,
}

/// Every effect, in the order to list them.
pub fn effects() -> &'static [Effect] {
    static EFFECTS: OnceLock<Vec<Effect>> = OnceLock::new();
    // Built in; a test checks that it parses.
    EFFECTS
        .get_or_init(|| {
            serde_json::from_str(include_str!("effects/effects.json")).unwrap_or_default()
        })
        .as_slice()
}

fn source(name: &str) -> Option<&'static str> {
    Some(match name {
        "adjust" => include_str!("effects/adjust.wgsl"),
        "blur" => include_str!("effects/blur.wgsl"),
        "drift" => include_str!("effects/drift.wgsl"),
        "fade" => include_str!("effects/fade.wgsl"),
        "glow" => include_str!("effects/glow.wgsl"),
        "grain" => include_str!("effects/grain.wgsl"),
        "mask" => include_str!("effects/mask.wgsl"),
        "pixelate" => include_str!("effects/pixelate.wgsl"),
        "pulse" => include_str!("effects/pulse.wgsl"),
        "ripple" => include_str!("effects/ripple.wgsl"),
        "shake" => include_str!("effects/shake.wgsl"),
        "shift" => include_str!("effects/shift.wgsl"),
        "sway" => include_str!("effects/sway.wgsl"),
        "tint" => include_str!("effects/tint.wgsl"),
        "vignette" => include_str!("effects/vignette.wgsl"),
        _ => return None,
    })
}

impl Effect {
    /// Whether it moves with these settings.
    fn moves(&self, f: &Fields<'_>) -> bool {
        let at_all = |key: &str| {
            let known = self.settings.iter().any(|s| s.key == key);
            !known || self.number(f, key) != 0.0
        };
        self.animated.as_deref().is_some_and(at_all) && at_all("amount")
    }

    /// Its fragment stage. One that does not move has no use for the time,
    /// so a picture that never changes is not drawn again and again.
    fn module(&self, moves: bool) -> Option<String> {
        let time = if moves { "g_Time" } else { "u_Still" };
        let mut settings = String::new();
        for s in &self.settings {
            let ty = match s.kind {
                Kind::Number { .. } | Kind::Choice { .. } | Kind::Flag { .. } => "f32",
                Kind::Color { .. } => "vec4<f32>",
                Kind::Image => continue,
            };
            let _ = writeln!(settings, "    {}: {ty},", s.key);
        }
        let passes = match self.build {
            Build::Single => "",
            Build::Blur => BLUR_PASSES,
        };
        let module = format!(
            "struct Settings {{
    g_Texture0Resolution: vec4<f32>,
    g_Texture1Resolution: vec4<f32>,
    u_Inner: vec4<f32>,
    u_Size: vec2<f32>,
    u_Layer: f32,
    u_Divisor: f32,
    u_Sigma: f32,
    g_Time: f32,
{settings}}}
{PRELUDE}{passes}{}",
            source(&self.name)?
        );
        Some(module.replace("g_Time", time))
    }

    fn number(&self, f: &Fields<'_>, key: &str) -> f32 {
        self.settings
            .iter()
            .find(|s| s.key == key)
            .map_or(0.0, |s| match s.kind {
                Kind::Number {
                    default, min, max, ..
                } => f.number(key, default, [min, max]),
                _ => 0.0,
            })
    }

    fn spread(&self, f: &Fields<'_>) -> f32 {
        let Some(spread) = &self.spread else {
            return 0.0;
        };
        if let Some((key, value)) = &spread.unless
            && f.text(key).as_ref() == Some(value)
        {
            return 0.0;
        }
        self.number(f, &spread.setting) * spread.times
    }
}

/// A layer's effects, read but not built.
pub(crate) struct List<'a> {
    effects: Vec<(&'static Effect, Fields<'a>)>,
    /// Room they need on each side, in whole pixels.
    pub spread: f32,
}

impl<'a> List<'a> {
    /// The effects of the layer `f` that are on.
    pub fn read(f: &Fields<'a>) -> Self {
        let mut list = Self {
            effects: Vec::new(),
            spread: 0.0,
        };
        let effects = match f.map.get("effects") {
            None => return list,
            Some(Value::Array(effects)) => effects,
            Some(_) => {
                f.problem("effects is not a list");
                return list;
            }
        };
        if effects.len() > MAX_EFFECTS {
            f.problem(&format!(
                "more than {MAX_EFFECTS} effects: the rest is left out"
            ));
        }
        for (i, value) in effects.iter().take(MAX_EFFECTS).enumerate() {
            let Some(map) = value.as_object() else {
                f.problem(&format!("effect {} is not an object", i + 1));
                continue;
            };
            let mut fields = Fields {
                map,
                at: f.at.clone(),
                ..*f
            };
            let name = fields.text("type").unwrap_or_default();
            let Some(effect) = effects_by_name(&name) else {
                f.problem(&format!("unknown effect {name}; left out"));
                continue;
            };
            fields.at = format!("{}, {name}", f.at);
            if fields.flag("visible", true) {
                list.spread += effect.spread(&fields);
                list.effects.push((effect, fields));
            }
        }
        list.spread = list.spread.min(MAX_SPREAD).ceil();
        list
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
}

fn effects_by_name(name: &str) -> Option<&'static Effect> {
    effects().iter().find(|e| e.name == name)
}

/// The layer effects are drawn for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frame {
    /// With the room around it; zero if only known when it is drawn (text).
    pub size: [f32; 2],
    /// That room, on each side.
    pub spread: f32,
}

impl Frame {
    fn constants(self) -> BTreeMap<String, Vec<f32>> {
        let [w, h] = self.size;
        let inner = if w > 0.0 {
            let room = [self.spread / w, self.spread / h];
            [
                room[0],
                room[1],
                1.0 / (1.0 - 2.0 * room[0]),
                1.0 / (1.0 - 2.0 * room[1]),
            ]
        } else {
            [0.0, 0.0, 1.0, 1.0]
        };
        BTreeMap::from([
            ("u_Size".to_owned(), vec![w, h]),
            ("u_Inner".to_owned(), inner.to_vec()),
        ])
    }
}

impl Compiler<'_> {
    /// Builds `list`; an effect that cannot be used is left out and named.
    pub(crate) fn effects(&mut self, list: &List<'_>, frame: Frame) -> Vec<SceneEffect> {
        let mut built = Vec::new();
        for (effect, f) in &list.effects {
            match self.effect(effect, f, frame) {
                Ok(e) => built.push(e),
                Err(message) => f.problem(&format!("{message}; left out")),
            }
        }
        built
    }

    fn effect(
        &mut self,
        effect: &'static Effect,
        f: &Fields<'_>,
        frame: Frame,
    ) -> Result<SceneEffect, String> {
        let mut constants = frame.constants();
        let mut image = None;
        for s in &effect.settings {
            let key = s.key.as_str();
            let value = match &s.kind {
                Kind::Number {
                    default, min, max, ..
                } => vec![f.number(key, *default, [*min, *max])],
                Kind::Choice { default, choices } => {
                    let names: Vec<(&str, f32)> = (0u8..)
                        .zip(choices)
                        .map(|(i, (name, _))| (name.as_str(), f32::from(i)))
                        .collect();
                    let default = names.iter().find(|(n, _)| n == default);
                    vec![f.choice(key, &names, default.map_or(0.0, |(_, i)| *i))]
                }
                Kind::Color { default } => {
                    let default = crate::fields::color(&Value::from(default.as_str()));
                    f.color(key, default.unwrap_or([1.0; 4])).to_vec()
                }
                Kind::Flag { default } => vec![f32::from(u8::from(f.flag(key, *default)))],
                Kind::Image => {
                    let path = f.text(key).ok_or_else(|| format!("no {key}"))?;
                    image = Some(self.image(&path, true)?);
                    continue;
                }
            };
            constants.insert(s.key.clone(), value);
        }
        let mut scene_effect = SceneEffect {
            name: effect.name.clone(),
            visible: true,
            fbos: Vec::new(),
            passes: Vec::new(),
        };
        match effect.build {
            Build::Single => {
                let pass = self.pass(effect, "fs", constants, image, effect.moves(f))?;
                scene_effect.passes.push(pass);
            }
            Build::Blur => self.blur(effect, f, &constants, &mut scene_effect)?,
        }
        Ok(scene_effect)
    }

    /// A pass over the layer's running picture, with `image` in slot 1.
    fn pass(
        &mut self,
        effect: &'static Effect,
        entry: &'static str,
        constants: BTreeMap<String, Vec<f32>>,
        image: Option<TextureId>,
        moves: bool,
    ) -> Result<Pass, String> {
        let mut textures = BTreeMap::from([(0, TextureBinding::Previous)]);
        if let Some(id) = image {
            textures.insert(1, TextureBinding::Asset(id));
        }
        Ok(Pass {
            program: self.effect_program(effect, entry, moves)?,
            textures,
            target: None,
            constants,
            blend: Blend::Opaque,
        })
    }

    /// Shrink, blur across and down, and draw the result.
    fn blur(
        &mut self,
        effect: &'static Effect,
        f: &Fields<'_>,
        constants: &BTreeMap<String, Vec<f32>>,
        out: &mut SceneEffect,
    ) -> Result<(), String> {
        let sigma = effect.number(f, "radius") / 2.0;
        // Small enough that the passes' reach (10 pixels) is three sigmas.
        let mut divisor: u8 = 1;
        while sigma / f32::from(divisor) > 3.2 && divisor < 32 {
            divisor *= 2;
        }
        let mut constants = constants.clone();
        let divisor_value = f32::from(divisor);
        constants.insert("u_Divisor".to_owned(), vec![divisor_value]);
        constants.insert("u_Sigma".to_owned(), vec![sigma / divisor_value]);
        out.fbos = ["a", "b"]
            .map(|name| Fbo {
                name: name.to_owned(),
                scale_div: u32::from(divisor),
            })
            .to_vec();
        // The layer itself stays in a slot for its size: the copies are smaller.
        let (layer, a, b) = (
            TextureBinding::Previous,
            TextureBinding::Fbo("a".to_owned()),
            TextureBinding::Fbo("b".to_owned()),
        );
        let steps = [
            ("shrink", layer.clone(), layer.clone(), 0.0, Some("a")),
            ("across", a.clone(), layer.clone(), 1.0, Some("b")),
            ("down", b, layer.clone(), 1.0, Some("a")),
            ("finish", layer, a, 0.0, None),
        ];
        for (entry, first, second, layer_slot, target) in steps {
            let mut constants = constants.clone();
            constants.insert("u_Layer".to_owned(), vec![layer_slot]);
            let mut pass = self.pass(effect, entry, constants, None, false)?;
            pass.textures = BTreeMap::from([(0, first), (1, second)]);
            pass.target = target.map(str::to_owned);
            out.passes.push(pass);
        }
        Ok(())
    }

    fn effect_program(
        &mut self,
        effect: &'static Effect,
        entry: &'static str,
        moves: bool,
    ) -> Result<ProgramId, String> {
        let key = (effect.name.as_str(), entry, moves);
        if let Some(id) = self.effect_programs.get(&key) {
            return Ok(*id);
        }
        let module = effect.module(moves).ok_or("no shader")?;
        let mut p = program(
            &format!("{} {entry}", effect.name),
            VERTEX,
            module,
            &[(0, 0), (1, 1)],
        );
        entry.clone_into(&mut p.fragment_entry);
        let id = ProgramId(self.scene.programs.len());
        self.scene.programs.push(p);
        self.effect_programs.insert(key, id);
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_are_sound() {
        let parsed: Result<Vec<Effect>, _> =
            serde_json::from_str(include_str!("effects/effects.json"));
        assert!(parsed.is_ok(), "{parsed:?}");
        assert!(effects().len() >= 15);
        for e in effects() {
            assert!(e.module(true).is_some(), "{} has no shader", e.name);
            assert_eq!(
                effects().iter().filter(|o| o.name == e.name).count(),
                1,
                "{}",
                e.name
            );
            for s in &e.settings {
                match &s.kind {
                    Kind::Number {
                        default, min, max, ..
                    } => assert!(min <= default && default <= max, "{}.{}", e.name, s.key),
                    Kind::Choice { default, choices } => assert!(
                        choices.iter().any(|(c, _)| c == default),
                        "{}.{}",
                        e.name,
                        s.key
                    ),
                    Kind::Color { default } => {
                        assert!(crate::fields::color(&Value::from(default.as_str())).is_some());
                    }
                    Kind::Flag { .. } | Kind::Image => {}
                }
            }
            let has = |key: &String| e.settings.iter().any(|s| s.key == *key);
            assert!(e.animated.as_ref().is_none_or(has), "{}", e.name);
            assert!(
                e.spread.as_ref().is_none_or(
                    |s| has(&s.setting) && s.unless.as_ref().is_none_or(|(k, _)| has(k))
                ),
                "{}",
                e.name
            );
        }
        let doc = include_str!("../../../docs/formats/diorama.md");
        for e in effects() {
            assert!(doc.contains(&format!("| `{}` |", e.name)), "{}", e.name);
        }
        // What the editor gets.
        let json = serde_json::to_value(&effects()[0]).unwrap();
        assert_eq!(json["settings"][0]["type"], "number");
        assert!(json.get("build").is_none());
    }
}
