# P6 — One maximized window pauses all monitors

- **Pain point:** Clone/span mode shares one swapchain in WE.
- **Fix:** Per-output present; each output pauses independently, also in span layout.
- **Automated:** no

## Steps
1. `dioramad --scene <dir> --layout span`.
2. Maximize a window on one monitor.

## Pass when
Only that monitor pauses.

## Last run
2026-10-14: passed (image, video and scene content).
