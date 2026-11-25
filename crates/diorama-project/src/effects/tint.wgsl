
@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let c = pixel(uv);
    return vec4(mix(c.rgb, luma(c.rgb) * s.color.rgb, s.amount * s.color.a), c.a);
}
