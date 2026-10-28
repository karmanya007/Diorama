# P8 — Assignments break when Windows renumbers monitors

- **Pain point:** Settings keyed by volatile display index.
- **Fix:** Stable output ids from `QueryDisplayConfig` device paths (EDID + connector).
- **Automated:** no

## Steps
1. `dioramad --list-outputs`; note ids.
2. Reboot or reconnect a monitor; list again.
3. Hotplug: run a scene on all monitors; Win+P → "PC screen only", wait 5 s, Win+P → "Extend" (or unplug/replug a cable).

## Pass when
Ids unchanged for the same physical monitors. After each hotplug step every connected monitor shows the wallpaper, covering it exactly (log: `rebuilding wallpaper events=[OutputsChanged]`, possibly `wallpaper window re-anchored`).

## Last run
Ids implemented; not yet used for per-monitor persistence (Phase 6).
2026-10-15 hotplug: first run failed — windows were placed before Explorer resized the WorkerW to the new virtual desktop, so both ended up offset by the old origin (-1080, -546), the portrait one off screen. Fixed by re-anchoring windows to their screen rect on every policy tick; rerun passed (2 → 1 → 2 outputs, re-anchored within 1 ms).
