#!/bin/bash
# Builds Diorama's Linux packages (.deb and AppImage) into
# target/release/bundle/: diorama-ui with dioramad, diorama-webhost, the
# `diorama` CLI and DXC (as resources). The Linux counterpart of
# scripts/bundle.ps1; no update files yet (the in-app updater is Windows
# only for now).
#
# Needs patchelf (the AppImage bundles GStreamer) besides the build
# packages in the README.
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
if [ -n "${SKIP_FRONTEND:-}" ]; then
    [ -f dist/index.html ] || { echo "SKIP_FRONTEND: apps/diorama-ui/dist is missing" >&2; exit 1; }
    config+=(--config '{"build":{"beforeBuildCommand":""}}')
fi
npx tauri build "${config[@]}"
ls -la "$root/target/release/bundle/deb" "$root/target/release/bundle/appimage" 2>/dev/null
