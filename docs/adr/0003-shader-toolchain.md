# ADR-003: Shader toolchain — HLSL via DXC, ingested by naga

Date: 2026-10-14 · Status: accepted

## Context
WE shaders are a GLSL/HLSL common subset that WE compiles **as HLSL** (prelude + source rewrite, see `docs/re-notes/shaders.md`). Shaders that work in WE may rely on HLSL-only behaviour: scalar→vector splat (`vec2 x = 1;`), implicit vector truncation (`float g = texSample2D(...)`), mixed scalar/vector intrinsics (`pow(vec3, float)`, `max(0, vec3)`), float→int conversion.

Spike (`wpdump shader-check`) over every distinct vertex/fragment pair in the WE install and the 55-scene corpus (336 pairs):

| Path | Default combos | All combos = 1 |
|---|---|---|
| A: rewrite to GLSL 450 → naga `glsl-in` → validate | 299 (89.0%) | 274 (81.5%) |
| **B: rewrite to HLSL → DXC 1.9 `-spirv` → naga `spv-in` → validate** | **330 (98.2%)** | **315 (93.8%)** |

Path A failures are dominated by HLSL implicit conversions; fixing them needs a type-aware rewriter (effectively a second HLSL compiler). Path B remaining failures are not compiler limits:
- `PerformLighting_V1` — WE injects a lighting library for `#require LightingV1` (we must supply our own; Phase 4 lighting).
- `BLENDROWCOUNT` — engine define for puppet warp (Phase 4).
- One workshop shader with a float literal in `#if`.
- One matrix-typed varying (needs column splitting in IO structs).

Reports: `docs/re-notes/shader-check-{hlsl,glsl}{,-allcombos}.md`.

## Decision
Use path B in production:
1. `lumen_we::shader` preprocesses (includes, combos, conditionals) and emits HLSL with per-stage cbuffers (`b0` vertex, `b1` fragment, space 0), textures/samplers in space 1 (`t2i`/`s2i+1`), static interface globals, generated entry points with `[[vk::location]]`s, and array varyings flattened.
2. DXC compiles with `-spirv -HV 2018 -fspv-target-env=vulkan1.1 -fvk-use-gl-layout`.
3. naga `spv-in` loads SPIR-V and validates; wgpu consumes the naga module on every backend.
4. Compiled SPIR-V is cached on disk, keyed by hash of (translated source, defines, DXC version), so DXC runs only on first use of a shader permutation.

Keep path A's translator (`shader::glsl`) for now: cheap to maintain, useful as a fallback on platforms where DXC is unavailable, and for cross-checking.

## Consequences
- **Native dependency:** ship `dxcompiler.dll` + `dxil.dll` (Windows, ~32 MB) / `libdxcompiler.so` (Linux, distro packages exist). macOS has no official binary — build from source in CI or fall back to path A + cache. DXC license (LLVM/NCSA + Apache-2.0 parts) must be included in distributions; it is not a Rust crate so cargo-deny does not see it.
- **Integration:** load DXC in-process (COM `DxcCreateInstance` via `hassle-rs` or a thin FFI) rather than spawning `dxc.exe`; decide in Phase 2.
- **Matrix convention:** source uses row-vector `mul(v, M)`; `-fvk-use-gl-layout` + our upload layout must be verified against RenderDoc cbuffer dumps in Phase 2.
- **Perf:** spike compiled 672 stages in ~3.4 s on 16 threads; with the cache this is a one-time cost per permutation.
- Optional later: wgpu SPIR-V passthrough on Vulkan to skip naga for shaders naga mis-handles.
