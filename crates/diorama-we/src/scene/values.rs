//! Lenient readers for scene/material JSON values.
//!
//! WE writes vectors as space-separated strings (`"1920.000 1080.000 0.000"`),
//! scalars as numbers or strings, and wraps user-bindable values as
//! `{"user": "...", "value": ...}` (sometimes with `"animation"`/`"script"`).
//! User bindings are resolved beforehand ([`super::properties`]); scripted
//! and animated values fall back to their embedded `value` until Phase 3.

use serde_json::Value;

/// Unwraps `{"value": x, ...}` user-property / animation wrappers.
pub fn unwrap_value(v: &Value) -> &Value {
    match v {
        Value::Object(map) => map.get("value").map_or(v, unwrap_value),
        _ => v,
    }
}

/// Number, bool, `"a b c"` string or array of numbers → floats.
pub fn floats(v: &Value) -> Option<Vec<f32>> {
    match unwrap_value(v) {
        Value::Number(n) => n.as_f64().map(|f| vec![f as f32]),
        Value::Bool(b) => Some(vec![if *b { 1.0 } else { 0.0 }]),
        Value::String(s) => s
            .split_whitespace()
            .map(|p| p.parse::<f32>().ok())
            .collect::<Option<Vec<_>>>()
            .filter(|v| !v.is_empty()),
        Value::Array(a) => a
            .iter()
            .map(|x| x.as_f64().map(|f| f as f32))
            .collect::<Option<Vec<_>>>(),
        _ => None,
    }
}

/// Exactly `N` floats, padding a shorter vector with `fill`.
pub fn vec_n<const N: usize>(v: Option<&Value>, fill: [f32; N]) -> [f32; N] {
    let mut out = fill;
    if let Some(vals) = v.and_then(floats) {
        for (o, x) in out.iter_mut().zip(vals) {
            *o = x;
        }
    }
    out
}

pub fn boolean(v: Option<&Value>, default: bool) -> bool {
    match v.map(unwrap_value) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => default,
    }
}

pub fn integer(v: Option<&Value>) -> Option<i64> {
    match v.map(unwrap_value)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_we_value_shapes() {
        assert_eq!(floats(&json!("1 0.5 0")), Some(vec![1.0, 0.5, 0.0]));
        assert_eq!(floats(&json!(0.25)), Some(vec![0.25]));
        assert_eq!(
            floats(&json!({"user": "x", "value": "2 2"})),
            Some(vec![2.0, 2.0])
        );
        assert_eq!(floats(&json!(true)), Some(vec![1.0]));
        assert_eq!(floats(&json!("abc")), None);
        assert_eq!(vec_n(Some(&json!("3 4")), [1.0, 1.0, 1.0]), [3.0, 4.0, 1.0]);
        assert!(!boolean(
            Some(&json!({"user": "show", "value": false})),
            true
        ));
        assert_eq!(integer(Some(&json!(1.0))), Some(1));
    }
}
