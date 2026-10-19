# Particles

Sources: corpus survey (294 particle objects, 143 distinct system files),
WE's `genericparticle` / `common_particles.h` shaders, and RenderDoc captures
289 (1132505365), 609 (2583499659), 289b (2468489223), 275 (2518958708),
1243 (2176097362), extracted with `tools/refcap/particledraws.py`.

## How WE draws them
- One draw per system, a **point list** expanded to quads by a geometry
  shader (`genericparticle.geom`). The vertex shader passes the particle
  through, so the captured VS output *is* the particle: position xyz, size,
  colour rgba, rotation xyz, then (THICKFORMAT) velocity xyz and the
  sprite-sheet phase.
- The shaders also have a geometry-shader-free path (CPU writes the quad
  corners: `a_TexCoordVec4 = (u, v, rotation.z, size)`, `a_TexCoordC2 =
  rotation.xy`, `a_Color`, `a_TexCoordVec4C1 = (velocity, phase)`); Lumen uses
  it with six vertices per particle (two triangles).
- Quad = `position ± size · right · (u − ½) ∓ size · up · (v − ½) · ratio`;
  orientation uniforms are the screen axes in a 2D scene.
- Combos the engine sets: `THICKFORMAT` (trails, sprite sheets),
  `TRAILRENDERER` (sprite trails), `SPRITESHEET` (texture with a `TEXS`
  trailer; `g_RenderVar1` = frame width, frame height in UV, frame count,
  frame aspect). For every bound one- or two-channel texture the engine sets
  `TEX<n>FORMAT` (RG88 = 8, R8 = 9) so `ConvertTexture0Format` reads it as
  luminance + alpha (applies to all materials, not just particles).

## Measured semantics
- **Size is half the editor's `sizerandom`**: fireflies 70–90 → 33–43 px
  quads, ember glow 500–1000 → 285–433, rain 5 × override 0.5 → 1.25.
- **Follow children run one instance per parent particle**: the firefly trail
  child (maxcount 10) is drawn once per firefly (7 draws of 5–9 points).
- **Audio-driven emitters** (`audioprocessingmode` ≠ 0) emit nothing in
  silence (1243: two note systems never drawn).
- **`flags & 4` = perspective**: positions keep z and are drawn through a
  perspective projection whose z = 0 plane equals the 2D view; the eye sits
  0.9163 × half the scene height away (P11 = 0.9163 in every captured case).
  Lumen applies this on the CPU (position and size scale by D / (D − z)).
- Instance overrides (`instanceoverride` on the scene object): `rate` has
  **no effect** on emission (live counts match the plain emitter rate in
  three captures: ash 10/s × 0.5 → 85 alive over 7–10 s lifetimes, raindrops
  25/s × 2 → 97 over 3–5 s, 3509243656's nebula 20/s × 0.5 → 79/83 over
  3–5 s); `count` (max count), `size`, `lifetime`, `speed`, `alpha`, `colorn`
  (× `brightness`), `controlpointN` (scene positions) — multipliers on the
  system; children inherit them.
- Control point `flags & 1` follows the cursor (fireflies flee it through
  `controlpointattract` with a negative scale).

## Perspective (3D) scenes
In scenes with a perspective camera (3509243656) particles are drawn through
the real camera (`g_ModelViewProjectionMatrix` = system world × view ×
projection) and billboarded along `g_OrientationRight/Up/Forward` = the
camera's right, up and backward axes in the system's local space (captured:
right = (0.883, 0, 0.470) for a system turned 28° about y). Lumen sets these
every frame and skips its 2D perspective emulation there.

## Defaults Lumen assumes where keys are absent
Taken from the editor's defaults where known, otherwise chosen to match the
captures: lifetime 1, size 20, colour white, alpha 1, alphafade 0.5 / 0.5,
sizechange 1 → 0, oscillators frequency 0–10, sphere distance 0–256 and
directions (1, 1, 0), box ±256. Movement only happens with a `movement`
operator; rotation only with `angularmovement`.

## Not reproduced yet
Rope trails (drawn as sprite trails), sprite-sheet blending, refraction and
lit particles, `turbulentvelocityrandom`'s noise field, vortex details,
exact emission timing (positions are random anyway: goldens compare density,
size and colour, not individual particles).
