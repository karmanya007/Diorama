//! Reads the user's own Wallpaper Engine settings (`<install>/config.json`)
//! so Lumen can start from the choices they already made in WE.

use std::collections::BTreeMap;
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
    /// `3`: native size, panned `x`/`y` percent (capture of 2981249186).
    Native { x: f32, y: f32 },
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

/// Display-placement keys, which Lumen applies itself (`alignmentfliph`,
/// the horizontal flip, stays: web pages apply it).
const PLACEMENT: [&str; 5] = [
    "alignment",
    "alignmentx",
    "alignmenty",
    "alignmentz",
    "alignmentposition",
];

/// The user's per-wallpaper choices made in WE's UI (property overrides and
/// options such as `cameraparallax`), without the display-placement keys.
pub fn user_properties_from_file(
    config: &Path,
    wallpaper: &Path,
    monitor: usize,
) -> BTreeMap<String, Value> {
    let Some(config) = std::fs::read_to_string(config)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return BTreeMap::new();
    };
    monitor_entry(&config, wallpaper, monitor)
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(k, _)| !PLACEMENT.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The `language` WE gives web pages for the settings in `config`
/// (`<user>.general.user.language`, or Steam's language when `default`).
pub fn web_language_from_file(config: &Path) -> Option<&'static str> {
    let text = std::fs::read_to_string(config).ok()?;
    let config: Value = serde_json::from_str(&text).ok()?;
    config.as_object()?.values().find_map(|user| {
        let settings = user.get("general")?.get("user")?;
        let text = |k: &str| settings.get(k).and_then(Value::as_str);
        crate::language::from_we_settings(text("language"), text("steamlanguage"))
    })
}

fn alignment_in(config: &Value, wallpaper: &Path, monitor: usize) -> Option<Alignment> {
    let monitor = monitor_entry(config, wallpaper, monitor)?;
    let pct = |k: &str| monitor.get(k).and_then(Value::as_f64).unwrap_or(50.0) as f32;
    Some(match monitor.get("alignment").and_then(Value::as_i64) {
        None => Alignment::Cover,
        Some(2) => Alignment::Stretch,
        Some(3) => Alignment::Native {
            x: pct("alignmentx"),
            y: pct("alignmenty"),
        },
        Some(4) => Alignment::Custom {
            x: pct("alignmentx"),
            y: pct("alignmenty"),
            zoom: monitor
                .get("alignmentz")
                .and_then(Value::as_f64)
                .unwrap_or(100.0) as f32,
        },
        Some(other) => Alignment::Other(other),
    })
}

/// `file` from the `project.json` in folder `dir`, if any.
fn project_file(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("project.json")).ok()?;
    let project: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    Some(project.get("file")?.as_str()?.to_owned())
}

/// `<user>.wproperties[<wallpaper file>].Monitor<n>` for `wallpaper`.
fn monitor_entry<'c>(config: &'c Value, wallpaper: &Path, monitor: usize) -> Option<&'c Value> {
    let wanted = normalise(&wallpaper.to_string_lossy());
    // A scene folder is stored as its scene.pkg / scene.json, or the file
    // its project.json names (`<name>.json` in some of WE's own projects).
    let mut candidates = vec![
        wanted.clone(),
        format!("{wanted}/scene.pkg"),
        format!("{wanted}/scene.json"),
    ];
    if let Some(file) = project_file(wallpaper) {
        candidates.push(format!("{wanted}/{}", file.to_lowercase()));
    }
    let users = config.as_object()?;
    for user in users.values() {
        let Some(props) = user.get("wproperties").and_then(Value::as_object) else {
            continue;
        };
        for (path, entry) in props {
            if !candidates.contains(&normalise(path)) {
                continue;
            }
            return entry.get(format!("Monitor{monitor}"));
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

    #[test]
    fn user_properties_skip_alignment_keys() {
        let dir = std::env::temp_dir().join(format!("lumen-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{"Bob": {"wproperties": {"C:/W/1/scene.pkg": {"Monitor0": {"alignment": 4, "alignmentz": 99, "alignmentfliph": true, "clock": "1", "cameraparallax": false}}},
                "general": {"user": {"language": "default", "steamlanguage": "german"}}}}"#,
        )
        .unwrap();
        let props = user_properties_from_file(&path, Path::new("C:/W/1"), 0);
        let language = web_language_from_file(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(props.len(), 3);
        assert_eq!(props["alignmentfliph"], true);
        assert_eq!(language, Some("de-de"));
        assert_eq!(props["clock"], "1");
        assert_eq!(props["cameraparallax"], false);
    }
}
