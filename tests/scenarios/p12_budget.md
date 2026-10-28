# P12 — Heavy 4K wallpapers

- **Pain point:** No budget: 4K textures cost VRAM/CPU regardless of the screen.
- **Fix:** Scenes composite at the size the outputs need; mip chains; decoded scenes dropped from RAM after upload; budget + cost report in Phase 6.
- **Automated:** unit tests `render_size_*`

## Steps
1. `dioramad --scene <4K scene>`; read working set after 10 s.

## Pass when
Working set well below the decoded texture size.

## Last run
2026-10-15: 4K scene 448 → 293 MB working set.
