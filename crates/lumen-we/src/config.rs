//! Reads the user's own Wallpaper Engine settings (`<install>/config.json`)
//! so Lumen can start from the choices they already made in WE.

use std::path::Path;

use serde_json::Value;

/// How WE places a wallpaper on a monitor
/// (`<user>.wproperties[<wallpaper file>].MonitorN.alignment`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Alignment {
    /// No setting: cover the monitor, cropping (WE's default).
    Cover,
    /// `2`: stretch to the monitor, ignoring aspect ratio (confirmed by captures).
    Stretch,
    /// `4`: user zoom/pan in percent. The exact mapping is not decoded yet.
    Custom { x: f32, y: f32, zoom: f32 },
    /// Any other value.
    Other(i64),
}

/// Alignment WE uses for `wallpaper` (its `scene.pkg`, `project.json` folder
/// or video file) on monitor `monitor`, if `config.json` mentions it.
pub fn alignment(we_install: &Path, wallpaper: &Path, monitor: usize) -> Option<Alignment> {
    alignment_from_file(&we_install.join("config.json"), wallpaper, monitor)
}

/// Like [`alignment`] for an explicit `config.json` (e.g. one saved with
/// reference captures).
pub fn alignment_from_file(config: &Path, wallpaper: &Path, monitor: usize) -> Option<Alignment> {
    let text = std::fs::read_to_string(config).ok()?;
    let config: Value = serde_json::from_str(&text).ok()?;
    alignment_in(&config, wallpaper, monitor)
}

fn normalise(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

fn alignment_in(config: &Value, wallpaper: &Path, monitor: usize) -> Option<Alignment> {
    let wanted = normalise(&wallpaper.to_string_lossy());
    // A scene folder is stored as its scene.pkg / scene.json.
    let candidates = [
        wanted.clone(),
        format!("{wanted}/scene.pkg"),
        format!("{wanted}/scene.json"),
    ];
    let users = config.as_object()?;
    for user in users.values() {
        let Some(props) = user.get("wproperties").and_then(Value::as_object) else {
            continue;
        };
        for (path, entry) in props {
            if !candidates.contains(&normalise(path)) {
                continue;
            }
            let monitor = entry.get(format!("Monitor{monitor}"))?;
            let pct = |k: &str| monitor.get(k).and_then(Value::as_f64).unwrap_or(50.0) as f32;
            return Some(match monitor.get("alignment").and_then(Value::as_i64) {
                None => Alignment::Cover,
                Some(2) => Alignment::Stretch,
                Some(4) => Alignment::Custom {
                    x: pct("alignmentx"),
                    y: pct("alignmenty"),
                    zoom: monitor
                        .get("alignmentz")
                        .and_then(Value::as_f64)
                        .unwrap_or(100.0) as f32,
                },
                Some(other) => Alignment::Other(other),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_scene_by_folder_or_package() {
        let config = json!({"?installdirectory": "x", "Alice": {"wproperties": {
            "C:/W/431960/1/scene.pkg": {"Monitor0": {"alignment": 2}},
            "C:/W/431960/2/scene.pkg": {"Monitor0": {"alignment": 4, "alignmentx": 54, "alignmentz": 99}},
            "C:/W/431960/3/video.mp4": {"Monitor0": {}}
        }}});
        assert_eq!(
            alignment_in(&config, Path::new(r"C:\W\431960\1"), 0),
            Some(Alignment::Stretch)
        );
        assert_eq!(
            alignment_in(&config, Path::new("C:/W/431960/2/scene.pkg"), 0),
            Some(Alignment::Custom {
                x: 54.0,
                y: 50.0,
                zoom: 99.0
            })
        );
        assert_eq!(
            alignment_in(&config, Path::new("C:/W/431960/3/video.mp4"), 0),
            Some(Alignment::Cover)
        );
        assert_eq!(alignment_in(&config, Path::new("C:/W/431960/1"), 1), None);
        assert_eq!(alignment_in(&config, Path::new("C:/W/431960/9"), 0), None);
    }
}
