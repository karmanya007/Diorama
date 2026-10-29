//! Top-level declarations and JSON annotations in WE shaders.

use serde_json::Value;

use crate::error::{Error, Result};

/// JSON as WE reads it: also accepts numbers with leading zeros
/// (`"range":[0,01]` in a workshop shader of 2216027178), trailing commas and
/// `//` / `/* */` comments (`//"cullmode"` in Fantastic Car's glass material).
pub fn lenient_json(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str(text) {
        return Some(v);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let (mut in_string, mut escaped) = (false, false);
    // Last significant character outside strings.
    let mut prev = ' ';
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
                out.push('\n');
                continue;
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut star = false;
                for c in chars.by_ref() {
                    if star && c == '/' {
                        break;
                    }
                    star = c == '*';
                }
                out.push(' ');
                continue;
            }
            '0' if !prev.is_ascii_digit() && prev != '.' => {
                // Drop leading zeros of an integer part ("01" → "1", "00" → "0").
                while chars.peek() == Some(&'0') {
                    chars.next();
                }
                if !chars.peek().is_some_and(char::is_ascii_digit) {
                    out.push('0');
                }
                prev = '0';
                continue;
            }
            ']' | '}' => {
                let trimmed = out.trim_end().len();
                if out[..trimmed].ends_with(',') {
                    out.truncate(trimmed - 1);
                }
            }
            _ => {}
        }
        out.push(c);
        if !c.is_whitespace() {
            prev = c;
        }
    }
    serde_json::from_str(&out).ok()
}

/// `// [COMBO] {"combo":"NAME","default":0,...}`
#[derive(Debug, Clone, PartialEq)]
pub struct ComboDecl {
    pub name: String,
    pub default: i64,
    pub raw: Value,
}

/// Collects `[COMBO]` annotations (ignores `[COMBO_OFF]`).
pub fn parse_combos(source: &str) -> Vec<ComboDecl> {
    source
        .lines()
        .filter_map(|line| {
            let json = line.split_once("[COMBO]")?.1.trim();
            let raw: Value = lenient_json(json)?;
            let name = raw["combo"].as_str()?.to_owned();
            let default = raw["default"].as_i64().unwrap_or(0);
            Some(ComboDecl { name, default, raw })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qualifier {
    Attribute,
    Varying,
    Uniform,
    /// `in uint gl_InstanceID;` — engine-provided builtin input.
    BuiltinIn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decl {
    pub qualifier: Qualifier,
    pub ty: String,
    pub name: String,
    /// Array length expression, e.g. `4` in `uniform vec4 g_X[4];`.
    pub array: Option<String>,
    /// Trailing `// {json}` annotation.
    pub annotation: Option<Value>,
}

impl Decl {
    pub fn is_texture(&self) -> bool {
        self.ty.starts_with("sampler")
    }
}

const PRECISION: [&str; 3] = ["highp", "mediump", "lowp"];

/// Parses one source line as an `attribute` / `varying` / `uniform` declaration.
/// Returns `Ok(None)` for any other line.
pub fn parse_decl_line(line: &str) -> Result<Option<Vec<Decl>>> {
    let (code, comment) = match line.split_once("//") {
        Some((code, comment)) => (code, Some(comment.trim())),
        None => (line, None),
    };
    let code = code.trim();
    let (qualifier, rest) = if let Some(r) = code.strip_prefix("attribute ") {
        (Qualifier::Attribute, r)
    } else if let Some(r) = code.strip_prefix("varying ") {
        (Qualifier::Varying, r)
    } else if let Some(r) = code.strip_prefix("uniform ") {
        (Qualifier::Uniform, r)
    } else if let Some(r) = code
        .strip_prefix("in ")
        .filter(|r| r.contains("gl_") && r.ends_with(';'))
    {
        // Only builtin declarations; `in` on a multi-line parameter list is left alone.
        (Qualifier::BuiltinIn, r)
    } else {
        return Ok(None);
    };

    let rest = rest
        .trim()
        .strip_suffix(';')
        .ok_or_else(|| Error::Shader(format!("declaration without ';': {line:?}")))?;
    let mut words = rest.split_whitespace().filter(|w| !PRECISION.contains(w));
    let ty = words
        .next()
        .ok_or_else(|| Error::Shader(format!("missing type: {line:?}")))?
        .to_owned();
    let names = words.collect::<Vec<_>>().join(" ");
    let annotation = comment
        .filter(|c| c.starts_with('{'))
        .and_then(lenient_json);

    let mut decls = Vec::new();
    for part in names.split(',') {
        // Drop initializers: defaults come from annotations.
        let part = part.split('=').next().unwrap_or("").trim();
        let (name, array) = match part.split_once('[') {
            Some((n, len)) => (n.trim(), Some(len.trim_end_matches(']').trim().to_owned())),
            None => (part, None),
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::Shader(format!("bad declaration name: {line:?}")));
        }
        decls.push(Decl {
            qualifier,
            ty: ty.clone(),
            name: name.to_owned(),
            array,
            annotation: annotation.clone(),
        });
    }
    Ok(Some(decls))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotations_parse_leniently() {
        // 2216027178's shift_hue: `[0,01]` is not JSON but WE reads it.
        let v = lenient_json(r#"{"material":"Speed","default":1,"range":[0,01]}"#).unwrap();
        assert_eq!(v["material"], "Speed");
        assert_eq!(v["range"][1], 1);
        let v = lenient_json(r#"{"a":"x,]0","b":[0.05,00,-007,10,],}"#).unwrap();
        assert_eq!(v["a"], "x,]0");
        assert_eq!(v["b"], serde_json::json!([0.05, 0, -7, 10]));
        assert!(lenient_json("{nope").is_none());
        // Fantastic Car's glass material comments a key out; slashes in strings stay.
        let v = lenient_json(
            "{\r\n\t\"a\": 1,\r\n\t//\"cullmode\": \"nocull\",\r\n\t\"b\": \"x//y\" /* note */\r\n}",
        )
        .unwrap();
        assert_eq!(v, serde_json::json!({"a": 1, "b": "x//y"}));
    }

    #[test]
    fn parses_annotated_uniform() {
        let line = r#"uniform float g_Strength; // {"material":"ripplestrength","default":0.1,"range":[0,1]}"#;
        let decls = parse_decl_line(line).unwrap().unwrap();
        assert_eq!(decls.len(), 1);
        let d = &decls[0];
        assert_eq!(
            (d.qualifier, d.ty.as_str(), d.name.as_str()),
            (Qualifier::Uniform, "float", "g_Strength")
        );
        assert_eq!(d.annotation.as_ref().unwrap()["material"], "ripplestrength");
    }

    #[test]
    fn parses_arrays_lists_and_precision() {
        let decls = parse_decl_line("uniform highp vec4 g_A[4], g_B;")
            .unwrap()
            .unwrap();
        assert_eq!(decls[0].name, "g_A");
        assert_eq!(decls[0].array.as_deref(), Some("4"));
        assert_eq!(decls[1].name, "g_B");
        assert_eq!(decls[1].ty, "vec4");
    }

    #[test]
    fn parses_builtin_inputs_only() {
        let d = parse_decl_line("in uint gl_InstanceID;").unwrap().unwrap();
        assert_eq!(
            (d[0].qualifier, d[0].name.as_str()),
            (Qualifier::BuiltinIn, "gl_InstanceID")
        );
        assert!(parse_decl_line("\tin vec3 normal,").unwrap().is_none());
    }

    #[test]
    fn ignores_other_lines() {
        assert!(parse_decl_line("void main() {").unwrap().is_none());
        assert!(
            parse_decl_line("  gl_Position = x; // uniform")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn texture_detection() {
        let d = parse_decl_line("uniform sampler2D g_Texture0; // {\"hidden\":true}")
            .unwrap()
            .unwrap();
        assert!(d[0].is_texture());
    }

    #[test]
    fn combos_from_annotations() {
        let src = "// [COMBO] {\"material\":\"x\",\"combo\":\"MASK\",\"type\":\"options\",\"default\":1}\n\
                   // [COMBO_OFF] {\"combo\":\"OFF\",\"default\":0}";
        let combos = parse_combos(src);
        assert_eq!(combos.len(), 1);
        assert_eq!((combos[0].name.as_str(), combos[0].default), ("MASK", 1));
    }
}
