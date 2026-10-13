# WE shader dialect — RE notes

Sources: `assets/shaders/**` (502 files, plaintext), string table of `wallpaper64.exe` (v2.8.42).

## What the dialect is
Shaders are written in a **common subset of GLSL and HLSL**. WE compiles them as **HLSL** (d3dcompiler_47 / dxcompiler) after:
1. Prepending a prelude that maps GLSL names to HLSL (found verbatim in the binary):
   ```c
   #define HLSL 1
   #define HLSL_SM40 1
   #define vec2 float2   // vec3, vec4, uvec4, mat2/3/4, mat4x3 likewise
   #define mix lerp
   #define mod(x, y) ((x)-(y)*floor((x)/(y)))
   #define CAST2(x) ((float2)(x))        // CASTI/CASTU/CASTF/CAST3/CAST4/CAST4U/CAST3X3
   #define texSample2D(s, u) s.Sample(s ## SamplerState, u)
   #define texSample2DLod(s, u, m) s.SampleLevel(s ## SamplerState, u, m)
   #define texSample2DCompare(s, u, d) s.SampleCmpLevelZero(s ## SamplerComparisonState, u, d)
   #define texLoad2D(s, u, r) s.Load(int3((u) * (r), 0))
   #define texSample3D(s, u) s.Sample(s ## SamplerState, u)
   #define DECLARE_SAMPLER2D_PARAMETER(t) Texture2D t, SamplerState t ## SamplerState
   #define MAKE_SAMPLER2D_ARGUMENT(t) t, t ## SamplerState
   ```
2. A source rewrite (not yet located) turning `attribute` / `varying` / `uniform` / `gl_Position` / `gl_FragColor` / `void main()` into HLSL structs, cbuffers and entry points.

Consequence: any shader that runs in WE is **valid HLSL** after prelude + rewrite, but may use HLSL-only permissiveness (scalar→vector splat, implicit vector truncation) that strict GLSL rejects.

## Constructs (count of files using each, built-in assets)
| construct | files | | construct | files |
|---|---|---|---|---|
| `#include` | 174 | | `mul(` | 200 |
| `// [COMBO] {json}` | 169 | | `CAST*(` | 132 |
| `attribute` | 240 | | `saturate(` | 68 |
| `varying` | 472 | | `frac(` | 51 |
| `gl_FragColor` | 239 | | `pow(` | 56 |
| `gl_Position` | 245 | | `inverse(` | 22 |
| `uniform sampler2D` | 239 | | `atan2(` | 13 |
| `uniform mat3/4` | 193 | | `ddx/ddy(` | 6 |
| `texSample2D(` | 228 | | `discard` | 11 |
| `texSample2DLod(` | 18 | | `#if…HLSL` | 38 |
| `#require` | 8 | | geometry shaders | 4 |

Not used: `texture(`, `texelFetch`, `tex2D`, `fmod`, `transpose`, `gl_FragCoord`, `while`.

## Conventions
- **Row-vector math**: `gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);` → HLSL `mul(v, M)`. Our GLSL path can map `mul(a, b)` → `((a) * (b))` provided matrices are uploaded with the same memory layout WE uses (verify with RenderDoc cbuffer dump).
- `#ifdef HLSL_SM30` blocks add half-texel offsets (D3D9 legacy); SM4+ path skips them. We define neither SM30 nor take those branches.
- `#require LightingV1` appears in lit generic shaders (`genericimage4`, `genericparticle`, …) — likely toggles the lighting uniform layout. Semantics TBD.
- Uniform annotations: trailing `// {"material":"key","label":…,"default":…,"range":[…]}` bind material/scene values to uniforms; `{"hidden":true}` for engine-fed samplers; `"combo":"MASK"` on samplers enables a combo when a texture is bound.
- Combo declarations: `// [COMBO] {"material":…,"combo":"NAME","type":"options","default":0}`.

## WE's own rewrite (from binary strings)
`struct VS_INPUT` / `VS_OUTPUT main(VS_INPUT IN)` / `PS_OUTPUT main(VS_OUTPUT IN)` with `float4 gl_FragColor : SV_TARGET`; attributes and varyings become `IN.x` / `OUT.x` member accesses; `gl_Position` → `SV_POSITION`; `gl_ViewportIndex` → `SV_ViewportArrayIndex`; geometry-shader variants `void main(triangle VS_OUTPUT IN[3], inout TriangleStream<PS_INPUT> OUT)` and `point VS_OUTPUT IN[1]`. Samplers: `SamplerState …:register(s…)`.

Engine-provided pieces our translator must supply itself:
- `#require LightingV1` → lighting library with `PerformLighting_V1(...)`.
- `BLENDROWCOUNT` define (puppet warp blend map rows).
- `in uint gl_InstanceID;` builtin input (instanced fur / shadow casters).

## Shader spike result → ADR-003
Decided: **HLSL → DXC → SPIR-V → naga** (98.2% of 336 distinct pairs vs 89.0% for GLSL → naga). See `docs/adr/0003-shader-toolchain.md` and the `shader-check-*.md` reports in this folder.
