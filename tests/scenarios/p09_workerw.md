# P9 — Win11 24H2 desktop layout and Explorer restarts

- **Pain point:** Wallpaper invisible with icons hidden; gone after Explorer restart.
- **Fix:** Detect legacy and 24H2 WorkerW layouts; rebuild on `TaskbarCreated` and on external window destruction.
- **Automated:** no

## Steps
1. Hide desktop icons; start `dioramad`.
2. Kill and restart `explorer.exe`.

## Pass when
Wallpaper visible behind icons both times; rebuild logged within a second.

## Last run
2026-10-14: passed on Win11 26200 (24H2 layout).

## Linux: the compositor restarts
1. `dioramad` in a headless Sway (or under Xvfb); kill the compositor (or X server).
2. Start it again within 10 seconds; or do not.

Pass when the wallpapers are back within seconds of the compositor; when it stays away, `dioramad` quits after 10 seconds, and nothing spins or floods the log meanwhile.

2026-11-24: first run failed (one core used, 605,482 log lines in 5 s, control socket kept). Fixed: the daemon restarts itself and waits for the desktop. Rerun passed: Sway (image, web) and Xvfb + Openbox, with and without a restart.
