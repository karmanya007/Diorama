//! Runs the importer over a local Wallpaper Engine install and Workshop folder.
//!
//! Locations: `LUMEN_WE_CORPUS` (Workshop `content/431960`) and `LUMEN_WE_INSTALL`
//! (WE install dir), else the default Steam paths. Each test is skipped (passes
//! with a note) when its folder is missing, e.g. in CI — WE content is never
//! committed to this repo.

#![allow(
    clippy::unwrap_used,
    reason = "test harness: I/O failures should fail the test loudly"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lumen_core::Limits;
use lumen_we::{Pkg, Tex, mip_to_rgba8};

const DEFAULT_CORPUS: &str = r"C:\Program Files (x86)\Steam\steamapps\workshop\content\431960";
const DEFAULT_INSTALL: &str = r"C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine";

fn dir_from_env(var: &str, default: &str) -> Option<PathBuf> {
    let dir = std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default));
    dir.is_dir().then_some(dir)
}

fn scene_pkgs(corpus: &Path) -> Vec<PathBuf> {
    let mut pkgs: Vec<_> = std::fs::read_dir(corpus)
        .unwrap()
        .map(|item| item.unwrap().path().join("scene.pkg"))
        .filter(|p| p.is_file())
        .collect();
    pkgs.sort();
    pkgs
}

fn loose_tex_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for item in std::fs::read_dir(dir).unwrap() {
        let path = item.unwrap().path();
        if path.is_dir() {
            loose_tex_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "tex") {
            out.push(path);
        }
    }
}

#[test]
fn every_corpus_pkg_parses() {
    let Some(corpus) = dir_from_env("LUMEN_WE_CORPUS", DEFAULT_CORPUS) else {
        eprintln!("corpus not found; skipping (set LUMEN_WE_CORPUS)");
        return;
    };

    let limits = Limits::default();
    let mut parsed = 0;
    let mut failures = Vec::new();

    for pkg_path in scene_pkgs(&corpus) {
        let bytes = std::fs::read(&pkg_path).unwrap();
        match Pkg::parse(&bytes, &limits) {
            Ok(pkg) if pkg.get("scene.json").is_some() => parsed += 1,
            Ok(_) => failures.push(format!("{}: no scene.json", pkg_path.display())),
            Err(e) => failures.push(format!("{}: {e}", pkg_path.display())),
        }
    }

    eprintln!("parsed {parsed} scene.pkg files");
    assert!(
        failures.is_empty(),
        "failed to parse:\n{}",
        failures.join("\n")
    );
}

/// Parses and decodes the first mip of every `.tex` found in the WE install and
/// inside every corpus `scene.pkg`.
#[test]
fn every_tex_parses_and_decodes() {
    let limits = Limits::default();
    let mut sources: Vec<(String, Vec<u8>)> = Vec::new();

    if let Some(install) = dir_from_env("LUMEN_WE_INSTALL", DEFAULT_INSTALL) {
        let mut files = Vec::new();
        loose_tex_files(&install.join("assets"), &mut files);
        for f in files {
            sources.push((f.display().to_string(), std::fs::read(&f).unwrap()));
        }
    }
    if let Some(corpus) = dir_from_env("LUMEN_WE_CORPUS", DEFAULT_CORPUS) {
        for pkg_path in scene_pkgs(&corpus) {
            let bytes = std::fs::read(&pkg_path).unwrap();
            let pkg = Pkg::parse(&bytes, &limits).unwrap();
            for entry in pkg.entries().iter().filter(|e| e.path.ends_with(".tex")) {
                let data = pkg.data(entry).unwrap().to_vec();
                sources.push((format!("{}!{}", pkg_path.display(), entry.path), data));
            }
        }
    }
    if sources.is_empty() {
        eprintln!("no WE install or corpus found; skipping");
        return;
    }

    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    for (name, bytes) in &sources {
        let tex = match Tex::parse(bytes, &limits) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("parse {name}: {e}"));
                continue;
            }
        };
        let kind = match (tex.is_video, tex.image_format) {
            (true, _) => "mp4".to_owned(),
            (false, Some(id)) => format!("freeimage-{id}"),
            (false, None) => format!("{:?}", tex.header.format),
        };
        *kinds
            .entry(format!("TEXB{:04} {kind}", tex.container_version))
            .or_default() += 1;
        if tex.is_video {
            continue;
        }
        let Some(mip) = tex.first_mip() else {
            failures.push(format!("{name}: no mips"));
            continue;
        };
        if let Err(e) = mip_to_rgba8(&tex, mip) {
            failures.push(format!("decode {name}: {e}"));
        }
    }

    eprintln!("checked {} textures", sources.len());
    for (kind, count) in &kinds {
        eprintln!("{count:>6}  {kind}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
