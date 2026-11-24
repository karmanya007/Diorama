#!/usr/bin/env bash
# Writes the AUR package `diorama-bin` (PKGBUILD and .SRCINFO) of a published
# release into <out>/diorama-bin/. It repackages the release's .deb, which
# uses the system's libraries.
#
#   scripts/aur-pkgbuild.sh [version] [out]   # defaults: Cargo.toml version, target/aur
#
# The files are downloaded from the release to take their SHA256, so run this
# after the release workflow has published the version. Pushing the result to
# aur.archlinux.org is done by the maintainer.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
version="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)}"
out="${2:-$root/target/aur}/diorama-bin"

repo=https://github.com/karmanya007/Diorama
raw=https://raw.githubusercontent.com/karmanya007/Diorama/v$version
desc='Live wallpapers: Wallpaper Engine content and your own'
deb="Diorama_${version}_amd64.deb"

# name::url, as makepkg names the downloads.
sources=(
  "$deb::$repo/releases/download/v$version/$deb"
  "LICENSE-MIT-$version::$raw/LICENSE-MIT"
  "LICENSE-APACHE-$version::$raw/LICENSE-APACHE"
)
depends=(alsa-lib dbus gst-plugins-base gst-plugins-good gtk3 gtk-layer-shell libxcb openssl vulkan-icd-loader webkit2gtk-4.1)
optdepends=(
  'gst-libav: H.264, HEVC and AAC decoding on the processor'
  'gst-plugin-va: hardware video decoding and conversion (AMD, Intel)'
  'gst-plugins-bad: hardware video decoding and conversion (NVIDIA), OpenH264'
  'gst-plugins-ugly: converting videos with x264'
  'vulkan-driver: scene, video and image wallpapers'
)

sums=()
for source in "${sources[@]}"; do
  echo "Downloading ${source#*::}" >&2
  sums+=("$(curl -fsSL "${source#*::}" | sha256sum | cut -d' ' -f1)")
done

mkdir -p "$out"

# quoted <indent> <items…>: one per line, single-quoted.
quoted() { local indent=$1; shift; printf "$indent'%s'\n" "$@"; }

{
  cat <<EOF
# Maintainer: karmanya007 <https://github.com/karmanya007>
pkgname=diorama-bin
pkgver=$version
pkgrel=1
pkgdesc='$desc'
arch=('x86_64')
url='$repo'
license=('MIT OR Apache-2.0' 'NCSA')
depends=(
$(quoted '  ' "${depends[@]}")
)
optdepends=(
$(quoted '  ' "${optdepends[@]}")
)
provides=('diorama')
conflicts=('diorama')
options=('!strip' '!debug')
source=(
$(quoted '  ' "${sources[@]}")
)
sha256sums=(
$(quoted '  ' "${sums[@]}")
)

EOF
  cat <<'EOF'
package() {
  bsdtar -xf data.tar.* -C "$pkgdir" --no-same-owner
  local licenses="$pkgdir/usr/share/licenses/$pkgname"
  install -Dm644 "LICENSE-MIT-$pkgver" "$licenses/LICENSE-MIT"
  install -Dm644 "LICENSE-APACHE-$pkgver" "$licenses/LICENSE-APACHE"
  install -Dm644 "$pkgdir/usr/lib/Diorama/dxc/LICENSE-LLVM.txt" "$licenses/LICENSE-DXC-LLVM"
  install -Dm644 "$pkgdir/usr/lib/Diorama/dxc/LICENCE-MIT.txt" "$licenses/LICENSE-DXC-MIT"
}
EOF
} >"$out/PKGBUILD"

# What `makepkg --printsrcinfo` prints.
{
  printf 'pkgbase = diorama-bin\n'
  field() { local key=$1; shift; printf "\t$key = %s\n" "$@"; }
  field pkgdesc "$desc"
  field pkgver "$version"
  field pkgrel 1
  field url "$repo"
  field arch x86_64
  field license 'MIT OR Apache-2.0' NCSA
  field depends "${depends[@]}"
  field optdepends "${optdepends[@]}"
  field provides diorama
  field conflicts diorama
  field options '!strip' '!debug'
  field source "${sources[@]}"
  field sha256sums "${sums[@]}"
  printf '\npkgname = diorama-bin\n'
} >"$out/.SRCINFO"

echo "Wrote $out" >&2
