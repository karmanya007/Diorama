
// What every effect starts with. `Settings` comes before this, made from the
// effect's settings. Colours are premultiplied by alpha, coordinates count
// from the top left, and distances are in the wallpaper's pixels.

@group(0) @binding(1) var<uniform> s: Settings;
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;
@group(1) @binding(2) var second: texture_2d<f32>;
@group(1) @binding(3) var second_sampler: sampler;

const TAU: f32 = 6.28318530718;

// The layer's size; a text's is only known when it is drawn.
fn layer_size() -> vec2<f32> {
    let drawn = select(s.g_Texture0Resolution.xy, s.g_Texture1Resolution.xy, s.u_Layer > 0.5);
    return select(drawn, s.u_Size, s.u_Size.x > 0.0);
}

// Where `uv` is within the layer's own picture, without the room around it
// that other effects need.
fn inner(uv: vec2<f32>) -> vec2<f32> {
    return (uv - s.u_Inner.xy) * s.u_Inner.zw;
}

fn pixel(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(image, image_sampler, uv, 0.0);
}

fn straight(c: vec4<f32>) -> vec3<f32> {
    return c.rgb / max(c.a, 1.0 / 1024.0);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3(0.2126, 0.7152, 0.0722));
}

// The way `degrees` points: 0 to the right, 90 down.
fn heading(degrees: f32) -> vec2<f32> {
    let a = radians(degrees);
    return vec2(cos(a), sin(a));
}

// A random number for each whole number pair.
fn hash(p: vec2<f32>) -> f32 {
    let q = bitcast<vec2<u32>>(vec2<i32>(floor(p)));
    var h = (q.x * 0x9E3779B1u) ^ (q.y * 0x85EBCA77u);
    h = (h ^ (h >> 15u)) * 0x2C1B3C6Du;
    h = (h ^ (h >> 12u)) * 0x297A2D39u;
    h = h ^ (h >> 15u);
    return f32(h) / 4294967295.0;
}

// Wanders smoothly in -1..1.
fn wander(t: f32, seed: f32) -> f32 {
    let f = fract(t);
    let k = f * f * (3.0 - 2.0 * f);
    return mix(hash(vec2(t, seed)), hash(vec2(t + 1.0, seed)), k) * 2.0 - 1.0;
}
