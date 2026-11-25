
// Slot 0 is a glyph atlas: how much of a pixel a glyph covers, in red.
fn source(uv: vec2<f32>) -> vec4<f32> {
    let coverage = textureSample(image, image_sampler, uv).r;
    return vec4(layer.g_Color4.rgb, layer.g_Color4.a * coverage);
}
