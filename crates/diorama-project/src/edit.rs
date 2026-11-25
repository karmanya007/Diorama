//! Making and changing wallpaper folders (the editor's file work).

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{Assets, DOCUMENT, Error, Folder, Result, VERSION};

/// What is changed and not saved yet.
const DRAFT: &str = "draft.json";
/// Largest video asset copied in.
const MAX_VIDEO: u64 = 4 << 30;
/// Longest folder or file name made here, in characters.
const MAX_NAME: usize = 60;

/// What an asset file is, by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Image,
    Video,
    Font,
}

impl AssetKind {
    pub const IMAGES: [&'static str; 7] = ["png", "jpg", "jpeg", "webp", "bmp", "tga", "gif"];
    pub const VIDEOS: [&'static str; 7] = ["mp4", "m4v", "mov", "webm", "mkv", "avi", "wmv"];
    pub const FONTS: [&'static str; 3] = ["ttf", "otf", "ttc"];

    pub fn of(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        let is = |known: &[&str]| known.contains(&ext.as_str());
        if is(&Self::IMAGES) {
            Some(Self::Image)
        } else if is(&Self::VIDEOS) {
            Some(Self::Video)
        } else if is(&Self::FONTS) {
            Some(Self::Font)
        } else {
            None
        }
    }

    fn largest(self) -> u64 {
        match self {
            Self::Image => crate::compile::MAX_IMAGE,
            Self::Video => MAX_VIDEO,
            Self::Font => crate::compile::MAX_FONT,
        }
    }
}

pub(crate) fn failed(path: &Path, e: &dyn std::fmt::Display) -> Error {
    Error::Asset {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

/// The document and `project.json` of the wallpaper in `dir`.
pub fn open(dir: &Path) -> Result<(Value, Value)> {
    let folder = Folder::new(dir);
    let read = |name: &str| {
        let data = folder.read(name, crate::MAX_JSON)?;
        crate::parse(&data).map_err(|e| failed(&dir.join(name), &e))
    };
    let (document, project) = (read(DOCUMENT)?, read("project.json")?);
    let version = document.get("version").and_then(Value::as_u64);
    match version {
        Some(v) if v > VERSION => Err(Error::Newer(v)),
        Some(_) if document.is_object() => Ok((document, project)),
        _ => Err(Error::Document("no version".into())),
    }
}

/// Makes an empty wallpaper in a new folder under `parent`.
pub fn create(parent: &Path, title: &str, size: [u32; 2]) -> Result<PathBuf> {
    let title = match title.trim() {
        "" => "New wallpaper",
        t => t,
    };
    let dir = new_dir(parent, title)?;
    let assets = dir.join("assets");
    fs::create_dir_all(&assets).map_err(|e| failed(&assets, &e))?;
    save_project(
        &dir,
        &json!({
            "title": title,
            "description": "",
            "type": "diorama",
            "file": DOCUMENT,
            "tags": [],
            "contentrating": "Everyone",
            "general": { "properties": {} },
        }),
    )?;
    save(
        &dir,
        &json!({
            "version": VERSION,
            "size": size,
            "background": "#202638",
            "parallax": { "amount": 0.03 },
            "layers": [],
        }),
    )?;
    Ok(dir)
}

/// Writes the document.
pub fn save(dir: &Path, document: &Value) -> Result<()> {
    write_json(&dir.join(DOCUMENT), document)
}

/// Writes `project.json`.
pub fn save_project(dir: &Path, project: &Value) -> Result<()> {
    write_json(&dir.join("project.json"), project)
}

/// What was changed and not saved yet, if anything: the document and
/// project.json, as [`open`] gives them.
pub fn draft(dir: &Path) -> Option<(Value, Value)> {
    let data = Folder::new(dir).read(DRAFT, 2 * crate::MAX_JSON).ok()?;
    let mut draft = crate::parse(&data).ok()?;
    let (document, project) = (draft["document"].take(), draft["project"].take());
    let known = document["version"].as_u64().is_some_and(|v| v <= VERSION);
    (known && project.is_object()).then_some((document, project))
}

/// Keeps what is not saved yet (`None`: nothing).
pub fn save_draft(dir: &Path, draft: Option<(&Value, &Value)>) -> Result<()> {
    let path = dir.join(DRAFT);
    match draft {
        Some((document, project)) => {
            let value = json!({ "document": document, "project": project });
            let data = serde_json::to_vec(&value).map_err(|e| failed(&path, &e))?;
            replace(&path, &data).map_err(|e| failed(&path, &e))
        }
        None => match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(failed(&path, &e)),
            _ => Ok(()),
        },
    }
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let mut data = serde_json::to_vec_pretty(value).map_err(|e| failed(path, &e))?;
    data.push(b'\n');
    if data.len() as u64 > crate::MAX_JSON {
        return Err(failed(path, &"too large"));
    }
    replace(path, &data).map_err(|e| failed(path, &e))
}

/// Writes a whole file beside `path`, then puts it there: a crash leaves the
/// old one.
fn replace(path: &Path, data: &[u8]) -> io::Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!("{name}.tmp"));
    let mut file = fs::File::create(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)
}

/// Copies the file at `source` into the wallpaper's assets; its path there.
pub fn add_asset(dir: &Path, source: &Path) -> Result<String> {
    let kind = AssetKind::of(source).ok_or_else(|| failed(source, &"not a usable file type"))?;
    let meta = fs::metadata(source).map_err(|e| failed(source, &e))?;
    if !meta.is_file() {
        return Err(failed(source, &"not a file"));
    }
    if meta.len() > kind.largest() {
        return Err(failed(source, &"too large"));
    }
    let assets = dir.join("assets");
    fs::create_dir_all(&assets).map_err(|e| failed(&assets, &e))?;
    // One that is in there already is used as it is.
    if let (Ok(a), Ok(b)) = (assets.canonicalize(), source.canonicalize())
        && b.parent() == Some(a.as_path())
        && let Some(name) = b.file_name().and_then(|n| n.to_str())
    {
        return Ok(format!("assets/{name}"));
    }
    let same = |path: &Path| {
        fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == meta.len())
            && same_content(path, source).unwrap_or(false)
    };
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    let ext = source.extension().unwrap_or_default().to_string_lossy();
    let (stem, ext) = (tidy(&stem, "asset"), ext.to_ascii_lowercase());
    for n in 1..1000 {
        let name = match n {
            1 => format!("{stem}.{ext}"),
            _ => format!("{stem}-{n}.{ext}"),
        };
        let target = assets.join(&name);
        if target.exists() {
            // Added before.
            if same(&target) {
                return Ok(format!("assets/{name}"));
            }
            continue;
        }
        fs::copy(source, &target).map_err(|e| failed(&target, &e))?;
        return Ok(format!("assets/{name}"));
    }
    Err(failed(source, &"too many files of that name"))
}

/// Saves the wallpaper's preview picture (RGBA) and names it in `project.json`.
pub fn save_preview(dir: &Path, rgba: &[u8], width: u32, height: u32) -> Result<()> {
    let path = dir.join("preview.png");
    let mut png = io::Cursor::new(Vec::new());
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| failed(&path, &"bad picture"))?;
    let image = image::DynamicImage::ImageRgba8(image)
        .thumbnail(640, 640)
        .into_rgb8();
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| failed(&path, &e))?;
    replace(&path, png.get_ref()).map_err(|e| failed(&path, &e))?;
    let (_, mut project) = open(dir)?;
    if project.get("preview").and_then(Value::as_str) != Some("preview.png")
        && let Some(map) = project.as_object_mut()
    {
        map.insert("preview".into(), "preview.png".into());
        save_project(dir, &project)?;
    }
    Ok(())
}

fn same_content(a: &Path, b: &Path) -> io::Result<bool> {
    let mut files = [a, b].map(fs::File::open).into_iter();
    let (Some(a), Some(b)) = (files.next(), files.next()) else {
        return Ok(false);
    };
    let (mut a, mut b) = (a?, b?);
    let mut blocks = [vec![0; 1 << 20], vec![0; 1 << 20]];
    let [x, y] = &mut blocks;
    loop {
        let n = a.read(x)?;
        if n == 0 {
            return Ok(b.read(y)? == 0);
        }
        b.read_exact(&mut y[..n])?;
        if x[..n] != y[..n] {
            return Ok(false);
        }
    }
}

/// A new folder under `parent` named after `title`.
pub(crate) fn new_dir(parent: &Path, title: &str) -> Result<PathBuf> {
    fs::create_dir_all(parent).map_err(|e| failed(parent, &e))?;
    let name = tidy(title, "wallpaper");
    for n in 1..1000 {
        let dir = parent.join(match n {
            1 => name.clone(),
            _ => format!("{name} {n}"),
        });
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(failed(&dir, &e)),
        }
    }
    Err(failed(parent, &"too many folders of that name"))
}

/// `name` as a file name every system takes.
fn tidy(name: &str, fallback: &str) -> String {
    let kept: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_".contains(c) {
                c
            } else {
                '-'
            }
        })
        .take(MAX_NAME)
        .collect();
    let kept = kept.trim_matches([' ', '-']);
    if kept.is_empty() || reserved(kept) {
        format!("{fallback} {kept}").trim().to_owned()
    } else {
        kept.to_owned()
    }
}

/// Names Windows keeps for devices.
pub(crate) fn reserved(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.ends_with(|c: char| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("diorama-edit-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_new_wallpaper_can_be_opened_changed_and_shown() {
        let root = temp("new");
        let dir = create(&root, "  My: wall/paper?  ", [1920, 1080]).unwrap();
        assert_eq!(dir.file_name().unwrap(), "My- wall-paper");
        assert!(crate::is_project(&dir));
        let second = create(&root, "My: wall/paper?", [1920, 1080]).unwrap();
        assert_eq!(second.file_name().unwrap(), "My- wall-paper 2");

        let (mut document, project) = open(&dir).unwrap();
        assert_eq!(project["title"], "My: wall/paper?");
        document["layers"] = json!([{"type": "color", "mine": 7}]);
        save(&dir, &document).unwrap();
        // What this version does not know is kept.
        assert_eq!(open(&dir).unwrap().0["layers"][0]["mine"], 7);
        let scene = crate::load(
            &dir,
            &Default::default(),
            &crate::NoHost,
            &diorama_core::Limits::default(),
        )
        .unwrap();
        assert_eq!(scene.layers.len(), 1);
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 3, "no leftovers: {names:?}");

        save_preview(&dir, &[200; 4 * 1280 * 720], 1280, 720).unwrap();
        assert_eq!(open(&dir).unwrap().1["preview"], "preview.png");
        let picture = image::open(dir.join("preview.png")).unwrap();
        assert_eq!((picture.width(), picture.height()), (640, 360));
        assert!(save_preview(&dir, &[0; 5], 1280, 720).is_err());

        save(&dir, &json!({"version": VERSION + 1})).unwrap();
        assert!(matches!(open(&dir), Err(Error::Newer(_))));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn drafts_are_kept_until_saved() {
        let root = temp("draft");
        let dir = create(&root, "w", [10, 10]).unwrap();
        assert!(draft(&dir).is_none());
        let (mut document, project) = open(&dir).unwrap();
        document["mine"] = json!(1);
        save_draft(&dir, Some((&document, &project))).unwrap();
        assert_eq!(draft(&dir), Some((document, project.clone())));
        // The wallpaper itself is as it was.
        assert!(open(&dir).unwrap().0.get("mine").is_none());
        save_draft(&dir, Some((&json!({"version": VERSION + 1}), &project))).unwrap();
        assert!(draft(&dir).is_none(), "of a newer version");
        fs::write(dir.join(DRAFT), "{").unwrap();
        assert!(draft(&dir).is_none(), "broken");
        save_draft(&dir, None).unwrap();
        save_draft(&dir, None).unwrap();
        assert!(!dir.join(DRAFT).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn assets_are_copied_under_names_of_their_own() {
        let root = temp("assets");
        let dir = create(&root, "w", [10, 10]).unwrap();
        let source = root.join("Photo: no 1.PNG".replace(':', ""));
        fs::write(&source, "one").unwrap();
        assert_eq!(add_asset(&dir, &source).unwrap(), "assets/Photo no 1.png");
        fs::write(&source, "two").unwrap();
        assert_eq!(add_asset(&dir, &source).unwrap(), "assets/Photo no 1-2.png");
        assert_eq!(fs::read(dir.join("assets/Photo no 1.png")).unwrap(), b"one");
        // The same file again is not copied again.
        assert_eq!(add_asset(&dir, &source).unwrap(), "assets/Photo no 1-2.png");
        fs::write(&source, "owt").unwrap();
        assert_eq!(add_asset(&dir, &source).unwrap(), "assets/Photo no 1-3.png");
        let inside = dir.join("assets/Photo no 1.png");
        assert_eq!(add_asset(&dir, &inside).unwrap(), "assets/Photo no 1.png");
        let program = root.join("run.exe");
        fs::write(&program, "x").unwrap();
        assert!(add_asset(&dir, &program).is_err());
        assert!(add_asset(&dir, &root.join("missing.png")).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn names_suit_every_system() {
        assert_eq!(tidy("con", "wallpaper"), "wallpaper con");
        assert_eq!(tidy("...", "wallpaper"), "wallpaper");
        assert_eq!(tidy("Été 2026", "wallpaper"), "Été 2026");
        assert_eq!(tidy(&"x".repeat(300), "wallpaper").len(), MAX_NAME);
        assert!(reserved("lpt1") && !reserved("lptx") && !reserved("comet"));
    }
}
