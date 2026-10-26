# ADR-005: Web wallpapers in a separate WebView process

Date: 2026-10-21 · Status: accepted

## Context

Wallpaper Engine web wallpapers are HTML pages (4 in the corpus) that talk to
WE through a small JavaScript API: `window.wallpaperPropertyListener`
(`applyUserProperties`, `applyGeneralProperties`, `setPaused`),
`wallpaperRegisterAudioListener` (128 levels about 30 times a second) and the
media-integration listeners. WE hosts them in CEF (`webwallpaper64.exe`).
Pages are untrusted Workshop content; some fetch from the network (one corpus
wallpaper calls a weather API, loads p5.js from a CDN and polls a local
service on `127.0.0.1`).

## Decision

- **`lumen-webhost`, one process per web wallpaper**, started by lumend. A
  page that crashes or leaks takes only its host down; lumend restarts it.
- **System WebView via `wry`**: WebView2 on Windows (shipped with Windows 11),
  WebKitGTK on Linux later. No bundled browser (P13).
- The host creates **its own wallpaper windows** with `lumen-platform` (the
  same WorkerW attachment as native wallpapers), one WebView per output, and
  recreates them on desktop resets. lumend creates no windows for web
  content; its session keeps per-output pause state only.
- **lumend drives the host over stdin, one JSON object per line**:
  `properties` (the user's WE choices, first line, before the page loads),
  `general`, `paused`, `audio`, `media` and `pointer`. The host exits when
  stdin closes, so it cannot outlive lumend. Audio capture, media sessions,
  cursor polling and the pause policy stay in lumend.
- **The WE API is our own script** (`apps/lumen-webhost/src/shim.js`), written
  from WE's public documentation, injected before page scripts. Pausing
  holds `requestAnimationFrame` callbacks, stops CSS animations, media
  elements and `AudioContext`s, and calls `setPaused`. The cursor is
  forwarded as synthetic DOM mouse events (the window behind the icons gets
  no input).
- **Files** come from a custom protocol (`lumen://<hash of folder>/…`, seen by
  the page as `https://lumen.<hash>/…`): paths stay inside the wallpaper
  folder, byte ranges are supported, and every wallpaper has its own origin
  (separate `localStorage`).
- **Network off by default.** All requests go to a proxy that does not exist,
  loopback included (`--proxy-server=http://127.0.0.1:9
  --proxy-bypass-list=<-loopback>`, WebRTC UDP disabled); `lumend
  --web-network` allows it. Top-level navigation is limited to the page's
  own origin and popups are denied.
- **One WebView2 profile per wallpaper and network mode.** WebView2 runs one
  browser per profile, and a browser still running from an earlier launch
  keeps that launch's arguments: with a shared profile a network-off page
  was handed a browser started with network on (tested).
- **`--snapshot`** renders a page off-screen and saves WebView2's own capture,
  so pages and the API are tested without touching the desktop.
- **Companion-app requests answered in the WebView.** Widgets that poll the
  third-party PerformanceMonitor app (`http://127.0.0.1:<port>/performance`)
  get the same JSON from Windows' own counters (`lumen_platform::SystemMonitor`:
  PDH CPU and GPU engines, memory status, fixed-drive free space, DXGI memory
  sizes, IP helper octets). A WebView2 resource filter answers before the
  network, so no port is opened and it works with the network off. Any page
  can read these load figures; they reveal no files or identities. The
  sampler runs only while a page asks (stops after 30 s idle).
- **Picked files through the wallpaper's origin.** Pages show files from
  `file` and `directory` properties as `'file:///' + value`, which Chromium
  refuses from an https page. The page script rewrites such addresses to
  `/__lumen_file__/<path>` on the page's own origin; the host serves only
  files the user picked in properties the project declares (directory
  properties: the folder's files of the declared `fileType`), so a page
  cannot read anything else on disk.
- **Problems reach the log.** The page script reports scripts and stylesheets
  that fail to load (with the `lumen set … --allow-network` hint when the
  network is off) and uncaught errors (debug level); a watchdog asks each
  page to run a trivial script every 5 s and warns when it has not answered
  for 10 s. Snapshots give up after 10 s instead of waiting on a stuck page.

## Consequences

- A second process and a WebView2 browser per web wallpaper (memory similar to
  WE's CEF host).
- Per-output pause is all-or-nothing for now (the host pauses every page).
- Per-wallpaper network permission, span layout (one page across monitors),
  user-chosen local files in properties, and WE's own colour adjustments
  (`wec_*`) are follow-ups.
