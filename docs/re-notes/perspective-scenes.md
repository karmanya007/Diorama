# Perspective (3D) scenes

Source: RenderDoc capture of corpus scene 3509243656 (`we_frame278`, the only
perspective scene in the corpus: 8 models, a sky sphere, text and image
layers placed in 3D), exported targets along the frame (`savetex.py`) and
particle draws (`particledraws.py`).

## Detection and camera
- A scene is perspective when `general.orthogonalprojection` is null/absent.
- `general.fov` (vertical, degrees), `nearz`, `farz`; the `camera` block
  (`eye`, `center`, `up`) is a fallback: a **camera object**
  (`"camera": "default"`, own `fov`, `origin`, `angles`) wins. In the capture
  `g_EyePosition` = the camera object's origin (0, 0, 6), not the `camera`
  block's eye.
- View = inverse of the camera object's world matrix (it looks down its −z).
- Projection: vertical fov, aspect of the swapchain (3000 × 1920 → 1.5625),
  reversed finite depth for D3D's 0..1 range: `z_clip = n/(f−n) · z_view +
  f·n/(f−n)`, `w = −z_view` (near → 1, far → 0). Captured
  `g_ViewProjectionMatrix` rows: (1.372484, 0, 0, 0), (0, 2.144507, 0, 0),
  (0, 0, 1e-6, −1), (0, 0, 0.009994, 6) — reproduced exactly by
  `diorama_scene::Camera` (unit test).

## Object transforms
- Row-vector matrices: `world = S · Rx · Ry · Rz · T · parent` (column form
  `T·Rz·Ry·Rx·S`). The x-then-y order is pinned by a star model turned about
  x and y by scripts; z last agrees with the 2D scenes.
- Image and text layers keep their pixel-sized quads; their world matrix
  (e.g. scale 0.003) brings them into world units:
  `g_ModelViewProjectionMatrix = local · world · view · projection`.
- Fullscreen layers (post-processing, bloom) stay in screen space.

## Models
- WE's `generic4` material; `g_ModelMatrix`, `g_NormalModelMatrix`
  (inverse transpose), `g_ViewProjectionMatrix`, `g_EyePosition` = camera.
- Opaque models draw first (all model draws precede the image layers listed
  above them in `scene.json`), with depth test/write per material
  (`depthtest`, `depthwrite`) and `cullmode` (`normal` = back faces,
  `nocull`). Fronts are counter-clockwise (see `docs/formats/mdl.md`).
- Packed PBR masks (`g_Texture2`, `components`) switch on every component
  combo; the stars glow only through the emissive alpha.
- A material texture `_rt_imageLayerComposite_<id>_a` of a layer that is not
  drawn (hidden) samples as empty: the red equator ring (`Hollow Cylinder`,
  translucent, red tint) is invisible in the capture.
- WE renders into a 2× MSAA RGBA16F target; Diorama does not multisample yet.

## Reflections (`_rt_Reflection`)
Default projects Audiophile and Fantastic Car (no Workshop scene in the corpus).
- Objects with `"reflected": true` are drawn a second time into
  `_rt_Reflection`, mirrored about the y = 0 plane; floors (their own `grid`
  shaders, or `generic4` with the `REFLECTION` combo, `g_Texture3`) sample it
  at `clip.xy / clip.w * 0.5 + 0.5` plus a bump or normal offset (WE's
  plaintext shaders: `model_fragment_v1.h` `ApplyReflection`, the projects'
  `grid.frag`).
- That screen position is not flipped for D3D, so it reads texture rows from
  the top: the reflection is stored upside down. Drawn unflipped, the floor
  showed the sky above the horizon instead of the car (checked against each
  project's `preview.jpg`).
- Diorama: reflection camera = flip-y · camera · flip-y (mirror the world,
  then the image; the winding stays), own depth buffer, cleared to the scene
  colour, drawn before the scene each frame at output size.
- Audiophile vs its WE captures (`scratch/captures9`): SSIM 0.983 → 0.9965.
- `general.camerafade` (camera paths): `false` in Demon Core, DNA Fragment
  and Neon Sunset, `true` in Ricepod, absent in Arsenal and Fantastic Car;
  the default and the fade itself are not decoded (`wallpaper64.exe`
  registers it as general-settings field `+0xe0`).

## Scripts in this scene
- The sky turns 3°/s of `engine.runtime` from scene start (+28° at 9.33 s):
  headless frames run scripts from t = 0 (at most a minute back).
- `applyUserProperties` runs once at load (per the typings); a bound script's
  own `scriptproperties` bindings must be resolved too (`menuEn` ←
  `menuinit` = false keeps the menu ring hidden).

## Results
Golden SSIM 0.007 → 0.918 (pre-post-processing frame 0.961 vs WE's at event
1093). Remaining: HDR star cores and their bloom are dimmer than WE's,
MSAA edges, the CRT workshop effect's constants.
