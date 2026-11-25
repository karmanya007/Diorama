
// Slot 0 is an image, tinted by the layer's colour.
fn source(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(image, image_sampler, uv) * layer.g_Color4;
}
