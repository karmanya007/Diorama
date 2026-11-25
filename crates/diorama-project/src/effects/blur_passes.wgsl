
// A Gaussian blur of what `taken` keeps of each pixel, in a smaller copy of
// the layer. `u_Divisor`: how much smaller; `u_Sigma`: the blur there, in
// its own pixels.

// A smaller copy is too coarse for a blur that has moved to (almost) nothing:
// then the layer itself shows.
fn smoothness() -> f32 {
    return select(1.0, clamp(s.u_Sigma / 0.8, 0.0, 1.0), s.u_Divisor > 1.5);
}

fn gauss(uv: vec2<f32>, axis: vec2<f32>) -> vec4<f32> {
    let texel = axis / vec2<f32>(textureDimensions(image));
    var sum = pixel(uv);
    var weights = 1.0;
    for (var i = 1; i <= 10; i++) {
        let d = f32(i);
        let w = exp(-0.5 * d * d / max(s.u_Sigma * s.u_Sigma, 1e-4));
        sum += w * (pixel(uv + d * texel) + pixel(uv - d * texel));
        weights += 2.0 * w;
    }
    return sum / weights;
}

@fragment
fn shrink(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    // Each sample covers two pixels a side, so half as many as it is smaller.
    let n = i32(max(1.0, s.u_Divisor / 2.0));
    let texel = 2.0 / vec2<f32>(textureDimensions(image));
    var sum = vec4(0.0);
    for (var y = 0; y < n; y++) {
        for (var x = 0; x < n; x++) {
            let at = vec2(f32(x), f32(y)) - f32(n - 1) / 2.0;
            sum += taken(pixel(uv + at * texel));
        }
    }
    return sum / f32(n * n);
}

@fragment
fn across(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return gauss(uv, vec2(1.0, 0.0));
}

@fragment
fn down(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return gauss(uv, vec2(0.0, 1.0));
}
