#!/usr/bin/env bash
# Enforces the architecture rules from docs/PLAN.md and ADR-002:
#   1. Lower layers never depend on higher ones; renderer/runtime never see WE formats.
#   2. OS-specific code (cfg(windows) etc.) lives only in lumen-platform.
# Crates that do not exist yet are skipped, so the rules can be written ahead of time.
set -euo pipefail

# crate:space-separated crates it must never depend on (directly or transitively)
rules=(
  "lumen-core:lumen-scene lumen-assets lumen-ipc lumen-we lumen-gpu lumen-runtime lumen-render lumen-platform"
  "lumen-scene:lumen-assets lumen-ipc lumen-we lumen-gpu lumen-runtime lumen-render lumen-platform"
  "lumen-assets:lumen-we lumen-gpu lumen-runtime lumen-render lumen-platform"
  "lumen-ipc:lumen-we lumen-gpu lumen-runtime lumen-render lumen-platform"
  "lumen-we:lumen-gpu lumen-runtime lumen-render lumen-platform"
  "lumen-gpu:lumen-we lumen-runtime lumen-render lumen-platform"
  "lumen-runtime:lumen-we lumen-platform"
  "lumen-render:lumen-we lumen-runtime lumen-platform"
  "lumen-platform:lumen-scene lumen-we lumen-gpu lumen-runtime lumen-render"
)

members=$(cargo metadata --no-deps --format-version 1 | grep -o '"name":"lumen-[a-z-]*"' | cut -d'"' -f4 | sort -u)
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

# OS-specific cfgs outside the platform crate (tools/ may hard-code paths but not cfg).
violations=$(grep -rnE 'cfg\((.*[^_])?(windows|unix|target_os)' crates --include='*.rs' | grep -v '^crates/lumen-platform/' || true)
if [[ -n "$violations" ]]; then
  echo "OS-specific cfg outside lumen-platform:"
  echo "$violations"
  fail=1
fi

if [[ $fail -eq 0 ]]; then
  echo "layer rules ok"
fi
exit $fail
