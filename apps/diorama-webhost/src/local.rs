//! Files the user picked in `file` and `directory` properties. Pages show
//! them as `'file:///' + value`, which Chromium refuses from the page's
//! https origin; the page script rewrites those addresses to `PREFIX` on the
//! wallpaper's own origin, and only the picked files are served there.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Where the page script sends `file:///` addresses (keep in sync with shim.js).
pub const PREFIX: &str = "/__diorama_file__/";
/// Files listed per directory property.
const MAX_LISTED: usize = 10_000;

/// The files pages may load from outside their folder.
#[derive(Debug, Default)]
pub struct LocalFiles {
    allowed: HashSet<PathBuf>,
}

impl LocalFiles {
    /// From the project's declared properties (`project`'s `general.properties`)
    /// and their current values (`properties`, `{name: {type, value}}`). Also
    /// returns the directory properties for the page script:
    /// `{name: {mode, files: [path]}}`.
    pub fn new(project: &Value, properties: &Value) -> (Self, Value) {
        let mut allowed = HashSet::new();
        let mut directories = Map::new();
        let declared = project["general"]["properties"].as_object();
        for (name, decl) in declared.into_iter().flatten() {
            let Some(value) = properties[name]["value"].as_str().filter(|v| !v.is_empty()) else {
                continue;
            };
            let path = from_value(value);
            match decl["type"].as_str() {
                Some("file") => match path.canonicalize() {
                    Ok(p) if p.is_file() => {
                        allowed.insert(p);
                    }
                    _ => tracing::warn!("web wallpaper property {name}: no file {value}"),
                },
                Some("directory") => {
                    let files = list(&path, decl["fileType"].as_str());
                    if files.is_empty() {
                        tracing::warn!("web wallpaper property {name}: no files in {value}");
                    }
                    let shown = files.iter().map(|f| to_value(f)).collect::<Vec<_>>();
                    allowed.extend(files);
                    let mode = decl["mode"].as_str().unwrap_or("fetchall");
                    directories.insert(
                        name.clone(),
                        serde_json::json!({ "mode": mode, "files": shown }),
                    );
                }
                _ => {}
            }
        }
        (Self { allowed }, Value::Object(directories))
    }

    /// The file a `PREFIX` address names (the rest of the path, percent
    /// decoded), if the user picked it.
    pub fn resolve(&self, rest: &str) -> Option<PathBuf> {
        let file = from_value(rest).canonicalize().ok()?;
        self.allowed.contains(&file).then_some(file)
    }
}

/// A property value (`C:/Users/…/a.jpg`, as pages put it after `file:///`)
/// as a path.
fn from_value(value: &str) -> PathBuf {
    let value = value.trim_start_matches('/');
    if cfg!(windows) {
        PathBuf::from(value)
    } else {
        Path::new("/").join(value)
    }
}

/// A path as WE hands it to pages: forward slashes, no `\\?\`.
fn to_value(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.trim_start_matches('/').to_owned()
    }
}

/// Files directly inside `dir` (canonical, sorted), of the property's
/// `fileType` (`image`, `video`, `audio`) when it has one.
fn list(dir: &Path, kind: Option<&str>) -> Vec<PathBuf> {
    let Ok(dir) = dir.canonicalize() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| dir.join(e.file_name()))
        .filter(|p| kind.is_none_or(|k| of_kind(p, k)))
        .take(MAX_LISTED)
        .collect();
    files.sort();
    files
}

fn of_kind(path: &Path, kind: &str) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let exts: &[&str] = match kind {
        "image" => &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif"],
        "video" => &["mp4", "webm", "ogv", "mov", "m4v"],
        "audio" => &["mp3", "ogg", "wav", "flac", "m4a", "aac", "opus"],
        _ => return true,
    };
    exts.contains(&ext.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_picked_files_are_served() {
        let dir = std::env::temp_dir().join("diorama-webhost-local-test");
        std::fs::create_dir_all(dir.join("pics")).unwrap();
        for f in [
            "a b.jpg",
            "pics/1.png",
            "pics/2.webp",
            "pics/notes.txt",
            "secret.txt",
        ] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let dir = dir.canonicalize().unwrap();
        let project = json!({"general": {"properties": {
            "bg": {"type": "file", "value": ""},
            "folder": {"type": "directory", "fileType": "image", "mode": "ondemand", "value": ""},
            "title": {"type": "textinput", "value": ""}
        }}});
        let properties = json!({
            "bg": {"value": to_value(&dir.join("a b.jpg"))},
            "folder": {"value": to_value(&dir.join("pics"))},
            "title": {"value": to_value(&dir.join("secret.txt"))}
        });
        let (local, dirs) = LocalFiles::new(&project, &properties);
        assert!(local.resolve(&to_value(&dir.join("a b.jpg"))).is_some());
        assert!(local.resolve(&to_value(&dir.join("pics/2.webp"))).is_some());
        assert!(
            local
                .resolve(&to_value(&dir.join("pics/notes.txt")))
                .is_none()
        );
        assert!(local.resolve(&to_value(&dir.join("secret.txt"))).is_none());
        assert!(
            local
                .resolve(&to_value(&dir.join("pics/../a b.jpg")))
                .is_some()
        );
        assert_eq!(dirs["folder"]["mode"], "ondemand");
        let files = dirs["folder"]["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert!(
            files[0].as_str().unwrap().ends_with("pics/1.png"),
            "{files:?}"
        );
        assert!(!files[0].as_str().unwrap().contains('\\'));
    }
}
