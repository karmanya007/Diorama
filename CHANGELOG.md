# Changelog

All notable changes to Diorama are listed here. Versions follow [Semantic Versioning](https://semver.org); before 1.0, minor versions may change behaviour.

## Unreleased

- Scenes in which nothing moves are not drawn again and again, and the system's sound is not captured for scenes that do not react to it.

## 1.2.0 (2026-11-25)

- Presets (Monitors page, `diorama preset`): save what every monitor shows, playlists included, under a number from 1 to 9 and bring it back. Wallpapers that already show where the preset wants them keep running. Global hotkeys (Ctrl+Alt and the number by default; off until turned on) work on Windows and X11; on Wayland, bind keys to `diorama preset 1` and so on in the desktop's settings. Asked for in #2.
- A time-of-day playlist shows what is due when Diorama starts; it kept the wallpaper from last time until the next change.

## 1.1.2 (2026-11-25)

### Linux

- Security: the control socket moved from the abstract namespace, where any user of the same machine could connect to it and control Diorama, to the user's own runtime folder. Affects 1.1.0 and 1.1.1 on Linux. A running older daemon is asked to quit when the new one starts.
- The AppImage plays videos: it was built without the plugins that decode H.264, HEVC and AAC, and uses no others. It now carries FFmpeg (GPL as Ubuntu builds it) and GStreamer's hardware decoders; `Diorama_AppImage_sources.txt` beside it lists every bundled package with its source.
- When the desktop goes away (logging out, a compositor or X server crash), `dioramad` no longer stays behind using a whole processor core and filling its log. It waits up to 10 seconds for the desktop, brings the wallpapers back if it returns, and quits otherwise, so the next sign-in can start it.
- Videos the graphics card cannot decode can be converted on Linux too, with the graphics card's encoder (VA-API, NVENC) when GStreamer has it, else x264 or OpenH264.
- The tray icon appears when the panel starts after Diorama, and again when the panel restarts.
- The `.deb` names the libraries it needs (ALSA, D-Bus, OpenSSL, Vulkan, XCB) and recommends the plugins for converting videos.
- GNOME: the overview's workspaces show the wallpaper, not GNOME's own picture.
- After sleep the GPU is set up afresh, since some drivers lose video memory.
- `dioramad` runs at a lower priority than other programs, as on Windows.

## 1.1.1 (2026-11-24)

### Fixes

- Scenes on Windows whose shaders add values up in a loop drew them wrong: the loop's running values were updated in the wrong order (a bug in the shader translator Diorama's DirectX 12 path uses). Neon Sunset's mountains came out at half their height; they now match Wallpaper Engine's.
- Web wallpapers on Windows that load scripts, data or images over `http://` (often older ones) now get them when their network access is on, as in Wallpaper Engine; they were blocked as mixed content and could stay blank (#2).

## 1.1.0 (2026-11-20)

Diorama comes to Linux as a preview, and Wallpaper Engine scenes work again when Steam is not installed on `C:`.

### Linux (preview)

Diorama runs on Linux: a `.deb` and an AppImage come with each release.

- Wallpapers on Wayland desktops with layer-shell (Sway, Hyprland, KDE Plasma, niri, COSMIC), on GNOME 46+ with Diorama's GNOME Shell extension (installed by the `.deb`; a zip for AppImage users), and on X11 (window managers with EWMH desktop windows, and others such as i3).
- Every wallpaper kind: Wallpaper Engine scenes (rendered the same as on Windows in our comparisons), videos through GStreamer (hardware decoding with VA-API or NVDEC when installed), web wallpapers in WebKitGTK, images.
- Sound and audio-reactive wallpapers through PipeWire or PulseAudio, now-playing information from MPRIS, the cursor on the desktop, per-monitor pausing behind fullscreen and maximized windows, battery and lock-screen rules, the tray icon, autostart and the command line.
- The AppImage updates itself from Settings → Updates; the `.deb` updates with your system's package tools.
- Not yet on Linux: converting videos, the OS fallback wallpaper, and the figures web widgets ask the PerformanceMonitor app for.

### Fixes

- Wallpaper Engine scenes showed only a grey (or single-colour) screen when Steam or Wallpaper Engine is installed somewhere other than `C:\Program Files (x86)\Steam` ([#2](https://github.com/karmanya007/Diorama/issues/2)): Diorama now finds Wallpaper Engine in any of Steam's library folders, and if it cannot find it, refuses the scene with a message saying so instead of showing an empty one.
- Audio playback and capture no longer reopen the system's audio connection every few seconds to check for a changed default device.

## 1.0.0 (2026-10-30)

First stable release for Windows. From 1.0 on, settings, the `diorama` command line and the folders Diorama uses stay compatible until 2.0.

- Diorama's background programs (`dioramad`, `diorama-webhost`, `diorama`) now carry their name, version and icon, so Task Manager lists them as "Diorama wallpaper daemon", "Diorama web wallpaper host" and "Diorama command line" instead of bare file names.
- Videos that Windows cannot decode at all (such as 10-bit H.264) are now refused with a clear reason, and the wallpaper on screen stays. They used to show a blank wallpaper reported as playing, and offered a conversion that could only fail.

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
