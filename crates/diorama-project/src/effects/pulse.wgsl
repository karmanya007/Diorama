
@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let k = (0.5 - 0.5 * cos(TAU * s.g_Time * s.speed)) * s.amount;
    switch i32(s.what) {
        case 1: { return pixel(uv) * (1.0 - k); }
        case 2: { return pixel((uv - 0.5) / (1.0 + k) + 0.5); }
        default: {
            let c = pixel(uv);
            return vec4(min(c.rgb * (1.0 + k), vec3(c.a)), c.a);
        }
    }
}
