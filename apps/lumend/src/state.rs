//! Persisted daemon state (`<config dir>/state.json`): restores the last
//! wallpaper on the next start (and after autostart at sign-in).

use std::io;
use std::path::{Path, PathBuf};

use lumen_ipc::{Fit, Layout, MediaKind};
use serde::{Deserialize, Serialize};

const FILE: &str = "state.json";
const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    #[serde(default)]
    pub wallpaper: Option<SavedWallpaper>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: VERSION,
            wallpaper: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedWallpaper {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub fit: Fit,
    pub layout: Layout,
}

/// Missing or unreadable state is not fatal: start empty and say why.
pub fn load(dir: &Path) -> State {
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
            version: VERSION,
            wallpaper: Some(SavedWallpaper {
                path: PathBuf::from("C:/walls/a.mp4"),
                kind: MediaKind::Video,
                fit: Fit::Fill,
                layout: Layout::Span,
            }),
        };
        save(&dir, &state).unwrap();
        assert_eq!(load(&dir), state);

        std::fs::write(dir.join(FILE), b"{not json").unwrap();
        assert_eq!(load(&dir), State::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
