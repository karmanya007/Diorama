# `.tex` — Wallpaper Engine texture

Status: **verified** — 1033/1033 textures parse (WE `assets/` + every `.tex` inside 55 corpus `scene.pkg`), 1025 decode to RGBA, 8 detected as MP4. Reader: `crates/diorama-we/src/tex.rs`, decoder: `tex_decode.rs`.

Integers are little-endian. Tags are ASCII, NUL-terminated (`"TEXV0005\0"` = 9 bytes).

```text
"TEXV0005\0"
"TEXI0001\0"
i32 format                 see table
u32 flags                  see table
u32 texture_width          allocated size, often power-of-two padded
u32 texture_height
u32 image_width            meaningful image inside the texture
u32 image_height
[u32 extra]                only if flags & 0x40
u32 unknown                looks like a hash/color (e.g. 0xFFE54489)

"TEXB000N\0"               N = 1..4
u32 image_count            >1 only for multi-image GIF-style textures
[i32 image_format]         N >= 3. FreeImage format id; -1 = raw pixels
[u32 is_video_mp4]         N == 4
image_count × {
    u32 mip_count
    mip_count × {
        u32 width
        u32 height
        [u32 depth]                    flags & 0x40
        [u32 lz4, u32 decompressed]    N >= 2. lz4 != 0 → LZ4 *block* (no frame header)
        u32 size
        size bytes
    }
}

["TEXS000N\0" ...]         only if flags & 0x4 (sprite sheet), see below
```

## Formats (`TEXI.format`)
| id | format | bytes | notes |
|---|---|---|---|
| 0 | RGBA8888 | w·h·4 | |
| 4 | DXT5 / BC3 | 16 per 4×4 block | |
| 6 | DXT3 / BC2 | 16 per block | |
| 7 | DXT1 / BC1 | 8 per block | |
| 8 | RG88 | w·h·2 | particle sprites; decoded as luminance+alpha (assumption, confirm via RenderDoc) |
| 9 | R8 | w·h | masks, mono sprites |

Sizes cross-checked against `decompressed` field on corpus files.

When `image_format != -1` the mip bytes are a complete encoded file. Seen: 13 = PNG (229 files), 2 = JPEG (28 files).

## Flags (`TEXI.flags`)
| bit | meaning | evidence |
|---|---|---|
| 0x1 | no interpolation (point sampling) | `grid.tex`, `camera.tex` |
| 0x2 | clamp UVs | most scene images |
| 0x4 | sprite sheet; `TEXS` trailer present | particle sheets |
| 0x20 | **video**: mip payload is an MP4 (`ftypisom`), even in TEXB0003 with image_format -1 | 8 corpus textures |
| 0x40 | extra `u32` in header and per mip | 32×32×32 color-grading LUTs (`assets/materials/lut/*`), value 32 |
| 0x80000 | unknown | particle sheets (`beam_flower_0`, `hose_1`, `arrow_sharp`) |

## Sprite sheet trailer
```text
"TEXS000N\0"               N = 1..3
u32 frame_count
[u32 width, u32 height]    N == 3
frame_count × {            32 bytes each
    u32 image_id
    f32 frametime          seconds; 0 = not animated (random frame per particle)
    f32 x, f32 y
    f32 width,  f32 width_y
    f32 height_x, f32 height
}
```
Verified by byte count: `fish1.tex` TEXS0003, 16 frames → 9+12+16·32 = 533 trailer bytes; `debris1.tex` TEXS0002, 8 frames → 269.

`TEXS0001` not present locally; reader assumes integer rectangle fields (unverified).

## Open questions
- Meaning of flag 0x80000 and the trailing `unknown` header field.
- RG88 channel semantics in shaders (confirm via RenderDoc).
- `image_count > 1` usage.
