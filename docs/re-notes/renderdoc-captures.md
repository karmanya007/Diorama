# RenderDoc reference captures — RE notes

Captured 2026-10-14 with RenderDoc 1.46 on WE 2.8.42 (D3D11), via `tools/refcap/capture.py` + `analyze.py`. Captures (`.rdc`), per-draw JSON and final PNGs live in the git-ignored `scratch/captures/` (they contain Workshop content). Machine: 2 monitors on one GPU, 1920×1080 + 1080×1920 portrait, WE in span layout.

| Capture | Scene | Draws | Features exercised |
|---|---|---|---|
| `we_frame289` | 1132505365 | 19 | image layer + waterripple + shake + godrays, 9 particle systems |
| `we_frame285` | 2134765860 | 63 | bloom, scripts, audio bars (`g_AudioSpectrum*`), many small layers |
| `we_frame597` | 3414847823 | 119 | 21 text layers, 90 scripts, bloom |
| `we_frame914` | 3195058405 | 118 | camera parallax (`g_ParallaxPosition`), particles, text, audio |

## Frame structure (verified on `we_frame289`)
```
clear swapchain + depth
for each image layer:
    albedo:   layer texture  → RT_a (layer size, e.g. 3840×2160)     genericimage: g_Brightness, g_UserAlpha
    effects:  RT_a → RT_b → RT_a …  (ping-pong, one draw per pass)   waterripple, shake, …
              multi-pass effects use their own smaller RTs            godrays: ½-res threshold → cast → blur X → blur Y → combine
    composite: final layer RT → swapchain, alpha blend (SrcAlpha, InvSrcAlpha), g_ModelViewProjectionMatrix
particles: drawn straight into the swapchain, additive (SrcAlpha, One), point lists (5–40 verts) → geometry-shader expansion
present
```
- **One swapchain spans the whole virtual desktop** (3000×1920 here), not one per monitor.
- Every layer renders into its **own RT sized to the layer** (`3840×2160`, `287×90` text, `3840×1131` strips…).
- **All render targets are `R8G8B8A8_UNORM`** — no sRGB, no HDR formats, even with bloom. Blending and effect math happen in gamma space; Lumen must use UNORM (non-sRGB) targets and textures to match.
- Bloom (when enabled): scene goes to a full-size RT (`3000×1920`), then ¼ (`750×480`) and ⅛ (`375×240`) downsample/blur chain, combined back.

## Matrix and projection conventions
Layer pass (`eid 40`), `g_ModelViewProjectionMatrix` as stored:
```
[ 2/3840   0        0       0 ]
[ 0        2/2160   0       0 ]
[ 0        0        0.0005  0 ]
[ -1      -1        0.5     1 ]
```
- Translation in the **4th row** → row-vector convention, `mul(v, M)` (matches shader source).
- Orthographic, z mapped to D3D 0..1 depth.

Composite pass (`eid 192`): x scale `0.000593 = 2/3375` with `3375 = 2160 × 3000/1920` → WE **cover-fits** the 3840×2160 scene to the desktop aspect, keeping full height and cropping width, centred.

Uninitialised cbuffer members are common (e.g. `g_ViewProjectionMatrix` in the composite PS, `g_ModelMatrix` all zeros) — WE only fills what the shader reads; golden tests must not depend on unused members.

## Uniform buffers
Engine cbuffers are named `g_bufDynamic` (per-draw: matrices, `g_Time`, texture rotation/translation, eye position) and `g_bufStatic` (material constants). Observed engine-fed uniforms: `g_ModelViewProjectionMatrix`, `g_ViewProjectionMatrix`, `g_ModelMatrix`, `g_EyePosition`, `g_Time`, `g_Texture0Rotation`, `g_Texture0Translation`, `g_Color4`, `g_Alpha`, `g_Brightness`, `g_UserAlpha`, `g_Bloom{Strength,Threshold,Tint}`, `g_AudioSpectrum{16,32}{Left,Right}`, `g_ParallaxPosition`.

## Implications for Lumen
1. Renderer targets and textures: `Rgba8Unorm` / `Bgra8Unorm`, never `*Srgb`, for WE content.
2. Upload matrices so `mul(v, M)` reproduces the values above; golden tests can feed captured matrices directly.
3. Per-layer RTs sized to the layer + effect ping-pong is the core render graph shape (Phase 2).
4. Span layout = one camera over the bounding box of all outputs, cover-fit. Lumen renders per output (P6/P7) but can reproduce span by giving each output a viewport into the same projection.
5. Particles use geometry shaders in WE; Lumen uses the shaders' non-GS path (instanced quads), so particle golden comparisons must allow for that.
