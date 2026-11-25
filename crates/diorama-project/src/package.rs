//! `.diorama` files: a wallpaper folder as a zip, to share. They come from
//! other people, so reading one is careful: only files a wallpaper has, names
//! that stay inside the folder, and limits on how much is unpacked.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;
use zip::read::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::edit::{AssetKind, failed, new_dir, open, reserved};
use crate::{DOCUMENT, Error, Folder, Result};

/// File name extension.
pub const EXTENSION: &str = "diorama";
const MAX_FILES: usize = 2000;
/// Most bytes unpacked from one package.
const MAX_BYTES: u64 = 4 << 30;
/// Longest path in a package.
const MAX_PATH: usize = 200;

/// Writes the wallpaper in `dir` to the package `file`: its document,
/// `project.json`, preview, and the assets the document uses.
pub fn export(dir: &Path, file: &Path) -> Result<()> {
    let (document, project) = open(dir)?;
    let folder = Folder::new(dir);
    let mut names = BTreeSet::new();
    files_named(&document, &folder, &mut names);
    if let Some(preview) = project.get("preview") {
        files_named(preview, &folder, &mut names);
    }
    let tmp = file.with_extension("diorama.tmp");
    let error = |e: &dyn std::fmt::Display| failed(file, e);
    let written = (|| {
        let out = fs::File::create(&tmp).map_err(|e| error(&e))?;
        let mut zip = ZipWriter::new(io::BufWriter::new(out));
        // Project and document first: what a reader looks for.
        for name in ["project.json", DOCUMENT]
            .into_iter()
            .chain(names.iter().map(String::as_str))
        {
            let packed = matches!(
                name.rsplit_once('.'),
                Some((_, "json" | "bmp" | "tga" | "ttf" | "otf"))
            );
            let options = SimpleFileOptions::default()
                .compression_method(if packed {
                    zip::CompressionMethod::Deflated
                } else {
                    zip::CompressionMethod::Stored
                })
                .large_file(true);
            zip.start_file(name, options).map_err(|e| error(&e))?;
            let path = folder.file(name)?;
            let mut input = fs::File::open(&path).map_err(|e| failed(&path, &e))?;
            io::copy(&mut input, &mut zip).map_err(|e| error(&e))?;
        }
        let mut out = zip.finish().map_err(|e| error(&e))?;
        out.flush().map_err(|e| error(&e))?;
        Ok(())
    })();
    let written = written.and_then(|()| fs::rename(&tmp, file).map_err(|e| error(&e)));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// The asset files whose paths are values anywhere in `value`.
fn files_named(value: &Value, folder: &Folder, names: &mut BTreeSet<String>) {
    match value {
        Value::String(s) => {
            if allowed(s) && folder.file(s).is_ok() && s != DOCUMENT && s != "project.json" {
                names.insert(s.clone());
            }
        }
        Value::Array(a) => a.iter().for_each(|v| files_named(v, folder, names)),
        Value::Object(o) => o.values().for_each(|v| files_named(v, folder, names)),
        _ => {}
    }
}

/// Unpacks `file` into a new folder under `parent`, named after its title.
pub fn import(file: &Path, parent: &Path) -> Result<PathBuf> {
    let input = fs::File::open(file).map_err(|e| failed(file, &e))?;
    unpack(io::BufReader::new(input), parent).map_err(|e| match e {
        Error::Asset { path, message } if path.is_empty() => failed(file, &message),
        e => e,
    })
}

/// [`import`] from any reader.
pub fn unpack(input: impl Read + Seek, parent: &Path) -> Result<PathBuf> {
    let bad = |message: &dyn std::fmt::Display| Error::Asset {
        path: String::new(),
        message: message.to_string(),
    };
    let mut zip = ZipArchive::new(input).map_err(|e| bad(&e))?;
    if zip.len() > MAX_FILES {
        return Err(bad(&"too many files"));
    }
    // Look before making anything.
    let mut seen = HashSet::new();
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i).map_err(|e| bad(&e))?;
        let name = entry.name().map_err(|e| bad(&e))?;
        if entry.is_dir() || !allowed(&name) {
            continue;
        }
        if entry.encrypted() || entry.is_symlink() {
            return Err(bad(&"not a Diorama wallpaper"));
        }
        if !seen.insert(name.to_lowercase()) {
            return Err(bad(&"two files of one name"));
        }
    }
    if !seen.contains("project.json") || !seen.contains(DOCUMENT) {
        return Err(bad(&"not a Diorama wallpaper"));
    }
    let dir = new_dir(parent, "unpacking")?;
    let unpacked = files(&mut zip, &dir, &bad).and_then(|()| {
        let (_, project) = open(&dir)?;
        if !crate::is_project(&dir) {
            return Err(bad(&"not a Diorama wallpaper"));
        }
        let title = project.get("title").and_then(Value::as_str);
        let named = new_dir(parent, title.unwrap_or("wallpaper"))?;
        // An empty folder's place can be taken on every system but Windows.
        fs::remove_dir(&named)
            .and_then(|()| fs::rename(&dir, &named))
            .map_err(|e| failed(&named, &e))?;
        Ok(named)
    });
    if unpacked.is_err() {
        let _ = fs::remove_dir_all(&dir);
    }
    unpacked
}

fn files<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    dir: &Path,
    bad: &dyn Fn(&dyn std::fmt::Display) -> Error,
) -> Result<()> {
    let mut left = MAX_BYTES;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| bad(&e))?;
        let name = entry.name().map_err(|e| bad(&e))?.into_owned();
        if entry.is_dir() || !allowed(&name) {
            continue;
        }
        let path = dir.join(&name);
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder).map_err(|e| failed(folder, &e))?;
        }
        let mut out = fs::File::create_new(&path).map_err(|e| failed(&path, &e))?;
        // Sizes in the package may lie: what is read counts.
        let copied =
            io::copy(&mut (&mut entry).take(left + 1), &mut out).map_err(|e| failed(&path, &e))?;
        if copied > left {
            return Err(bad(&"too large"));
        }
        left -= copied;
    }
    Ok(())
}

/// Whether a wallpaper may have a file `name`.
fn allowed(name: &str) -> bool {
    let part = |p: &str| {
        let stem = p.split('.').next().unwrap_or_default();
        !p.is_empty()
            && !p.starts_with('.')
            && !p.ends_with(['.', ' '])
            && !reserved(stem.trim_end())
            && !p.contains(|c: char| c.is_control() || "\\:<>\"|?*".contains(c))
    };
    name.len() <= MAX_PATH
        && name.split('/').count() <= 8
        && name.split('/').all(part)
        && (name.to_lowercase().ends_with(".json") || AssetKind::of(Path::new(name)).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("diorama-package-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn zipped(files: &[(&str, &[u8])]) -> io::Cursor<Vec<u8>> {
        let mut zip = ZipWriter::new(io::Cursor::new(Vec::new()));
        for (name, data) in files {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        let mut out = zip.finish().unwrap();
        out.set_position(0);
        out
    }

    const PROJECT: &[u8] = br#"{"type": "diorama", "title": "Shared: one"}"#;
    const WALLPAPER: &[u8] = br#"{"version": 1, "size": [10, 10], "layers": []}"#;

    #[test]
    fn wallpapers_travel() {
        let root = temp("travel");
        let dir = edit::create(&root, "Travel", [100, 50]).unwrap();
        let photo = root.join("photo.png");
        fs::write(&photo, "pixels").unwrap();
        let used = edit::add_asset(&dir, &photo).unwrap();
        fs::write(&photo, "others").unwrap();
        let unused = edit::add_asset(&dir, &photo).unwrap();
        fs::write(dir.join("notes.txt"), "mine").unwrap();
        let mut document = edit::open(&dir).unwrap().0;
        document["layers"] = json!([
            {"type": "image", "source": used, "extra": ["../travel.diorama", "C:/x.png"]},
        ]);
        edit::save(&dir, &document).unwrap();
        edit::save_preview(&dir, &[9; 400], 10, 10).unwrap();
        edit::save_draft(&dir, Some((&document, &json!({})))).unwrap();

        let file = root.join("travel.diorama");
        export(&dir, &file).unwrap();
        let there = root.join("there");
        let got = import(&file, &there).unwrap();
        assert_eq!(got.file_name().unwrap(), "Travel");
        assert_eq!(edit::open(&got).unwrap(), edit::open(&dir).unwrap());
        assert_eq!(fs::read(got.join(&used)).unwrap(), b"pixels");
        assert!(got.join("preview.png").is_file());
        assert!(!got.join(&unused).exists(), "unused assets stay behind");
        assert!(!got.join("notes.txt").exists());
        assert!(edit::draft(&got).is_none());
        // Again: beside the first.
        assert_eq!(
            import(&file, &there).unwrap().file_name().unwrap(),
            "Travel 2"
        );
        assert_eq!(
            fs::read_dir(&there).unwrap().count(),
            2,
            "nothing left over"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn strange_packages_are_refused_or_cleaned() {
        let root = temp("strange");
        let outside = [
            ("project.json", PROJECT),
            ("wallpaper.json", WALLPAPER),
            ("../escape.png", b"x".as_slice()),
            ("/root.png", b"x"),
            ("a/../../b.png", b"x"),
            ("c:\\d.png", b"x"),
            ("run.exe", b"x"),
            ("assets/con.png", b"x"),
            ("assets/.hidden.png", b"x"),
            ("assets/ok.png", b"x"),
        ];
        let dir = unpack(zipped(&outside), &root.join("in")).unwrap();
        assert_eq!(dir.file_name().unwrap(), "Shared- one");
        let mut names: Vec<_> = walk(&dir);
        names.sort();
        assert_eq!(names, ["assets/ok.png", "project.json", "wallpaper.json"]);
        assert_eq!(walk(&root).len(), 3, "nothing outside");

        let refused = |files: &[(&str, &[u8])]| unpack(zipped(files), &root.join("no")).is_err();
        assert!(refused(&[("project.json", PROJECT)]), "no document");
        assert!(
            refused(&[
                ("project.json", br#"{"type": "web"}"#),
                ("wallpaper.json", WALLPAPER)
            ]),
            "another type"
        );
        assert!(
            refused(&[
                ("project.json", PROJECT),
                ("wallpaper.json", br#"{"version": 99, "size": [1, 1]}"#)
            ]),
            "a newer format"
        );
        assert!(
            refused(&[
                ("project.json", PROJECT),
                ("wallpaper.json", WALLPAPER),
                ("assets/a.png", b"1"),
                ("assets/A.png", b"2")
            ]),
            "names that differ in case"
        );
        assert!(unpack(io::Cursor::new(b"not a zip".to_vec()), &root.join("no")).is_err());
        assert!(
            !root.join("no").exists() || fs::read_dir(root.join("no")).unwrap().count() == 0,
            "nothing left over"
        );
        let _ = fs::remove_dir_all(&root);
    }

    fn walk(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                if e.path().is_dir() {
                    stack.push(e.path());
                } else {
                    let path = e.path();
                    let relative = path.strip_prefix(dir).unwrap().to_string_lossy();
                    out.push(relative.replace('\\', "/"));
                }
            }
        }
        out
    }
}
