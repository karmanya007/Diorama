# P14 — Desktop blank when the daemon is not running

- **Pain point:** Wallpaper exists only while WE runs.
- **Fix:** `--os-fallback` sets an image/snapshot as the Windows wallpaper (opt-in).
- **Automated:** no

## Steps
1. Back up the Windows wallpaper settings.
2. `dioramad --os-fallback --scene <dir>`; quit.
3. Restore the settings.

## Pass when
Windows shows the snapshot after Diorama exits.

## Last run
2026-10-14: passed with backup/restore.
