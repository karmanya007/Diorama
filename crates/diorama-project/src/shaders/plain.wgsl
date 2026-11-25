// The fragment stage of a layer the GPU blends. `source` is in image.wgsl or
// text.wgsl.

struct Layer {
    g_Color4: vec4<f32>,
}
@group(0) @binding(1) var<uniform> layer: Layer;
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;

@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return source(uv);
}
