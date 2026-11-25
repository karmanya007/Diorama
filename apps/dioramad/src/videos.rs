//! Video textures: decodes every scene texture that shows one and hands
//! the frame for the current scene time to the renderer.

use diorama_gpu::{Gpu, wgpu};
use diorama_media::{MediaSource, Nv12Frame, VideoPlayer};
use diorama_render::scene::SceneRenderer;
use diorama_render::{Nv12Planes, VideoBlit};
use diorama_scene::{Scene, VideoData};
use std::time::Duration;

pub struct SceneVideos {
    videos: Vec<VideoTexture>,
}

struct VideoTexture {
    /// Index of the texture in the scene.
    texture: usize,
    data: VideoData,
    player: VideoPlayer,
    /// Decoded but not yet due.
    pending: Option<Nv12Frame>,
    /// Seconds into the video of the frame that shows.
    shown: f32,
    pace: Pace,
    /// Converts its frames (made with the first).
    blit: Option<VideoBlit>,
}

/// How far a video plays on without a new frame; it skips as much at most.
const SLACK: f32 = 0.1;

/// Where a video is. It plays at its own speed or, when its frames come too
/// late, waits for them: it never runs to catch up.
#[derive(Debug, Default)]
struct Pace {
    /// Scene time it was last asked at.
    asked: Option<f32>,
    /// Seconds played.
    played: f32,
}

impl Pace {
    /// Plays up to scene time `time`.
    fn advance(&mut self, time: f32) -> f32 {
        // From the first time asked: opening it took a while.
        let asked = self.asked.replace(time).unwrap_or(time);
        self.played += (time - asked).max(0.0);
        self.played
    }

    /// There were no frames after `last` yet.
    fn wait(&mut self, last: f32) {
        self.played = self.played.min(last + SLACK);
    }
}

impl SceneVideos {
    /// `None` when the scene has no video textures (or none could be opened).
    pub fn start(scene: &Scene) -> Option<Self> {
        Self::follow(None, scene)
    }

    /// The videos of `scene`, where those `before` also played go on.
    pub fn follow(before: Option<Self>, scene: &Scene) -> Option<Self> {
        let mut old = before.map(|b| b.videos).unwrap_or_default();
        let videos: Vec<VideoTexture> = scene
            .textures
            .iter()
            .enumerate()
            .filter_map(|(texture, t)| {
                let data = t.video.clone()?;
                if let Some(i) = old.iter().position(|v| v.data == data) {
                    return Some(VideoTexture {
                        texture,
                        ..old.swap_remove(i)
                    });
                }
                let source = match &data {
                    VideoData::Memory(data) => MediaSource::Memory {
                        data: data.clone(),
                        name: format!("{}.mp4", t.name),
                    },
                    VideoData::File(path) => MediaSource::File(path.clone()),
                };
                match VideoPlayer::spawn_source(source) {
                    Ok(player) => Some(VideoTexture {
                        texture,
                        data,
                        player,
                        pending: None,
                        shown: 0.0,
                        pace: Pace::default(),
                        blit: None,
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
    /// Videos that are not visible stand still.
    pub fn frame(&mut self, gpu: &Gpu, renderer: &SceneRenderer, time: f32) {
        for v in &mut self.videos {
            if !renderer.shows_texture(v.texture) {
                v.pace.asked = None;
                continue;
            }
            let played = v.pace.advance(time);
            let mut due = None;
            loop {
                let Some(f) = v.pending.take().or_else(|| v.player.try_next()) else {
                    v.pace.wait(v.shown);
                    break;
                };
                let pts = f.pts.as_secs_f32();
                if pts > played {
                    v.pending = Some(f);
                    break;
                }
                v.shown = pts;
                due = Some(f);
            }
            if let Some(f) = due {
                v.show(gpu, renderer, &f);
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
                v.show(gpu, renderer, &f);
            }
        }
    }
}

impl VideoTexture {
    fn show(&mut self, gpu: &Gpu, renderer: &SceneRenderer, frame: &Nv12Frame) {
        let size = (frame.width, frame.height);
        let blit = match &mut self.blit {
            Some(blit) if blit.size() == size => blit,
            other => {
                let format = wgpu::TextureFormat::Rgba8Unorm;
                other.insert(VideoBlit::new(gpu, format, size.0, size.1))
            }
        };
        blit.upload(
            gpu,
            Nv12Planes {
                width: frame.width,
                height: frame.height,
                stride: frame.stride,
                luma: frame.luma(),
                chroma: frame.chroma(),
            },
        );
        renderer.show_video(gpu, self.texture, blit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What shows at `time`s of a video with a frame every 20 ms, of which
    /// those before `decoded(time)` seconds have been decoded.
    fn play(times: &[f32], decoded: impl Fn(f32) -> f32) -> Vec<f32> {
        let mut pace = Pace::default();
        let (mut next, mut shown) = (0u32, 0.0f32);
        let pts = |n: u32| n as f32 * 0.02;
        times
            .iter()
            .map(|&t| {
                let played = pace.advance(t);
                loop {
                    if pts(next) >= decoded(t) {
                        pace.wait(shown);
                        break;
                    }
                    if pts(next) > played {
                        break;
                    }
                    shown = pts(next);
                    next += 1;
                }
                shown
            })
            .collect()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.03
    }

    #[test]
    fn a_video_plays_at_its_speed_whatever_the_frame_rate() {
        for step in [0.004, 0.016, 0.033, 0.1] {
            let times: Vec<f32> = (0..(4.0 / step) as u32)
                .map(|i| 7.0 + i as f32 * step)
                .collect();
            let shown = play(&times, |_| f32::MAX);
            for (t, s) in times.iter().zip(shown) {
                assert!(close(s, t - 7.0), "{s} at {t}");
            }
        }
    }

    #[test]
    fn a_video_held_up_goes_on_from_where_it_was() {
        let times: Vec<f32> = (0..300).map(|i| i as f32 * 0.016).collect();
        // Nothing is decoded between 1 s and 2 s.
        let shown = play(&times, |t| {
            if t < 1.0 {
                t + 0.1
            } else {
                1.1 + (t - 2.0).max(0.0)
            }
        });
        for pair in shown.windows(2) {
            assert!(pair[1] - pair[0] <= SLACK + 0.03, "{pair:?}");
        }
        // It does not run: about a second behind at the end.
        assert!(
            (shown[299] - (times[299] - 1.0)).abs() < 0.25,
            "{}",
            shown[299]
        );
        for i in 130..299 {
            let speed = shown[i + 1] - shown[i];
            assert!(speed < 0.05, "{speed} at {}", times[i]);
        }
    }

    #[test]
    fn a_decoder_at_half_speed_plays_at_half_speed() {
        let times: Vec<f32> = (0..500).map(|i| i as f32 * 0.016).collect();
        let shown = play(&times, |t| t * 0.5 + 0.05);
        for (t, s) in times.iter().zip(shown).skip(50) {
            assert!((s - t * 0.5).abs() < 0.1, "{s} at {t}");
        }
    }

    #[test]
    fn time_that_passes_unseen_is_not_played() {
        let mut pace = Pace::default();
        pace.advance(1.0);
        pace.advance(1.5);
        pace.asked = None;
        assert!(close(pace.advance(9.0), 0.5));
        assert!(close(pace.advance(9.1), 0.6));
        assert!(close(pace.advance(2.0), 0.6), "the clock went back");
    }
}
