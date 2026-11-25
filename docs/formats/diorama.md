# Diorama wallpapers (format 1)

Diorama's own wallpapers ([ADR-009](../adr/0009-editor.md)). They need neither Wallpaper Engine nor a shader compiler. Read by `crates/diorama-project`; [examples/wallpapers/ridges](../../examples/wallpapers/ridges) is one. The format is new and not final until the editor is released.

## Folder

```
my-wallpaper/
  project.json      title, tags, preview, user properties
  wallpaper.json    the document
  assets/           images, videos, fonts
```

`project.json` has the shape web wallpapers use, with `"type": "diorama"` and `"file": "wallpaper.json"`. `general.properties` declares what the user may change.

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
| `type` | `image`, `video`, `color` or `text` (required). | |
| `id`, `name` | `id` is the editor's and stays; `name` is the user's. | |
| `visible` | | `true` |
| `position` | The layer's middle (text: its anchor), in pixels from the wallpaper's middle; x to the right, y down. | `[0, 0]` |
| `scale` | One number or `[x, y]`; negative mirrors. | `1` |
| `rotation` | Degrees, clockwise. | `0` |
| `opacity` | 0–1. | `1` |
| `blend` | `normal`, `add`, `multiply`, `screen`, `overlay`, `darken`, `lighten`, `hard-light`, `soft-light`, `difference`. | `normal` |
| `depth` | How much parallax moves it; negative moves with the cursor. | `0` |
| `effects` | Reserved; not read yet. | |

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
| | `font` | A TrueType or OpenType file. | the system's |
| | `font-size` | Pixels. | `64` |
| | `color` | | white |
| | `align` | `left`, `center`, `right` of `position`. | `center` |
| | `vertical-align` | `top`, `middle`, `bottom`. | `middle` |
| | `max-width`, `max-lines`, `ellipsis` | Wrapping; `ellipsis` marks text that did not fit. | none, none, `true` |

### Values

A colour is `"#rgb"`, `"#rrggbb"`, `"#rrggbbaa"`, `[r, g, b]` or `[r, g, b, a]` (0–1).

Any value except `id`, `type` and `version` can come from a user property:

```json
{ "type": "text", "text": { "property": "words" }, "color": { "property": "accent" } }
```

Numbers outside a key's range are moved into it. A value of the wrong type counts as missing. A layer that cannot be used (a missing image, an unknown type) is left out; the log names it, and the rest of the wallpaper shows. Unknown keys are ignored.

### Limits

Documents up to 16 MB, images up to 256 MB, fonts up to 64 MB, 16384 pixels a side, and 512 MB of decoded pixels together.
