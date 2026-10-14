# P9 — Win11 24H2 desktop layout and Explorer restarts

- **Pain point:** Wallpaper invisible with icons hidden; gone after Explorer restart.
- **Fix:** Detect legacy and 24H2 WorkerW layouts; rebuild on `TaskbarCreated` and on external window destruction.
- **Automated:** no

## Steps
1. Hide desktop icons; start `lumend`.
2. Kill and restart `explorer.exe`.

## Pass when
Wallpaper visible behind icons both times; rebuild logged within a second.

## Last run
2026-10-14: passed on Win11 26200 (24H2 layout).
