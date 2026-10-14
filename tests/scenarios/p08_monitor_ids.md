# P8 — Assignments break when Windows renumbers monitors

- **Pain point:** Settings keyed by volatile display index.
- **Fix:** Stable output ids from `QueryDisplayConfig` device paths (EDID + connector).
- **Automated:** no

## Steps
1. `lumend --list-outputs`; note ids.
2. Reboot or reconnect a monitor; list again.

## Pass when
Ids unchanged for the same physical monitors.

## Last run
Ids implemented; not yet used for per-monitor persistence (Phase 6).
