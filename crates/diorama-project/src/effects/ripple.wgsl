
@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let size = layer_size();
    let at = uv * size / s.length;
    let t = s.g_Time * s.speed;
    // Two waves each way, so it does not look woven.
    let by = vec2(
        sin(TAU * (at.y + t)) + 0.5 * sin(TAU * (at.y * 1.7 + at.x * 0.6 - t * 1.3)),
        cos(TAU * (at.x + t * 0.9)) + 0.5 * cos(TAU * (at.x * 1.9 - at.y * 0.4 + t * 1.1)),
    ) / 1.5;
    return pixel(uv + by * s.amount / size);
}
