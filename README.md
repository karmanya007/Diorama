# Lumen (working name)

Open, cross-platform live wallpaper engine that can play Wallpaper Engine Workshop content you own. Windows first, then Linux, then macOS.

Status: Phase 0 (recon and tooling). See [PROGRESS.md](PROGRESS.md).

## Layout

| Path | Purpose |
|---|---|
| `crates/lumen-core` | Clock, limits, shared foundation types |
| `crates/lumen-we` | Wallpaper Engine importer (pkg, tex, json, shaders) |
| `tools/wpdump` | Inspect / unpack WE content |
| `docs/formats` | File format specs |
| `docs/adr` | Architecture decisions |
| `docs/re-notes` | Reverse-engineering notes (RenderDoc, Ghidra) |

Planned crates (added when their phase starts): `lumen-scene`, `lumen-assets`, `lumen-ipc`, `lumen-gpu`, `lumen-runtime`, `lumen-render`, `lumen-platform`, and apps `lumend`, `lumen-cli`, `lumen-ui`, `lumen-webhost`.

## Usage

```sh
cargo run -p wpdump -- list   "<workshop item>/scene.pkg"
cargo run -p wpdump -- cat    "<workshop item>/scene.pkg" scene.json
cargo run -p wpdump -- unpack "<workshop item>/scene.pkg" extracted/<id>
cargo run -p wpdump -- tex-info "<file>.tex"                       # or: <scene.pkg> --entry materials/x.tex
cargo run -p wpdump -- tex2png  "<file>.tex" out.png --crop
cargo run -p wpdump -- survey --out docs/re-notes/corpus-survey.md   # aggregate feature usage
cargo run -p wpdump -- shader-check --backend hlsl --dxc <path>/dxc.exe   # shader translation pass rate
```

`shader-check --backend hlsl` needs DXC from the [DirectXShaderCompiler releases](https://github.com/microsoft/DirectXShaderCompiler/releases); see [ADR-003](docs/adr/0003-shader-toolchain.md).

Tests: `cargo test --workspace`. Corpus tests read your local Workshop folder (`LUMEN_WE_CORPUS`) and WE install (`LUMEN_WE_INSTALL`), defaulting to the Steam paths, and skip when missing.

Fuzzing (Linux, nightly): `cargo +nightly fuzz run pkg` / `cargo +nightly fuzz run tex` from the repo root.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

## Content and legal

This project does not ship Wallpaper Engine binaries, assets, shaders, or Workshop content. It reads them from your own installation at runtime. Never commit extracted content.
