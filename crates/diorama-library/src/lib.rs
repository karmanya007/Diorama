//! The user's wallpapers: Wallpaper Engine Workshop items and projects found
//! through Steam's library folders, plus folders the user adds (project
//! folders, images and videos). Content is untrusted: `project.json` is read
//! with size limits and nothing in it is followed outside its folder.

mod vdf;

use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// Wallpaper Engine's Steam app id (its Workshop folder name).
pub const WE_APP_ID: &str = "431960";
/// Largest `project.json` read.
const MAX_PROJECT: u64 = 4 << 20;
/// Files counted per item for its size on disk.
const MAX_FILES: usize = 20_000;

/// Where to look.
#[derive(Debug, Clone, Default)]
pub struct Roots {
    /// Steam library folders (each holding `steamapps`).
    pub steam_libraries: Vec<PathBuf>,
    /// Folders the user added: project folders, images and videos inside.
    pub folders: Vec<PathBuf>,
}

impl Roots {
    /// Steam's library folders from its install folder (`libraryfolders.vdf`),
    /// the install folder itself first.
    pub fn from_steam(steam_dir: &Path) -> Self {
        let mut libraries = vec![steam_dir.to_path_buf()];
        let vdf = steam_dir.join("steamapps").join("libraryfolders.vdf");
        if let Ok(text) = read_limited(&vdf, MAX_PROJECT) {
            for path in vdf::library_paths(&text) {
                if !libraries.iter().any(|l| same(l, &path)) {
                    libraries.push(path);
                }
            }
        }
        Self {
            steam_libraries: libraries,
            folders: Vec::new(),
        }
    }
}

/// What a wallpaper is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Scene,
    Video,
    Web,
    Image,
    /// A WE application wallpaper (runs a program; Diorama does not play these).
    Application,
    /// A WE preset of another wallpaper.
    Preset,
    Unknown,
}

impl Kind {
    /// Diorama can show it.
    pub fn playable(self) -> bool {
        matches!(self, Self::Scene | Self::Video | Self::Web | Self::Image)
    }
}

/// Where an item came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// A subscribed Steam Workshop item.
    Workshop,
    /// One of WE's own projects (`projects/defaultprojects`, `myprojects`).
    WallpaperEngine,
    /// A folder the user added.
    Folder,
}

/// The author's age rating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rating {
    Everyone,
    Questionable,
    Mature,
    Unknown,
}

/// One wallpaper.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// Workshop id, or the folder or file name.
    pub id: String,
    /// The project folder, or the image/video file.
    pub path: PathBuf,
    pub source: Source,
    pub kind: Kind,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub rating: Rating,
    /// Preview image or GIF (inside the item's folder).
    pub preview: Option<PathBuf>,
    /// Declared properties (`general.properties` of `project.json`).
    pub properties: Value,
    pub workshop_id: Option<String>,
    /// Bytes on disk.
    pub size: u64,
    /// Last change, seconds since the Unix epoch.
    pub modified: u64,
}

/// Every wallpaper under `roots`, Workshop items first.
pub fn scan(roots: &Roots) -> Vec<Item> {
    let mut items = Vec::new();
    for lib in &roots.steam_libraries {
        let apps = lib.join("steamapps");
        let workshop = apps.join("workshop").join("content").join(WE_APP_ID);
        items.extend(projects_in(&workshop, Source::Workshop));
        let projects = apps
            .join("common")
            .join("wallpaper_engine")
            .join("projects");
        for sub in ["myprojects", "defaultprojects"] {
            items.extend(projects_in(&projects.join(sub), Source::WallpaperEngine));
        }
    }
    for folder in &roots.folders {
        items.extend(folder_items(folder));
    }
    items
}

/// Project folders (with `project.json`) directly inside `dir`.
fn projects_in(dir: &Path, source: Source) -> Vec<Item> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<Item> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| project(&e.path(), source))
        .collect();
    items.sort_by(|a, b| a.id.cmp(&b.id));
    items
}

/// A user folder: itself as a project, or the projects, images and videos in it.
fn folder_items(dir: &Path) -> Vec<Item> {
    if let Some(item) = project(dir, Source::Folder) {
        return vec![item];
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<Item> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let t = e.file_type().ok()?;
            if t.is_dir() {
                project(&path, Source::Folder)
            } else if t.is_file() {
                media_file(&path)
            } else {
                None
            }
        })
        .collect();
    items.sort_by_key(|i| i.title.to_lowercase());
    items
}

/// A WE project folder from its `project.json`.
pub fn project(dir: &Path, source: Source) -> Option<Item> {
    let text = read_limited(&dir.join("project.json"), MAX_PROJECT).ok()?;
    let p: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let text_of = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().trim();
    let id = dir.file_name()?.to_string_lossy().into_owned();
    let kind = match text_of("type").to_ascii_lowercase().as_str() {
        "scene" => Kind::Scene,
        "video" => Kind::Video,
        "web" => Kind::Web,
        "application" => Kind::Application,
        "preset" => Kind::Preset,
        // Some of WE's own projects have no type: go by their file.
        _ => kind_of_file(text_of("file")),
    };
    let rating = match text_of("contentrating").to_ascii_lowercase().as_str() {
        "everyone" => Rating::Everyone,
        "questionable" => Rating::Questionable,
        "mature" => Rating::Mature,
        _ => Rating::Unknown,
    };
    let workshop_id = match p.get("workshopid") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => (source == Source::Workshop).then(|| id.clone()),
    };
    let (size, modified) = disk_usage(dir);
    Some(Item {
        title: match text_of("title") {
            "" => id.clone(),
            t => t.to_owned(),
        },
        id,
        path: dir.to_path_buf(),
        source,
        kind,
        description: text_of("description").to_owned(),
        tags: p
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        rating,
        preview: inside(dir, text_of("preview")),
        properties: p
            .get("general")
            .and_then(|g| g.get("properties"))
            .cloned()
            .unwrap_or(Value::Null),
        workshop_id,
        size,
        modified,
    })
}

/// A project's kind from its `file`.
fn kind_of_file(file: &str) -> Kind {
    let ext = Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "json" | "pkg" => Kind::Scene,
        "html" | "htm" => Kind::Web,
        "exe" => Kind::Application,
        "mp4" | "webm" | "mkv" | "mov" | "avi" | "wmv" | "m4v" => Kind::Video,
        _ => Kind::Unknown,
    }
}

/// An image or video file as an item.
fn media_file(path: &Path) -> Option<Item> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let kind = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp" | "tif" | "tiff" => Kind::Image,
        "mp4" | "webm" | "mkv" | "mov" | "avi" | "wmv" | "m4v" => Kind::Video,
        _ => return None,
    };
    let meta = std::fs::metadata(path).ok()?;
    let title = path.file_stem()?.to_string_lossy().into_owned();
    Some(Item {
        id: path.file_name()?.to_string_lossy().into_owned(),
        path: path.to_path_buf(),
        source: Source::Folder,
        kind,
        title,
        description: String::new(),
        tags: Vec::new(),
        rating: Rating::Unknown,
        preview: (kind == Kind::Image).then(|| path.to_path_buf()),
        properties: Value::Null,
        workshop_id: None,
        size: meta.len(),
        modified: seconds(meta.modified().ok()),
    })
}

/// `name` inside `dir`, if it is a plain relative path to an existing file.
fn inside(dir: &Path, name: &str) -> Option<PathBuf> {
    let rel = Path::new(name);
    if name.is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let path = dir.join(rel);
    path.is_file().then_some(path)
}

/// Bytes under `dir` and the newest change (bounded walk).
fn disk_usage(dir: &Path) -> (u64, u64) {
    let (mut size, mut newest, mut files) = (0u64, 0u64, 0usize);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                stack.push(e.path());
            } else if t.is_file()
                && let Ok(m) = e.metadata()
            {
                size += m.len();
                newest = newest.max(seconds(m.modified().ok()));
                files += 1;
                if files >= MAX_FILES {
                    return (size, newest);
                }
            }
        }
    }
    (size, newest)
}

fn seconds(t: Option<std::time::SystemTime>) -> u64 {
    t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

fn read_limited(path: &Path, max: u64) -> std::io::Result<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(max)
        .read_to_string(&mut text)?;
    Ok(text)
}

fn same(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_wallpapers_are_listed_with_previews_and_settings() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/wallpapers");
        let items = folder_items(&dir);
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Aurora", "Dunes", "Paper Valley"]);
        for item in &items {
            assert_eq!(item.kind, Kind::Web, "{}", item.title);
            assert!(
                item.preview.as_ref().is_some_and(|p| p.is_file()),
                "{}",
                item.title
            );
            assert!(
                item.properties.get("schemecolor").is_some(),
                "{}",
                item.title
            );
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("diorama-library-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_workshop_items_projects_and_loose_files() {
        let steam = temp("steam");
        let workshop = steam.join("steamapps/workshop/content/431960/123");
        std::fs::create_dir_all(&workshop).unwrap();
        std::fs::write(
            workshop.join("project.json"),
            "\u{feff}{\"title\":\"Rain\",\"type\":\"Scene\",\"tags\":[\"Nature\"],\"contentrating\":\"Everyone\",\"preview\":\"preview.gif\",\"general\":{\"properties\":{\"speed\":{\"type\":\"slider\",\"value\":1}}}}",
        )
        .unwrap();
        std::fs::write(workshop.join("preview.gif"), b"GIF89a").unwrap();
        let mine = steam.join("steamapps/common/wallpaper_engine/projects/myprojects/clock");
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::write(
            mine.join("project.json"),
            r#"{"title":"Clock","file":"index.html","preview":"../../../../../secret.png"}"#,
        )
        .unwrap();
        let folder = temp("folder");
        std::fs::write(folder.join("beach.mp4"), b"x").unwrap();
        std::fs::write(folder.join("notes.txt"), b"x").unwrap();

        let roots = Roots {
            folders: vec![folder.clone()],
            ..Roots::from_steam(&steam)
        };
        let items = scan(&roots);
        let _ = std::fs::remove_dir_all(&steam);
        let _ = std::fs::remove_dir_all(&folder);
        assert_eq!(items.len(), 3, "{items:#?}");
        let rain = &items[0];
        assert_eq!(
            (rain.title.as_str(), rain.kind, rain.source),
            ("Rain", Kind::Scene, Source::Workshop)
        );
        assert_eq!(rain.workshop_id.as_deref(), Some("123"));
        assert_eq!(rain.rating, Rating::Everyone);
        assert_eq!(rain.tags, ["Nature"]);
        assert!(
            rain.preview
                .as_ref()
                .is_some_and(|p| p.ends_with("preview.gif"))
        );
        assert_eq!(rain.properties["speed"]["value"], 1);
        assert!(rain.size > 0);
        let clock = &items[1];
        assert_eq!(
            (clock.kind, clock.source),
            (Kind::Web, Source::WallpaperEngine)
        );
        assert!(
            clock.preview.is_none(),
            "preview outside the folder is ignored"
        );
        assert_eq!(
            (items[2].kind, items[2].title.as_str()),
            (Kind::Video, "beach")
        );
    }
}
