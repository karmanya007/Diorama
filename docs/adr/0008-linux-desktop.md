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
  The background surface receives pointer events while the cursor is over
  bare desktop, which is when Wallpaper Engine reacts to it (planned).
- **Event loop**: the Wayland connection and a socket-pair waker are polled
  together, so other threads wake `poll_events` as on Windows.
- **Later in Phase 7**: GStreamer for video (system-provided, picks VA-API
  hardware decoding itself, delivers NV12 like Media Foundation), PipeWire
  for the audio loopback, MPRIS for media information, UPower and logind over
  D-Bus for battery and lock, WebKitGTK with `gtk-layer-shell` for web
  wallpapers, X11 (`_NET_WM_WINDOW_TYPE_DESKTOP`), and a GNOME Shell
  extension last.

## Consequences

- Development and tests run in a headless Sway (no window on the host):
  `grim` captures each virtual output, `swaymsg create_output` hotplugs one
  and a fullscreen terminal checks pausing per output.
- On GNOME the Wayland backend fails at start with a message naming the
  missing layer-shell until the extension exists.
- Two identical monitors swapped between ports swap their wallpapers.
