# P13 — Slow startup / heavy UI

- **Pain point:** Bundled Chromium UI; slow start.
- **Fix:** Daemon starts without UI; startup measured.
- **Automated:** no

## Steps
1. Time from process start to first frame (log timestamps).

## Pass when
Image < 1 s; video < 0.5 s; scene with warm shader cache < 1 s.

## Last run
2026-10-14: video 0.40 s, image 0.77 s. 2026-10-15: scene ready 0.18 s after GPU init (warm cache); cold DXC compile adds 0.5–0.6 s.
2026-10-29 UI (installed 0.1.1): window 0.08 s after start; 7 processes (diorama-ui + WebView2), 372 MB private; the UI is optional and closes completely (tray Quit / `--quit`), the daemon runs without it.
