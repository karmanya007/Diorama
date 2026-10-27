# Changelog

All notable changes to Lumen are listed here. Versions follow [Semantic Versioning](https://semver.org); before 1.0, minor versions may change behaviour.

## 0.1.0 (unreleased)

First public preview (Windows).

### Wallpapers

- Wallpaper Engine scenes:
  - layers and effect chains, bloom;
  - particles (sprites, trails, ropes, child systems);
  - static 3D models with lights;
  - text layers;
  - SceneScript;
  - audio and cursor reactivity, parallax, user properties.
- Videos with hardware decoding, plus a one-time conversion for codecs the graphics card cannot decode.
- Web wallpapers in a separate process: Wallpaper Engine's property, audio and media APIs, PerformanceMonitor widgets, and network off unless allowed.
- Images; your own folders of wallpapers.

### Desktop

- Wallpapers behind the desktop icons:
  - per monitor or spanning all monitors, with fill / fit / stretch;
  - seamless switching between wallpapers.
- Pausing:
  - each monitor pauses on its own behind fullscreen apps or maximized windows, while locked or on battery;
  - presets: Balanced, Quality, Gaming, Battery;
  - static wallpapers stop drawing.
- Recovers from Explorer restarts, graphics driver resets, sleep, monitor changes and audio device changes.

### The Lumen app

- Library with search, filters, mature-content hiding and a measured cost per wallpaper.
- Per-wallpaper settings, starting from your Wallpaper Engine choices.
- Playlists by interval or time of day, per monitor.
- Notification-area icon; start at sign-in; `lumen` command line.

### Installation

- Per-user Windows installer with in-app updates (checked only when you ask).
