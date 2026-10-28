# ADR-004: Video decoding — OS decoders behind `diorama-media`

Date: 2026-10-14 · Status: accepted

## Context
The plan assumed ffmpeg for video. On Windows that means shipping FFmpeg DLLs (LGPL build hygiene, codec patent questions) and a bindgen/clang build dependency, while Windows already ships hardware-accelerated decoders through Media Foundation — the same stack WE uses (`MFPlat`/`MFReadWrite` imports).

## Decision
- `diorama-media` exposes OS-neutral `VideoDecoder` / `VideoPlayer` producing NV12 frames; per-OS backends live inside it (like `diorama-platform`, it may contain OS `cfg`s — enforced by `scripts/check-layers.sh`).
- **Windows:** Media Foundation Source Reader with a D3D11 device manager (DXVA hardware decode), advanced video processing to NV12, hardware transforms enabled. Software decode is the automatic fallback.
- **Video only:** no audio stream is opened, so playback never depends on an audio device (P10). Audio playback becomes a separate, optional path later.
- Frames are read back to CPU memory and uploaded as `R8` + `Rg8` planes; BT.709 limited-range → RGB in the shader. Zero-copy (shared D3D11→D3D12 textures) is a later optimisation.
- Linux (Phase 7): VA-API via GStreamer or ffmpeg; macOS (Phase 8): VideoToolbox — decided when those phases start.

## Measurements (RTX 4060, 2 monitors, release build)
| Video | Decode path | CPU | Memory |
|---|---|---|---|
| 1080p24 | software | 49% of one core | 393 MB |
| 1080p24 | DXVA | **8.4% of one core (0.53% of machine)** | 274 MB |
| 4K30 | software | 99% of one core | 954 MB |
| 4K30 | DXVA | **16.3% of one core (1.0% of machine)** | 493 MB |

All 21 corpus videos (1080p–4K, 24–60 fps) decode faster than real time on the DXVA path (`cargo test -p diorama-media --test corpus_video`).

## Consequences
- No extra native dependencies on Windows; codec support = what Windows has installed (HEVC needs the Store extension, same as WE).
- CPU readback costs some bandwidth; acceptable at the measured numbers, revisit with zero-copy if 4K60 or multi-video playlists need it.
