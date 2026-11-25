
// Only what is bright enough shines.
fn taken(c: vec4<f32>) -> vec4<f32> {
    let k = (luma(straight(c)) - s.threshold) / (1.0 - s.threshold);
    return c * clamp(k, 0.0, 1.0);
}

@fragment
fn finish(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let c = pixel(uv);
    let blurred = textureSampleLevel(second, second_sampler, uv, 0.0).rgb;
    let light = mix(taken(c).rgb, blurred, smoothness());
    let shine = min(light * s.color.rgb * s.color.a * s.strength, vec3(1.0));
    // As a screen: what is bright already keeps its drawing.
    let rgb = c.rgb + shine * (1.0 - c.rgb);
    // Light needs alpha of its own to show beside the layer.
    return vec4(rgb, max(c.a, max(rgb.r, max(rgb.g, rgb.b))));
}
