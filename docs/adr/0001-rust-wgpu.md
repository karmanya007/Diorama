# ADR-001: Rust + wgpu

Date: 2026-10-14 · Status: accepted

## Context
Need one renderer for Windows (first), Linux, macOS. Original Wallpaper Engine is D3D11-only. Workshop content is untrusted input.

## Decision
Rust (edition 2024) with wgpu (DX12/Vulkan on Windows, Vulkan/GL on Linux, Metal on macOS). Own thin platform layer per OS (`windows` crate, smithay-client-toolkit, x11rb, objc2); no winit, no Bevy.

## Consequences
- Memory safety for parsers of untrusted files.
- Shader translation needed from WE's GLSL dialect (see ADR-003).
- winit/Bevy rejected: neither supports WorkerW parenting or wlr-layer-shell, and Bevy's idle footprint is too large for a background app.
