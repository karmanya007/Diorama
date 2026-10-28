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
Not run yet.
