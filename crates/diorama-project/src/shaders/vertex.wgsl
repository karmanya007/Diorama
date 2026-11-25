// The vertex stage of every layer program. `vector * matrix`: see
// `diorama_scene::ShaderLanguage::Wgsl`.

struct Engine {
    g_ModelViewProjectionMatrix: mat4x4<f32>,
}
@group(0) @binding(0) var<uniform> engine: Engine;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>) -> Vertex {
    return Vertex(vec4(position, 1.0) * engine.g_ModelViewProjectionMatrix, uv);
}
