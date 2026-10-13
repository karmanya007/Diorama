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
```

Tests: `cargo test --workspace`. The corpus test reads your local Workshop folder (`LUMEN_WE_CORPUS` or the default Steam path) and skips when it is missing.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

## Content and legal

This project does not ship Wallpaper Engine binaries, assets, shaders, or Workshop content. It reads them from your own installation at runtime. Never commit extracted content.
