// Draws one image over the whole target. `scale` maps target UVs to image UVs
// around the centre; UVs outside [0, 1] show the clear colour (letterboxing).

struct Params {
    scale: vec2<f32>,
    _pad: vec2<f32>,
    clear: vec4<f32>,
};

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Fullscreen triangle: no vertex buffer needed.
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.position = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let uv = (in.uv - 0.5) * params.scale + 0.5;
    let color = textureSample(image, image_sampler, uv);
    let inside = all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
    return select(params.clear, vec4<f32>(color.rgb, 1.0), inside);
}
