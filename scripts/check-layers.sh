#!/usr/bin/env bash
# Enforces the architecture rules of ADR-002 (docs/adr):
#   1. Lower layers never depend on higher ones; renderer/runtime never see WE formats.
#   2. OS-specific code (cfg(windows) etc.) lives only in diorama-platform and diorama-media.
# Crates that do not exist yet are skipped, so the rules can be written ahead of time.
set -euo pipefail

# crate:space-separated crates it must never depend on (directly or transitively)
rules=(
  "diorama-core:diorama-scene diorama-assets diorama-ipc diorama-we diorama-gpu diorama-runtime diorama-render diorama-platform"
  "diorama-text:diorama-scene diorama-assets diorama-ipc diorama-we diorama-gpu diorama-runtime diorama-render diorama-platform"
  "diorama-scene:diorama-assets diorama-ipc diorama-we diorama-gpu diorama-runtime diorama-render diorama-platform"
  "diorama-assets:diorama-we diorama-gpu diorama-runtime diorama-render diorama-platform"
  "diorama-ipc:diorama-we diorama-gpu diorama-runtime diorama-render diorama-platform"
  "diorama-library:diorama-we diorama-gpu diorama-runtime diorama-render diorama-platform diorama-media"
  "diorama-we:diorama-gpu diorama-runtime diorama-render diorama-platform"
  "diorama-gpu:diorama-we diorama-runtime diorama-render diorama-platform"
  "diorama-runtime:diorama-we diorama-platform"
  "diorama-particles:diorama-we diorama-gpu diorama-render diorama-platform diorama-media diorama-script"
  "diorama-script:diorama-we diorama-gpu diorama-render diorama-platform diorama-media"
  "diorama-render:diorama-we diorama-runtime diorama-platform diorama-media"
  "diorama-media:diorama-we diorama-gpu diorama-runtime diorama-render diorama-platform"
  "diorama-platform:diorama-scene diorama-we diorama-gpu diorama-runtime diorama-render"
)

members=$(cargo metadata --no-deps --format-version 1 | grep -o '"name":"diorama-[a-z-]*"' | cut -d'"' -f4 | sort -u)
fail=0

for rule in "${rules[@]}"; do
  crate=${rule%%:*}
  forbidden=${rule#*:}
  grep -qx "$crate" <<<"$members" || continue
  deps=$(cargo tree -p "$crate" -e normal --prefix none --format '{p}' | awk '{print $1}' | sort -u)
  for f in $forbidden; do
    if grep -qx "$f" <<<"$deps"; then
      echo "layer violation: $crate depends on $f"
      fail=1
    fi
  done
done

# OS-specific cfgs outside the OS-facing crates (tools/ may hard-code paths but not cfg).
violations=$(grep -rnE 'cfg\((.*[^_])?(windows|unix|target_os)' crates --include='*.rs' | grep -vE '^crates/diorama-(platform|media)/' || true)
if [[ -n "$violations" ]]; then
  echo "OS-specific cfg outside diorama-platform/diorama-media:"
  echo "$violations"
  fail=1
fi

if [[ $fail -eq 0 ]]; then
  echo "layer rules ok"
fi
exit $fail
