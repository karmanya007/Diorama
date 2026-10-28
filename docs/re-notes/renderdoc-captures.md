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
- **All render targets are `R8G8B8A8_UNORM`** — no sRGB, no HDR formats, even with bloom. Blending and effect math happen in gamma space; Diorama must use UNORM (non-sRGB) targets and textures to match.
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

## Implications for Diorama
1. Renderer targets and textures: `Rgba8Unorm` / `Bgra8Unorm`, never `*Srgb`, for WE content.
2. Upload matrices so `mul(v, M)` reproduces the values above; golden tests can feed captured matrices directly.
3. Per-layer RTs sized to the layer + effect ping-pong is the core render graph shape (Phase 2).
4. Span layout = one camera over the bounding box of all outputs, cover-fit. Diorama renders per output (P6/P7) but can reproduce span by giving each output a viewport into the same projection.
5. Particles use geometry shaders in WE; Diorama uses the shaders' non-GS path (instanced quads), so particle golden comparisons must allow for that.

## Verified by reimplementation (Phase 2)
Diorama's renderer reproduces frame 289 (scene 1132505365: image layer + waterripple + shake + godrays) at SSIM 0.996 / PSNR 40.9 dB using exactly the structure above:
- albedo → target A (no blending), ripple A→B, shake B→A, godrays: downsample A→½ FBO1, cast →FBO2, blur x →FBO1, blur y →FBO2, combine (FBO2 + A) → B, composite B with the material's `translucent` blending;
- effect quads: clip-space positions, MVP `diag(1, 1, 0.0005)` + row 3 `(0, 0, 0.5, 1)`;
- `g_TextureNResolution` = `(allocated w, h, image w, h)` for textures, `(w, h, w, h)` for render targets.

## Depth, sampling and sprite sizes (Phase 6 captures, 2026-10-27)
From captures of the default projects Neon Sunset, Audiophile and Dino Run (D3D11 pipeline state read per draw with RenderDoc):
- **Depth writes follow the blend, not the material.** Perspective scenes use reversed Z (`GREATER`, cleared to 0). Opaque model draws test and write depth; every blended draw (`translucent`, `additive`) tests but does not write, even when its material says `"depthwrite": "enabled"`. Neon Sunset's translucent sun quad would otherwise hide the mountains behind its transparent corners.
- **`nointerpolation` layers stay on point samplers to the end.** Dino Run's image layers render into scene-unit targets (272×160, 544×160, 32×32) and every draw touching them — albedo, effect passes and the draw onto the 3000×1920 back buffer — uses point filtering; only the scene's post-processing after them is linear.
- **The last effect pass draws straight into the scene.** Each Dino Run layer with one effect is two draws: albedo into its target, then the effect pass (e.g. `scroll` with `repeat 14 1`) with the layer's placement and blend onto the back buffer — there is no separate composite copy, so a pass that tiles a 32×32 target keeps full detail. Diorama does this for pixel-art layers; other layers keep the composite copy (same image when the effect does not tile).
- **Sprite-sheet layers are one frame big.** A layer sized by its texture (`autosize` models such as Dino Run's coin, 16×16 frames on a 256×64 sheet) takes the first `TEXS` frame's size, not the sheet's.
- Neon Sunset's terrain is an fbm of `frac(sin(x)·43758.5)` hashes in the vertex shader; FXC's and DXC's `sin` differ in the last bits, so the peaks land elsewhere (same mean height). Not reproducible short of emulating the driver's `sin`.
