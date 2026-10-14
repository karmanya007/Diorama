#!/bin/bash
# Renders every captured scene in <captures>/manifest.json at the capture's
# g_Time (and recorded cursor) with `lumend --frame-out` and compares it with
# WE's frame using imgdiff, fitted the way WE presented it: the wallpaper's
# WE alignment from <captures>/we-config.json (the WE config at capture time)
# — 2 = stretch, absent = cover; 4 (custom zoom/pan) is not reproduced and is
# reported as "custom".
# Captures and WE content stay local; nothing here is committed.
# Usage: CAPTURES=scratch/captures tools/refcap/golden.sh
#        (after `cargo build --release -p lumend -p imgdiff`)
set -euo pipefail
caps=${CAPTURES:-scratch/captures}
export LUMEN_DXC=${LUMEN_DXC:-$PWD/.tools/dxc/dxc.exe}

python - "$caps" <<'PY' | tr -d '\r' | while IFS='|' read -r frame scene time pointer fit; do
import json, sys, pathlib
caps = pathlib.Path(sys.argv[1])
alignments = {}
try:
    config = json.load(open(caps / "we-config.json", encoding="utf-8"))
    for user in config.values():
        if isinstance(user, dict):
            for path, props in user.get("wproperties", {}).items():
                monitor = props.get("Monitor0", {})
                alignments[pathlib.PurePosixPath(path).parent.name] = monitor.get("alignment")
except OSError:
    pass
for entry in json.load(open(caps / "manifest.json")):
    rdc = pathlib.Path(entry["capture"]).stem            # we_frame289
    data = json.load(open(caps / f"{rdc}.json"))
    t = next((cb["g_Time"][0] for a in data["actions"]
              for k in ("vs_cbuffers", "ps_cbuffers")
              for cb in (a.get(k) or {}).values() if "g_Time" in cb), 0.0)
    # Cursor (if recorded) as a fraction of the captured swapchain: a
    # 3000x1920 swapchain spans this machine's desktop from (-1080, -546).
    sw, sh = data["swapchains"][0]["size"]
    ox, oy = (-1080, -546) if (sw, sh) == (3000, 1920) else (0, 0)
    cx, cy = entry.get("cursor", [ox + sw / 2, oy + sh / 2])
    scene = pathlib.Path(entry["scene"]).parent
    fit = {2: "stretch", 4: "custom"}.get(alignments.get(scene.name), "cover")
    print(rdc, scene, t, f"{(cx - ox) / sw},{(cy - oy) / sh}", fit, sep="|")
PY
  out="$caps/$frame.lumen.png"
  ./target/release/lumend --scene "$scene" --frame-out "$out" --time "$time" --pointer "$pointer" >/dev/null 2>&1
  echo -n "$frame $(basename "$scene") [$fit]: "
  mode=--cover
  [ "$fit" = stretch ] && mode=--stretch
  ./target/release/imgdiff "$caps/$frame.final.png" "$out" $mode --side-by-side "$caps/$frame.sbs.png"
done
