# WE settings model (`config.json`) — RE notes

Source: `<install>/config.json` (structure only; values are personal and not recorded). Useful for (a) designing Diorama's settings, (b) an optional "import my WE setup" feature.

```text
"?installdirectory": string
"<steam user name>": {
  "general": {
    "browser":  { filters, folders, authorblocklistnames, explore, results per page, … }
    "editor":   { autosave, grid, camera speeds, recentfiles, savedlayouts, … }
    "shared":   { hotkeys[], slideshowwarningenabled }
    "user": {                         // runtime/playback settings
      playbackfullscreen, playbackmaximized, playbackfocus, playbackonbattery,
      playbacksleep, playbackaudio                      // pause/stop/keep-running policies
      fps, msaa, postprocessing, reflection, resolution, preset,
      processpriority, highprecisiontimer, pausevram,
      audioinputdevice, audioinputthreshold, audioinputvolume, reloadaudio,
      mediaintegration, mediablocklist,
      monitordetection, monitormap,                     // monitor identity (see P8)
      iconopacity, adjustdwmcolormode, overridelockscreen, overridewallpaper,
      autostart, autostartscheduler, autostartx64, anticrash, apprules,
      plugins, plugindelay, cefcommandline, language, loglevel, …
    }
  }
  "wallpaperconfig":         { layout, profile, selectedwallpapers }   // per-monitor selection
  "wallpaperconfigrecent":   [10 × previous configs]
  "wallpaperconfigscreensaver": { layout, sameaswallpaper }
}
"version": number
"wproperties": {                      // user-property overrides
  "<absolute path to project file or video>": { "Monitor0": { … } }
}
```

## Takeaways for Diorama
- WE keys user-property overrides **by absolute path + monitor index** (`Monitor0`) — fragile when monitors are renumbered (pain point P8). Diorama keys them by wallpaper id + stable output id.
- Pause policies are a flat set of per-condition enums (`playbackfullscreen`, `playbackmaximized`, …). Diorama's per-output policy presets (P17) should cover each of these conditions.
- `apprules` = per-application overrides (pause/stop when app X runs) — worth keeping.
- `monitordetection` option exists because display indices change (P8); WE offers it as a setting, Diorama should make stable identity the default.
