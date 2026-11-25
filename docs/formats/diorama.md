# Diorama wallpapers (format 1)

Diorama's own wallpapers ([ADR-009](../adr/0009-editor.md)). They need neither Wallpaper Engine nor a shader compiler. Read by `crates/diorama-project`; [ridges](../../examples/wallpapers/ridges) and [lanterns](../../examples/wallpapers/lanterns) are examples. Files of version 1 stay readable. The editor (*New wallpaper* in the library) writes it.

## Folder

```
my-wallpaper/
  project.json      title, tags, preview, user properties
  wallpaper.json    the document
  assets/           images, videos, fonts
```

`project.json` has the shape web wallpapers use, with `"type": "diorama"` and `"file": "wallpaper.json"`. `general.properties` declares what the user may change.

The editor keeps changes that are not saved in `draft.json` (the document and project as edited); nothing else reads it and packages leave it out.

Paths in the document are relative to the folder and use `/`. Anything that leads out of the folder is refused, links included.

## Document

| Key | Meaning | Default |
|---|---|---|
| `version` | 1 (required). A newer one is refused. | |
| `size` | Width and height in pixels (required). Other screens scale it. | |
| `background` | Colour behind all layers. | black |
| `parallax` | `amount`: how far a layer of depth 1 moves while the cursor crosses the screen, as a fraction of the wallpaper. `smoothing`: seconds. | none; `0.05`, `0.15` |
| `layers` | Back to front, at most 1000. | |

### Layers

| Key | Meaning | Default |
|---|---|---|
| `type` | `image`, `video`, `color`, `text` or `adjustment` (required). | |
| `id`, `name` | `id` is the editor's and stays; `name` is the user's. | |
| `visible` | | `true` |
| `position` | The layer's middle (text: its anchor), in pixels from the wallpaper's middle; x to the right, y down. | `[0, 0]` |
| `scale` | One number or `[x, y]`; negative mirrors. | `1` |
| `rotation` | Degrees, clockwise. | `0` |
| `opacity` | 0–1. | `1` |
| `blend` | `normal`, `add`, `multiply`, `screen`, `overlay`, `darken`, `lighten`, `hard-light`, `soft-light`, `difference`. | `normal` |
| `depth` | How much parallax moves it; negative moves with the cursor. | `0` |
| `effects` | See [Effects](#effects). | none |

By type:

| Type | Key | Meaning | Default |
|---|---|---|---|
| `image` | `source` | PNG, JPEG, WebP, BMP, TGA or GIF (its first frame). | |
| | `size` | One number or `[w, h]` in pixels. | the image's |
| | `smooth` | `false` keeps pixels sharp. | `true` |
| | `tint` | Colour multiplied in. | white |
| `video` | `source` | A video the system plays. It loops; its sound does not play. | |
| | `size`, `tint` | As for images. | |
| `color` | `color` | | white |
| | `size` | | the whole wallpaper |
| `text` | `text` | At most 10 000 characters; `\n` breaks lines. | |
| | `font` | A TrueType or OpenType asset, or `system:` and the name of an installed family (`system:Georgia`). One that cannot be had is stood in for. | the system's |
| | `bold`, `italic` | Faces of an installed family. | `false` |
| | `font-size` | Pixels. | `64` |
| | `color` | | white |
| | `align` | `left`, `center`, `right` of `position`. | `center` |
| | `vertical-align` | `top`, `middle`, `bottom`. | `middle` |
| | `max-width`, `max-lines`, `ellipsis` | Wrapping; `ellipsis` marks text that did not fit. | none, none, `true` |

An `adjustment` applies its effects to everything below it. It covers the whole wallpaper: `position`, `scale`, `rotation` and `depth` do not count.

### Effects

`effects` is a list, first to last, at most 32 per layer. Each is an object with `type`, its settings, and `visible` (default `true`). They change the layer's picture; the layer's opacity and blending come after them.

| Type | What it does | Setting | Values | Default |
|---|---|---|---|---|
| `blur` | Softens the layer. | `radius` | 0 to 200 px | 12 |
|  |  | `edges` | `soft`, `hard` | `soft` |
| `glow` | Bright parts shine over their surroundings. | `radius` | 0 to 200 px | 40 |
|  |  | `strength` | 0 to 5 | 1 |
|  |  | `threshold` | 0 to 0.99 | 0.5 |
|  |  | `color` | a colour | `#ffffff` |
| `tint` | Colours the layer in shades of one colour. | `color` | a colour | `#ffb060` |
|  |  | `amount` | 0 to 1 | 0.5 |
| `adjust` | Brightness, contrast, saturation and hue. | `brightness` | -1 to 1 | 0 |
|  |  | `contrast` | -1 to 1 | 0 |
|  |  | `saturation` | -1 to 1 | 0 |
|  |  | `hue` | -180 to 180° | 0 |
| `vignette` | Darkens (or colours) towards the corners. | `color` | a colour | `#000000` |
|  |  | `amount` | 0 to 1 | 0.6 |
|  |  | `size` | 0 to 1 | 0.4 |
|  |  | `softness` | 0.01 to 1 | 0.6 |
| `grain` | Fine noise, as on film. | `amount` | 0 to 1 | 0.08 |
|  |  | `size` | 1 to 50 px | 2 |
|  |  | `speed` | 0 to 60 | 12 |
| `shift` | Red and blue drift apart, as through a cheap lens. | `amount` | 0 to 100 px | 4 |
|  |  | `direction` | -180 to 180° | 0 |
| `pixelate` | Large square pixels. | `size` | 1 to 400 px | 12 |
| `drift` | Moves the picture across the layer and starts over: clouds, fog, patterns that repeat. | `speed` | -1000 to 1000 px/s | 30 |
|  |  | `direction` | -180 to 180° | 0 |
| `sway` | Bends to and fro, as plants and hair do in the wind. | `amount` | 0 to 300 px | 12 |
|  |  | `speed` | 0 to 5 /s | 0.3 |
|  |  | `length` | 10 to 5000 px | 600 |
|  |  | `anchor` | `bottom`, `top`, `left`, `right`, `none` | `bottom` |
| `ripple` | Waves, as on water. | `amount` | 0 to 100 px | 4 |
|  |  | `length` | 4 to 2000 px | 120 |
|  |  | `speed` | -5 to 5 /s | 0.5 |
| `shake` | Wanders around its place, as from a hand-held camera. | `amount` | 0 to 200 px | 6 |
|  |  | `speed` | 0 to 30 /s | 1 |
| `pulse` | Grows and fades in a steady beat. | `what` | `brightness`, `opacity`, `zoom` | `brightness` |
|  |  | `amount` | 0 to 1 | 0.3 |
|  |  | `speed` | 0 to 10 /s | 0.5 |
| `mask` | Shows the layer where an image is bright and hides it where it is dark or transparent. | `image` | an image asset (required) |  |
|  |  | `amount` | 0 to 1 | 1 |
|  |  | `invert` | `true` or `false` | `false` |
| `fade` | The layer fades to nothing towards one side. | `direction` | -180 to 180° | 90 |
|  |  | `start` | 0 to 1 | 0.5 |
|  |  | `end` | 0 to 1 | 1 |

Distances are in the wallpaper's pixels, before the layer's scale. Directions are in degrees: 0 to the right, 90 down. Speeds marked /s are beats per second. `mask` and `fade` on an adjustment limit where the adjustment shows.

### Values

A colour is `"#rgb"`, `"#rrggbb"`, `"#rrggbbaa"`, `[r, g, b]` or `[r, g, b, a]` (0–1).

Any value except `id`, `type` and `version` can come from a user property:

```json
{ "type": "text", "text": { "property": "words" }, "color": { "property": "accent" } }
```

#### Values that move

`position`, `scale`, `rotation`, `opacity`, `tint`, `color` of a layer (not of the wallpaper) and the numbers and colours of effects can move. The exception is an effect's `speed`. Other values keep what they would start with.

Between keys:

```json
{ "keys": [{ "t": 0, "v": [0, 0] }, { "t": 2, "v": [300, 0], "ease": "linear" }], "length": 3, "repeat": "loop" }
```

| Key | Meaning | Default |
|---|---|---|
| `keys` | At most 1000, each with `t` (seconds), `v` (the value) and `ease`: how it goes on to the next key. | |
| `ease` | `in-out` (smooth), `linear`, `in` (starts slowly), `out` (ends slowly), `hold` (stays, then jumps). | `in-out` |
| `length` | Seconds until it repeats or ends. | the last key |
| `repeat` | `loop` starts over; in the time between the last key and `length` it goes on to the first key. `mirror` goes there and back. `once`. | `loop` |

With the sound the system plays:

```json
{ "audio": "bass", "from": 0.3, "to": 1 }
```

| Key | Meaning | Default |
|---|---|---|
| `audio` | What it listens to: `bass` (to about 250 Hz), `middle` (to about 4 kHz), `treble`, `all`. | |
| `from`, `to` | The value in silence and at its loudest. | |
| `threshold` | Levels below count as silence, 0–0.95. Levels are relative to how loud it has lately been. | `0.3` |
| `smooth` | Seconds it takes to follow. | `0.1` |

`v`, `from` and `to` can be properties. Rotations are not shortened: from 0 to 720 turns twice.

Numbers outside a key's range are moved into it. A value of the wrong type counts as missing. A layer that cannot be used (a missing image, an unknown type) is left out; the log names it, and the rest of the wallpaper shows. Unknown keys are ignored.

### Sharing

A `.diorama` file is the folder as a zip: `project.json`, `wallpaper.json`, the preview and the assets the document uses. Opening one takes only those kinds of files (JSON, images, videos, fonts), at most 2000 of them and 4 GB, and refuses names that leave the folder.

### Limits

Documents up to 16 MB, images up to 256 MB, fonts up to 64 MB, 16384 pixels a side, and 512 MB of decoded pixels together.
