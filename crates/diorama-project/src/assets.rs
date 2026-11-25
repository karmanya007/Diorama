//! A wallpaper's files. Paths in documents are relative, with `/`, and stay
//! inside the wallpaper.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use diorama_scene::VideoData;

use crate::{Error, Result};

pub trait Assets {
    /// The file at `path`, unless it is larger than `max` bytes.
    fn read(&self, path: &str, max: u64) -> Result<Vec<u8>>;
    /// The video at `path`, which may stay on disk.
    fn video(&self, path: &str) -> Result<VideoData>;
    /// Changes when the file at `path` does; `None` if that is not known.
    fn stamp(&self, path: &str) -> Option<u64>;
}

fn bad(path: &str, message: impl Into<String>) -> Error {
    Error::Asset {
        path: path.to_owned(),
        message: message.into(),
    }
}

/// A wallpaper folder.
#[derive(Debug, Clone)]
pub struct Folder(PathBuf);

impl Folder {
    pub fn new(dir: &Path) -> Self {
        Self(dir.to_path_buf())
    }

    /// The file at `path`, which must be inside the folder also after
    /// following links.
    pub(crate) fn file(&self, path: &str) -> Result<PathBuf> {
        let relative = Path::new(path);
        let plain = !path.is_empty()
            && !path.contains(['\\', ':', '\0'])
            && relative
                .components()
                .all(|c| matches!(c, Component::Normal(_)));
        if !plain {
            return Err(bad(path, "not a path inside the wallpaper"));
        }
        let missing = |e: std::io::Error| bad(path, e.to_string());
        let root = self.0.canonicalize().map_err(missing)?;
        let file = root.join(relative).canonicalize().map_err(missing)?;
        if !file.starts_with(&root) {
            return Err(bad(path, "not inside the wallpaper"));
        }
        if !file.is_file() {
            return Err(bad(path, "not a file"));
        }
        // Without Windows' verbatim prefix, which not every decoder takes.
        Ok(self.0.join(relative))
    }
}

impl Assets for Folder {
    fn read(&self, path: &str, max: u64) -> Result<Vec<u8>> {
        let file = std::fs::File::open(self.file(path)?).map_err(|e| bad(path, e.to_string()))?;
        let mut data = Vec::new();
        file.take(max.saturating_add(1))
            .read_to_end(&mut data)
            .map_err(|e| bad(path, e.to_string()))?;
        if data.len() as u64 > max {
            return Err(bad(path, "too large"));
        }
        Ok(data)
    }

    fn video(&self, path: &str) -> Result<VideoData> {
        Ok(VideoData::File(self.file(path)?))
    }

    fn stamp(&self, path: &str) -> Option<u64> {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let meta = std::fs::metadata(self.file(path).ok()?).ok()?;
        let mut hasher = DefaultHasher::new();
        (meta.len(), meta.modified().ok()?).hash(&mut hasher);
        Some(hasher.finish())
    }
}

/// Files in memory (tests, fuzzing).
#[derive(Debug, Clone, Default)]
pub struct Memory(pub BTreeMap<String, Vec<u8>>);

impl Assets for Memory {
    fn read(&self, path: &str, max: u64) -> Result<Vec<u8>> {
        match self.0.get(path) {
            Some(data) if data.len() as u64 <= max => Ok(data.clone()),
            Some(_) => Err(bad(path, "too large")),
            None => Err(bad(path, "not found")),
        }
    }

    fn video(&self, path: &str) -> Result<VideoData> {
        let data: Arc<[u8]> = self.read(path, u64::MAX)?.into();
        Ok(VideoData::Memory(data))
    }

    fn stamp(&self, _path: &str) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assets_stay_inside_the_folder() {
        let root = std::env::temp_dir().join(format!("diorama-assets-{}", std::process::id()));
        let dir = root.join("wallpaper");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets").join("a.txt"), "inside").unwrap();
        std::fs::write(root.join("secret.txt"), "outside").unwrap();
        let folder = Folder::new(&dir);
        assert_eq!(folder.read("assets/a.txt", 100).unwrap(), b"inside");
        assert!(folder.read("assets/a.txt", 3).is_err(), "too large");
        assert!(folder.read("assets", 100).is_err(), "a folder");
        assert!(folder.read("assets/b.txt", 100).is_err(), "missing");
        let absolute = root.join("secret.txt").display().to_string();
        for path in [
            "",
            "../secret.txt",
            "assets/../../secret.txt",
            "./assets/a.txt",
            "..\\secret.txt",
            "/etc/passwd",
            "C:secret.txt",
            "assets/a.txt:stream",
            absolute.as_str(),
        ] {
            assert!(folder.read(path, 100).is_err(), "{path}");
        }
        assert!(matches!(
            folder.video("assets/a.txt"),
            Ok(VideoData::File(_))
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
}
