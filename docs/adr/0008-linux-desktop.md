# ADR-008: Linux desktop integration

Status: accepted (2026-11-01)

## Context

Phase 7 ports Diorama to Linux. Everything above `diorama-platform` and
`diorama-media` already builds and passes its tests there; what is missing
is putting wallpapers behind the desktop, knowing what covers each monitor,
video and audio, and the web host. Linux desktops differ more than Windows
versions do: most Wayland compositors (Sway, Hyprland, KDE, niri, COSMIC)
implement `wlr-layer-shell`, GNOME does not, and X11 sessions remain.

## Decision

- **Wayland first, through `wlr-layer-shell`** (`smithay-client-toolkit`):
  one layer surface per output on the background layer, anchored to every
  edge, no keyboard input, namespace `diorama`. Vulkan draws to it through
  the `wl_surface` handle like any window.
  - No stacking API between surfaces of one layer is needed: a layer surface
    appears only with its first buffer, so a replaced wallpaper stays
    visible until the new one has drawn (Windows needs `create_wallpaper_below`
    and `raise` for that).
  - Output identity: make, model and connector name (`DP-1`). Wayland
    exposes no EDID serial; the connector keeps identities apart when two
    identical monitors are attached.
  - Positions and sizes are logical values times the output's integer scale.
    Fractional scaling is left to the compositor for now.
- **What covers an output** comes from `wlr-foreign-toplevel-management`:
  windows the compositor reports fullscreen or maximized on that output,
  minimized ones excluded. Compositors without it (GNOME) never pause on
  windows; battery and lock rules still apply.
- **The cursor**: Wayland clients never see the global cursor position.
  The wallpaper surfaces receive pointer events while the cursor is over
  bare desktop, which is when Wallpaper Engine reacts to it. Positions map
  to virtual-desktop pixels through each surface's output; the last one
  stays when the cursor goes over a window. Left-button presses and
  releases are queued as on Windows, and a drag that starts on the desktop
  keeps reporting until release (Wayland's implicit grab). Entering a
  wallpaper sets the default arrow (`cursor-shape`, else the cursor theme).
- **Event loop**: the Wayland connection and a socket-pair waker are polled
  together, so other threads wake `poll_events` as on Windows.
- **Battery** from `/sys/class/power_supply` (what UPower reads): on battery
  when no mains or USB supply is online and a system battery discharges;
  device batteries (mice, headsets) do not count. Polled like on Windows.
- **Lock** from logind on the system bus (zbus, own thread): the session's
  `Lock`/`Unlock` signals and its `LockedHint`. The session is ours
  (`XDG_SESSION_ID`, else our process), else the user's graphical session,
  else the user's first: a daemon started by a systemd user unit belongs to
  none. A lock that sends neither (`swaylock` run directly) goes unnoticed; their
  lock screen hides the wallpaper anyway.
- **Video through the system's GStreamer** (gstreamer-rs, LGPL libraries
  linked dynamically): `decodebin` picks the decoder by rank, so VA-API,
  NVDEC or V4L2 hardware decoders win when their plugins are installed and
  libav decodes on the CPU otherwise; `videoconvert` hands NV12 to the same
  upload path as Media Foundation. Only the video pad is linked (P10).
  Videos inside packages come through a seekable `appsrc`. Looping rebuilds
  the pipeline, like the Windows decoder reopens its reader. The probe
  reports the chosen decoder (hardware from its class) and names streams
  no plugin decodes. Converting (P4) waits: the encoders a distribution
  ships vary, and CPU decoding of every codec is the common case there.
- **Sound through cpal's PulseAudio host** (the protocol in pure Rust, so
  no client library; PipeWire's `pipewire-pulse` serves it on current
  desktops; plain ALSA otherwise). The player (playlists, gaps, ring
  buffer, resampling) is shared with Windows; only the track reader is per
  platform, GStreamer decoding to interleaved float here. Audio-reactive
  wallpapers record the default sink's `.monitor` source, which is what
  WASAPI loopback gives on Windows. Tests run in a private PipeWire whose
  only sink is a null sink, so nothing is heard.
- **Media information from MPRIS** (`org.mpris.MediaPlayer2.*` on the
  session bus), polled like the Windows media session. Several players can
  run; wallpapers follow one that plays (keeping the one they followed while
  it still plays), else the last one followed. Album art comes only from
  `file://` URLs: remote art would be a network request Diorama does not
  make on its own.
- **Session**: autostart is an XDG autostart entry
  (`~/.config/autostart/diorama.desktop`, which desktops list in their
  startup settings); `g_Daytime` uses the local time zone (`localtime_r`);
  Steam is also looked for in its Flatpak's folder.
- **Load figures from `/proc` and sysfs**: CPU per processor
  (`/proc/stat`), memory (`MemAvailable`), network from interfaces with a
  device behind them, process trees from `/proc/<pid>/stat` parents. GPU
  load and memory only where the driver publishes them in sysfs (amdgpu);
  per-process GPU figures stay 0. Disks are left out: the wallpaper API
  names them by Windows drive letter.
- **Tray icon through StatusNotifierItem** with a `com.canonical.dbusmenu`
  menu, served by zbus (the `ksni` crate's licence is outside the allowed
  list). KDE, waybar and most panels host it; GNOME needs the AppIndicator
  extension; without a host the icon is skipped with a log line, as on
  Windows without a notification area.
- **No OS fallback wallpaper (P14) yet**: there is no common API. A
  wallpaper client like `swaybg` would take a background-layer surface that
  can stack above Diorama's, and writing GNOME's or Plasma's own wallpaper
  setting is a lasting change to the desktop that needs testing on those
  desktops first.
- **Later in Phase 7**: WebKitGTK with `gtk-layer-shell` for web
  wallpapers, X11 (`_NET_WM_WINDOW_TYPE_DESKTOP`), and a GNOME Shell
  extension last.

## Consequences

- Development and tests run in a headless Sway (no window on the host):
  `grim` captures each virtual output, `swaymsg create_output` hotplugs one
  and a fullscreen terminal checks pausing per output. Headless Sway has no
  input devices; a small virtual-pointer client (`wlr-virtual-pointer`)
  drives the cursor.
- On GNOME the Wayland backend fails at start with a message naming the
  missing layer-shell until the extension exists.
- Two identical monitors swapped between ports swap their wallpapers.
