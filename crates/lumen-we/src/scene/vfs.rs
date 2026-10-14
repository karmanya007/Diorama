//! Layered file lookup used while importing a project, in WE's own order:
//! the project's `scene.pkg`, then the project directory, then the WE
//! install's `assets` directory (built-in shaders, effects, `util/*`).

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use lumen_core::Limits;

use crate::error::{Error, Result};
use crate::pkg::Pkg;

#[derive(Debug, Default)]
pub struct Vfs {
    pkg_bytes: Vec<u8>,
    /// Entry path → byte range in `pkg_bytes`.
    pkg_entries: HashMap<String, (usize, usize)>,
    project_dir: Option<PathBuf>,
    assets_dir: Option<PathBuf>,
}

impl Vfs {
    /// `project_dir` may contain `scene.pkg`; `assets_dir` is
    /// `<WE install>/assets` when available.
    pub fn open(project_dir: &Path, assets_dir: Option<&Path>, limits: &Limits) -> Result<Self> {
        let mut vfs = Self {
            project_dir: Some(project_dir.to_path_buf()),
            assets_dir: assets_dir.map(Path::to_path_buf),
            ..Self::default()
        };
        let pkg_path = project_dir.join("scene.pkg");
        if let Ok(bytes) = std::fs::read(&pkg_path) {
            let pkg = Pkg::parse(&bytes, limits)?;
            let base = bytes.as_ptr() as usize;
            for entry in pkg.entries() {
                if let Some(data) = pkg.data(entry) {
                    let start = data.as_ptr() as usize - base;
                    vfs.pkg_entries
                        .insert(entry.path.clone(), (start, start + data.len()));
                }
            }
            vfs.pkg_bytes = bytes;
        }
        Ok(vfs)
    }

    /// In-memory VFS for tests: `files` act as pkg entries.
    pub fn from_files(files: &[(&str, &[u8])]) -> Self {
        let mut vfs = Self::default();
        for (path, data) in files {
            let start = vfs.pkg_bytes.len();
            vfs.pkg_bytes.extend_from_slice(data);
            vfs.pkg_entries
                .insert((*path).to_owned(), (start, vfs.pkg_bytes.len()));
        }
        vfs
    }

    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(&(start, end)) = self.pkg_entries.get(path) {
            return self.pkg_bytes.get(start..end).map(<[u8]>::to_vec);
        }
        let rel = safe_relative(path).ok()?;
        for root in [&self.project_dir, &self.assets_dir].into_iter().flatten() {
            if let Ok(bytes) = std::fs::read(root.join(&rel)) {
                return Some(bytes);
            }
        }
        None
    }

    pub fn read_string(&self, path: &str) -> Option<String> {
        self.read(path).and_then(|b| String::from_utf8(b).ok())
    }

    pub fn read_json(&self, path: &str) -> Result<serde_json::Value> {
        let text = self.read_string(path).ok_or_else(|| Error::Unsupported {
            what: "missing file",
            value: path.to_owned(),
        })?;
        serde_json::from_str(&text).map_err(|e| Error::Unsupported {
            what: "invalid JSON",
            value: format!("{path}: {e}"),
        })
    }
}

/// Rejects absolute paths and `..` so content cannot read outside the roots.
fn safe_relative(path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return Err(Error::UnsafePath(path.to_owned())),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(Error::UnsafePath(path.to_owned()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escaping_paths() {
        assert!(safe_relative("../secret").is_err());
        assert!(safe_relative("/etc/passwd").is_err());
        assert!(safe_relative("C:\\Windows\\win.ini").is_err());
        assert!(safe_relative("").is_err());
        assert_eq!(
            safe_relative("materials/./a.tex").unwrap(),
            Path::new("materials").join("a.tex")
        );
    }

    #[test]
    fn in_memory_lookup() {
        let vfs = Vfs::from_files(&[("a.json", b"{\"x\":1}")]);
        assert_eq!(vfs.read_json("a.json").unwrap()["x"], 1);
        assert!(vfs.read("b.json").is_none());
    }
}
