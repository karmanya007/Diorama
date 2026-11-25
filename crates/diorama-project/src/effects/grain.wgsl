
@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let c = pixel(uv);
    let cell = floor(uv * layer_size() / s.size);
    let frame = floor(s.g_Time * s.speed);
    let n = hash(cell + frame * vec2(37.0, 113.0)) * 2.0 - 1.0;
    let rgb = c.rgb + n * s.amount * c.a;
    return vec4(clamp(rgb, vec3(0.0), vec3(c.a)), c.a);
}
