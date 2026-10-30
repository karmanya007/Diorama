# Changelog

All notable changes to Diorama are listed here. Versions follow [Semantic Versioning](https://semver.org); before 1.0, minor versions may change behaviour.

## Unreleased

- Diorama's background programs (`dioramad`, `diorama-webhost`, `diorama`) now carry their name, version and icon, so Task Manager lists them as "Diorama wallpaper daemon", "Diorama web wallpaper host" and "Diorama command line" instead of bare file names.

## 0.2.0 (2026-10-29)

- Reflective floors in 3D scenes (Wallpaper Engine's `reflected` objects): Fantastic Car's floor now reflects the car and Audiophile's the clouds, as in Wallpaper Engine.
- Scene files with comments now load like in Wallpaper Engine: Fantastic Car (a default project) lost its whole car model to one commented-out line and now shows wheels, glass and interior.
- New pause rule action **Stop and free memory** (like Wallpaper Engine's "Stop (free memory)"): when it applies to every monitor a wallpaper shows on, the wallpaper is unloaded (video memory, web page, sound) and comes back when one of those monitors would play again. The Gaming preset now stops behind fullscreen apps instead of pausing. `diorama policy --on-fullscreen stop` sets it from the command line; `diorama status` shows stopped monitors.
- Web wallpapers now follow the frame rate cap (Settings → When to pause), like scenes. They used to draw at the monitor's refresh rate: a 180 Hz display ran pages six times as often as the default 30 fps.

## 0.1.1 (2026-10-29)

- **Renamed from Lumen to Diorama** (the old name clashed with Unreal Engine's Lumen). Programs, folders and the installer now use the new name: `diorama`, `dioramad`, `%APPDATA%\Diorama`, `%LOCALAPPDATA%\Diorama`. If Lumen 0.1.0 is installed, the Diorama installer removes it, and its settings move over the first time Diorama starts.
- Example wallpapers made for Diorama, free to use (`examples/wallpapers`).

## 0.1.0 (2026-10-28, released as Lumen)

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

### The Diorama app

- Library with search, filters, mature-content hiding and a measured cost per wallpaper.
- Per-wallpaper settings, starting from your Wallpaper Engine choices.
- Playlists by interval or time of day, per monitor.
- Notification-area icon; start at sign-in; `diorama` command line.

### Installation

- Per-user Windows installer with in-app updates (checked only when you ask).
