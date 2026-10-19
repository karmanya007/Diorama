//! Runs a scene's SceneScript values and applies their changes to the
//! renderer each frame.

use std::time::{SystemTime, UNIX_EPOCH};

use lumen_gpu::Gpu;
use lumen_platform::{DesktopPointer, MediaInfo, MediaPlayback, MediaWatcher};
use lumen_render::scene::{LayerUpdate, SceneRenderer};
use lumen_scene::Scene;

use crate::sounds::SceneSounds;
use lumen_script::{Inputs, ScriptHost, thumbnail_colors};
use serde_json::json;

/// What one script frame sees.
#[derive(Debug, Clone, Copy)]
pub struct ScriptFrame {
    /// Scene time, seconds.
    pub time: f32,
    /// Wall clock, Unix milliseconds.
    pub clock_ms: f64,
    /// Cursor in scene UV (y down).
    pub pointer: [f32; 2],
    /// Mouse on the wallpaper; `None` when the cursor is elsewhere.
    pub buttons: Option<DesktopPointer>,
    /// (left, right) 64-band audio spectrum.
    pub audio: [[f32; 64]; 2],
}

pub struct SceneScripts {
    host: ScriptHost,
    /// Layer names, for logs.
    names: Vec<String>,
    size: [f32; 2],
    last_time: Option<f32>,
    /// Now-playing source for media events (live sessions only).
    media: Option<Media>,
}

struct Media {
    watcher: MediaWatcher,
    last: Option<MediaInfo>,
    /// Scene time the last timeline event went out.
    timeline_at: f32,
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
    /// `live`: follow the system's now-playing media (not for headless frames,
    /// which must not depend on what happens to be playing).
    pub fn new(scene: &Scene, clock_ms: f64, live: bool) -> Option<Self> {
        match ScriptHost::new(scene, [scene.width, scene.height], clock_ms) {
            Ok(Some(host)) => {
                tracing::info!(scripts = scene.scripts.len(), "scene scripts loaded");
                Some(Self {
                    host,
                    names: scene.layers.iter().map(|l| l.name.clone()).collect(),
                    size: [scene.width as f32, scene.height as f32],
                    last_time: None,
                    media: live.then(|| Media {
                        watcher: MediaWatcher::start(),
                        last: None,
                        timeline_at: f32::NEG_INFINITY,
                    }),
                })
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("scene scripts disabled: {e}");
                None
            }
        }
    }

    /// Runs one script frame and applies the changes to `renderer`.
    pub fn frame(
        &mut self,
        gpu: &Gpu,
        renderer: &mut SceneRenderer,
        mut sounds: Option<&mut SceneSounds>,
        frame: &ScriptFrame,
    ) {
        let ScriptFrame {
            time,
            clock_ms,
            pointer,
            buttons,
            audio,
        } = *frame;
        let frametime = self.last_time.map_or(0.0, |t| (time - t).max(0.0));
        self.last_time = Some(time);
        if let Some(media) = &mut self.media {
            media.send_changes(&mut self.host, time);
        }
        if let Some(sounds) = sounds.as_deref() {
            self.host.set_sound_states(sounds.playing());
        }
        let changes = self.host.update(&Inputs {
            clock_ms,
            runtime: f64::from(time),
            frametime: f64::from(frametime),
            cursor: [pointer[0] * self.size[0], (1.0 - pointer[1]) * self.size[1]],
            daytime: day_fraction_at(clock_ms),
            pointer_active: buttons.is_some_and(|b| b.over_desktop),
            left_down: buttons.is_some_and(|b| b.left_down),
            audio,
        });
        for command in self.host.take_sound_commands() {
            tracing::debug!(?command, "script sound command");
            if let Some(sounds) = sounds.as_deref_mut() {
                sounds.command(command);
            }
        }
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
                    texture_frame: c.texture_frame,
                },
            );
        }
    }
}

impl Media {
    /// Turns what changed in the system's now-playing state into WE's media
    /// events (`mediaStatusChanged`, `mediaPropertiesChanged`, …).
    fn send_changes(&mut self, host: &mut ScriptHost, time: f32) {
        let now = self.watcher.latest();
        // The watcher's first read takes a moment; don't announce "nothing
        // playing" before it (scripts would log an empty track).
        if self.last.is_none() && !now.enabled && time < 3.0 {
            return;
        }
        let last = self.last.take();
        let first = last.is_none();
        let last = last.unwrap_or_default();
        if first || now.enabled != last.enabled {
            host.send("mediaStatusChanged", json!({"enabled": now.enabled}));
        }
        let props = |m: &MediaInfo| {
            (
                m.title.clone(),
                m.artist.clone(),
                m.subtitle.clone(),
                m.album_title.clone(),
                m.album_artist.clone(),
                m.genres.clone(),
            )
        };
        if first || props(&now) != props(&last) {
            host.send(
                "mediaPropertiesChanged",
                json!({
                    "title": now.title, "artist": now.artist, "subTitle": now.subtitle,
                    "albumTitle": now.album_title, "albumArtist": now.album_artist,
                    "genres": now.genres, "contentType": "audio",
                }),
            );
        }
        if first || now.playback != last.playback {
            let state = match now.playback {
                MediaPlayback::Stopped => 0,
                MediaPlayback::Playing => 1,
                MediaPlayback::Paused => 2,
            };
            host.send("mediaPlaybackChanged", json!({"state": state}));
        }
        if first
            || now.thumbnail_key != last.thumbnail_key
            || now.thumbnail.is_some() != last.thumbnail.is_some()
        {
            host.send("mediaThumbnailChanged", thumbnail_event(&now));
        }
        // "Sent frequently while media is playing."
        let playing = now.playback == MediaPlayback::Playing;
        if (playing && time - self.timeline_at >= 1.0) || first || now.duration != last.duration {
            self.timeline_at = time;
            host.send(
                "mediaTimelineChanged",
                json!({"position": now.position, "duration": now.duration}),
            );
        }
        self.last = Some(now);
    }
}

fn thumbnail_event(m: &MediaInfo) -> serde_json::Value {
    let decoded = m
        .thumbnail
        .as_ref()
        .and_then(|bytes| image::load_from_memory(bytes).ok())
        .map(|img| img.thumbnail(64, 64).to_rgba8());
    let Some(img) = decoded else {
        return json!({"hasThumbnail": false});
    };
    let c = thumbnail_colors(img.as_raw());
    let v = |c: [f32; 3]| json!({"vec3": c});
    json!({
        "hasThumbnail": true,
        "primaryColor": v(c.primary),
        "secondaryColor": v(c.secondary),
        "tertiaryColor": v(c.tertiary),
        "textColor": v(c.text),
        "highContrastColor": v(c.high_contrast),
    })
}
