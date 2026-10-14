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

## Phase 2 follow-up (2026-10-15)
- **Integration decided: external `dxc.exe` process for now.** `lumen_gpu::shader::ShaderCompiler` writes the HLSL to a temp file, runs DXC with the flags above and loads the SPIR-V. Cold compiles cost ≈50 ms per program (0.5–0.6 s extra for a 8–12-program scene, measured on scenes 1132505365 and 3414847823); warm starts hit the cache and never run DXC. In-process COM via `hassle-rs` would remove the temp files but adds an `unsafe` FFI surface for little gain. Revisit if first-run latency matters (programs can also be compiled in parallel). Packaging (Phase 6) places `dxc.exe` + `dxcompiler.dll` beside `lumend.exe`; `ShaderCompiler::locate` already looks there, then `LUMEN_DXC`, then `PATH`. Development and CI use `scripts/fetch-dxc.ps1` (pinned v1.9.2607, SHA-256 checked).
- **Cache key** = FNV-1a over (`dxc --version`, flags, profile, entry, source); entries live in `%LOCALAPPDATA%\Lumen\cache\shaders`.
- **Matrix convention verified** (`crates/lumen-gpu/tests/matrix_convention.rs`): after HLSL → DXC → naga, `mul(v, M)` reads naga column *j* as HLSL column *j*, so WE's row-major matrices (translation in row 3) upload transposed. naga `spv-in` must run with `adjust_coordinate_space: false`; DXC keeps HLSL's y-up clip space.
- **HLSL globals:** every non-`static` global (even `const`) is a uniform in HLSL and lands in DXC's `$Globals` at `b0`; the translator now emits WE top-level variables as `static`, and the renderer rejects programs with any other uniform buffer.
- **`#require LightingV1`** is spliced in by the importer as a generated `PerformLighting_V1` with one block per scene point light (WE generates it per scene too).
