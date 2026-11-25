
@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let t = s.g_Time * s.speed;
    let by = vec2(wander(t, 11.0), wander(t, 47.0)) * s.amount;
    return pixel(uv - by / layer_size());
}
