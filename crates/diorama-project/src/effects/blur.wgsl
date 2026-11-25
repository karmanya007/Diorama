
fn taken(c: vec4<f32>) -> vec4<f32> {
    return c;
}

@fragment
fn finish(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSampleLevel(second, second_sampler, uv, 0.0);
}
