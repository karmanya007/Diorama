// The fragment stage of a layer blended in a way the GPU has no state for:
// slot 1 is the picture so far. The result is drawn over it with the layer's
// alpha. `source` is in image.wgsl or text.wgsl.

struct Layer {
    g_Color4: vec4<f32>,
    // Which of `mix_colors`.
    mode: f32,
}
@group(0) @binding(1) var<uniform> layer: Layer;
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;
@group(1) @binding(2) var below: texture_2d<f32>;
@group(1) @binding(3) var below_sampler: sampler;

fn overlay(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    return select(1.0 - 2.0 * (1.0 - a) * (1.0 - b), 2.0 * a * b, a < vec3(0.5));
}

fn soft_light(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let d = select(sqrt(a), ((16.0 * a - 12.0) * a + 4.0) * a, a <= vec3(0.25));
    let lighter = a + (2.0 * b - 1.0) * (d - a);
    let darker = a - (1.0 - 2.0 * b) * a * (1.0 - a);
    return select(lighter, darker, b <= vec3(0.5));
}

// `a` is below, `b` the layer (the formulas of the W3C's compositing and
// blending, on encoded values as browsers and image editors do).
fn mix_colors(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    switch i32(layer.mode) {
        case 1: { return a * b; }
        case 2: { return a + b - a * b; }
        case 3: { return overlay(a, b); }
        case 4: { return min(a, b); }
        case 5: { return max(a, b); }
        case 6: { return overlay(b, a); }
        case 7: { return soft_light(a, b); }
        case 8: { return abs(a - b); }
        default: { return b; }
    }
}

@fragment
fn fs(
    @location(0) uv: vec2<f32>,
    @builtin(position) position: vec4<f32>,
) -> @location(0) vec4<f32> {
    let top = source(uv);
    let size = vec2<f32>(textureDimensions(below));
    let under = textureSample(below, below_sampler, position.xy / size).rgb;
    return vec4(mix_colors(under, top.rgb), top.a);
}
