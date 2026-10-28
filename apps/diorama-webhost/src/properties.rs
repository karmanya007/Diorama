//! User properties as WE hands them to `applyUserProperties`.

use std::path::Path;

use serde_json::{Map, Value};

/// `{name: {type, value}}` from the project's `general.properties`, with
/// `choices` (`{name: value}`, the user's settings) replacing the defaults.
pub fn user_properties(project: &Value, choices: &Map<String, Value>) -> Value {
    let mut out = Map::new();
    let defaults = project["general"]["properties"].as_object();
    for (name, p) in defaults.into_iter().flatten() {
        let value = choices
            .get(name)
            .cloned()
            .unwrap_or_else(|| p["value"].clone());
        let mut entry = Map::new();
        if let Some(t) = p.get("type") {
            entry.insert("type".into(), t.clone());
        }
        entry.insert("value".into(), value);
        out.insert(name.clone(), Value::Object(entry));
    }
    // Choices the project does not declare (WE sends them too).
    for (name, value) in choices {
        out.entry(name.clone())
            .or_insert_with(|| serde_json::json!({ "value": value }));
    }
    Value::Object(out)
}

/// The project's `project.json`, if readable. Used by the desktop host,
/// which Linux does not have yet.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub fn read_project(dir: &Path) -> Value {
    std::fs::read(dir.join("project.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(strip_bom(&b)).ok())
        .unwrap_or(Value::Null)
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn strip_bom(b: &[u8]) -> &[u8] {
    b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn choices_replace_defaults() {
        let project = json!({"general": {"properties": {
            "schemecolor": {"type": "color", "value": "0 0 0"},
            "speed": {"type": "slider", "value": 3}
        }}});
        let choices = json!({"speed": 7, "extra": true});
        let p = user_properties(&project, choices.as_object().unwrap());
        assert_eq!(p["speed"], json!({"type": "slider", "value": 7}));
        assert_eq!(p["schemecolor"]["value"], "0 0 0");
        assert_eq!(p["extra"]["value"], true);
    }
}
