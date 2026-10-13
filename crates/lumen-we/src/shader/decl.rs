//! Top-level declarations and JSON annotations in WE shaders.

use serde_json::Value;

use crate::error::{Error, Result};

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
            let raw: Value = serde_json::from_str(json).ok()?;
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
        .and_then(|c| serde_json::from_str(c).ok());

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
