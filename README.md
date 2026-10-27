# Lumen (working name)

Open, cross-platform live wallpaper engine that can play Wallpaper Engine Workshop content you own. Windows first, then Linux, then macOS.

Status: Phase 1 (Windows: image/video wallpapers behind the icons, pause policies, CLI). See [PROGRESS.md](PROGRESS.md).

## Layout

| Path | Purpose |
|---|---|
| `crates/lumen-core` | Clock, limits, shared foundation types |
| `crates/lumen-we` | Wallpaper Engine importer (pkg, tex, json, shaders) |
| `crates/lumen-platform` | OS integration: monitors, wallpaper windows behind the icons, coverage, power |
| `crates/lumen-gpu` | wgpu device and per-monitor surfaces |
| `crates/lumen-render` | Image/video drawing, fit and span placement |
| `crates/lumen-media` | Video decoding (Media Foundation + DXVA on Windows) |
| `crates/lumen-ipc` | Daemon ↔ client protocol over a per-user local socket |
| `apps/lumend` | Wallpaper daemon |
| `apps/lumen-cli` | `lumen` control command |
| `tools/wpdump` | Inspect / unpack WE content |
| `docs/formats` | File format specs |
| `docs/adr` | Architecture decisions |
| `docs/re-notes` | Reverse-engineering notes (RenderDoc, Ghidra) |

Planned crates (added when their phase starts): `lumen-scene`, `lumen-assets`, `lumen-runtime`, and apps `lumen-ui`, `lumen-webhost`.

## Usage

Run the daemon, then control it (Windows):

Wallpaper Engine scenes need the DirectX Shader Compiler: run `scripts/fetch-dxc.ps1` once (pinned, hash-checked, into `.tools/dxc`) and set `LUMEN_DXC` to its `dxc.exe` (or put `dxc.exe` + `dxcompiler.dll` beside `lumend.exe`). Compiled shaders are cached in `%LOCALAPPDATA%\Lumen\cache\shaders`.

```sh
cargo build --release
$env:LUMEN_DXC = "$PWD\.tools\dxc\dxc.exe"           # PowerShell; needed for scenes
target/release/lumend                                   # starts empty; or --image/--video/--scene <path>
target/release/lumen set <image|video|scene folder> [--fit fill|fit|stretch] [--layout per-output|span]
target/release/lumen status                             # what is playing, per-monitor state
target/release/lumen volume 0-100                       # video soundtrack (0 = muted, default)
target/release/lumen pause | resume | quit
target/release/lumen autostart enable | disable | status   # start at sign-in (per user)
```

Scenes play at `--fps` (default 30), follow the cursor for parallax, and default to the fit chosen for that wallpaper in Wallpaper Engine (if any). `--we-assets` points at Wallpaper Engine's `assets` folder (built-in shaders and effects); the default is the Steam install path. `lumend --list-outputs` shows monitors.

`lumend --help` lists pause rules (`--on-fullscreen`, `--on-maximized`, `--on-battery`, `--on-lock`). The last wallpaper is remembered in `%APPDATA%\Lumen\state.json`. `--background` detaches from the console and logs to `%APPDATA%\Lumen\lumend.log`. `--os-fallback` also sets each wallpaper (a snapshot for videos) as the Windows background, so the desktop isn't blank when Lumen isn't running — this changes your Windows wallpaper setting.

Developer tools:

```sh
cargo run -p wpdump -- list   "<workshop item>/scene.pkg"
cargo run -p wpdump -- cat    "<workshop item>/scene.pkg" scene.json
cargo run -p wpdump -- unpack "<workshop item>/scene.pkg" extracted/<id>
cargo run -p wpdump -- tex-info "<file>.tex"                       # or: <scene.pkg> --entry materials/x.tex
cargo run -p wpdump -- tex2png  "<file>.tex" out.png --crop
cargo run -p wpdump -- survey --out docs/re-notes/corpus-survey.md   # aggregate feature usage
cargo run -p wpdump -- shader-check --backend hlsl --dxc <path>/dxc.exe   # shader translation pass rate
target/release/lumend --scene <dir> --frame-out out.png --time 10 [--pointer 0.5,0.5] [--dump-targets]   # one offscreen frame
target/release/imgdiff reference.png candidate.png --cover --side-by-side sbs.png   # SSIM/PSNR
CAPTURES=scratch/captures tools/refcap/golden.sh      # score local RenderDoc captures of WE (see tools/refcap)
```

`shader-check --backend hlsl` needs DXC from the [DirectXShaderCompiler releases](https://github.com/microsoft/DirectXShaderCompiler/releases); see [ADR-003](docs/adr/0003-shader-toolchain.md).

Installer (Windows): `scripts/bundle.ps1` builds `target/release/bundle/nsis/Lumen_<version>_x64-setup.exe` (lumen-ui, lumend, lumen-webhost, `lumen` and DXC; needs Node). With `TAURI_SIGNING_PRIVATE_KEY`/`_PASSWORD` set it also signs the update file. Releases: bump the workspace version, push a tag `v<version>`, and the release workflow publishes the installer and `latest.json` for the in-app updater; see [ADR-007](docs/adr/0007-packaging-updates.md).

Tests: `cargo test --workspace`. Corpus tests read your local Workshop folder (`LUMEN_WE_CORPUS`) and WE install (`LUMEN_WE_INSTALL`), defaulting to the Steam paths, and skip when missing.

Fuzzing (Linux, nightly): `cargo +nightly fuzz run pkg` / `cargo +nightly fuzz run tex` from the repo root.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

## Content and legal

This project does not ship Wallpaper Engine binaries, assets, shaders, or Workshop content. It reads them from your own installation at runtime. Never commit extracted content.
