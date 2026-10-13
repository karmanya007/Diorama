//! Parses every `scene.pkg` in a local Wallpaper Engine Workshop folder.
//!
//! Corpus location: `LUMEN_WE_CORPUS` env var, else the default Steam path.
//! Skipped (passes with a note) when the folder does not exist, e.g. in CI —
//! Workshop content is never committed to this repo.

use std::path::PathBuf;

use lumen_core::Limits;
use lumen_we::Pkg;

const DEFAULT_CORPUS: &str = r"C:\Program Files (x86)\Steam\steamapps\workshop\content\431960";

fn corpus_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("LUMEN_WE_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CORPUS));
    dir.is_dir().then_some(dir)
}

#[test]
fn every_corpus_pkg_parses() {
    let Some(dir) = corpus_dir() else {
        eprintln!("corpus not found; skipping (set LUMEN_WE_CORPUS)");
        return;
    };

    let limits = Limits::default();
    let mut parsed = 0;
    let mut failures = Vec::new();

    for item in std::fs::read_dir(&dir).unwrap() {
        let pkg_path = item.unwrap().path().join("scene.pkg");
        if !pkg_path.is_file() {
            continue;
        }
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
