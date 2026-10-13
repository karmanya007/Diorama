# Shader check (GLSL 450 → naga)

Variant: all combos = 1

**274/336 distinct pairs pass (81.5%)**

## `#require` directives

- `LightingV1`: 8

## Failures by cause

| count | cause | example |
|---:|---|---|
| 16 | parse vertex: Error { kind: SemanticError("…'CreateAudioResponse'"), meta: Span { start: #, end: # } } | `builtin:effects\pulse\shaders\effects\pulse.vert` |
| 6 | parse fragment: Error { kind: UnknownVariable("…"), meta: Span { start: #, end: # } } | `builtin:zcompat\scene\shaders\2084198056\Simple_Audio_Bars.vert` |
| 6 | validate fragment: Function { handle: [#], name: "…", source: InvalidStoreTypes { pointer: [#], value: [#] } } | `builtin:effects\glitter\preview\shaders\effects\glitter_combine.vert` |
| 5 | parse fragment: Error { kind: SemanticError("…'pow'"), meta: Span { start: #, end: # } } | `builtin:effects\filmgrain\preview\shaders\effects\filmgrain.vert` |
| 4 | parse fragment: Error { kind: SemanticError("…'PerformLighting_V#'"), meta: Span { start: #, end: # } } | `builtin:effects\fluidsimulation\shaders\effects\fluidsimulation_combine.vert` |
| 3 | parse fragment: Error { kind: InvalidToken(Identifier("…"), [Token(RightParen)]), meta: Span { start: #, end: # } } | `builtin:shaders\chroma4.vert` |
| 2 | parse fragment: Error { kind: PreprocessorError(TooFewDefineArguments), meta: Span { start: #, end: # } }Error { kind: Invalid | `builtin:shaders\generic3.vert` |
| 2 | parse fragment: Error { kind: SemanticError("…'max'"), meta: Span { start: #, end: # } } | `builtin:effects\nitro\shaders\effects\nitro.vert` |
| 2 | parse fragment: Error { kind: SemanticError("…'mix'"), meta: Span { start: #, end: # } } | `2216027178:shaders/workshop/2114826643/effects/shift_hue.vert` |
| 2 | parse fragment: Error { kind: SemanticError("…'rotateVec#'"), meta: Span { start: #, end: # } } | `builtin:effects\shimmer\shaders\effects\shimmer.vert` |
| 2 | parse vertex: Error { kind: InvalidToken(Identifier("…"), [Token(RightParen)]), meta: Span { start: #, end: # } } | `builtin:shaders\shadowcasterfoliage4.vert` |
| 2 | validate vertex: Function { handle: [#], name: "…", source: InvalidStoreTypes { pointer: [#], value: [#] } } | `builtin:effects\cloudmotion\preview\shaders\effects\cloudmotion.vert` |
| 1 | parse fragment: Error { kind: SemanticError("…'t resolve type: InvalidAccess { expr: [#], indexed: true }"… | `3195058405:shaders/workshop/3184560948/effects/color_grading.vert` |
| 1 | parse vertex: Error { kind: SemanticError("…'pow'"), meta: Span { start: #, end: # } } | `builtin:effects\foliagesway\preview\shaders\effects\foliagesway.vert` |
| 1 | parse vertex: Error { kind: UnknownVariable("…"), meta: Span { start: #, end: # } } | `builtin:shaders\puppettexturechannels.vert` |
| 1 | prepare: shader: unexpected '…' in #if | `3088099655:shaders/workshop/2811235087/effects/lens_distorsion.vert` |
| 1 | validate fragment: Function { handle: [#], name: "…", source: Expression { handle: [#], source: InvalidBinaryOperandTypes { op: M | `3000562427:shaders/workshop/2078835426/effects/pixelate.vert` |
| 1 | validate fragment: Function { handle: [#], name: "…", source: Expression { handle: [#], source: InvalidBinaryOperandTypes { op: S | `2979524338:shaders/workshop/2138904733/effects/cutout_vignette.vert` |
| 1 | validate fragment: Function { handle: [#], name: "…", source: InvalidIfType([#]) } | `3415190399:shaders/workshop/3586981034/effects/frame_builder.vert` |
| 1 | validate fragment: Function { handle: [#], name: "…", source: LocalVariable { handle: [#], name: "…", source: InitializerType } } | `builtin:shaders\editorpaintbrush.vert` |
| 1 | validate vertex: EntryPoint { stage: Vertex, name: "…", source: Result(NotIOShareableType([#])) } | `builtin:effects\cursorripple\preview\shaders\effects\cursorripple_apply_force.vert` |
| 1 | validate vertex: Function { handle: [#], name: "…", source: Expression { handle: [#], source: InvalidBinaryOperandTypes { op: M | `builtin:shaders\shadowcaster.vert` |
