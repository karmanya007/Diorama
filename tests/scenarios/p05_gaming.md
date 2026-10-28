# P5 — Games stutter with the wallpaper running

- **Pain point:** Wallpaper competes with games for GPU time.
- **Fix:** Per-output pause on fullscreen/maximized windows; below-normal process priority.
- **Automated:** unit tests for coverage classification (`diorama-platform`)

## Steps
1. Run a scene on both monitors.
2. Start a fullscreen game on monitor 1.
3. `diorama status`.

## Pass when
Monitor 1 paused (coverage fullscreen); monitor 2 keeps animating; no frame drops in the game.

## Last run
2026-10-14: fullscreen VS Code paused the main output, portrait kept playing.
