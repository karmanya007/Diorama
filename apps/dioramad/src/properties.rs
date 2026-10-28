//! Wallpaper property values: the project's defaults, then the choices the
//! user made in Wallpaper Engine, then the ones made in Diorama (overrides,
//! remembered in the state file).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

/// Property name → value.
pub type Values = BTreeMap<String, Value>;

/// WE's choices for the wallpaper WE knows as `key` (a scene folder, a web
/// page, a video file), with `overrides` on top.
pub fn choices(we_config: Option<&Path>, key: &Path, overrides: &Values) -> Values {
    let mut values = we_config
        .map(|c| diorama_we::config::user_properties_from_file(c, key, 0))
        .unwrap_or_default();
    values.extend(overrides.iter().map(|(k, v)| (k.clone(), v.clone())));
    values
}

/// Every declared property's value as the wallpaper in `dir` sees it, plus
/// any other choices.
pub fn effective(dir: &Path, key: &Path, we_config: Option<&Path>, overrides: &Values) -> Values {
    let mut values: Values = project(dir)["general"]["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, p)| Some((name.clone(), p.get("value")?.clone())))
        .collect();
    values.extend(choices(we_config, key, overrides));
    values
}

/// `changes` applied to `overrides`: values replace, `null` removes.
pub fn merge(overrides: &mut Values, changes: &Map<String, Value>) {
    for (name, value) in changes {
        if value.is_null() {
            overrides.remove(name);
        } else {
            overrides.insert(name.clone(), value.clone());
        }
    }
}

/// The folder's `project.json` (`Null` if missing or unreadable).
pub fn project(dir: &Path) -> Value {
    std::fs::read(dir.join("project.json"))
        .ok()
        .and_then(|b| {
            let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&b);
            serde_json::from_slice(b).ok()
        })
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_then_we_then_overrides() {
        let dir = std::env::temp_dir().join(format!("diorama-props-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("project.json"),
            r#"{"general":{"properties":{"speed":{"type":"slider","value":1},"color":{"type":"color","value":"0 0 0"},"title":{"type":"text","value":"hi"}}}}"#,
        )
        .unwrap();
        let config = dir.join("config.json");
        let key = dir.to_string_lossy().replace('\\', "/");
        std::fs::write(
            &config,
            json!({"U": {"wproperties": {key: {"Monitor0": {"speed": 5, "color": "1 0 0"}}}}})
                .to_string(),
        )
        .unwrap();
        let mut overrides = Values::new();
        merge(
            &mut overrides,
            json!({"color": "0 1 0", "gone": null}).as_object().unwrap(),
        );
        let values = effective(&dir, &dir, Some(&config), &overrides);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(values["speed"], 5);
        assert_eq!(values["color"], "0 1 0");
        assert_eq!(values["title"], "hi");
        merge(&mut overrides, json!({"color": null}).as_object().unwrap());
        assert!(overrides.is_empty());
    }
}
