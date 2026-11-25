//! Plays a scene's moving values ([`diorama_scene::LayerMotion`]).

use diorama_scene::{self as ir, AudioTrack, LayerMotion, MotionTarget, Numbers, Track};

use crate::scene::{COMPOSITE, FrameInputs, LayerUpdate};

/// Changes smaller than this do not show.
const SAME: f32 = 1e-5;

pub(crate) struct Motions {
    layers: Vec<Moving>,
    last_time: Option<f32>,
}

struct Moving {
    motion: LayerMotion,
    /// The layer's node as the scene has it.
    node: ir::Node,
    /// Opacity is a constant of the composite pass, not part of the colour.
    composite: bool,
    /// Passes of each effect.
    passes: Vec<usize>,
    /// Levels of the tracks that follow the sound.
    levels: Vec<Option<f32>>,
    /// The tracks' values now, and as the layer has them.
    now: Vec<Numbers>,
    applied: Option<Vec<Numbers>>,
}

impl Motions {
    pub fn new(scene: &ir::Scene) -> Self {
        let layers = scene
            .motions
            .iter()
            .filter_map(|m| {
                let layer = scene.layers.get(m.layer)?;
                Some(Moving {
                    node: scene.nodes.get(layer.node?)?.clone(),
                    composite: layer.composite.is_some(),
                    passes: layer.effects.iter().map(|e| e.passes.len()).collect(),
                    levels: vec![None; m.tracks.len()],
                    now: vec![[0.0; 4]; m.tracks.len()],
                    applied: None,
                    motion: m.clone(),
                })
            })
            .collect();
        Self {
            layers,
            last_time: None,
        }
    }

    pub fn uses_audio(&self) -> bool {
        self.layers
            .iter()
            .flat_map(|l| &l.motion.tracks)
            .any(|(_, t)| matches!(t, Track::Audio(_)))
    }

    /// Moves on to these inputs; whether anything has to change.
    pub fn advance(&mut self, inputs: &FrameInputs) -> bool {
        let last = self.last_time.replace(inputs.time);
        let dt = last.map_or(0.0, |t| inputs.time - t);
        let mut changed = false;
        for l in &mut self.layers {
            for (i, (_, track)) in l.motion.tracks.iter().enumerate() {
                l.now[i] = match track {
                    Track::Keys(keys) => keys.at(inputs.time),
                    Track::Audio(audio) => audio.at(heard(audio, &mut l.levels[i], inputs, dt)),
                };
            }
            changed |= l.changed();
        }
        changed
    }

    /// The layers that have to change, from now on as they are now.
    pub fn take(&mut self) -> Vec<(usize, LayerUpdate)> {
        let changed = self.layers.iter_mut().filter(|l| l.changed());
        changed
            .map(|l| {
                l.applied = Some(l.now.clone());
                (l.motion.layer, l.update())
            })
            .collect()
    }
}

fn heard(audio: &AudioTrack, level: &mut Option<f32>, inputs: &FrameInputs, dt: f32) -> f32 {
    let loud = audio.level(&inputs.audio);
    let next = level.map_or(loud, |now| audio.follow(now, loud, dt));
    *level = Some(next);
    next
}

impl Moving {
    fn changed(&self) -> bool {
        self.applied.as_ref().is_none_or(|old| {
            let numbers = old.iter().flatten().zip(self.now.iter().flatten());
            numbers
                .into_iter()
                .any(|(a, b)| (a - b).abs() > SAME * a.abs().max(1.0))
        })
    }

    fn update(&self) -> LayerUpdate {
        let mut update = LayerUpdate::default();
        let mut node = self.node.clone();
        let (mut placed, mut colored, mut faded) = (false, false, false);
        let (mut color, mut opacity) = (self.motion.color, self.motion.opacity);
        for ((target, _), &v) in self.motion.tracks.iter().zip(&self.now) {
            match target {
                MotionTarget::Position => {
                    [node.origin[0], node.origin[1]] = [v[0], v[1]];
                    placed = true;
                }
                MotionTarget::Scale => {
                    [node.scale[0], node.scale[1]] = [v[0], v[1]];
                    placed = true;
                }
                MotionTarget::Rotation => {
                    node.angles[2] = v[0];
                    placed = true;
                }
                MotionTarget::Color => {
                    color = v;
                    colored = true;
                }
                MotionTarget::Opacity => {
                    opacity = v[0];
                    faded = true;
                }
                MotionTarget::Setting {
                    effect,
                    uniform,
                    numbers,
                } => {
                    let passes = self.passes.get(*effect).copied().unwrap_or(0);
                    let value = v[..(*numbers).min(4)].to_vec();
                    update
                        .constants
                        .extend((0..passes).map(|p| (*effect, p, uniform.clone(), value.clone())));
                }
            }
        }
        if placed {
            update.transform = Some(node.local());
        }
        if self.composite {
            if faded {
                let opacity = ("opacity".to_owned(), vec![opacity]);
                update.constants.push((COMPOSITE, 0, opacity.0, opacity.1));
            }
            if colored {
                update.color = Some(color);
            }
        } else if colored || faded {
            color[3] *= opacity;
            update.color = Some(color);
        }
        update
    }
}
