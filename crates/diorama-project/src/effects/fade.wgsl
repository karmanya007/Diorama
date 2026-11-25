
@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let along = dot(inner(uv) - 0.5, heading(s.direction)) + 0.5;
    let gone = clamp((along - s.start) / max(s.end - s.start, 1e-4), 0.0, 1.0);
    return pixel(uv) * (1.0 - gone * gone * (3.0 - 2.0 * gone));
}
