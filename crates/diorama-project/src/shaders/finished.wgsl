
// Slot 0 is a layer after its effects, premultiplied by alpha.
fn source(uv: vec2<f32>) -> vec4<f32> {
    let c = textureSample(image, image_sampler, uv);
    return vec4(c.rgb / max(c.a, 1.0 / 1024.0), c.a * layer.opacity);
}
