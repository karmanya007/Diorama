#!/bin/bash
# Lists the distribution packages an AppImage carries, with where their exact
# source is, followed by the licences they come under (some are GPL, such as
# FFmpeg as Debian and Ubuntu build it). Published beside the AppImage.
#
# scripts/appimage-sources.sh <AppDir> > sources.txt
# Run where the AppImage was built: the packages are the host's.
set -euo pipefail

appdir=${1:?AppDir}
release=$(. /etc/os-release && echo "$PRETTY_NAME")

cat <<EOF
Third-party software in Diorama's AppImage
==========================================

Diorama is MIT or Apache-2.0 (https://github.com/karmanya007/Diorama). Its
AppImage also carries the unmodified libraries and plugins listed here, from
these $release packages. The complete corresponding source of each, with
Ubuntu's build files, is at the address beside it. If one is not available
there, open an issue at https://github.com/karmanya007/Diorama/issues.

The packages' copyright notices are in usr/share/doc inside the AppImage
(--appimage-extract shows them); the licence texts they refer to follow the
list. DirectX Shader Compiler (usr/lib/Diorama/dxc) is from
https://github.com/microsoft/DirectXShaderCompiler and has its licences
beside it.

EOF

# Library paths differ between the AppDir and the system, names do not.
index=$(mktemp)
trap 'rm -f "$index"' EXIT
for list in /var/lib/dpkg/info/*.list; do
    package=$(basename "$list" .list)
    awk -v p="${package%%:*}" -F/ '/\.so/ { print $NF "\t" p }' "$list"
done | sort -u > "$index"

find "$appdir" -name '*.so*' \( -type f -o -type l \) -printf '%f\n' | sort -u |
    join -t $'\t' - "$index" | cut -f2 | sort -u |
    while read -r package; do
        dpkg-query -W -f '${Package}\t${Version}\t${source:Package}\t${source:Version}\n' "$package" 2>/dev/null || true
    done | sort -u |
    while IFS=$'\t' read -r package version source source_version; do
        echo "$package $version"
        echo "    https://launchpad.net/ubuntu/+source/$source/$source_version"
    done

for licence in GPL-3 GPL-2 LGPL-3 LGPL-2.1 LGPL-2 MPL-2.0 Apache-2.0; do
    printf '\n\n%s\n%s\n\n' "$licence" "$(printf '%.0s=' $(seq ${#licence}))"
    cat "/usr/share/common-licenses/$licence"
done
