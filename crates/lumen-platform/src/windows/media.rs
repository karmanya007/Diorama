//! "Now playing" from the Windows media session (the system media
//! transport controls every player reports to), for wallpapers' media
//! integration. Polled on a background thread; WinRT calls block there.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Foundation::DateTime;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};

use crate::{MediaInfo, MediaPlayback};

const POLL: Duration = Duration::from_millis(750);
/// Thumbnails larger than this are ignored (untrusted size from the player).
const MAX_THUMBNAIL: u64 = 16 << 20;

/// Keeps polling the media session until dropped.
#[derive(Debug)]
pub struct MediaWatcher {
    latest: Arc<Mutex<MediaInfo>>,
    stop: Arc<AtomicBool>,
}

impl MediaWatcher {
    pub fn start() -> Self {
        let latest = Arc::new(Mutex::new(MediaInfo::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (out, quit) = (latest.clone(), stop.clone());
        let spawned = std::thread::Builder::new()
            .name("media session".into())
            .spawn(move || {
                // SAFETY: initialises WinRT for this thread only.
                if let Err(e) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
                    tracing::debug!("media session unavailable: {}", e.message());
                    return;
                }
                let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
                    .and_then(|op| op.join());
                let manager = match manager {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::debug!("media session unavailable: {}", e.message());
                        return;
                    }
                };
                let mut thumbnail_key = String::new();
                while !quit.load(Ordering::Relaxed) {
                    let mut info = read(&manager, &mut thumbnail_key);
                    if let Ok(mut latest) = out.lock() {
                        // Keep the previous thumbnail when unchanged.
                        if info.thumbnail.is_none() && info.thumbnail_key == latest.thumbnail_key {
                            info.thumbnail = latest.thumbnail.clone();
                        }
                        *latest = info;
                    }
                    std::thread::sleep(POLL);
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("media session thread: {e}");
        }
        Self { latest, stop }
    }

    /// The latest state (cheap: the thumbnail is shared).
    pub fn latest(&self) -> MediaInfo {
        self.latest.lock().map(|m| m.clone()).unwrap_or_default()
    }
}

impl Drop for MediaWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn read(
    manager: &GlobalSystemMediaTransportControlsSessionManager,
    thumbnail_key: &mut String,
) -> MediaInfo {
    let Ok(session) = manager.GetCurrentSession() else {
        return MediaInfo {
            enabled: true,
            ..MediaInfo::default()
        };
    };
    let mut info = MediaInfo {
        enabled: true,
        active: true,
        ..MediaInfo::default()
    };
    if let Ok(props) = session
        .TryGetMediaPropertiesAsync()
        .and_then(|op| op.join())
    {
        let s = |r: windows::core::Result<windows::core::HSTRING>| {
            r.map(|h| h.to_string_lossy()).unwrap_or_default()
        };
        info.title = s(props.Title());
        info.artist = s(props.Artist());
        info.subtitle = s(props.Subtitle());
        info.album_title = s(props.AlbumTitle());
        info.album_artist = s(props.AlbumArtist());
        info.genres = props
            .Genres()
            .map(|g| {
                g.into_iter()
                    .map(|h| h.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        // A new track (or player) means a new thumbnail.
        info.thumbnail_key = format!(
            "{}\u{1f}{}\u{1f}{}",
            info.title, info.artist, info.album_title
        );
        if info.thumbnail_key != *thumbnail_key {
            info.thumbnail = props.Thumbnail().ok().and_then(|t| {
                let stream = t.OpenReadAsync().ok()?.join().ok()?;
                let size = stream
                    .Size()
                    .ok()
                    .filter(|s| *s > 0 && *s <= MAX_THUMBNAIL)?;
                let reader = DataReader::CreateDataReader(&stream).ok()?;
                let n = reader
                    .LoadAsync(u32::try_from(size).ok()?)
                    .ok()?
                    .join()
                    .ok()?;
                let mut bytes = vec![0u8; n as usize];
                reader.ReadBytes(&mut bytes).ok()?;
                Some(Arc::new(bytes))
            });
            *thumbnail_key = info.thumbnail_key.clone();
        }
    }
    info.playback = playback(&session);
    if let Ok(t) = session.GetTimelineProperties() {
        let secs = |d: windows::core::Result<windows::Foundation::TimeSpan>| {
            d.map_or(0.0, |d| d.Duration as f64 / 1e7)
        };
        let mut position = secs(t.Position());
        // The player reports its position now and then; extrapolate while playing.
        if info.playback == MediaPlayback::Playing
            && let (Ok(updated), Ok(now)) = (t.LastUpdatedTime(), now())
        {
            position += ((now.UniversalTime - updated.UniversalTime) as f64 / 1e7).max(0.0);
        }
        info.duration = secs(t.EndTime()) - secs(t.StartTime());
        info.position = position.min(info.duration.max(0.0));
    }
    info
}

fn playback(session: &GlobalSystemMediaTransportControlsSession) -> MediaPlayback {
    match session.GetPlaybackInfo().and_then(|p| p.PlaybackStatus()) {
        Ok(Status::Playing) => MediaPlayback::Playing,
        Ok(Status::Paused) => MediaPlayback::Paused,
        _ => MediaPlayback::Stopped,
    }
}

/// Now as a WinRT `DateTime` (100 ns ticks since 1601).
fn now() -> windows::core::Result<DateTime> {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Ok(DateTime {
        UniversalTime: 116_444_736_000_000_000 + (unix.as_nanos() / 100) as i64,
    })
}
