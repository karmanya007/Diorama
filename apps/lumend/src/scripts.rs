//! Runs a scene's SceneScript values and applies their changes to the
//! renderer each frame.

use std::time::{SystemTime, UNIX_EPOCH};

use lumen_gpu::Gpu;
use lumen_platform::DesktopPointer;
use lumen_render::scene::{LayerUpdate, SceneRenderer};
use lumen_scene::Scene;
use lumen_script::{Inputs, ScriptHost};

pub struct SceneScripts {
    host: ScriptHost,
    /// Layer names, for logs.
    names: Vec<String>,
    size: [f32; 2],
    last_time: Option<f32>,
}

/// Milliseconds since the Unix epoch, now.
pub fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// Fraction of the local day at `clock_ms`, using today's UTC offset.
pub fn day_fraction_at(clock_ms: f64) -> f32 {
    let utc_now = (now_ms() / 1000.0).rem_euclid(86_400.0);
    let local_now = f64::from(lumen_platform::day_fraction()) * 86_400.0;
    // Offset in seconds, folded into −12 h..+12 h.
    let offset = (local_now - utc_now + 43_200.0).rem_euclid(86_400.0) - 43_200.0;
    ((clock_ms / 1000.0 + offset).rem_euclid(86_400.0) / 86_400.0) as f32
}

impl SceneScripts {
    /// `None` when the scene has no scripts or the runtime failed to start.
    pub fn new(scene: &Scene, clock_ms: f64) -> Option<Self> {
        match ScriptHost::new(scene, [scene.width, scene.height], clock_ms) {
            Ok(Some(host)) => {
                tracing::info!(scripts = scene.scripts.len(), "scene scripts loaded");
                Some(Self {
                    host,
                    names: scene.layers.iter().map(|l| l.name.clone()).collect(),
                    size: [scene.width as f32, scene.height as f32],
                    last_time: None,
                })
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("scene scripts disabled: {e}");
                None
            }
        }
    }

    /// Runs one script frame at scene time `time` and applies the changes.
    /// `pointer` is the cursor in scene UV (y down); `buttons` is `None`
    /// when the cursor is not on the wallpaper (no cursor events).
    pub fn frame(
        &mut self,
        gpu: &Gpu,
        renderer: &mut SceneRenderer,
        time: f32,
        clock_ms: f64,
        pointer: [f32; 2],
        buttons: Option<DesktopPointer>,
    ) {
        let frametime = self.last_time.map_or(0.0, |t| (time - t).max(0.0));
        self.last_time = Some(time);
        let changes = self.host.update(&Inputs {
            clock_ms,
            runtime: f64::from(time),
            frametime: f64::from(frametime),
            cursor: [pointer[0] * self.size[0], (1.0 - pointer[1]) * self.size[1]],
            daytime: day_fraction_at(clock_ms),
            pointer_active: buttons.is_some_and(|b| b.over_desktop),
            left_down: buttons.is_some_and(|b| b.left_down),
        });
        for (layer, c) in changes {
            tracing::trace!(
                layer = self.names.get(layer).map(String::as_str),
                ?c,
                "script change"
            );
            renderer.update_layer(
                gpu,
                layer,
                &LayerUpdate {
                    transform: c.transform,
                    color: c.color,
                    visible: c.visible,
                    text: c.text,
                    effects: c.effects,
                    constants: c.constants,
                },
            );
        }
    }
}
