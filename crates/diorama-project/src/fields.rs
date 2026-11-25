//! Reading a document's values: constants, a user property's value
//! (`{"property": "name"}`), or ones that move (`{"keys"}`, `{"audio"}`). What is missing or of another type falls back
//! to its default, with a note in the problems.

use std::cell::RefCell;

use diorama_scene::motion::{AudioRange, Ease, Key, Keys, Repeat};
use diorama_scene::{AudioTrack, Numbers, Track};
use serde_json::{Map, Value};

use crate::Values;

/// Problems noted per document; one of nonsense needs no list of all of it.
const MAX_PROBLEMS: usize = 200;

/// Keys per value.
const MAX_KEYS: usize = 1000;
/// Latest key and longest movement, in seconds.
const MAX_TIME: f32 = 86_400.0;
/// Values within values.
const MAX_DEPTH: usize = 4;

const EASES: [(&str, Ease); 5] = [
    ("linear", Ease::Linear),
    ("in", Ease::In),
    ("out", Ease::Out),
    ("in-out", Ease::InOut),
    ("hold", Ease::Hold),
];
const REPEATS: [(&str, Repeat); 3] = [
    ("loop", Repeat::Loop),
    ("mirror", Repeat::Mirror),
    ("once", Repeat::Once),
];
const RANGES: [(&str, AudioRange); 4] = [
    ("all", AudioRange::All),
    ("bass", AudioRange::Bass),
    ("middle", AudioRange::Middle),
    ("treble", AudioRange::Treble),
];

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
        let problem = format!("{}: {message}", self.at);
        // A value may be read more than once.
        if problems.len() < MAX_PROBLEMS && !problems.contains(&problem) {
            problems.push(problem);
        }
    }

    /// `key`'s value, with a property looked up; of one that moves, what it
    /// starts with. `None` if there is none.
    fn get(&self, key: &str) -> Option<&'a Value> {
        self.resolve(key, self.map.get(key)?, MAX_DEPTH)
    }

    fn resolve(&self, key: &str, value: &'a Value, depth: usize) -> Option<&'a Value> {
        let Some(object) = value.as_object() else {
            return Some(value);
        };
        if let Some(name) = object.get("property").and_then(Value::as_str) {
            let found = self.values.get(name);
            if found.is_none() {
                self.problem(&format!("{key}: no property {name}"));
            }
            return found;
        }
        let start = match object.get("keys") {
            Some(keys) => {
                let time = |k: &&Value| k.get("t").and_then(number).unwrap_or(0.0);
                let keys = keys.as_array().into_iter().flatten();
                keys.min_by(|a, b| time(a).total_cmp(&time(b)))
                    .and_then(|k| k.get("v"))
            }
            None if object.contains_key("audio") => object.get("from"),
            None => None,
        };
        match start.filter(|_| depth > 0) {
            Some(value) => self.resolve(key, value, depth - 1),
            None => {
                self.problem(&format!("{key} is neither a value nor a property"));
                None
            }
        }
    }

    /// How `key` moves, if it does; `read` reads its values.
    pub fn motion(&self, key: &str, read: impl Fn(&Self, &str) -> Numbers) -> Option<Track> {
        let map = self.map.get(key)?.as_object()?;
        let within = |map| Self {
            map,
            at: format!("{}, {key}", self.at),
            ..*self
        };
        if map.contains_key("property") {
            return None;
        }
        let f = within(map);
        if let Some(list) = map.get("keys").and_then(Value::as_array) {
            if list.len() > MAX_KEYS {
                f.problem(&format!("more than {MAX_KEYS} keys: the rest is left out"));
            }
            let mut keys = Vec::new();
            for value in list.iter().take(MAX_KEYS) {
                let Some(map) = value.as_object() else {
                    f.problem("a key is not an object");
                    continue;
                };
                let k = within(map);
                keys.push(Key {
                    time: k.number("t", 0.0, [0.0, MAX_TIME]),
                    value: read(&k, "v"),
                    ease: k.choice("ease", &EASES, Ease::default()),
                });
            }
            let length = f.optional_number("length", [0.0, MAX_TIME]);
            let repeat = f.choice("repeat", &REPEATS, Repeat::default());
            return Keys::new(keys, length, repeat).map(Track::Keys);
        }
        map.contains_key("audio").then(|| {
            Track::Audio(AudioTrack {
                range: f.choice("audio", &RANGES, AudioRange::default()),
                from: read(&f, "from"),
                to: read(&f, "to"),
                smoothing: f.number("smooth", 0.1, [0.0, 10.0]),
                threshold: f.number("threshold", 0.3, [0.0, 0.95]),
            })
        })
    }

    /// The number `key`; of one that moves, the one furthest from zero.
    pub fn extreme(&self, key: &str, default: f32, range: [f32; 2]) -> f32 {
        let start = self.number(key, default, range);
        let track = self.motion(key, |f, key| [f.number(key, default, range), 0.0, 0.0, 0.0]);
        let moving = track.iter().flat_map(Track::values).map(|v| v[0]);
        moving.fold(start, |most, v| if v.abs() > most.abs() { v } else { most })
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
pub(crate) fn color(v: &Value) -> Option<[f32; 4]> {
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
    fn values_that_move() {
        let document = json!({
            "turn": {"keys": [
                {"t": 2, "v": 90, "ease": "linear"}, {"t": 0, "v": {"property": "start"}, "ease": "hold"}, 7,
            ], "length": 3, "repeat": "mirror"},
            "shine": {"audio": "bass", "from": 0.5, "to": 9, "threshold": 0},
            "none": {"keys": []}, "deep": {"keys": [{"v": {"keys": [{"v": {"keys": [{"v":
                {"keys": [{"v": {"keys": [{"v": 1}]}}]}}]}}]}}]},
            "linked": {"property": "start", "keys": [{"t": 0, "v": 1}]},
        });
        let values: Values = [("start".to_owned(), json!(30))].into();
        let problems = RefCell::new(Vec::new());
        let f = Fields {
            map: document.as_object().unwrap(),
            values: &values,
            at: "layer".into(),
            problems: &problems,
        };
        let one = |f: &Fields<'_>, key: &str| [f.number(key, 0.0, [-360.0, 360.0]), 0.0, 0.0, 0.0];
        assert_eq!(f.number("turn", 0.0, [-360.0, 360.0]), 30.0, "its start");
        let Some(Track::Keys(turn)) = f.motion("turn", one) else {
            panic!("no keys");
        };
        for (t, v) in [(1.9, 30.0), (2.0, 90.0), (3.5, 90.0), (5.0, 30.0)] {
            assert_eq!(turn.at(t)[0], v, "at {t}");
        }
        assert_eq!(f.extreme("turn", 0.0, [-360.0, 360.0]), 90.0);

        assert_eq!(f.number("shine", 0.0, [0.0, 1.0]), 0.5);
        let shine = f.motion("shine", |f, key| {
            [f.number(key, 0.0, [0.0, 1.0]), 0.0, 0.0, 0.0]
        });
        let Some(Track::Audio(shine)) = shine else {
            panic!("no audio");
        };
        assert_eq!((shine.from[0], shine.to[0]), (0.5, 1.0));
        assert_eq!((shine.smoothing, shine.threshold), (0.1, 0.0));
        assert_eq!(problems.borrow().len(), 1, "{:?}", problems.borrow());

        assert!(f.motion("none", one).is_none());
        assert_eq!(f.number("none", 0.2, [0.0, 1.0]), 0.2);
        assert_eq!(f.number("deep", 0.2, [0.0, 1.0]), 0.2);
        assert!(f.motion("missing", one).is_none());
        assert!(f.motion("linked", one).is_none());
        assert_eq!(f.number("linked", 0.0, [0.0, 99.0]), 30.0);
    }

    /// The same cases as the editor's own reading of keys (motion.test.js).
    #[test]
    fn keys_as_the_editor_shows_them() {
        let cases: Value = serde_json::from_str(include_str!("../tests/motion.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let problems = RefCell::new(Vec::new());
            let f = Fields {
                map: case.as_object().unwrap(),
                values: &Values::new(),
                at: "case".into(),
                problems: &problems,
            };
            let read =
                |f: &Fields<'_>, key: &str| [f.number(key, 0.0, [-99.0, 99.0]), 0.0, 0.0, 0.0];
            let Some(Track::Keys(keys)) = f.motion("motion", read) else {
                panic!("{case}");
            };
            for at in case["at"].as_array().unwrap() {
                let [t, v] = [0, 1].map(|i| at[i].as_f64().unwrap() as f32);
                let got = keys.at(t)[0];
                assert!(
                    (got - v).abs() < 1e-3,
                    "{}: {got}, not {v} at {t}",
                    case["name"]
                );
            }
        }
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
