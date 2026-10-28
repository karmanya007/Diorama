# P12 — Heavy 4K wallpapers

- **Pain point:** No budget: 4K textures cost VRAM/CPU regardless of the screen.
- **Fix:** Scenes composite at the size the outputs need; mip chains; decoded scenes dropped from RAM after upload; measured cost per wallpaper (`diorama costs`, library badges and sort). Not done: downscaling textures larger than the screen needs.
- **Automated:** unit tests `render_size_*`

## Steps
1. `dioramad --scene <4K scene>`; read working set after 10 s.

## Pass when
Working set well below the decoded texture size.

## Last run
2026-10-15: 4K scene 448 → 293 MB working set.
2026-10-29 cost report live (installed 0.1.1, span over 1080p + portrait 1080p): Paper Valley 17.3% GPU / 5.2% CPU / 459 MB VRAM (before the web frame cap), a 4K video 12.3% GPU / 0.7% CPU / 344 MB VRAM.
