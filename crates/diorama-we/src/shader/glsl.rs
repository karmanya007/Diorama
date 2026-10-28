//! Translation of preprocessed WE shaders to Vulkan-style GLSL 450 (path A of
//! the ADR-003 spike).
//!
//! - HLSL builtins used by the dialect (`frac`, `saturate`, `mul`, …) become
//!   GLSL via a macro prelude.
//! - Loose uniforms are gathered into one std140 block shared by both stages.
//! - `uniform sampler2D X` becomes a separate `texture2D X` + `sampler X_s`;
//!   every use of `X` in code becomes `X, X_s` so `texSample2D(X, uv)` expands
//!   to `texture(sampler2D(X, X_s), uv)`.
//! - `attribute` / `varying` get explicit locations; varyings are matched by
//!   name across stages.

use std::collections::HashMap;
use std::fmt::Write as _;

use super::decl::{Decl, Qualifier, parse_decl_line};
use crate::error::{Error, Result};

const PRELUDE: &str = r"#define frac fract
#define lerp mix
#define saturate(x) clamp((x), 0.0, 1.0)
#define mul(x, y) ((x) * (y))
#define atan2 atan
#define ddx dFdx
#define ddy dFdy
#define rsqrt inversesqrt
#define log10(x) (log(x) * 0.43429448190325176)
#define float2 vec2
#define float3 vec3
#define float4 vec4
#define float2x2 mat2
#define float3x3 mat3
#define float4x4 mat4
#define int2 ivec2
#define int3 ivec3
#define int4 ivec4
#define uint2 uvec2
#define uint3 uvec3
#define uint4 uvec4
#define CASTI(x) int(x)
#define CASTU(x) uint(x)
#define CASTF(x) float(x)
#define CAST2(x) vec2(x)
#define CAST3(x) vec3(x)
#define CAST4(x) vec4(x)
#define CAST4U(x) uvec4(x)
#define CAST3X3(x) mat3(x)
#define texSample2D(t, s, u) texture(sampler2D(t, s), (u))
#define texSample2DLod(t, s, u, m) textureLod(sampler2D(t, s), (u), (m))
#define texSample3D(t, s, u) texture(sampler3D(t, s), (u))
#define texLoad2D(t, s, u, r) texelFetch(sampler2D(t, s), ivec2((u) * (r)), 0)
#define texSample2DBackBuffer(t, s, u, r) texture(sampler2D(t, s), (u))
#define fmod(x, y) ((x) - (y) * trunc((x) / (y)))
#define clip(x) if (any(lessThan(vec4(x), vec4(0.0)))) discard
#define half float
#define half2 vec2
#define half3 vec3
#define half4 vec4
";

const FRAG_OUT: &str = "diorama_FragColor";

#[derive(Debug, Clone, PartialEq)]
pub struct GlslPair {
    pub vertex: String,
    pub fragment: String,
    /// Vertex-stage uniforms in block order (set 0, binding 0).
    pub vertex_uniforms: Vec<Decl>,
    /// Fragment-stage uniforms in block order (set 0, binding 1).
    pub fragment_uniforms: Vec<Decl>,
    /// Textures in binding order: texture at `2i`, sampler at `2i + 1` (set 1).
    pub textures: Vec<Decl>,
    pub attributes: Vec<Decl>,
}

#[derive(Debug, Default)]
pub(super) struct Stage {
    pub(super) decls: Vec<Decl>,
    pub(super) body: String,
}

pub(super) fn split_stage(source: &str) -> Result<Stage> {
    let source = strip_comments_keep_annotations(source);
    let mut stage = Stage::default();
    for line in source.lines() {
        match parse_decl_line(line)? {
            Some(decls) => {
                stage.decls.extend(decls);
                stage.body.push('\n');
            }
            None => {
                stage.body.push_str(strip_line_comment(line));
                stage.body.push('\n');
            }
        }
    }
    Ok(stage)
}

fn strip_line_comment(line: &str) -> &str {
    line.split_once("//").map_or(line, |(code, _)| code)
}

/// Removes `/* */` comments (keeping newlines). `//` comments are kept here
/// because declaration annotations live in them; they are dropped per line.
fn strip_comments_keep_annotations(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(start) = rest.find("/*") {
        // A `/*` inside a `//` comment is not a block comment.
        let line_start = rest[..start].rfind('\n').map_or(0, |i| i + 1);
        if rest[line_start..start].contains("//") {
            let line_end = rest[start..].find('\n').map_or(rest.len(), |i| start + i);
            out.push_str(&rest[..line_end]);
            rest = &rest[line_end..];
            continue;
        }
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => {
                let comment = &rest[start..start + 2 + end + 2];
                out.extend(comment.chars().filter(|&c| c == '\n'));
                rest = &rest[start + 2 + end + 2..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Merges declarations of one qualifier across stages, keeping first-seen order.
pub(super) fn union(stages: [&Stage; 2], keep: impl Fn(&Decl) -> bool) -> Result<Vec<Decl>> {
    let mut out: Vec<Decl> = Vec::new();
    for stage in stages {
        for d in stage.decls.iter().filter(|d| keep(d)) {
            match out.iter().find(|e| e.name == d.name) {
                Some(existing) if existing.ty != d.ty || existing.array != d.array => {
                    return Err(Error::Shader(format!(
                        "{} declared as {} and {}",
                        d.name, existing.ty, d.ty
                    )));
                }
                Some(_) => {}
                None => out.push(d.clone()),
            }
        }
    }
    Ok(out)
}

fn texture_types(sampler_ty: &str) -> Result<(&'static str, &'static str)> {
    Ok(match sampler_ty {
        "sampler2D" | "sampler2DBackBuffer" => ("texture2D", "sampler"),
        "sampler3D" => ("texture3D", "sampler"),
        "samplerCube" => ("textureCube", "sampler"),
        "sampler2DShadow" | "sampler2DComparison" => ("texture2D", "samplerShadow"),
        other => return Err(Error::Shader(format!("unsupported sampler type {other}"))),
    })
}

pub(super) fn array_suffix(d: &Decl) -> String {
    d.array
        .as_ref()
        .map(|n| format!("[{n}]"))
        .unwrap_or_default()
}

pub(super) fn is_integer_type(ty: &str) -> bool {
    matches!(ty, "int" | "uint" | "bool") || ty.starts_with("ivec") || ty.starts_with("uvec")
}

/// Interface slots a declaration occupies: matrices take one per column,
/// arrays multiply. Unparseable array lengths count as one.
pub(super) fn location_slots(d: &Decl, defines: &HashMap<String, String>) -> usize {
    let per = match d.ty.as_str() {
        "mat2" => 2,
        "mat3" | "mat4x3" => 3,
        "mat4" => 4,
        _ => 1,
    };
    let count = d
        .array
        .as_deref()
        .and_then(|n| {
            n.parse::<usize>()
                .ok()
                .or_else(|| defines.get(n).and_then(|v| v.parse().ok()))
        })
        .unwrap_or(1);
    per * count.max(1)
}

/// Words legal as HLSL identifiers but reserved in GLSL 450. `texture` is safe
/// to rename because the dialect never calls it; our prelude macros (not
/// rewritten) are the only callers.
const GLSL_RESERVED: [&str; 26] = [
    "sample",
    "input",
    "output",
    "filter",
    "common",
    "partition",
    "active",
    "buffer",
    "shared",
    "smooth",
    "flat",
    "noperspective",
    "patch",
    "resource",
    "texture",
    "superp",
    "precise",
    "subroutine",
    "coherent",
    "volatile",
    "restrict",
    "readonly",
    "writeonly",
    "namespace",
    "interface",
    "this",
];

/// Rewrites identifier tokens: texture names → `X, X_s`, `gl_FragColor` →
/// fragment output, GLSL-reserved words and `gl_*` varyings → `diorama_` names.
fn rewrite_identifiers(body: &str, textures: &[Decl], renamed_builtins: &[&str]) -> String {
    let names: HashMap<&str, ()> = textures.iter().map(|t| (t.name.as_str(), ())).collect();
    let mut out = String::with_capacity(body.len() + body.len() / 8);
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &body[start..i];
            if names.contains_key(ident) {
                let _ = write!(out, "{ident}, {ident}_s");
            } else if ident == "gl_FragColor" {
                out.push_str(FRAG_OUT);
            } else if ident == "gl_InstanceID" {
                out.push_str("gl_InstanceIndex");
            } else if ident == "gl_VertexID" {
                out.push_str("gl_VertexIndex");
            } else if GLSL_RESERVED.contains(&ident) || renamed_builtins.contains(&ident) {
                let _ = write!(out, "diorama_{ident}");
            } else {
                out.push_str(ident);
            }
        } else if c.is_ascii_digit() {
            // Numbers like `1e5` or `0x1F` must not be split into identifiers.
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
                i += 1;
            }
            out.push_str(&body[start..i]);
        } else {
            let ch_len = body[i..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&body[i..i + ch_len]);
            i += ch_len;
        }
    }
    out
}

/// Translates a preprocessed vertex/fragment pair. `defines` are emitted as
/// `#define` lines so macro uses of combo values in code still resolve.
pub fn to_glsl450(vertex: &str, fragment: &str, defines: &[(String, String)]) -> Result<GlslPair> {
    let vs = split_stage(vertex)?;
    let fs = split_stage(fragment)?;

    let textures: Vec<Decl> = union([&vs, &fs], |d| {
        d.qualifier == Qualifier::Uniform && d.is_texture()
    })?;
    // WE compiles each stage with its own constant buffer, so the same name may
    // carry different types per stage.
    let stage_uniforms = |s: &Stage| -> Vec<Decl> {
        s.decls
            .iter()
            .filter(|d| d.qualifier == Qualifier::Uniform && !d.is_texture())
            .cloned()
            .collect()
    };
    let vertex_uniforms = stage_uniforms(&vs);
    let fragment_uniforms = stage_uniforms(&fs);
    let attributes = union([&vs, &vs], |d| d.qualifier == Qualifier::Attribute)?;

    // A body `#define X` must win over a header define of the same name
    // (C redefinition semantics), so skip those in the header.
    let body_defines = body_defines(&vs, &fs);
    let define_table: HashMap<String, String> = defines.iter().cloned().collect();

    let mut common =
        String::from("#version 450\n// Generated by diorama-we from a Wallpaper Engine shader.\n");
    for (name, value) in defines {
        if !body_defines.contains(&name.as_str()) {
            let _ = writeln!(common, "#define {name} {value}");
        }
    }
    // Shaders sometimes carry their own copy of a prelude macro (e.g. `#define frac fract`).
    for line in PRELUDE.lines() {
        let name = line
            .trim_start_matches("#define ")
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .next()
            .unwrap_or("");
        if !body_defines.contains(&name) {
            common.push_str(line);
            common.push('\n');
        }
    }
    for (i, t) in textures.iter().enumerate() {
        let (tex_ty, sampler_ty) = texture_types(&t.ty)?;
        let _ = writeln!(
            common,
            "layout(set = 1, binding = {}) uniform {tex_ty} {}{};\n\
             layout(set = 1, binding = {}) uniform {sampler_ty} {}_s{};",
            2 * i,
            t.name,
            array_suffix(t),
            2 * i + 1,
            t.name,
            array_suffix(t),
        );
    }

    let mut vertex_out = common.clone();
    let mut fragment_out = common;
    write_uniform_block(&mut vertex_out, "WeVertexUniforms", 0, &vertex_uniforms);
    write_uniform_block(
        &mut fragment_out,
        "WeFragmentUniforms",
        1,
        &fragment_uniforms,
    );

    let mut location = 0;
    for a in &attributes {
        let _ = writeln!(
            vertex_out,
            "layout(location = {location}) in {} {};",
            a.ty, a.name
        );
        location += location_slots(a, &define_table);
    }

    // Varyings: matched by name; each stage keeps its own declared type
    // (HLSL lets the pixel shader read fewer components than the vertex shader writes).
    let mut renamed_builtins: Vec<&str> = Vec::new();
    let mut locations: Vec<(&str, usize)> = Vec::new();
    let mut next = 0;
    for v in vs
        .decls
        .iter()
        .chain(&fs.decls)
        .filter(|d| d.qualifier == Qualifier::Varying)
    {
        if v.name.starts_with("gl_") {
            renamed_builtins.push(&v.name);
        } else if !locations.iter().any(|(n, _)| *n == v.name) {
            locations.push((&v.name, next));
            next += location_slots(v, &define_table);
        }
    }
    for (stage, out, dir) in [
        (&vs, &mut vertex_out, "out"),
        (&fs, &mut fragment_out, "in"),
    ] {
        for v in stage
            .decls
            .iter()
            .filter(|d| d.qualifier == Qualifier::Varying)
        {
            let decl = format!("{} {}{}", v.ty, v.name, array_suffix(v));
            if v.name.starts_with("gl_") {
                // Builtins like gl_ViewportIndex (layered rendering) become plain globals.
                let _ = writeln!(out, "{} diorama_{}{};", v.ty, v.name, array_suffix(v));
                continue;
            }
            let Some(&(_, loc)) = locations.iter().find(|(n, _)| *n == v.name) else {
                continue;
            };
            let flat = if is_integer_type(&v.ty) { "flat " } else { "" };
            let _ = writeln!(out, "layout(location = {loc}) {flat}{dir} {decl};");
        }
    }
    let _ = writeln!(fragment_out, "layout(location = 0) out vec4 {FRAG_OUT};");

    vertex_out.push_str(&rewrite_identifiers(&vs.body, &textures, &renamed_builtins));
    fragment_out.push_str(&rewrite_identifiers(&fs.body, &textures, &renamed_builtins));

    Ok(GlslPair {
        vertex: vertex_out,
        fragment: fragment_out,
        vertex_uniforms,
        fragment_uniforms,
        textures,
        attributes,
    })
}

/// Names of macros the shader body `#define`s itself.
pub(super) fn body_defines<'a>(vs: &'a Stage, fs: &'a Stage) -> Vec<&'a str> {
    [&vs.body, &fs.body]
        .into_iter()
        .flat_map(|b| b.lines())
        .filter_map(|l| {
            let rest = l
                .trim_start()
                .strip_prefix('#')?
                .trim_start()
                .strip_prefix("define")?;
            rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .find(|w| !w.is_empty())
        })
        .collect()
}

fn write_uniform_block(out: &mut String, name: &str, binding: u32, uniforms: &[Decl]) {
    if uniforms.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "layout(set = 0, binding = {binding}, std140) uniform {name} {{"
    );
    for u in uniforms {
        let _ = writeln!(out, "    {} {}{};", u.ty, u.name, array_suffix(u));
    }
    out.push_str("};\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    const VS: &str = "uniform mat4 g_ModelViewProjectionMatrix;\n\
        attribute vec3 a_Position;\n\
        attribute vec2 a_TexCoord;\n\
        varying vec2 v_TexCoord;\n\
        void main() {\n\
            gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);\n\
            v_TexCoord = a_TexCoord;\n\
        }\n";

    const FS: &str = "uniform sampler2D g_Texture0; // {\"hidden\":true}\n\
        uniform float g_Alpha; /* block */ // {\"material\":\"alpha\",\"default\":1}\n\
        varying vec2 v_TexCoord;\n\
        void main() {\n\
            gl_FragColor = texSample2D(g_Texture0, v_TexCoord) * g_Alpha;\n\
        }\n";

    #[test]
    fn translates_basic_pair() {
        let out = to_glsl450(VS, FS, &[("MASK".into(), "0".into())]).unwrap();
        assert_eq!(out.vertex_uniforms.len(), 1);
        assert_eq!(out.fragment_uniforms.len(), 1);
        assert_eq!(out.textures.len(), 1);
        assert_eq!(out.attributes.len(), 2);
        assert_eq!(
            out.fragment_uniforms[0].annotation.as_ref().unwrap()["material"],
            "alpha"
        );
        assert!(
            out.vertex
                .contains("layout(location = 0) in vec3 a_Position;")
        );
        assert!(
            out.vertex
                .contains("layout(location = 0) out vec2 v_TexCoord;")
        );
        assert!(
            out.fragment
                .contains("layout(location = 0) in vec2 v_TexCoord;")
        );
        assert!(
            out.fragment
                .contains("texSample2D(g_Texture0, g_Texture0_s, v_TexCoord)")
        );
        assert!(out.fragment.contains("diorama_FragColor ="));
        assert!(out.fragment.contains("#define MASK 0"));
        assert!(out.vertex.contains("uniform WeVertexUniforms"));
        assert!(
            out.fragment
                .contains("binding = 1, std140) uniform WeFragmentUniforms")
        );
        assert!(!out.vertex.contains("g_Alpha;"));
    }

    #[test]
    fn varyings_keep_per_stage_types_and_slot_sizes() {
        let vs = "varying vec4 v_A[4];\nvarying vec4 v_B;\nvarying uint gl_ViewportIndex;\nvoid main() { gl_ViewportIndex = 0u; }\n";
        let fs = "varying vec2 v_B;\nvoid main() {}\n";
        let out = to_glsl450(vs, fs, &[]).unwrap();
        assert!(out.vertex.contains("layout(location = 0) out vec4 v_A[4];"));
        assert!(out.vertex.contains("layout(location = 4) out vec4 v_B;"));
        assert!(out.fragment.contains("layout(location = 4) in vec2 v_B;"));
        assert!(out.vertex.contains("uint diorama_gl_ViewportIndex;"));
        assert!(out.vertex.contains("diorama_gl_ViewportIndex = 0u;"));
    }

    #[test]
    fn body_defines_override_header_defines() {
        let fs = "#define RESOLUTION 32\nvoid main() {}\n";
        let out = to_glsl450(
            "void main() {}\n",
            fs,
            &[("RESOLUTION".into(), "16".into())],
        )
        .unwrap();
        assert!(!out.fragment.contains("#define RESOLUTION 16"));
        assert!(out.fragment.contains("#define RESOLUTION 32"));
    }

    #[test]
    fn reserved_words_are_renamed() {
        let out = rewrite_identifiers("vec4 sample = x; sampleCount", &[], &[]);
        assert_eq!(out, "vec4 diorama_sample = x; sampleCount");
    }

    #[test]
    fn rewrite_keeps_numbers_and_similar_names() {
        let tex = parse_decl_line("uniform sampler2D g_Texture0;")
            .unwrap()
            .unwrap();
        let out = rewrite_identifiers(
            "g_Texture0Resolution 1e5 g_Texture0 x.g_Texture0",
            &tex,
            &[],
        );
        assert_eq!(
            out,
            "g_Texture0Resolution 1e5 g_Texture0, g_Texture0_s x.g_Texture0, g_Texture0_s"
        );
    }

    #[test]
    fn conflicting_declarations_are_errors() {
        // Textures share one binding table, so their types must agree across stages.
        let vs = "uniform sampler2D g_Texture0;\nvoid main() {}\n";
        let fs = "uniform sampler3D g_Texture0;\nvoid main() {}\n";
        assert!(to_glsl450(vs, fs, &[]).is_err());
    }

    #[test]
    fn block_comments_removed_but_annotations_survive() {
        let src = "a /* x\ny */ b // {\"k\":1} /* not a block\nc";
        let out = strip_comments_keep_annotations(src);
        assert_eq!(out, "a \n b // {\"k\":1} /* not a block\nc");
    }
}
