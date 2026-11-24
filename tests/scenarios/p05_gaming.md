# P5 — Games stutter with the wallpaper running

- **Pain point:** Wallpaper competes with games for GPU time.
- **Fix:** Per-output pause on fullscreen/maximized windows; below-normal process priority; optional stop (unload, free video memory) when a stop rule covers every monitor a wallpaper shows on (Gaming preset: fullscreen → stop).
- **Automated:** unit tests for coverage classification (`diorama-platform`) and decisions (`dioramad::policy`, stop wins over pause)

## Steps
1. Run a scene on both monitors.
2. Start a fullscreen game on monitor 1.
3. `diorama status`.

4. `diorama policy gaming`; wallpaper on monitor 1 only; fullscreen app on monitor 1; `diorama status`; leave fullscreen.

## Pass when
Monitor 1 paused (coverage fullscreen); monitor 2 keeps animating; no frame drops in the game. With gaming: monitor 1 `stopped`, dioramad's dedicated video memory drops, and the wallpaper is back within about a second of leaving fullscreen.

## Last run
2026-10-14: fullscreen VS Code paused the main output, portrait kept playing.
2026-10-29 stop (gaming preset, 4K scene 1132505365 on the main monitor, image on the portrait): fullscreen app → `stopped` at once, dioramad's dedicated video memory 462 → 189 MB (the rest is the other monitor's image and the device); back 0.27 s after leaving fullscreen (import 147 ms). A brief focus change reloaded the scene, so waking now waits until the monitors have been uncovered for 1 s: covered 6 s, 0.5 s gap, covered 3 s → one stop, no reload in the gap, shown 1.5 s after uncovering.

## Linux
2026-11-24: `dioramad` and everything it starts after its first moments run at nice 5 (36 of its threads; the 7 from before, which only wait, stay at 0). Fullscreen and maximized windows pause their output (headless Sway, GNOME Shell, Xvfb). GPU priority is not lowered.
