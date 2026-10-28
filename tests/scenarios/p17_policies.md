# P17 — Confusing pause settings

- **Pain point:** Interacting toggles.
- **Fix:** One rule per situation (fullscreen app, maximized window, battery, locked) → keep, pause or stop, plus a frame rate cap; presets Balanced, Quality, Gaming, Battery (Settings → When to pause, `diorama policy`).
- **Automated:** unit tests in `dioramad::policy` (defaults, user pause, stop over pause)

## Steps
1. `diorama policy gaming`, then `diorama policy`.
2. `diorama policy --on-maximized keep`; `diorama policy` names it custom.
3. Settings → When to pause shows the same preset and rules.

## Pass when
The rules printed match the preset; changes apply without a restart and survive one.

## Last run
2026-10-26 presets live (CLI and UI); 2026-10-29 stop action added (unit tests).
