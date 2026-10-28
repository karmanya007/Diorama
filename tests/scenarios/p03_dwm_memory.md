# P3 — DWM memory growth

- **Pain point:** `dwm.exe` balloons while WE runs.
- **Fix:** Swapchains sized exactly to each output, 2 buffers, latency 1, never recreated per frame.
- **Automated:** no

## Steps
1. Note `dwm.exe` private bytes.
2. Run `dioramad --scene <dir>` for 30 min, moving windows occasionally.
3. Note `dwm.exe` private bytes again.

## Pass when
DWM memory stable (no monotonic growth).

## Last run
2026-10-29, Win11 24H2, 180 Hz 1080p + 144 Hz portrait 1080p, desktop in normal use: 4K scene 1132505365 on the main monitor (24 fps) and an image on the other, 30 min: `dwm.exe` private bytes 282.2 → 282.6 MB (range 282.1–282.7), working set 95.7–96.6 MB. Web wallpaper (span, installed 0.1.1), 15 min: 316.8–316.9 MB, one step to 322 MB when the Diorama UI was opened, flat after.
