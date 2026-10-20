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

/// Scene facts the engine bakes into generated shader code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct EngineContext {
    /// Point lights in the scene (`g_LPoint_*` array length).
    pub point_lights: usize,
    /// Tube lights in the scene (`g_LTube_*` array length).
    pub tube_lights: usize,
}

/// `#require LightingV1`: WE's engine generates `PerformLighting_V1` per
/// scene as one unrolled block per light (string fragments in
/// `wallpaper64.exe`). Lumen emits point and tube lights; shadows, cookies,
/// spot and directional lights (none in the corpus) are not generated yet.
fn lighting_v1(engine: &EngineContext) -> String {
    let n = engine.point_lights;
    let tubes = engine.tube_lights;
    let mut out = String::new();
    if n > 0 {
        out.push_str(&format!(
            "uniform vec4 g_LPoint_Origin[{n}];\nuniform vec4 g_LPoint_Color[{n}];\n"
        ));
    }
    if tubes > 0 {
        out.push_str(&format!(
            "uniform vec4 g_LTube_OriginA[{tubes}];\nuniform vec4 g_LTube_OriginB[{tubes}];\n\
             uniform vec4 g_LTube_Color[{tubes}];\n"
        ));
    }
    out.push_str(
        "vec3 PerformLighting_V1(vec3 worldPos, vec3 color, vec3 normal, vec3 viewVector, \
         vec3 specularTint, vec3 ambient, float roughness, float metallic)\n{\n\
         \tvec3 light = CAST3(0.0);\n",
    );
    for i in 0..n {
        out.push_str(&format!(
            "\t{{\n\tconst uint i = {i};\n\
             \tvec3 lightDelta = g_LPoint_Origin[i].xyz - worldPos;\n\
             \tlight += ComputePBRLightShadow(normal, lightDelta, viewVector, color, \
             g_LPoint_Color[i].rgb, g_LPoint_Color[i].w, g_LPoint_Origin[i].w, specularTint, \
             ambient, roughness, metallic, 1.0);\n\t}}\n"
        ));
    }
    for i in 0..tubes {
        out.push_str(&format!(
            "\t{{\n\tconst uint i = {i};\n\
             \tvec3 lightDelta = PointSegmentDelta(worldPos, g_LTube_OriginA[i].xyz, \
             g_LTube_OriginB[i].xyz);\n\
             \tlight += ComputePBRLightShadow(normal, lightDelta, viewVector, color, \
             g_LTube_Color[i].rgb, g_LTube_Color[i].w, g_LTube_OriginA[i].w, specularTint, \
             ambient, roughness, metallic, 1.0);\n\t}}\n"
        ));
    }
    out.push_str("\treturn light;\n}\n");
    out
}

fn splice_requires(text: &str, engine: &EngineContext) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        match line
            .trim()
            .strip_prefix("// ")
            .and_then(|l| l.strip_prefix(REQUIRE_MARKER))
        {
            Some("LightingV1") => out.push_str(&lighting_v1(engine)),
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
    engine: &EngineContext,
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
        vertex: splice_requires(&vs.text, engine),
        fragment: splice_requires(&fs.text, engine),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighting_v1_unrolls_one_block_per_light() {
        let none = lighting_v1(&EngineContext::default());
        assert!(!none.contains("uniform") && none.contains("return light;"));
        let src = lighting_v1(&EngineContext {
            point_lights: 2,
            tube_lights: 1,
        });
        assert!(src.contains("uniform vec4 g_LPoint_Origin[2];"));
        assert!(src.contains("uniform vec4 g_LTube_OriginB[1];"));
        assert_eq!(src.matches("ComputePBRLightShadow(").count(), 3);
        // WE's fragment: tube radius in Color.w, exponent in OriginA.w.
        assert!(src.contains("g_LTube_Color[i].w, g_LTube_OriginA[i].w"));
        assert!(src.contains("PointSegmentDelta(worldPos, g_LTube_OriginA[i].xyz"));
    }
}
