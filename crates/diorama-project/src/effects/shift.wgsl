
@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let by = heading(s.direction) * s.amount / layer_size();
    let red = pixel(uv - by);
    let green = pixel(uv);
    let blue = pixel(uv + by);
    return vec4(red.r, green.g, blue.b, max(green.a, max(red.a, blue.a)));
}
