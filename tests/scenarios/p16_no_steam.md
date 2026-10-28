# P16 — Steam dependency

- **Pain point:** WE needs Steam running.
- **Fix:** Diorama reads local Workshop folders and never talks to Steam.
- **Automated:** yes (corpus tests read files directly)

## Steps
1. Quit Steam; play a Workshop scene with `dioramad --scene`.

## Pass when
Plays normally.

## Last run
2026-10-14: passed.
