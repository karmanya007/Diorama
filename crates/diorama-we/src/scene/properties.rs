//! User properties (`project.json` → `general.properties`) and the scene
//! values bound to them.
//!
//! A bound value is `{"user": "name", "value": default}`; a conditional one
//! is `{"user": {"name": "name", "condition": "1"}, "value": default}` and is
//! true when the property's value equals the condition. Bindings are resolved
//! once, before import, so the importer only ever sees plain values.
//! Scripted values (`"script"`) keep their object form for Phase 3.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// Property name → current value (project defaults, then overrides).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Properties(pub BTreeMap<String, Value>);

impl Properties {
    /// Defaults from a parsed `project.json`.
    pub fn from_project(project: &Value) -> Self {
        let mut out = BTreeMap::new();
        if let Some(map) = project["general"]["properties"].as_object() {
            for (name, prop) in map {
                if let Some(v) = prop.get("value") {
                    out.insert(name.clone(), v.clone());
                }
            }
        }
        Self(out)
    }

    /// Applies user choices on top of the defaults.
    pub fn with_overrides(mut self, overrides: &BTreeMap<String, Value>) -> Self {
        for (k, v) in overrides {
            self.0.insert(k.clone(), v.clone());
        }
        self
    }

    /// Replaces every binding inside `scene` with its resolved value.
    pub fn resolve(&self, scene: &mut Value) {
        match scene {
            Value::Object(map) => {
                if let Some(resolved) = self.binding(map) {
                    *scene = resolved;
                    // A bound script still carries bound script properties.
                    if let Value::Object(kept) = scene {
                        kept.values_mut().for_each(|v| self.resolve(v));
                    }
                    return;
                }
                for v in map.values_mut() {
                    self.resolve(v);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| self.resolve(v)),
            _ => {}
        }
    }

    fn binding(&self, map: &Map<String, Value>) -> Option<Value> {
        let user = map.get("user")?;
        let default = map.get("value").cloned().unwrap_or(Value::Null);
        let resolved = match user {
            Value::String(name) => self
                .0
                .get(name)
                .map_or(default.clone(), |v| coerce(v, &default)),
            Value::Object(cond) => {
                let name = cond.get("name")?.as_str()?;
                let condition = cond.get("condition").map(as_text).unwrap_or_default();
                match self.0.get(name) {
                    Some(v) => Value::Bool(as_text(v) == condition),
                    None => default.clone(),
                }
            }
            _ => return None,
        };
        if map.contains_key("script") {
            // Keep the script; it starts from the resolved value.
            let mut kept = map.clone();
            kept.remove("user");
            kept.insert("value".into(), resolved);
            Some(Value::Object(kept))
        } else {
            Some(resolved)
        }
    }
}

/// Property values are loosely typed (`"1"` for combos, `true` for bools);
/// shape them like the scene's own default.
fn coerce(v: &Value, like: &Value) -> Value {
    match (like, v) {
        (Value::Bool(_), Value::String(s)) => Value::Bool(s == "1" || s == "true"),
        (Value::Bool(_), Value::Number(n)) => Value::Bool(n.as_f64().is_some_and(|f| f != 0.0)),
        (Value::Number(_), Value::String(s)) => s
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or_else(|| v.clone(), Value::Number),
        (Value::Number(_), Value::Bool(b)) => Value::from(i32::from(*b)),
        _ => v.clone(),
    }
}

/// Text form used for condition comparisons (`true` → "true", `1` → "1").
fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.as_i64().map_or_else(|| n.to_string(), |i| i.to_string()),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn props() -> Properties {
        Properties::from_project(&json!({"general": {"properties": {
            "light": {"type": "combo", "value": "1"},
            "stars": {"type": "bool", "value": false},
            "speed": {"type": "slider", "value": 0.25},
            "tint": {"type": "color", "value": "1 0 0"}
        }}}))
    }

    #[test]
    fn resolves_plain_and_conditional_bindings() {
        let mut scene = json!({"objects": [
            {"visible": {"user": {"name": "light", "condition": "0"}, "value": true}},
            {"visible": {"user": {"name": "light", "condition": "1"}, "value": false}},
            {"visible": {"user": "stars", "value": true}},
            {"speed": {"user": "speed", "value": 1.0}},
            {"color": {"user": "tint", "value": "1 1 1"}},
            {"alpha": {"user": "missing", "value": 0.5}},
        ]});
        props().resolve(&mut scene);
        let o = &scene["objects"];
        assert_eq!(o[0]["visible"], false);
        assert_eq!(o[1]["visible"], true);
        assert_eq!(o[2]["visible"], false);
        assert_eq!(o[3]["speed"], 0.25);
        assert_eq!(o[4]["color"], "1 0 0");
        assert_eq!(o[5]["alpha"], 0.5);
    }

    #[test]
    fn scripts_keep_their_object_with_resolved_start_value() {
        let mut v =
            json!({"user": "speed", "script": "export function update(v) {}", "value": 1.0});
        props().resolve(&mut v);
        assert_eq!(v["value"], 0.25);
        assert!(v.get("script").is_some() && v.get("user").is_none());
    }

    #[test]
    fn bound_scripts_resolve_their_script_properties() {
        // 3509243656: a bound `visible` script whose `menuEn` setting is
        // bound to a property that defaults to false.
        let mut v = json!({"user": "stars", "script": "", "value": true,
            "scriptproperties": {"menuEn": {"user": "stars", "value": true}}});
        props().resolve(&mut v);
        assert_eq!(v["value"], false);
        assert_eq!(v["scriptproperties"]["menuEn"], false);
    }

    #[test]
    fn overrides_win_and_types_coerce() {
        let p = props().with_overrides(&BTreeMap::from([("stars".into(), json!("1"))]));
        let mut v = json!({"user": "stars", "value": false});
        p.resolve(&mut v);
        assert_eq!(v, true);
    }
}
