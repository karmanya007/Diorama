//! Persisted daemon state (`<config dir>/state.json`): restores the last
//! wallpapers on the next start (and after autostart at sign-in).

use std::io;
use std::path::{Path, PathBuf};

use lumen_ipc::{Fit, Layout, MediaKind};
use serde::{Deserialize, Serialize};

const FILE: &str = "state.json";
/// 2: `wallpapers` (one per monitor group) replaced `wallpaper`.
const VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    /// What plays where, in the order the user set them.
    #[serde(default)]
    pub wallpapers: Vec<SavedWallpaper>,
    /// Version 1's single wallpaper (read only).
    #[serde(default, skip_serializing)]
    pub wallpaper: Option<SavedWallpaper>,
    /// Video soundtrack volume 0–100.
    #[serde(default)]
    pub volume: u8,
    /// Web wallpaper folders the user let use the network.
    #[serde(default)]
    pub web_network: Vec<PathBuf>,
    /// When to pause and how fast scenes run.
    #[serde(default)]
    pub policy: Option<lumen_ipc::Policy>,
    /// Property values set in Lumen, per wallpaper.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<SavedProperties>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedProperties {
    /// The wallpaper's folder (scenes, web pages) or file.
    pub path: PathBuf,
    pub values: crate::properties::Values,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: VERSION,
            wallpapers: Vec::new(),
            wallpaper: None,
            volume: 0,
            web_network: Vec::new(),
            policy: None,
            properties: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedWallpaper {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub fit: Fit,
    pub layout: Layout,
    /// Output ids it shows on; empty: every monitor no other wallpaper has.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
}

/// Missing or unreadable state is not fatal: start empty and say why.
pub fn load(dir: &Path) -> State {
    let mut state = read(dir);
    if let Some(w) = state.wallpaper.take()
        && state.wallpapers.is_empty()
    {
        state.wallpapers.push(w);
    }
    state.version = VERSION;
    state
}

fn read(dir: &Path) -> State {
    let path = dir.join(FILE);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!("ignoring unreadable {}: {e}", path.display());
            State::default()
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => State::default(),
        Err(e) => {
            tracing::warn!("cannot read {}: {e}", path.display());
            State::default()
        }
    }
}

/// Write to a temp file then rename, so a crash never leaves a half-written state.
pub fn save(dir: &Path, state: &State) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    let bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, dir.join(FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_garbage() {
        let dir = std::env::temp_dir().join(format!("lumen-state-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), State::default());

        let state = State {
            wallpapers: vec![
                SavedWallpaper {
                    path: PathBuf::from("C:/walls/a.mp4"),
                    kind: MediaKind::Video,
                    fit: Fit::Fill,
                    layout: Layout::Span,
                    outputs: Vec::new(),
                },
                SavedWallpaper {
                    path: PathBuf::from("C:/walls/b.png"),
                    kind: MediaKind::Image,
                    fit: Fit::Fit,
                    layout: Layout::PerOutput,
                    outputs: vec!["DEL-1".into()],
                },
            ],
            volume: 40,
            web_network: vec![PathBuf::from("C:/web/clock")],
            properties: vec![SavedProperties {
                path: PathBuf::from("C:/web/clock"),
                values: [("speed".to_owned(), serde_json::json!(3))].into(),
            }],
            ..State::default()
        };
        save(&dir, &state).unwrap();
        assert_eq!(load(&dir), state);

        // Version 1 files still load.
        std::fs::write(
            dir.join(FILE),
            br#"{"version":1,"wallpaper":{"path":"C:/w.png","kind":"image","fit":"fill","layout":"per-output"},"volume":5}"#,
        )
        .unwrap();
        let old = load(&dir);
        assert_eq!(old.wallpapers.len(), 1);
        assert_eq!(old.wallpapers[0].path, PathBuf::from("C:/w.png"));
        assert_eq!(old.volume, 5);

        std::fs::write(dir.join(FILE), b"{not json").unwrap();
        assert_eq!(load(&dir), State::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
