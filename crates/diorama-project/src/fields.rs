//! Reading a document's values: constants, or a user property's value
//! (`{"property": "name"}`). What is missing or of another type falls back
//! to its default, with a note in the problems.

use std::cell::RefCell;

use serde_json::{Map, Value};

use crate::Values;

/// Problems noted per document; one of nonsense needs no list of all of it.
const MAX_PROBLEMS: usize = 200;

/// One JSON object of the document.
pub(crate) struct Fields<'a> {
    pub map: &'a Map<String, Value>,
    pub values: &'a Values,
    /// Where it is, for problems.
    pub at: String,
    pub problems: &'a RefCell<Vec<String>>,
}

impl<'a> Fields<'a> {
    pub fn problem(&self, message: &str) {
        let mut problems = self.problems.borrow_mut();
        if problems.len() < MAX_PROBLEMS {
            problems.push(format!("{}: {message}", self.at));
        }
    }

    /// `key`'s value, with a property looked up; `None` if there is none.
    fn get(&self, key: &str) -> Option<&'a Value> {
        let value = self.map.get(key)?;
        let Some(property) = value.as_object() else {
            return Some(value);
        };
        let Some(name) = property.get("property").and_then(Value::as_str) else {
            self.problem(&format!("{key} is neither a value nor a property"));
            return None;
        };
        let found = self.values.get(name);
        if found.is_none() {
            self.problem(&format!("{key}: no property {name}"));
        }
        found
    }

    /// `key` read by `read`, or `default`.
    fn read<T>(
        &self,
        key: &str,
        what: &str,
        default: T,
        read: impl FnOnce(&Value) -> Option<T>,
    ) -> T {
        let Some(value) = self.get(key) else {
            return default;
        };
        read(value).unwrap_or_else(|| {
            self.problem(&format!("{key} is not {what}"));
            default
        })
    }

    /// A number, kept within `range`.
    pub fn number(&self, key: &str, default: f32, range: [f32; 2]) -> f32 {
        self.read(key, "a number", default, number)
            .clamp(range[0], range[1])
    }

    pub fn optional_number(&self, key: &str, range: [f32; 2]) -> Option<f32> {
        self.read(key, "a number", None, |v| number(v).map(Some))
            .map(|n| n.clamp(range[0], range[1]))
    }

    /// Two numbers, or one for both.
    pub fn pair(&self, key: &str, default: [f32; 2], range: [f32; 2]) -> [f32; 2] {
        self.read(key, "two numbers", default, pair)
            .map(|n| n.clamp(range[0], range[1]))
    }

    pub fn optional_pair(&self, key: &str, range: [f32; 2]) -> Option<[f32; 2]> {
        self.read(key, "two numbers", None, |v| pair(v).map(Some))
            .map(|p| p.map(|n| n.clamp(range[0], range[1])))
    }

    pub fn flag(&self, key: &str, default: bool) -> bool {
        self.read(key, "true or false", default, |v| match v {
            Value::Bool(b) => Some(*b),
            _ => number(v).map(|n| n != 0.0),
        })
    }

    /// RGBA, 0..1.
    pub fn color(&self, key: &str, default: [f32; 4]) -> [f32; 4] {
        self.read(key, "a colour", default, color)
    }

    pub fn text(&self, key: &str) -> Option<String> {
        self.read(key, "text", None, |v| match v {
            Value::String(s) => Some(Some(s.clone())),
            Value::Number(n) => Some(Some(n.to_string())),
            _ => None,
        })
    }

    /// One of `names`.
    pub fn choice<T: Copy>(&self, key: &str, names: &[(&str, T)], default: T) -> T {
        self.read(key, "one of its choices", default, |v| {
            let name = v.as_str()?;
            names.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
        })
    }
}

fn number(v: &Value) -> Option<f32> {
    let n = match v {
        Value::Number(n) => n.as_f64()? as f32,
        Value::String(s) => s.trim().parse().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

fn pair(v: &Value) -> Option<[f32; 2]> {
    match v {
        Value::Array(a) if a.len() == 2 => Some([number(&a[0])?, number(&a[1])?]),
        _ => number(v).map(|n| [n, n]),
    }
}

/// `#rgb`, `#rrggbb`, `#rrggbbaa`, `[r, g, b]` and `[r, g, b, a]` (0..1), and
/// what the properties form stores: `"r g b"` (0..1).
fn color(v: &Value) -> Option<[f32; 4]> {
    let parts: Vec<f32> = match v {
        Value::Array(a) => a.iter().map(number).collect::<Option<_>>()?,
        Value::String(s) => match s.trim().strip_prefix('#') {
            Some(hex) => return hex_color(hex),
            None => s
                .split_whitespace()
                .map(|p| p.parse().ok().filter(|n: &f32| n.is_finite()))
                .collect::<Option<_>>()?,
        },
        _ => return None,
    };
    let rgba = match parts[..] {
        [r, g, b] => [r, g, b, 1.0],
        [r, g, b, a] => [r, g, b, a],
        _ => return None,
    };
    Some(rgba.map(|c| c.clamp(0.0, 1.0)))
}

fn hex_color(hex: &str) -> Option<[f32; 4]> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(hex.get(i..=i)?, 16).ok();
    let byte = |i: usize| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok();
    let rgba = match hex.len() {
        3 => [digit(0)? * 17, digit(1)? * 17, digit(2)? * 17, 255],
        6 => [byte(0)?, byte(1)?, byte(2)?, 255],
        8 => [byte(0)?, byte(1)?, byte(2)?, byte(3)?],
        _ => return None,
    };
    Some(rgba.map(|c| f32::from(c) / 255.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_properties_and_defaults() {
        let document = json!({
            "opacity": 2, "scale": 3, "position": [1, "-2.5"], "shown": false, "blend": "add",
            "name": {"property": "title"}, "tint": {"property": "accent"},
            "speed": {"property": "gone"}, "odd": {"x": 1}, "size": "big", "font": null,
        });
        let values: Values = [
            ("title".to_owned(), json!("Hi")),
            ("accent".to_owned(), json!("1 0.5 0")),
        ]
        .into();
        let problems = RefCell::new(Vec::new());
        let f = Fields {
            map: document.as_object().unwrap(),
            values: &values,
            at: "layer".into(),
            problems: &problems,
        };
        assert_eq!(f.number("opacity", 1.0, [0.0, 1.0]), 1.0);
        assert_eq!(f.pair("scale", [1.0; 2], [-10.0, 10.0]), [3.0, 3.0]);
        assert_eq!(f.pair("position", [0.0; 2], [-10.0, 10.0]), [1.0, -2.5]);
        assert!(!f.flag("shown", true));
        assert_eq!(f.choice("blend", &[("normal", 0), ("add", 1)], 0), 1);
        assert_eq!(f.text("name").as_deref(), Some("Hi"));
        assert_eq!(f.color("tint", [0.0; 4]), [1.0, 0.5, 0.0, 1.0]);
        assert_eq!(f.text("missing"), None);
        assert_eq!(f.optional_pair("missing", [0.0, 1.0]), None);
        assert_eq!(f.optional_number("missing", [0.0, 1.0]), None);
        assert!(problems.borrow().is_empty(), "{:?}", problems.borrow());

        assert_eq!(f.number("speed", 0.5, [0.0, 1.0]), 0.5);
        assert_eq!(f.number("odd", 0.5, [0.0, 1.0]), 0.5);
        assert_eq!(f.optional_pair("size", [0.0, 1.0]), None);
        assert_eq!(f.text("font"), None);
        assert_eq!(problems.borrow().len(), 4, "{:?}", problems.borrow());
    }

    #[test]
    fn colours() {
        let c = |v: Value| color(&v);
        assert_eq!(c(json!("#f00")), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(c(json!("#00ff0000")), Some([0.0, 1.0, 0.0, 0.0]));
        assert_eq!(c(json!([0, 0, 2])), Some([0.0, 0.0, 1.0, 1.0]));
        for bad in [
            json!("#12"),
            json!("#+1+1+1"),
            json!("#éé"),
            json!("red"),
            json!([1]),
            json!(1),
            json!("nan 0 0"),
        ] {
            assert_eq!(c(bad.clone()), None, "{bad}");
        }
    }
}
