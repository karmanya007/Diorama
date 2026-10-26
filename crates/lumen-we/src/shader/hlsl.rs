//! Translation of preprocessed WE shaders to HLSL (path B of the ADR-003
//! spike), mirroring what Wallpaper Engine itself does before calling the D3D
//! compiler. Compile the output with DXC (`-spirv` for wgpu).
//!
//! Differences from WE's own rewrite: interface variables become `static`
//! globals copied in/out by generated entry points (so helper functions may
//! touch them too), and every interface member carries `[[vk::location]]` so
//! SPIR-V vertex outputs and fragment inputs match by name.

use std::collections::HashMap;
use std::fmt::Write as _;

use super::decl::{Decl, Qualifier};
use super::glsl::{
    Stage, array_suffix, body_defines, is_integer_type, location_slots, split_stage, union,
};
use crate::error::{Error, Result};

/// GLSL-name → HLSL prelude, as found in `wallpaper64.exe`.
const PRELUDE: &str = r"#define vec2 float2
#define vec3 float3
#define vec4 float4
#define ivec2 int2
#define ivec3 int3
#define ivec4 int4
#define uvec2 uint2
#define uvec3 uint3
#define uvec4 uint4
#define bvec2 bool2
#define bvec3 bool3
#define bvec4 bool4
#define mat4 float4x4
#define mat4x3 float4x3
#define mat3 float3x3
#define mat2 float2x2
#define mix lerp
#define mod(x, y) ((x)-(y)*floor((x)/(y)))
#define CASTI(x) ((int)(x))
#define CASTU(x) ((uint)(x))
#define CASTF(x) ((float)(x))
#define CAST2(x) ((float2)(x))
#define CAST3(x) ((float3)(x))
#define CAST4U(x) ((uint4)(x))
#define CAST4(x) ((float4)(x))
#define CAST3X3(x) ((float3x3)(x))
// `LUMEN_SS(s)` expands `s` before pasting, so a sampler aliased by a
// macro (`#define g_NormalMapSampler g_Texture1`) finds its sampler state.
#define LUMEN_PASTE_SS(s) s ## SamplerState
#define LUMEN_SS(s) LUMEN_PASTE_SS(s)
#define DECLARE_SAMPLER2D_PARAMETER(t) Texture2D t, SamplerState LUMEN_SS(t)
#define MAKE_SAMPLER2D_ARGUMENT(t) t, LUMEN_SS(t)
#define texSample2D(s, u) s.Sample(LUMEN_SS(s), u)
#define texSample2DLod(s, u, m) s.SampleLevel(LUMEN_SS(s), u, m)
#define texLoad2D(s, u, r) s.Load(int3((u) * (r), 0))
#define texSample3D(s, u) s.Sample(LUMEN_SS(s), u)
#define texSample2DBackBuffer(s, u, r) texSample2D(s, (u))
";

#[derive(Debug, Clone, PartialEq)]
pub struct HlslPair {
    pub vertex: String,
    pub fragment: String,
    pub vertex_entry: &'static str,
    pub fragment_entry: &'static str,
    /// Vertex-stage uniforms in `cbuffer` order (space 0, `b0`).
    pub vertex_uniforms: Vec<Decl>,
    /// Fragment-stage uniforms in `cbuffer` order (space 0, `b1`).
    pub fragment_uniforms: Vec<Decl>,
    /// Textures in binding order: texture at `t(2i)`, sampler at `s(2i + 1)` (space 1).
    pub textures: Vec<Decl>,
    /// Vertex inputs; locations are assigned in this order (see `location_slots`).
    pub attributes: Vec<Decl>,
}

fn hlsl_texture_type(sampler_ty: &str) -> Result<&'static str> {
    Ok(match sampler_ty {
        "sampler2D" | "sampler2DBackBuffer" => "Texture2D",
        "sampler3D" => "Texture3D",
        "samplerCube" => "TextureCube",
        other => return Err(Error::Shader(format!("unsupported sampler type {other}"))),
    })
}

/// Renames `main`, `gl_Position`, `gl_FragColor` and `gl_*` varyings.
fn rewrite(body: &str, renamed: &[&str]) -> String {
    let mut out = String::with_capacity(body.len());
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
            match ident {
                "main" => out.push_str("lumen_main"),
                "gl_Position" => out.push_str("lumen_Position"),
                "gl_FragColor" => out.push_str("lumen_FragColor"),
                _ if renamed.contains(&ident) => {
                    let _ = write!(out, "lumen_{ident}");
                }
                _ => out.push_str(ident),
            }
        } else if c.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
                i += 1;
            }
            out.push_str(&body[start..i]);
        } else {
            let len = body[i..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&body[i..i + len]);
            i += len;
        }
    }
    out
}

/// Type names a top-level variable declaration can start with.
const GLOBAL_TYPES: [&str; 21] = [
    "float", "int", "uint", "bool", "vec2", "vec3", "vec4", "ivec2", "ivec3", "ivec4", "uvec2",
    "uvec3", "uvec4", "mat2", "mat3", "mat4", "mat4x3", "float2", "float3", "float4", "float4x4",
];

/// In HLSL every global that is not `static` is a uniform: DXC gathers them
/// into an implicit `$Globals` buffer at `b0`, colliding with our vertex
/// cbuffer. GLSL globals (`const` or not) are plain values, so mark
/// top-level variable declarations `static`.
fn staticize_globals(body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 64);
    // Brace and parenthesis depth: continuation lines of a multi-line
    // parameter list (`float radius, ...`) look like declarations too.
    let (mut braces, mut parens) = (0i32, 0i32);
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if braces == 0 && parens == 0 && is_global_variable(trimmed) {
            let indent = &line[..line.len() - trimmed.len()];
            out.push_str(indent);
            out.push_str("static ");
            out.push_str(trimmed);
        } else {
            out.push_str(line);
        }
        for c in line.chars() {
            match c {
                '{' => braces += 1,
                '}' => braces -= 1,
                '(' => parens += 1,
                ')' => parens -= 1,
                _ => {}
            }
        }
    }
    out
}

fn is_global_variable(line: &str) -> bool {
    if line.starts_with("static ") || line.starts_with('#') {
        return false;
    }
    let rest = line.strip_prefix("const ").unwrap_or(line);
    let mut words = rest.split_whitespace();
    let Some(ty) = words.next() else {
        return false;
    };
    if !GLOBAL_TYPES.contains(&ty) {
        return false;
    }
    // `type name = ...;` / `type name[3];`, not `type name(args)` (a function).
    let after_type = rest[ty.len()..].trim_start();
    let name_end = after_type
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(after_type.len());
    if name_end == 0 {
        return false;
    }
    let tail = after_type[name_end..].trim_start();
    tail.starts_with('=') || tail.starts_with(';') || tail.starts_with('[') || tail.starts_with(',')
}

fn builtin_semantic(name: &str) -> Option<&'static str> {
    match name {
        "gl_InstanceID" => Some("SV_InstanceID"),
        "gl_VertexID" => Some("SV_VertexID"),
        _ => None,
    }
}

fn interp(ty: &str) -> &'static str {
    if is_integer_type(ty) {
        "nointerpolation "
    } else {
        ""
    }
}

pub fn to_hlsl(vertex: &str, fragment: &str, defines: &[(String, String)]) -> Result<HlslPair> {
    let vs = split_stage(vertex)?;
    let mut fs = split_stage(fragment)?;
    separate_stage_uniforms(&vs, &mut fs);
    let textures: Vec<Decl> = union([&vs, &fs], |d| {
        d.qualifier == Qualifier::Uniform && d.is_texture()
    })?;
    let body_defines = body_defines(&vs, &fs);
    let define_table: HashMap<String, String> = defines.iter().cloned().collect();

    let mut common = String::from("// Generated by lumen-we from a Wallpaper Engine shader.\n");
    for (name, value) in defines {
        if !body_defines.contains(&name.as_str()) {
            let _ = writeln!(common, "#define {name} {value}");
        }
    }
    common.push_str(PRELUDE);
    // Texture i → t(2i), its sampler → s(2i+1): distinct SPIR-V bindings in set 1.
    for (i, t) in textures.iter().enumerate() {
        let ty = hlsl_texture_type(&t.ty)?;
        let _ = writeln!(
            common,
            "{ty} {name}{arr} : register(t{tb}, space1);\nSamplerState {name}SamplerState{arr} : register(s{sb}, space1);",
            name = t.name,
            arr = array_suffix(t),
            tb = 2 * i,
            sb = 2 * i + 1,
        );
    }

    // Varying locations matched by name across stages.
    let mut renamed: Vec<&str> = Vec::new();
    let mut locations: Vec<(&str, usize)> = Vec::new();
    let mut next = 0;
    for v in vs
        .decls
        .iter()
        .chain(&fs.decls)
        .filter(|d| d.qualifier == Qualifier::Varying)
    {
        if v.name.starts_with("gl_") {
            renamed.push(&v.name);
        } else if !locations.iter().any(|(n, _)| *n == v.name) {
            locations.push((&v.name, next));
            next += location_slots(v, &define_table);
        }
    }
    let location_of = |name: &str| locations.iter().find(|(n, _)| *n == name).map(|(_, l)| *l);

    let vertex = emit_vertex(&common, &vs, &define_table, &renamed, &location_of);
    let fragment = emit_fragment(&common, &fs, &define_table, &renamed, &location_of);
    Ok(HlslPair {
        vertex,
        fragment,
        vertex_entry: "lumen_vs",
        fragment_entry: "lumen_ps",
        vertex_uniforms: stage_uniforms(&vs),
        fragment_uniforms: stage_uniforms(&fs),
        textures,
        attributes: vs
            .decls
            .iter()
            .filter(|d| d.qualifier == Qualifier::Attribute)
            .cloned()
            .collect(),
    })
}

/// A uniform both stages declare for different material keys holds two
/// values in WE (foliagesway: `g_Speed` is `speed` in the vertex shader and
/// `speeduv` in the fragment shader; capture of 2581587068 has 1 and 3), so
/// the fragment one is renamed `<name>_ps` to get its own constant.
fn separate_stage_uniforms(vs: &Stage, fs: &mut Stage) {
    let key = |d: &Decl| {
        d.annotation
            .as_ref()
            .and_then(|a| a["material"].as_str())
            .map(str::to_owned)
    };
    let mut renames = Vec::new();
    for d in fs
        .decls
        .iter_mut()
        .filter(|d| d.qualifier == Qualifier::Uniform && !d.is_texture())
    {
        let Some(k) = key(d) else {
            continue;
        };
        let clash = vs.decls.iter().any(|v| {
            v.qualifier == Qualifier::Uniform
                && v.name == d.name
                && key(v).is_some_and(|kv| kv != k)
        });
        if clash {
            let new = format!("{}_ps", d.name);
            renames.push((d.name.clone(), new.clone()));
            d.name = new;
        }
    }
    for (old, new) in renames {
        fs.body = replace_word(&fs.body, &old, &new);
    }
}

/// `text` with every whole identifier `from` replaced by `to`.
fn replace_word(text: &str, from: &str, to: &str) -> String {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(from) {
        let before = rest[..i].chars().next_back();
        let after = rest[i + from.len()..].chars().next();
        out.push_str(&rest[..i]);
        if before.is_some_and(is_ident) || after.is_some_and(is_ident) {
            out.push_str(from);
        } else {
            out.push_str(to);
        }
        rest = &rest[i + from.len()..];
    }
    out.push_str(rest);
    out
}

fn stage_uniforms(stage: &Stage) -> Vec<Decl> {
    stage
        .decls
        .iter()
        .filter(|d| d.qualifier == Qualifier::Uniform && !d.is_texture())
        .cloned()
        .collect()
}

fn stage_cbuffer(out: &mut String, stage: &Stage, name: &str, register: u32) {
    let uniforms = stage_uniforms(stage);
    if uniforms.is_empty() {
        return;
    }
    let _ = writeln!(out, "cbuffer {name} : register(b{register}, space0) {{");
    for u in uniforms {
        let _ = writeln!(out, "    {} {}{};", u.ty, u.name, array_suffix(&u));
    }
    out.push_str("};\n");
}

fn varyings(stage: &Stage) -> impl Iterator<Item = &Decl> {
    stage
        .decls
        .iter()
        .filter(|d| d.qualifier == Qualifier::Varying)
}

fn emit_vertex(
    common: &str,
    vs: &Stage,
    defines: &HashMap<String, String>,
    renamed: &[&str],
    location_of: &dyn Fn(&str) -> Option<usize>,
) -> String {
    let mut out = String::from(common);
    stage_cbuffer(&mut out, vs, "WeVertexUniforms", 0);

    let attributes: Vec<&Decl> = vs
        .decls
        .iter()
        .filter(|d| d.qualifier == Qualifier::Attribute)
        .collect();
    out.push_str("struct LumenVsIn {\n");
    let mut loc = 0;
    for a in &attributes {
        let _ = writeln!(
            out,
            "    [[vk::location({loc})]] {} {} : ATTRIBUTE{loc};",
            a.ty, a.name
        );
        loc += location_slots(a, defines);
    }
    let builtins: Vec<(&Decl, &str)> = vs
        .decls
        .iter()
        .filter(|d| d.qualifier == Qualifier::BuiltinIn)
        .filter_map(|d| builtin_semantic(&d.name).map(|s| (d, s)))
        .collect();
    for (b, semantic) in &builtins {
        let _ = writeln!(out, "    {} lumen_in_{} : {semantic};", b.ty, b.name);
    }
    out.push_str("};\n");
    out.push_str("struct LumenVsOut {\n    float4 lumen_position : SV_Position;\n");
    let members = interface_members(vs, defines, location_of);
    write_members(&mut out, &members);
    out.push_str("};\n");

    for a in &attributes {
        let _ = writeln!(out, "static {} {};", a.ty, a.name);
    }
    for (b, _) in &builtins {
        let _ = writeln!(out, "static {} {};", b.ty, b.name);
    }
    for v in varyings(vs) {
        let name = if v.name.starts_with("gl_") {
            format!("lumen_{}", v.name)
        } else {
            v.name.clone()
        };
        let _ = writeln!(out, "static {} {name}{};", v.ty, array_suffix(v));
    }
    out.push_str("static float4 lumen_Position;\n");

    out.push_str(&staticize_globals(&rewrite(&vs.body, renamed)));

    out.push_str("\nLumenVsOut lumen_vs(LumenVsIn lumen_in) {\n");
    for a in &attributes {
        let _ = writeln!(out, "    {0} = lumen_in.{0};", a.name);
    }
    for (b, _) in &builtins {
        let _ = writeln!(out, "    {0} = lumen_in.lumen_in_{0};", b.name);
    }
    out.push_str("    lumen_main();\n    LumenVsOut lumen_out;\n    lumen_out.lumen_position = lumen_Position;\n");
    for m in &members {
        let _ = writeln!(out, "    lumen_out.{} = {};", m.member, m.global);
    }
    out.push_str("    return lumen_out;\n}\n");
    out
}

/// One stage-IO struct member. Array varyings are flattened to one member per
/// element because naga (WGSL rules) rejects array-typed stage IO.
struct Member {
    member: String,
    global: String,
    ty: String,
    location: usize,
}

fn interface_members(
    stage: &Stage,
    defines: &HashMap<String, String>,
    location_of: &dyn Fn(&str) -> Option<usize>,
) -> Vec<Member> {
    let mut out = Vec::new();
    for v in varyings(stage) {
        let Some(base) = location_of(&v.name) else {
            continue;
        };
        let len = v.array.as_deref().and_then(|n| {
            n.parse::<usize>()
                .ok()
                .or_else(|| defines.get(n).and_then(|d| d.parse().ok()))
        });
        match len {
            Some(n) => {
                let element = Decl {
                    array: None,
                    ..v.clone()
                };
                let stride = location_slots(&element, defines);
                for k in 0..n {
                    out.push(Member {
                        member: format!("{}_{k}", v.name),
                        global: format!("{}[{k}]", v.name),
                        ty: v.ty.clone(),
                        location: base + k * stride,
                    });
                }
            }
            None => out.push(Member {
                member: v.name.clone(),
                global: v.name.clone(),
                ty: v.ty.clone(),
                location: base,
            }),
        }
    }
    out
}

fn write_members(out: &mut String, members: &[Member]) {
    for m in members {
        let l = m.location;
        let _ = writeln!(
            out,
            "    [[vk::location({l})]] {}{} {} : TEXCOORD{l};",
            interp(&m.ty),
            m.ty,
            m.member
        );
    }
}

fn emit_fragment(
    common: &str,
    fs: &Stage,
    defines: &HashMap<String, String>,
    renamed: &[&str],
    location_of: &dyn Fn(&str) -> Option<usize>,
) -> String {
    let mut out = String::from(common);
    stage_cbuffer(&mut out, fs, "WeFragmentUniforms", 1);

    out.push_str("struct LumenPsIn {\n    float4 lumen_position : SV_Position;\n");
    let members = interface_members(fs, defines, location_of);
    write_members(&mut out, &members);
    out.push_str("};\n");
    for v in varyings(fs) {
        let name = if v.name.starts_with("gl_") {
            format!("lumen_{}", v.name)
        } else {
            v.name.clone()
        };
        let _ = writeln!(out, "static {} {name}{};", v.ty, array_suffix(v));
    }
    out.push_str("static float4 lumen_FragColor;\n");

    out.push_str(&staticize_globals(&rewrite(&fs.body, renamed)));

    out.push_str("\nfloat4 lumen_ps(LumenPsIn lumen_in) : SV_Target0 {\n");
    for m in &members {
        let _ = writeln!(out, "    {} = lumen_in.{};", m.global, m.member);
    }
    out.push_str("    lumen_main();\n    return lumen_FragColor;\n}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_variables_become_static() {
        let body = "const mat3 M = mat3(a, b, c);\nfloat g = 1.0;\nvec3 f(vec3 x) {\n    float local = 2.0;\n    return x;\n}\nconst float K[2] = {1.0, 2.0};\n";
        let out = staticize_globals(body);
        assert!(out.contains("static const mat3 M"));
        assert!(out.contains("static float g = 1.0;"));
        assert!(out.contains("static const float K[2]"));
        assert!(out.contains("vec3 f(vec3 x) {"));
        assert!(out.contains("    float local = 2.0;"));
        assert!(!out.contains("static vec3 f"));
        let signature =
            "vec3 Light(vec3 n,\n    float radius, float exponent)\n{\n    return n;\n}\n";
        assert_eq!(staticize_globals(signature), signature);
    }

    #[test]
    fn stages_keep_their_own_material_uniforms() {
        // foliagesway: `speed` drives the vertex shader, `speeduv` the fragment.
        let vs = "uniform float g_Speed; // {\"material\":\"speed\",\"default\":1}
                  uniform float g_Phase; // {\"material\":\"phase\",\"default\":0}
                  attribute vec3 a_Position;
void main() {
                  gl_Position = vec4(a_Position * g_Speed * g_Phase, 1.0);
}
";
        let fs = "uniform float g_Speed; // {\"material\":\"speeduv\",\"default\":5}
                  uniform float g_Phase; // {\"material\":\"phase\",\"default\":0}
                  void main() {
float g_SpeedX = 1.0;
                  gl_FragColor = vec4(g_Speed * g_Phase * g_SpeedX);
}
";
        let out = to_hlsl(vs, fs, &[]).unwrap();
        let names: Vec<&str> = out
            .fragment_uniforms
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(names, ["g_Speed_ps", "g_Phase"]);
        assert_eq!(out.vertex_uniforms[0].name, "g_Speed");
        assert!(out.fragment.contains("g_Speed_ps * g_Phase * g_SpeedX"));
        assert!(out.vertex.contains("g_Speed * g_Phase"));
    }

    #[test]
    fn emits_entry_points_and_bindings() {
        let vs = "uniform mat4 g_ModelViewProjectionMatrix;\nattribute vec3 a_Position;\n\
                  varying vec2 v_TexCoord;\nvoid main() {\n\
                  gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);\n\
                  v_TexCoord = a_Position.xy;\n}\n";
        let fs = "uniform sampler2D g_Texture0;\nvarying vec2 v_TexCoord;\nvoid main() {\n\
                  gl_FragColor = texSample2D(g_Texture0, v_TexCoord);\n}\n";
        let out = to_hlsl(vs, fs, &[]).unwrap();
        assert!(
            out.vertex
                .contains("cbuffer WeVertexUniforms : register(b0, space0)")
        );
        // Declared GLSL type names stay; the prelude macros map them to HLSL.
        assert!(
            out.vertex
                .contains("[[vk::location(0)]] vec2 v_TexCoord : TEXCOORD0;")
        );
        assert!(out.vertex.contains("void lumen_main()"));
        assert!(out.vertex.contains("lumen_Position = mul("));
        assert!(
            out.fragment
                .contains("Texture2D g_Texture0 : register(t0, space1);")
        );
        assert!(
            out.fragment
                .contains("SamplerState g_Texture0SamplerState : register(s1, space1);")
        );
        assert!(
            out.fragment
                .contains("float4 lumen_ps(LumenPsIn lumen_in) : SV_Target0")
        );
        assert!(
            out.fragment
                .contains("lumen_FragColor = texSample2D(g_Texture0, v_TexCoord);")
        );
    }
}
