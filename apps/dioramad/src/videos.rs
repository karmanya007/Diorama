//! Video textures: decodes every scene texture that holds an MP4 and hands
//! the frame for the current scene time to the renderer.

use diorama_gpu::Gpu;
use diorama_media::{MediaSource, Nv12Frame, VideoPlayer};
use diorama_render::scene::SceneRenderer;
use diorama_scene::Scene;
use std::time::Duration;

pub struct SceneVideos {
    videos: Vec<VideoTexture>,
}

struct VideoTexture {
    /// Index of the texture in the scene.
    texture: usize,
    player: VideoPlayer,
    /// Decoded but not yet due.
    pending: Option<Nv12Frame>,
}

impl SceneVideos {
    /// `None` when the scene has no video textures (or none could be opened).
    pub fn start(scene: &Scene) -> Option<Self> {
        let videos: Vec<VideoTexture> = scene
            .textures
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                let mp4 = t.video.clone()?;
                let source = MediaSource::Memory {
                    data: mp4,
                    name: format!("{}.mp4", t.name),
                };
                match VideoPlayer::spawn_source(source) {
                    Ok(player) => Some(VideoTexture {
                        texture: i,
                        player,
                        pending: None,
                    }),
                    Err(e) => {
                        tracing::warn!(texture = %t.name, "video texture unavailable: {e}");
                        None
                    }
                }
            })
            .collect();
        (!videos.is_empty()).then_some(Self { videos })
    }

    /// Shows each video's newest frame due at scene time `time`; never waits.
    pub fn frame(&mut self, gpu: &Gpu, renderer: &SceneRenderer, time: f32) {
        for v in &mut self.videos {
            let mut due = None;
            while let Some(f) = v.pending.take().or_else(|| v.player.try_next()) {
                if f.pts.as_secs_f32() <= time {
                    due = Some(f);
                } else {
                    v.pending = Some(f);
                    break;
                }
            }
            if let Some(f) = due {
                upload(gpu, renderer, v.texture, &f);
            }
        }
    }

    /// Shows the frame at scene time `time`, decoding up to it (headless
    /// frames, where nothing plays in real time).
    pub fn seek(&mut self, gpu: &Gpu, renderer: &SceneRenderer, time: f32) {
        for v in &mut self.videos {
            let mut shown = None;
            while let Some(f) = v
                .pending
                .take()
                .or_else(|| v.player.next_timeout(Duration::from_secs(2)))
            {
                if f.pts.as_secs_f32() <= time || shown.is_none() {
                    let late = f.pts.as_secs_f32() > time;
                    shown = Some(f);
                    if late {
                        break;
                    }
                } else {
                    v.pending = Some(f);
                    break;
                }
            }
            if let Some(f) = shown {
                upload(gpu, renderer, v.texture, &f);
            }
        }
    }
}

fn upload(gpu: &Gpu, renderer: &SceneRenderer, texture: usize, frame: &Nv12Frame) {
    let rgba = frame.to_rgba();
    renderer.write_texture(gpu, texture, frame.width, frame.height, &rgba);
}
