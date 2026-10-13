# Shader check (HLSL → DXC SPIR-V → naga)

Variant: default combos

**330/336 distinct pairs pass (98.2%)**

## `#require` directives

- `LightingV1`: 6

## Failures by cause

| count | cause | example |
|---:|---|---|
| 4 | dxc fragment: use of undeclared identifier '…' | `builtin:shaders\chroma4.vert` |
| 1 | dxc vertex: use of undeclared identifier '…' | `builtin:shaders\puppettexturechannels.vert` |
| 1 | prepare: shader: unexpected '…' in #if | `3088099655:shaders/workshop/2811235087/effects/lens_distorsion.vert` |
