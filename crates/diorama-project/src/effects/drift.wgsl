
@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return pixel(fract(uv - heading(s.direction) * s.speed * s.g_Time / layer_size()));
}
