# ADR-009: The editor and Diorama's own wallpaper format

Date: 2026-11-25 · Status: accepted

Replaces one sentence of ADR-002 ("Our `.lwp` format is the serialized IR").

## Context

Phase 9 is an editor for wallpapers of Diorama's own. ADR-002 planned to save
the Scene IR as the file format. The IR has since become a compiled form:
shaders are translated HLSL, textures are decoded pixels, layers, programs and
textures refer to each other by index, and effect settings are uniform values
without names, ranges or units. Saving it would

- lose what an editor needs (which effect this is, what its settings mean),
- make large files that break whenever the renderer changes,
- write Wallpaper Engine's shaders into our files as soon as a WE effect is
  used. Those may not be redistributed.

What already exists and can be reused: the renderer (effect chains, blending,
text, particles, parallax, bloom), off-screen rendering
(`dioramad --scene … --frame-out`), video textures, keyframe animations,
SceneScript, audio spectrum, the library, and the properties form the UI
builds from `project.json`.

## Decision

### 1. A document that is compiled to the IR

A new crate `diorama-project` (L2, beside `diorama-we`) reads a *document* and
produces a `diorama_scene::Scene`. The renderer, scripts and daemon do not
change: to them it is another importer. It must not depend on `diorama-we`
(added to `check-layers.sh`).

A wallpaper is a folder:

```
my-wallpaper/
  project.json      title, tags, preview, user properties; "type": "diorama"
  wallpaper.json    the document (below)
  assets/           images, videos, fonts, sounds as the user added them
```

`project.json` keeps the shape the library, the properties form and web
wallpapers already use, so listing, searching, per-wallpaper settings and
presets work unchanged.

```json
{
  "version": 1,
  "size": [3840, 2160],
  "background": "#101018",
  "parallax": { "amount": 0.5 },
  "layers": [
    {
      "id": "sky", "type": "image", "name": "Sky", "source": "assets/sky.png",
      "position": [0, 0], "scale": [1, 1], "rotation": 0, "opacity": 1,
      "blend": "normal", "depth": 0.1,
      "effects": [
        { "type": "drift", "speed": 0.2, "direction": 90 },
        { "type": "tint", "color": { "property": "accent" }, "amount": 0.3 }
      ]
    }
  ]
}
```

Any value can be a constant, a user property, an animation (keyframes), an
audio band or, later, a script. Ids are stable, unknown fields are kept on
saving, and each version has a migration from the one before.

Sharing uses one file, `name.diorama`: the folder as a zip. It counts as
untrusted input like a Workshop package: size limits, no paths outside the
folder, bounded parsers, a fuzz target.

### 2. Our own effects, in WGSL

Effects are written for Diorama and shipped with it under its license: a
manifest (name, settings with ranges and defaults) and a WGSL fragment shader.
`ShaderProgram` gains a WGSL source beside HLSL; naga reads it directly, so
these wallpapers need neither DXC nor a Wallpaper Engine install. The renderer
already finds uniforms by name through naga, so feeding them does not change.

First set: blur, tint, colour adjust, vignette, grain, chromatic shift,
drift (scroll), sway, ripple, shake, pulse, glow, pixelate, mask, fade.
None is copied from WE; names and settings are our own.

### 3. Editing in the UI, drawing in a process of its own

- The editor is a view of `diorama-ui`, opened with *New wallpaper* or *Edit*.
- The picture comes from a child process, `dioramad --preview <folder>`: it
  compiles the document, renders off-screen at the size of the preview area
  and sends frames to the UI, which draws them on a canvas. The UI stays free
  of GPU code (P13), and a crash in a driver or an effect costs a restart of
  the preview and never the document.
- Selection boxes and handles are drawn by the page over the canvas, from
  layer rectangles the preview sends along. That works the same on Windows,
  Wayland, X11 and macOS; a native child window under the webview does not.
- The document lives in the UI; each change goes to the preview as a small
  patch. Undo and redo keep snapshots. Autosave writes beside the file and
  replaces it only when complete.

Risk: 960×540 at 30 fps is about 62 MB/s of raw frames through the webview
bridge. Milestone 0 measured it (see below): raw frames are fast enough.

### 4. Wallpaper Engine scenes are not converted

Editing a WE scene would mean carrying WE's shaders and the author's assets
into our file. The editor does not import them. Changing a WE wallpaper's user
properties stays possible as today. The user's own images, videos and fonts
are added as assets.

## Milestones

Each is usable and tested alone; the first release can stop after 3.

| # | Content | Done when |
|---|---|---|
| 0 | Spike: preview process, frames to a canvas, WGSL program in the renderer | 30 fps at 960×540 on Windows and in WebKitGTK; numbers recorded here |
| 1 | Format and compiler: image, video, solid colour and text layers; transform, opacity, blend, parallax; `dioramad` and the library play it | golden images on Windows and Linux (lavapipe); fuzz target; an example in `examples/` |
| 2 | Effects: engine, manifests, the first set | a golden image per effect; none needs DXC |
| 3 | Editor: layer list, adding and ordering, handles, settings from manifests, undo, autosave, preview, export to `.diorama`, opening one | a wallpaper made, shared as a file and shown on another machine without touching JSON |
| 4 | Motion: keyframe timeline, audio-driven values, user properties made in the editor | the example wallpapers use each |
| 5 | Particles: emitter presets and their settings | golden images; cost shown while editing |
| 6 | Scripts on values (the SceneScript engine, our own API names) and custom WGSL effects | limits and watchdog tested with hostile input |

## Milestone 0 (2026-11-25)

Raw RGBA frames from a child process's standard output, fetched by the page
over a custom protocol one at a time and drawn with `putImageData` (release
build; WebView2 on Windows 11, WebKitGTK in headless Sway under WSL):

| Frame | WebView2 | WebKitGTK |
|---|---|---|
| 960×540 | 58 fps | 76 fps |
| 1280×720 | 34 fps | 44 fps |
| 1920×1080 | 15 fps | 20 fps |

Almost all of it is the fetch; drawing takes under 1 ms (WebView2) to 2 ms,
and WebGL is no faster. The pipe carries far more than the webview takes.

`dioramad --preview <folder>` (`diorama_ipc::preview`) holds 30 fps at each of
these sizes with two Workshop scenes on Windows and reaches 180–240 when not
held back, waiting for every frame's readback. The first message comes after
about half a second.

WGSL programs draw the same pixels as their HLSL twins
(`crates/diorama-render/tests/scene_render.rs`), without DXC.

So: previews up to 1280×720 as raw frames, larger areas scaled up by the page.

## Milestone 1 (2026-11-25)

`crates/diorama-project` compiles documents (`docs/formats/diorama.md`) to the
IR; `dioramad`, the library and `--preview` play them. Ten blend modes (two in
fixed function, eight in the shader over the picture so far). Pixel tests pass
on Windows (DX12) and Linux (lavapipe, no DXC); an example is
`examples/wallpapers/ridges`.

Open: video layers convert frames to RGBA on the CPU (fine for a layer, dear
at 4K); text without a font asset uses the system's, so the editor should add
one; a wallpaper that does not move is still drawn at the frame rate cap.

## Alternatives

- **Editing WE's `scene.json` in place.** Reuses the importer fully and the
  results play in WE, but every useful effect is WE's own file, so nothing
  made this way could be shared outside the Workshop, and the editor would
  follow a format that is not ours. Could still come later as an "advanced
  settings" view.
- **Serializing the IR** (ADR-002): see Context.
- **A native editor (egui, wgpu window).** Fastest preview, but a second UI
  toolkit, and the properties form, library and look would be rebuilt.
- **Preview on the real desktop only.** Nothing to build, but editing behind
  other windows is not usable, and a broken wallpaper would be on screen.

## Settled on approval (2026-11-25)

- Own format and effects; WE scenes are not converted.
- The file extension is `.diorama`.
- The first release with the editor follows milestone 3.
