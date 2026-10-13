//! Decodes the first frames of every video wallpaper in the local Workshop
//! folder (`LUMEN_WE_CORPUS`, else the default Steam path). Skipped when absent.

#![allow(
    clippy::unwrap_used,
    reason = "test harness: I/O failures should fail the test loudly"
)]

use std::path::PathBuf;
use std::time::Instant;

const DEFAULT_CORPUS: &str = r"C:\Program Files (x86)\Steam\steamapps\workshop\content\431960";
const FRAMES: usize = 90;

fn videos() -> Vec<PathBuf> {
    let dir = std::env::var_os("LUMEN_WE_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CORPUS));
    let Ok(items) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.flatten() {
        let Ok(project) = std::fs::read_to_string(item.path().join("project.json")) else {
            continue;
        };
        let json: serde_json::Value = serde_json::from_str(&project).unwrap_or_default();
        if json["type"]
            .as_str()
            .is_some_and(|t| t.eq_ignore_ascii_case("video"))
            && let Some(file) = json["file"].as_str()
        {
            out.push(item.path().join(file));
        }
    }
    out.sort();
    out
}

#[test]
fn decodes_corpus_videos() {
    let videos = videos();
    if videos.is_empty() {
        eprintln!("no corpus videos; skipping");
        return;
    }
    let mut failures = Vec::new();
    for path in &videos {
        let name = path.file_name().unwrap().to_string_lossy();
        let mut decoder = match lumen_media::open(path) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{name}: open: {e}"));
                continue;
            }
        };
        let info = decoder.info();
        let start = Instant::now();
        let mut frames = 0;
        let mut last_pts = None;
        while frames < FRAMES {
            match decoder.next_frame() {
                Ok(Some(f)) => {
                    assert_eq!((f.width, f.height), (info.width, info.height));
                    assert!(f.luma().len() >= (f.stride * f.height) as usize);
                    assert!(f.chroma().len() >= (f.stride * f.height / 2) as usize);
                    if let Some(prev) = last_pts {
                        assert!(f.pts >= prev, "{name}: pts went backwards");
                    }
                    last_pts = Some(f.pts);
                    frames += 1;
                }
                Ok(None) => break,
                Err(e) => {
                    failures.push(format!("{name}: frame {frames}: {e}"));
                    break;
                }
            }
        }
        let secs = start.elapsed().as_secs_f64();
        eprintln!(
            "{:>4}x{:<4} {:>6.2} fps src  decode {:>7.1} fps  {}",
            info.width,
            info.height,
            info.frame_rate.unwrap_or(0.0),
            frames as f64 / secs.max(1e-9),
            name
        );
        if frames == 0 {
            failures.push(format!("{name}: no frames"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
