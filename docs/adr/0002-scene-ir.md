# ADR-002: Engine-neutral Scene IR

Date: 2026-10-14 · Status: accepted

## Context
Renderer could consume Wallpaper Engine JSON directly. That couples rendering, the future editor, and our own format to WE's quirks.

## Decision
`diorama-scene` defines an engine-neutral IR (layers, materials, passes, effect graph, particles, properties, script refs). `diorama-we` is an importer (anti-corruption layer) converting WE content into the IR. Renderer, runtime, and editor depend only on the IR. Our `.lwp` format is the serialized IR.

## Consequences
- Extra translation step, cached after first import.
- Other importers (Lively, Shadertoy) plug in the same way.
- CI checks the dependency rule: `diorama-render`/`diorama-runtime` must not depend on `diorama-we`.
