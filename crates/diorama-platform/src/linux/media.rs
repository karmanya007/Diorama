//! "Now playing" from MPRIS (`org.mpris.MediaPlayer2.*` on the session
//! bus), which Linux players expose, for wallpapers' media integration.
//! Polled on a background thread like the Windows media session.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedValue;

use crate::{MediaInfo, MediaPlayback};

const POLL: Duration = Duration::from_millis(750);
/// Thumbnails larger than this are ignored (untrusted size from the player).
const MAX_THUMBNAIL: u64 = 16 << 20;
const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

/// Keeps polling MPRIS players until dropped.
#[derive(Debug)]
pub struct MediaWatcher {
    latest: Arc<Mutex<MediaInfo>>,
    stop: Arc<AtomicBool>,
}

impl MediaWatcher {
    pub fn start() -> Self {
        let latest = Arc::new(Mutex::new(MediaInfo::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (out, quit) = (Arc::clone(&latest), Arc::clone(&stop));
        let spawned = std::thread::Builder::new()
            .name("mpris".into())
            .spawn(move || {
                let conn = match Connection::session() {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::debug!("media information unavailable (no session bus): {e}");
                        return;
                    }
                };
                let mut current = None::<String>;
                let mut thumbnail_key = String::new();
                while !quit.load(Ordering::Relaxed) {
                    let mut info = read(&conn, &mut current, &mut thumbnail_key);
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
            tracing::warn!("mpris thread: {e}");
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

/// What a player reports, as `GetAll` returns it.
type Properties = HashMap<String, OwnedValue>;

fn read(conn: &Connection, current: &mut Option<String>, thumbnail_key: &mut String) -> MediaInfo {
    let mut info = MediaInfo {
        enabled: true,
        ..MediaInfo::default()
    };
    let players: Vec<(String, Properties)> = players(conn)
        .into_iter()
        .filter_map(|name| Some((properties(conn, &name)?, name)))
        .map(|(props, name)| (name, props))
        .collect();
    let Some((name, props)) = choose(&players, current.as_deref()) else {
        *current = None;
        return info;
    };
    *current = Some(name.to_owned());
    info.active = true;
    info.playback = match string(props.get("PlaybackStatus")).as_str() {
        "Playing" => MediaPlayback::Playing,
        "Paused" => MediaPlayback::Paused,
        _ => MediaPlayback::Stopped,
    };
    let metadata: Properties = props
        .get("Metadata")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Properties::try_from(v).ok())
        .unwrap_or_default();
    info.title = string(metadata.get("xesam:title"));
    info.artist = strings(metadata.get("xesam:artist"));
    info.album_title = string(metadata.get("xesam:album"));
    info.album_artist = strings(metadata.get("xesam:albumArtist"));
    info.genres = strings(metadata.get("xesam:genre"));
    info.duration = micros(metadata.get("mpris:length"));
    info.position = micros(props.get("Position")).min(info.duration.max(0.0));
    // A new track (or player) means a new thumbnail.
    info.thumbnail_key = format!(
        "{}\u{1f}{}\u{1f}{}",
        info.title, info.artist, info.album_title
    );
    if info.thumbnail_key != *thumbnail_key {
        info.thumbnail = art(&string(metadata.get("mpris:artUrl"))).map(Arc::new);
        thumbnail_key.clone_from(&info.thumbnail_key);
    }
    info
}

/// Bus names of the running MPRIS players.
fn players(conn: &Connection) -> Vec<String> {
    let Ok(bus) = Proxy::new(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    ) else {
        return Vec::new();
    };
    let mut names: Vec<String> = bus
        .call::<_, _, Vec<String>>("ListNames", &())
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n.starts_with(PREFIX))
        .collect();
    names.sort();
    names
}

fn properties(conn: &Connection, name: &str) -> Option<Properties> {
    let props = zbus::blocking::proxy::Builder::<Proxy>::new(conn)
        .destination(name)
        .ok()?
        .path(PATH)
        .ok()?
        .interface("org.freedesktop.DBus.Properties")
        .ok()?
        .cache_properties(CacheProperties::No)
        .build()
        .ok()?;
    props.call("GetAll", &(PLAYER,)).ok()
}

/// The player wallpapers follow: one that plays (the one followed so far
/// if it still does), else the one followed so far, else the first.
fn choose<'a>(
    players: &'a [(String, Properties)],
    current: Option<&str>,
) -> Option<(&'a str, &'a Properties)> {
    let playing = |(_, p): &&(String, Properties)| string(p.get("PlaybackStatus")) == "Playing";
    let is_current = |(n, _): &&(String, Properties)| Some(n.as_str()) == current;
    players
        .iter()
        .find(|p| playing(p) && is_current(p))
        .or_else(|| players.iter().find(playing))
        .or_else(|| players.iter().find(is_current))
        .or_else(|| players.first())
        .map(|(n, p)| (n.as_str(), p))
}

fn string(value: Option<&OwnedValue>) -> String {
    value
        .and_then(|v| v.downcast_ref::<String>().ok())
        .unwrap_or_default()
}

/// A string list (artists, genres), comma-separated like on Windows.
fn strings(value: Option<&OwnedValue>) -> String {
    value
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .map(|v| v.join(","))
        .unwrap_or_default()
}

/// Microseconds (players send `x` or `t`) as seconds.
fn micros(value: Option<&OwnedValue>) -> f64 {
    let Some(v) = value else {
        return 0.0;
    };
    let us = v
        .downcast_ref::<i64>()
        .map(|us| us as f64)
        .or_else(|_| v.downcast_ref::<u64>().map(|us| us as f64))
        .unwrap_or(0.0);
    (us / 1e6).max(0.0)
}

/// Album art from a local file. Remote art is not fetched: Diorama makes
/// no network requests on its own.
fn art(url: &str) -> Option<Vec<u8>> {
    let path = file_path(url)?;
    let size = std::fs::metadata(&path).ok()?.len();
    if size == 0 || size > MAX_THUMBNAIL {
        return None;
    }
    std::fs::read(path).ok()
}

/// The path of a `file://` URL, percent-decoded.
fn file_path(url: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let encoded = url.strip_prefix("file://")?;
    // `file://host/path`: only the local host.
    let encoded = encoded.strip_prefix("localhost").unwrap_or(encoded);
    if !encoded.starts_with('/') {
        return None;
    }
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        match (bytes[i], bytes.get(i + 1), bytes.get(i + 2)) {
            (b'%', Some(&h), Some(&l)) if hex(h).is_some() && hex(l).is_some() => {
                out.push((hex(h).unwrap_or(0) * 16 + hex(l).unwrap_or(0)) as u8);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(std::ffi::OsString::from_vec(out).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_decode_to_local_paths() {
        assert_eq!(
            file_path("file:///home/me/Music/My%20Album/cover%2B1.jpg"),
            Some("/home/me/Music/My Album/cover+1.jpg".into())
        );
        assert_eq!(
            file_path("file://localhost/tmp/a.png"),
            Some("/tmp/a.png".into())
        );
        assert_eq!(file_path("file://server/share/a.png"), None);
        assert_eq!(file_path("https://example.com/a.png"), None);
    }

    fn player(status: &str) -> Properties {
        let mut p = Properties::new();
        p.insert(
            "PlaybackStatus".into(),
            OwnedValue::try_from(zbus::zvariant::Value::from(status)).unwrap(),
        );
        p
    }

    #[test]
    fn a_playing_player_wins_and_stays_chosen() {
        let players = vec![
            ("a".to_owned(), player("Paused")),
            ("b".to_owned(), player("Playing")),
            ("c".to_owned(), player("Playing")),
        ];
        assert_eq!(choose(&players, None).map(|p| p.0), Some("b"));
        assert_eq!(choose(&players, Some("c")).map(|p| p.0), Some("c"));
        let paused = vec![
            ("a".to_owned(), player("Paused")),
            ("b".to_owned(), player("Stopped")),
        ];
        assert_eq!(choose(&paused, Some("b")).map(|p| p.0), Some("b"));
        assert_eq!(choose(&paused, Some("gone")).map(|p| p.0), Some("a"));
        assert_eq!(choose(&[], None).map(|p| p.0), None);
    }
}
