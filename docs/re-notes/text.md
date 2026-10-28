# Text layers

Sources: RenderDoc captures of 3414847823 (27 text draws), 3431125458, 2980001176,
2837297807, 2518958708, 2134765860, 3195058405, extracted with
`tools/refcap/textdraws.py` (post-VS glyph quads, atlases, and every draw that
reads a text render target). Strings from `wallpaper64.exe`. Font metrics
compared with `wpdump text`.

## Rasterization
- `wallpaper64.exe` links FreeType and HarfBuzz (plus DirectWrite). Glyphs are
  shaped with kerning and rasterized one by one into a single-channel atlas
  (`R8_UNORM`, 512×512 seen), packed left to right in order of first use.
- **Point size is rendered at 300 dpi**: pixels per em = `pointsize × 300 / 72`.
  Atami 32 pt: digit "1" (bbox 255×738 units) → 35×99 px bitmap, bitmap left 6,
  exactly FreeType's floor/ceil of the scaled outline box.
- A glyph bitmap sits at the pen position plus its bearing; captured positions
  match an unhinted rasterizer to ±1 px (WE hints; Diorama does not).
- Line metrics follow FreeType's size metrics: ascender = ceil(asc·s),
  descender = floor(desc·s), line height = round((asc − desc + gap)·s), from
  `hhea`. Quicksand Bold 32 pt: 127 / −29 / 155. Anurati 32 pt: 120 / −40 / 160.
- `systemfont_*` → Windows font files: arial.ttf, calibri.ttf, cambria.ttc,
  comic.ttf, consola.ttf, micross.ttf (sansserif), segoeui.ttf, verdana.ttf.
  Fallback chain strings (arialuni, seguisym, seguiemj, Malgun, msyh) exist but
  are not reproduced yet.

## Shader
`shaders/font.{vert,frag}`, materials `materials/fonts/basefont*.json`
(translucent, no depth). Basic path: `gl_FragColor = (g_Color4.rgb, R8 × g_Color4.a)`.
Combos `MSDF`, `COLORFONT`, outline/blur/drop-shadow (`g_RenderVar0..3`) exist;
none of the captured scenes used them.

## Placement
Coordinates in the object's local space (pixels before the object's scale), y up.
- A line's width is the right edge of its last glyph bitmap measured from the
  pen start (left bearing included, not trimmed).
- Horizontal: left → pen starts at the origin; center → at −width/2; right → at
  −width. Multi-line text aligns each line on its own.
- Vertical (first baseline relative to the origin):
  - top: −ceil(asc) (origin = top of the first line),
  - bottom: −floor(desc) for one line (origin = lowest descent),
  - center: −asc/2 (half an ascender above the baseline; Anurati exactly 60 of
    its 120 ascender, Quicksand 63.7 of 127).
  Further lines go down by the line height; for several lines Diorama assumes
  center lifts the block by half the extra rows (not yet captured).
- The object's `size` is a stale editor value; WE recomputes the box from the text.

## Render targets
- Text without effects (and without `copybackground`) is drawn as glyph quads
  straight into the scene with SrcAlpha/InvSrcAlpha.
- Otherwise the text renders into its own target: width = text width + 2·padding,
  height = rows · line height + 2·padding; first baseline at padding + ceil(asc)
  from the top. The target is centred on the text box (so the box keeps the
  placement above), effects ping-pong on it, and it is composited with
  SrcAlpha/InvSrcAlpha.
- Before the glyphs, the target is filled from a copy of the frame
  (`_rt_FullFrameBuffer`-like texture) under the layer's placement (no
  blending); glyphs then blend on top. This happens with `copybackground: true`
  and also when the key is absent (captures 914, 924); effects such as
  edge_glow with "Background alpha 1" rely on it (otherwise the transparent
  margin turns opaque black).
- `padding` is a number or `"x y"`.

## Not yet covered
`anchor` (none/center/left/top), `limitwidth`/`limitrows`/ellipsis exact rules,
`opaquebackground` + `backgroundcolor`, `blockalign`, font fallback for missing
glyphs, MSDF fonts, per-frame text changes (SceneScript).
