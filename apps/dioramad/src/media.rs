//! What the system plays now, as the changes WE's media events report
//! (scene scripts and web wallpapers each build their own payloads).

use diorama_platform::{MediaInfo, MediaPlayback, MediaWatcher};
use serde_json::{Value, json};

/// One media event's worth of change.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaChange {
    Status {
        enabled: bool,
    },
    /// `{title, artist, subTitle, albumTitle, albumArtist, genres, contentType}`.
    Properties(Value),
    /// 0 stopped, 1 playing, 2 paused.
    Playback(u8),
    /// The album art changed; see [`MediaTracker::current`].
    Thumbnail,
    /// `{position, duration}` (seconds).
    Timeline(Value),
}

pub struct MediaTracker {
    watcher: MediaWatcher,
    last: Option<MediaInfo>,
    /// Time (seconds, caller's clock) the last timeline event went out.
    timeline_at: f32,
}

impl MediaTracker {
    pub fn start() -> Self {
        Self {
            watcher: MediaWatcher::start(),
            last: None,
            timeline_at: f32::NEG_INFINITY,
        }
    }

    /// The state the last [`MediaTracker::changes`] reported.
    pub fn current(&self) -> Option<&MediaInfo> {
        self.last.as_ref()
    }

    /// What changed since the last call; everything on the first report.
    /// `time` (seconds, any steady clock) paces timeline events.
    pub fn changes(&mut self, time: f32) -> Vec<MediaChange> {
        let now = self.watcher.latest();
        // The watcher's first read takes a moment; don't announce "nothing
        // playing" before it (pages would show an empty track).
        if self.last.is_none() && !now.enabled && time < 3.0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let last = self.last.take();
        let first = last.is_none();
        let last = last.unwrap_or_default();
        if first || now.enabled != last.enabled {
            out.push(MediaChange::Status {
                enabled: now.enabled,
            });
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
            out.push(MediaChange::Properties(json!({
                "title": now.title, "artist": now.artist, "subTitle": now.subtitle,
                "albumTitle": now.album_title, "albumArtist": now.album_artist,
                "genres": now.genres, "contentType": "audio",
            })));
        }
        if first || now.playback != last.playback {
            out.push(MediaChange::Playback(match now.playback {
                MediaPlayback::Stopped => 0,
                MediaPlayback::Playing => 1,
                MediaPlayback::Paused => 2,
            }));
        }
        if first
            || now.thumbnail_key != last.thumbnail_key
            || now.thumbnail.is_some() != last.thumbnail.is_some()
        {
            out.push(MediaChange::Thumbnail);
        }
        // "Sent frequently while media is playing."
        let playing = now.playback == MediaPlayback::Playing;
        if (playing && time - self.timeline_at >= 1.0) || first || now.duration != last.duration {
            self.timeline_at = time;
            out.push(MediaChange::Timeline(
                json!({"position": now.position, "duration": now.duration}),
            ));
        }
        self.last = Some(now);
        out
    }
}
