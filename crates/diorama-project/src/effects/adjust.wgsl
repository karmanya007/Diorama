
// Turns a colour around the grey axis.
fn turn(c: vec3<f32>, degrees: f32) -> vec3<f32> {
    let axis = vec3(0.57735026919);
    let a = radians(degrees);
    return c * cos(a) + cross(axis, c) * sin(a) + axis * dot(axis, c) * (1.0 - cos(a));
}

@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let c = pixel(uv);
    var rgb = straight(c) * (1.0 + s.brightness);
    rgb = (rgb - 0.5) * (1.0 + s.contrast) + 0.5;
    rgb = mix(vec3(luma(rgb)), rgb, 1.0 + s.saturation);
    rgb = turn(rgb, s.hue);
    return vec4(clamp(rgb, vec3(0.0), vec3(1.0)) * c.a, c.a);
}
