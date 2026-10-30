#!/usr/bin/env bash
# Writes the winget manifests (version, installer, en-US locale) of a
# published release into <out>/manifests/k/karmanya007/Diorama/<version>/,
# the layout of microsoft/winget-pkgs, and checks them with `winget validate`
# when winget is available.
#
#   scripts/winget-manifest.sh [version] [out]   # defaults: Cargo.toml version, target/winget
#
# The installer is downloaded from the GitHub release to take its SHA256, so
# run this after the release workflow has published the version. Submitting
# the manifests to winget-pkgs is a pull request made by the maintainer.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
version="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)}"
out="${2:-$root/target/winget}"

id=karmanya007.Diorama
repo=https://github.com/karmanya007/Diorama
url="$repo/releases/download/v$version/Diorama_${version}_x64-setup.exe"
schema=1.12.0

# The release date from the changelog heading `## <version> (YYYY-MM-DD)`.
release_date="$(sed -n "s/^## ${version//./\\.} (\([0-9-]*\)).*/\1/p" "$root/CHANGELOG.md" | head -n1)"

echo "Downloading $url"
sha="$(curl -fsSL "$url" | sha256sum | cut -d' ' -f1 | tr a-f A-F)"

dir="$out/manifests/k/karmanya007/Diorama/$version"
mkdir -p "$dir"

# manifest <file> <type>, body on stdin.
manifest() {
  {
    printf '# yaml-language-server: $schema=https://aka.ms/winget-manifest.%s.%s.schema.json\n\n' "$2" "$schema"
    printf 'PackageIdentifier: %s\nPackageVersion: %s\n' "$id" "$version"
    cat
    printf 'ManifestType: %s\nManifestVersion: %s\n' "$2" "$schema"
  } >"$dir/$1"
}

manifest "$id.yaml" version <<EOF
DefaultLocale: en-US
EOF

# Per-user NSIS installer (no administrator rights); `/S` installs silently.
# The uninstall entry is HKCU\...\Uninstall\Diorama.
manifest "$id.installer.yaml" installer <<EOF
InstallerType: nullsoft
Scope: user
InstallModes:
- silent
- silentWithProgress
UpgradeBehavior: install
ProductCode: Diorama
${release_date:+ReleaseDate: $release_date
}MinimumOSVersion: 10.0.17763.0
Installers:
- Architecture: x64
  InstallerUrl: $url
  InstallerSha256: $sha
EOF

manifest "$id.locale.en-US.yaml" defaultLocale <<EOF
PackageLocale: en-US
Publisher: Diorama
PublisherUrl: $repo
PublisherSupportUrl: $repo/issues
PrivacyUrl: $repo/blob/main/PRIVACY.md
PackageName: Diorama
PackageUrl: $repo
License: MIT OR Apache-2.0
LicenseUrl: $repo#license
Copyright: Diorama contributors
ShortDescription: Live wallpapers for Windows, including Wallpaper Engine content you already have.
Description: |-
  Diorama plays scene, video, web and image wallpapers behind your desktop icons, per monitor or spanning all of them.
  It plays the Wallpaper Engine scenes, videos and web wallpapers you already have, pauses or unloads them behind games and fullscreen apps, and has no telemetry or accounts.
  Scenes need an installed copy of Wallpaper Engine for its built-in shaders; videos, web wallpapers and images do not.
Moniker: diorama
Tags:
- desktop
- live-wallpaper
- wallpaper
- wallpaper-engine
ReleaseNotesUrl: $repo/releases/tag/v$version
EOF

echo "Manifests in $dir"
if command -v winget >/dev/null; then
  winget validate --manifest "$dir"
fi
