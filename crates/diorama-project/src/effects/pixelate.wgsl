
@fragment
fn fs(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let cells = layer_size() / s.size;
    return pixel((floor(uv * cells) + 0.5) / cells);
}
