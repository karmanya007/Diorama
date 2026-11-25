
@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let c = pixel(uv);
    // 0 in the middle, 1 in the corners.
    let r = length((inner(uv) - 0.5) * 2.0) / sqrt(2.0);
    let k = smoothstep(s.size, s.size + s.softness, r) * s.amount * s.color.a;
    return vec4(mix(c.rgb, s.color.rgb * c.a, k), c.a);
}
