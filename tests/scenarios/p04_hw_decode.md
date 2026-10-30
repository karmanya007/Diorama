# P4 — Video CPU without hardware decode

- **Pain point:** Silent software fallback makes videos expensive.
- **Fix:** Media Foundation + DXVA (D3D11 device manager); software fallback is logged.
- **Automated:** partly (`cargo test -p diorama-media --test corpus_video` decodes all corpus videos)

## Steps
1. `dioramad --video <4K mp4> --duration 60`; check the log says DXVA.
2. Measure CPU.

## Pass when
4K30 < 3% CPU with DXVA. Fallback logged when DXVA is unavailable.

## Last run
2026-10-14: 1080p24 0.53%, 4K30 1.0% machine CPU. UI warning/transcode offer: Phase 6.
2026-10-27: DXVA probe per codec/profile/size; `diorama convert` made an H.264 copy (60 s 1080p → 640×360 in 2.5 s, sound kept) that dioramad swapped in. All 21 corpus videos and this GPU decode in hardware, so the UI offer has only been seen in the mock UI.
2026-10-30 live (installed 0.2.0, RTX 4060, user clicking in the UI): ffmpeg-made test videos (not committed). MPEG-4 Part 2 1080p30: status "decoded by the CPU" (14.4% of one core), the details panel offered Convert, the copy was made in 2.4 s and swapped in without a flash (7.1% of one core after). 10-bit H.264: Windows has no decoder for it, yet the player was shown (blank, status "playing") and Convert failed with `ReadSample failed: CopyDecodedFrame failed`. Fixed the same day: the probe decodes the first frame when the GPU cannot, so `set` and `convert` refuse such videos with a plain reason and the previous wallpaper stays (checked live with the dev daemon; `--test convert` covers both files via `DIORAMA_TEST_VIDEO` / `DIORAMA_TEST_UNDECODABLE_VIDEO`).
