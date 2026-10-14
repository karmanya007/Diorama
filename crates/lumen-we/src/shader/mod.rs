//! Wallpaper Engine shader dialect: preprocessing and translation.
//! See `docs/re-notes/shaders.md`.

pub mod decl;
pub mod glsl;
pub mod hlsl;
pub mod preprocess;

use std::collections::{BTreeMap, HashMap};

pub use decl::{ComboDecl, Decl, Qualifier};
pub use glsl::GlslPair;
pub use hlsl::HlslPair;

use crate::error::Result;

/// Defines the engine sets for every shader before combos.
///
/// `HLSL` is defined because wgpu follows Direct3D conventions (top-left
/// texture origin, 0..1 clip depth), matching the branches WE compiles on
/// Windows. `HLSL_SM30` (D3D9 half-texel offsets) is not.
pub fn engine_defines() -> Vec<(String, String)> {
    vec![
        ("HLSL".to_owned(), "1".to_owned()),
        ("HLSL_SM40".to_owned(), "1".to_owned()),
    ]
}

/// Comment left where a `#require` stood; [`prepare_pair`] replaces it with
/// the engine-provided library.
pub const REQUIRE_MARKER: &str = "lumen-require:";

/// Code WE's engine generates for `#require LightingV1`: a loop over the
/// scene's point/spot/tube/directional lights (strings in `wallpaper64.exe`).
/// Lumen has no dynamic lights yet, so lit materials get ambient only.
const LIGHTING_V1: &str = "vec3 PerformLighting_V1(vec3 worldPos, vec3 color, vec3 normal, \
    vec3 viewVector, vec3 specularTint, vec3 ambient, float roughness, float metallic) \
    { vec3 light = CAST3(0.0); return light; }\n";

fn splice_requires(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        match line
            .trim()
            .strip_prefix("// ")
            .and_then(|l| l.strip_prefix(REQUIRE_MARKER))
        {
            Some("LightingV1") => out.push_str(LIGHTING_V1),
            Some(_) => out.push('\n'),
            None => out.push_str(line),
        }
    }
    out
}

/// A preprocessed vertex/fragment pair, ready for a backend translator.
#[derive(Debug, Clone)]
pub struct PreparedPair {
    pub combos: Vec<ComboDecl>,
    /// Final define set: engine defines, then combo values (overrides win).
    pub defines: Vec<(String, String)>,
    pub requires: Vec<String>,
    /// Sources after include expansion and conditional evaluation.
    pub vertex: String,
    pub fragment: String,
}

impl PreparedPair {
    pub fn glsl(&self) -> Result<GlslPair> {
        glsl::to_glsl450(&self.vertex, &self.fragment, &self.defines)
    }

    pub fn hlsl(&self) -> Result<HlslPair> {
        hlsl::to_hlsl(&self.vertex, &self.fragment, &self.defines)
    }
}

/// Runs include expansion, combo collection and conditional evaluation.
/// `overrides` set combo values (e.g. from a material's `combos`).
pub fn prepare_pair(
    vertex: &str,
    fragment: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
    base_defines: &[(String, String)],
    overrides: &BTreeMap<String, i64>,
) -> Result<PreparedPair> {
    let vs = preprocess::expand_includes(vertex, resolve)?;
    let fs = preprocess::expand_includes(fragment, resolve)?;

    let mut combos = decl::parse_combos(&vs);
    for c in decl::parse_combos(&fs) {
        if !combos.iter().any(|e| e.name == c.name) {
            combos.push(c);
        }
    }

    let mut defines: Vec<(String, String)> = base_defines.to_vec();
    for c in &combos {
        let value = overrides.get(&c.name).copied().unwrap_or(c.default);
        defines.push((c.name.clone(), value.to_string()));
    }
    for (name, value) in overrides {
        if !combos.iter().any(|c| &c.name == name) {
            defines.push((name.clone(), value.to_string()));
        }
    }
    let table: HashMap<String, String> = defines.iter().cloned().collect();

    let vs = preprocess::evaluate_conditionals(&vs, &table)?;
    let fs = preprocess::evaluate_conditionals(&fs, &table)?;
    let mut requires = vs.requires;
    requires.extend(fs.requires);
    requires.dedup();

    Ok(PreparedPair {
        combos,
        defines,
        requires,
        vertex: splice_requires(&vs.text),
        fragment: splice_requires(&fs.text),
    })
}
