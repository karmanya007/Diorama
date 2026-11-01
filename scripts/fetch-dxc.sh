#!/bin/bash
# Downloads the pinned DirectX Shader Compiler release for Linux into
# .tools/dxc (gitignored) and verifies its SHA-256 (the digest GitHub lists
# for the asset). Same release as scripts/fetch-dxc.ps1. Diorama finds
# .tools/dxc/bin/dxc in development builds, or through DIORAMA_DXC.
# The release keeps libdxcompiler.so in ../lib next to bin/, as dxc expects.
set -euo pipefail

tag=v1.9.2607
asset=linux_dxc_2026_07_29.x86_x64.tar.gz
sha256=55665c87824051ed4774ff3280a79ccbbb7d39243b9736ca5e98222134112d54

root=$(cd "$(dirname "$0")/.." && pwd)
dest="$root/.tools/dxc"
if [ -x "$dest/bin/dxc" ] && [ -f "$dest/lib/libdxcompiler.so" ] && [ -f "$dest/LICENSE-LLVM.txt" ]; then
    echo "DXC already present in $dest"
    exit 0
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/$asset" "https://github.com/microsoft/DirectXShaderCompiler/releases/download/$tag/$asset"
echo "$sha256  $tmp/$asset" | sha256sum -c --quiet -
dir=${asset%.tar.gz}
# Only what Diorama runs (the archive is mostly static libraries) and the licences.
tar -xzf "$tmp/$asset" -C "$tmp" "$dir/bin/dxc" "$dir/lib/libdxcompiler.so" \
    "$dir/LICENSE-LLVM.txt" "$dir/LICENCE-MIT.txt" "$dir/LICENSE-MS.txt"
mkdir -p "$dest/bin" "$dest/lib"
cp "$tmp/$dir/bin/dxc" "$dest/bin/"
cp "$tmp/$dir/lib/libdxcompiler.so" "$dest/lib/"
cp "$tmp/$dir"/LICEN* "$dest/"
echo "DXC $tag installed in $dest"
