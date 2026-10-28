# P2 — Idle GPU on static wallpapers

- **Pain point:** WE keeps rendering static wallpapers (30–45% GPU reported).
- **Fix:** Static images render once; scenes and web pages render only at the frame rate cap (`--fps`, policy) and pause with their outputs.
- **Automated:** no

## Steps
1. `dioramad --image <png> --duration 60`
2. After 10 s, read `\GPU Engine(pid_<dioramad>*engtype_3D)\Utilization Percentage` (PowerShell `Get-Counter`).

## Pass when
Image: GPU ≈ 0%. Scene at default 30 fps: low single-digit % on the dev machine.

## Last run
2026-10-14 image: 0.00% GPU, ~0.5% of a core. 2026-10-15 4K scene (1132505365): 1.4% GPU, 4.1% of a core.
2026-10-29 web: pages drew at the display's refresh rate (Paper Valley, 180 Hz span: 17.3% GPU in the cost report); animation frames now follow the cap — probe page measured 24/30/60/144 fps exactly at those caps (180 uncapped).
