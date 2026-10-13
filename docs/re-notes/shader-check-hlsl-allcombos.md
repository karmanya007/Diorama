# Shader check (HLSL → DXC SPIR-V → naga)

Variant: all combos = 1

**315/336 distinct pairs pass (93.8%)**

## `#require` directives

- `LightingV1`: 8

## Failures by cause

| count | cause | example |
|---:|---|---|
| 15 | dxc fragment: use of undeclared identifier '…' | `builtin:effects\fluidsimulation\shaders\effects\fluidsimulation_combine.vert` |
| 2 | dxc vertex: use of undeclared identifier '…' | `builtin:shaders\puppettexturechannels.vert` |
| 2 | validate vertex: Function { handle: [#], name: "…", source: Expression { handle: [#], source: Type(OutOfBoundsIndex { expr: [#] | `builtin:shaders\shadowcaster.vert` |
| 1 | prepare: shader: unexpected '…' in #if | `3088099655:shaders/workshop/2811235087/effects/lens_distorsion.vert` |
| 1 | validate vertex: EntryPoint { stage: Vertex, name: "…", source: Result(NotIOShareableType([#])) } | `builtin:effects\cursorripple\preview\shaders\effects\cursorripple_apply_force.vert` |
