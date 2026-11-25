
@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let size = layer_size();
    let at = inner(uv);
    let anchor = i32(s.anchor);
    // Held at the left or right it bends up and down, else from side to side.
    let sideways = anchor == 2 || anchor == 3;
    var free = 1.0;
    switch anchor {
        case 0: { free = 1.0 - at.y; }
        case 1: { free = at.y; }
        case 2: { free = at.x; }
        case 3: { free = 1.0 - at.x; }
        default: {}
    }
    let along = select(uv.y * size.y, uv.x * size.x, sideways);
    let by = sin(TAU * (s.g_Time * s.speed + along / s.length)) * s.amount * clamp(free, 0.0, 1.0);
    let offset = select(vec2(by, 0.0), vec2(0.0, by), sideways);
    return pixel(uv - offset / size);
}
