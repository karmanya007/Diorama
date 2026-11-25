
@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let m = textureSample(second, second_sampler, inner(uv));
    let bright = luma(m.rgb) * m.a;
    let k = select(bright, 1.0 - bright, s.invert > 0.5);
    return pixel(uv) * mix(1.0, k, s.amount);
}
