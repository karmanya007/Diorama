#!/bin/bash
# Builds Diorama's Linux packages (.deb and AppImage) into
# target/release/bundle/: diorama-ui with dioramad, diorama-webhost, the
# `diorama` CLI and DXC (as resources). The Linux counterpart of
# scripts/bundle.ps1.
#
# With TAURI_SIGNING_PRIVATE_KEY (and _PASSWORD) set, the AppImage is also
# signed (`.AppImage.sig`) for the in-app updater; without them the
# packages are built alone (local test builds). The .deb updates through
# the system's package tools instead.
#
# Needs patchelf and GStreamer's plugins (good, bad, libav: the AppImage
# bundles what is installed here) besides the build packages in the README.
#
# SKIP_FRONTEND=1 uses an apps/diorama-ui/dist built elsewhere (for hosts
# whose Node.js is too old for Vite).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
ui="$root/apps/diorama-ui"
bins="$ui/src-tauri/binaries"

bash "$root/scripts/fetch-dxc.sh"

cd "$root"
cargo build --release --locked -p dioramad -p diorama-webhost -p diorama-cli

# Tauri's externalBin wants each program named for its target triple.
triple=$(rustc -vV | sed -n 's/^host: //p')
mkdir -p "$bins"
for name in dioramad diorama-webhost diorama; do
    cp "target/release/$name" "$bins/$name-$triple"
done

cd "$ui"
if [ -n "${CI:-}" ]; then npm ci; else npm install; fi
config=(--config src-tauri/tauri.bundle.linux.conf.json)
if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
    config+=(--config '{"bundle":{"createUpdaterArtifacts":true}}')
else
    echo "TAURI_SIGNING_PRIVATE_KEY not set: building without update files"
fi
if [ -n "${SKIP_FRONTEND:-}" ]; then
    [ -f dist/index.html ] || { echo "SKIP_FRONTEND: apps/diorama-ui/dist is missing" >&2; exit 1; }
    config+=(--config '{"build":{"beforeBuildCommand":""}}')
fi
npx tauri build "${config[@]}"

# The AppImage uses no plugins but its own: without these, videos (H.264,
# AAC) do not play and nothing can be converted.
appimage="$root/target/release/bundle/appimage"
for plugin in libav isomp4 playback audioparsers videoparsersbad openh264 pulseaudio va nvcodec; do
    [ -f "$appimage/Diorama.AppDir/usr/lib/gstreamer-1.0/libgst$plugin.so" ] ||
        { echo "the AppImage lacks GStreamer's $plugin plugin: install the GStreamer packages named above" >&2; exit 1; }
done
bash "$root/scripts/appimage-sources.sh" "$appimage/Diorama.AppDir" > "$appimage/Diorama_AppImage_sources.txt"
ls -la "$root/target/release/bundle/deb" "$root/target/release/bundle/appimage" 2>/dev/null
