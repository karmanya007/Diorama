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
- **When the desktop goes away** (its connection fails for good) the daemon
  replaces itself with a fresh process, which waits up to 10 s for a
  compositor under the same name and plays the saved wallpapers, or quits.
  Starting over leaves no GPU or toolkit state tied to the dead connection
  to repair. A run shorter than 5 s does not try.
- **After sleep** (logind's `PrepareForSleep(false)`) the GPU is set up
  afresh: some drivers lose video memory without reporting a lost device.
- **Priority** is nice 5, inherited by threads and processes started later.
  Vulkan's global priority is not reachable through wgpu.
- **The control socket** is `$XDG_RUNTIME_DIR/diorama.sock` (without it,
  `~/.cache/diorama`), where other users cannot connect; a file lock beside
  it keeps a second daemon out and tells a live socket from one a killed
  daemon left. The abstract socket used before had no access control.
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
  Windows without a notification area, and the icon is registered whenever
  a host's bus name appears.
- **Scenes** use the same DXC release (`scripts/fetch-dxc.sh`) and the
  Wallpaper Engine install Steam keeps for Proton, found through Steam's
  library folders. WE's `config.json` names wallpapers by Windows or Wine
  paths (`C:/…`, `Z:/home/…`); entries match from `steamapps/` on, so the
  user's choices apply whichever system reads them. Headless frames
  (`--frame-out`) need no desktop. Corpus goldens on lavapipe equal
  Windows' (mean SSIM 0.9369 on both over 53 captures, largest difference
  0.022) when run in the same time zone, which time-of-day scenes follow.
- **Web wallpapers in WebKitGTK**: wry cannot put WebKitGTK into a raw
  Wayland surface, so the web host makes its own GTK windows background
  layer surfaces with `gtk-layer-shell` (created hidden and shown when the
  page has painted, as on Windows) and runs the GTK main loop; Diorama's
  Wayland connection still reports outputs and their changes. The pages
  get real pointer input there, so dioramad forwards none. Network off is
  a proxy that is not there, set on the page's data manager before it
  runs, as with WebView2. The `diorama:` scheme handler lives in the web
  context the pages share and is registered once. Not on Linux: answers
  for the PerformanceMonitor companion (WebKitGTK cannot intercept
  `http://127.0.0.1` requests), and `--snapshot` of pages that animate
  with CSS transitions (WebKit throttles an off-screen window's page).
- **No OS fallback wallpaper (P14) yet**: there is no common API. A
  wallpaper client like `swaybg` would take a background-layer surface that
  can stack above Diorama's, and writing GNOME's or Plasma's own wallpaper
  setting is a lasting change to the desktop that needs testing on those
  desktops first.
- **X11 when there is no Wayland session** (`WAYLAND_DISPLAY` unset;
  `DIORAMA_DISPLAY=x11|wayland` overrides), through x11rb over libxcb so
  Vulkan gets an XCB surface. Outputs from RandR, identity from the EDID
  (manufacturer, product, serial, connector). Wallpapers are EWMH desktop
  windows (`_NET_WM_WINDOW_TYPE_DESKTOP`, below, sticky, on every
  workspace) when the window manager lists that type in `_NET_SUPPORTED`;
  otherwise (i3, no window manager) override-redirect windows the window
  manager never sees, kept at the bottom of the stack and raised only above
  Diorama's other windows. Coverage reads `_NET_CLIENT_LIST`,
  `_NET_WM_STATE` and `_NET_WM_DESKTOP`: a window the window manager calls
  fullscreen covers its output even when it is a few pixels short
  (terminals size in whole characters), and one filling 85 % of its output
  counts as maximized, since tiling window managers have no maximize. The
  cursor comes from `QueryPointer` (X11 tells any client), presses from
  XInput 2.2 raw events, which arrive even while the window manager grabs
  the pointer. Web wallpapers on X11 are GTK popup windows lowered to the
  bottom; they get the pointer themselves on both display systems, so
  dioramad forwards none on Linux. Desktop icon programs (xfdesktop,
  pcmanfm) draw their own desktop window, which may hide Diorama's.
- **GNOME through a GNOME Shell extension** (`gnome/`, GNOME 46+). Mutter
  has no layer-shell, so dioramad opens xdg-shell windows (app id
  `io.github.karmanya007.diorama.wallpaper`, title
  `Diorama wallpaper|<connector>`) and the extension keeps each one on its
  monitor, lowered below every window after each restack, on every
  workspace, never focused (GNOME raises what it focuses), and out of the
  overview, its thumbnails, Alt+Tab and the dash. Mutter keeps windows out
  of the top bar's strip, so the window actor is translated up under it
  (input follows the actor). GNOME tells applications nothing about other
  windows, so the extension answers `Coverage()` on GNOME Shell's bus name
  (`/io/github/karmanya007/Diorama`), per connector; dioramad refuses to
  start without it rather than open windows that would cover the desktop.
  The overview still shows GNOME's own wallpaper behind workspaces.

## Consequences

- Development and tests run in a headless Sway (no window on the host):
  `grim` captures each virtual output, `swaymsg create_output` hotplugs one
  and a fullscreen terminal checks pausing per output. Headless Sway has no
  input devices; a small virtual-pointer client (`wlr-virtual-pointer`)
  drives the cursor.
- On GNOME the Wayland backend fails at start with a message naming the
  missing layer-shell until the extension exists.
- Two identical monitors swapped between ports swap their wallpapers.
