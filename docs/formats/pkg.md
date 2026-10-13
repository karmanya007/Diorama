# `.pkg` — Wallpaper Engine scene archive

Status: **verified** against corpus item 1132505365 (38 entries, PKGV0001). Reader: `crates/lumen-we/src/pkg.rs`.

Flat archive, no compression. All integers little-endian `u32`. Strings are `u32` byte length + UTF-8 bytes, not NUL-terminated.

```text
string  version          "PKGV" + 4 digits, e.g. "PKGV0001"
u32     entry_count
entry_count × {
    string  path         relative, '/'-separated (e.g. "materials/foo.tex")
    u32     offset       relative to the start of the data section
    u32     size
}
data section             begins immediately after the last table entry
```

## Observed contents
`scene.json` (always), `materials/*.json` + `*.tex`, `models/*.json`, `effects/<name>/effect.json`,
`shaders/effects/*.{vert,frag}`, `particles/presets/*.json`, `materials/masks/*.tex` (painted effect masks).
Entry order is not alphabetical and not grouped.

## Reader hardening
- Version must be 8 bytes starting `PKGV`.
- Entry count, string lengths capped by `lumen_core::Limits`.
- Every entry range checked against data-section length (checked arithmetic).
- Paths rejected if empty, absolute, containing `\`, `:`, NUL, empty / `.` / `..` components.
- Duplicate paths rejected.

## Open questions
- Other version numbers in the wild (survey corpus for `PKGV00xx` values).
