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
  rotation.xy`, `a_Color`, `a_TexCoordVec4C1 = (velocity, phase)`); Diorama uses
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
  Diorama applies this on the CPU (position and size scale by D / (D − z)).
- Instance overrides (`instanceoverride` on the scene object): `rate` has
  **no effect** on emission (live counts match the plain emitter rate in
  three captures: ash 10/s × 0.5 → 85 alive over 7–10 s lifetimes, raindrops
  25/s × 2 → 97 over 3–5 s, 3509243656's nebula 20/s × 0.5 → 79/83 over
  3–5 s); `count` (max count), `size`, `lifetime`, `speed`, `alpha`, `colorn`
  (× `brightness`), `controlpointN` (scene positions) — multipliers on the
  system; children inherit them.
- Control point `flags & 1` follows the cursor (fireflies flee it through
  `controlpointattract` with a negative scale).

## Rope trails (capture 914, 285)
- WE draws `ropetrail` systems with `genericropeparticle` whatever shader
  the material names, `TRAILRENDERER` UVs, thin vertex format: one segment
  per pair of consecutive trail points (head first, then history, newest
  first), each `(start, size, end, history length, control points, colour)`.
- Both captured systems (lengths 0.4 and 4, no `segments`) hold **4 history
  points**; samples are `length / (segments − 1)` apart on a timer shared by
  the whole system: `g_RenderVar0.z` = phase since the last sample (0.6 in
  914, so the first segment is 0.6 of a full one); `g_RenderVar0.w` = count.
- Width is the (half-rule) particle size: the shooting stars are ~4 px wide,
  ~170 px long.

## Follow children: emitter-local vs world space
- System `flags & 1` = world space. Firefly trails (`flags 1`, capture 289)
  stay where they were emitted (absolute positions, scene MVP).
- Without it particles are emitter-local: 914's shooting-star glows are drawn
  one draw per parent with the parent's position in the MVP and particle
  position (0, 0, 0) — they ride along with the parent and its instance ends
  with it. Their `instantaneous: 1` burst fires once per parent (22 stars →
  22 glows), and each instance holds at most the child's own `maxcount`.
- All 35 captured shooting-star trails run at exactly 45°: the stars are at
  terminal velocity `gravity / drag`, which Diorama's per-second drag does not
  reach within their lifetime — WE's drag is stronger (not calibrated yet).

## Perspective (3D) scenes
In scenes with a perspective camera (3509243656) particles are drawn through
the real camera (`g_ModelViewProjectionMatrix` = system world × view ×
projection) and billboarded along `g_OrientationRight/Up/Forward` = the
camera's right, up and backward axes in the system's local space (captured:
right = (0.883, 0, 0.470) for a system turned 28° about y). Diorama sets these
every frame and skips its 2D perspective emulation there.

## Defaults Diorama assumes where keys are absent
Taken from the editor's defaults where known, otherwise chosen to match the
captures: lifetime 1, size 20, colour white, alpha 1, alphafade 0.5 / 0.5,
sizechange 1 → 0, oscillators frequency 0–10, sphere distance 0–256 and
directions (1, 1, 0), box ±256. Movement only happens with a `movement`
operator; rotation only with `angularmovement`.

## `colorn` instance override (open)
Vertex colours in 21 captured systems with a `colorn` override
(`scratch/colorn.py` compares each system's captured colours with `colorn`
and `colorn²`, both × `brightness`):

- `colorn²` exactly in 16 systems over scene versions 0–4, e.g. 3195058405's
  `Shooting_Star_01`: `colorrandom` 180 144 61 .. 193 93 218 with `colorn`
  1 0 0.553 gives exactly (1, 0, 0.306) for every particle, so the override
  replaces the initializer's random colour rather than multiplying it.
  1569133561's Torch (no colour initializer, a `colorchange` operator) is
  squared as well.
- `colorn` once in 4: Birds in 2915844448 and 3413921910 and 2208593521's
  rain (no colour initializer, only `movement`), and 3509243656's nebula
  (`colorrandom` white, scene version 5, HDR, `brightness` 0.3).
- 2915844448's fireflies match neither: green stays 1, red and blue vary
  around `colorn`.

None of scene version, HDR, blending, material, `alpha` override or system
flags separates the groups. Diorama applies `colorn` once until a rule fits
all of them.

## Not reproduced yet
Rope trails (drawn as sprite trails), sprite-sheet blending, refraction and
lit particles, `turbulentvelocityrandom`'s noise field, vortex details,
exact emission timing (positions are random anyway: goldens compare density,
size and colour, not individual particles).
