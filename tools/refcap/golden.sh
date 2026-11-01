#!/bin/bash
# Renders every captured scene in <captures>/manifest.json at the capture's
# g_Time (and recorded cursor) with `dioramad --frame-out`, presented into a
# frame the size of WE's swapchain with the wallpaper's own WE fit (read from
# <captures>/we-config.json, the WE config at capture time), and compares it
# with WE's frame using imgdiff. Scripts see the capture's wall clock (the
# .rdc file's modification time).
# Captures and WE content stay local; nothing here is committed.
# Usage: CAPTURES=scratch/captures tools/refcap/golden.sh
#        (after `cargo build --release -p dioramad -p imgdiff`)
# On Linux (WSL) the manifest's Windows scene paths map to /mnt/<drive>;
# set WE_ASSETS when Steam's library does not hold Wallpaper Engine, and
# OUT (default `diorama`) to keep each platform's frames apart.
set -euo pipefail
caps=${CAPTURES:-scratch/captures}
py=$(command -v python3 || command -v python)
if [ -x .tools/dxc/bin/dxc ]; then
  export DIORAMA_DXC=${DIORAMA_DXC:-$PWD/.tools/dxc/bin/dxc}
else
  export DIORAMA_DXC=${DIORAMA_DXC:-$PWD/.tools/dxc/dxc.exe}
fi
config_args=()
[ -f "$caps/we-config.json" ] && config_args=(--we-config "$caps/we-config.json")
[ -n "${WE_ASSETS:-}" ] && config_args+=(--we-assets "$WE_ASSETS")

"$py" - "$caps" <<'PY' | tr -d '\r' | while IFS='|' read -r frame scene time pointer size; do
import json, os, re, sys, pathlib
caps = pathlib.Path(sys.argv[1])

def local(path):
    # A Windows path read on Linux (WSL): C:\x -> /mnt/c/x.
    m = re.match(r"^([A-Za-z]):[\\/](.*)", path)
    if os.name != "nt" and m:
        return f"/mnt/{m[1].lower()}/" + m[2].replace("\\", "/")
    return path

for entry in json.load(open(caps / "manifest.json")):
    rdc = pathlib.PurePath(local(entry["capture"])).stem  # we_frame289
    data = json.load(open(caps / f"{rdc}.json"))
    # Some passes carry g_Time = 0 (engine passes) and RenderDoc sometimes
    # reads garbage (1e30); the frame's time is the largest plausible one.
    t = max((cb["g_Time"][0] for a in data["actions"]
             for k in ("vs_cbuffers", "ps_cbuffers")
             for cb in (a.get(k) or {}).values() if "g_Time" in cb and 0 <= cb["g_Time"][0] < 1e5), default=0.0)
    # Frames without a timed shader: capture.py lets each scene run 10 s
    # (SCENE_SETTLE_S) before capturing; scripts and videos need that time.
    t = t or 10.0
    # Cursor (if recorded) as a fraction of the captured swapchain: a
    # 3000x1920 swapchain spans this machine's desktop from (-1080, -546).
    sw, sh = data["swapchains"][0]["size"]
    ox, oy = (-1080, -546) if (sw, sh) == (3000, 1920) else (0, 0)
    cx, cy = entry.get("cursor", [ox + sw / 2, oy + sh / 2])
    scene = pathlib.PurePath(local(entry["scene"])).parent
    print(rdc, scene, t, f"{(cx - ox) / sw},{(cy - oy) / sh}", f"{sw}x{sh}", sep="|")
PY
  out="$caps/$frame.${OUT:-diorama}.png"
  clock=$("$py" -c "import os,sys; print(int(os.path.getmtime(sys.argv[1]) * 1000))" "$caps/$frame.rdc")
  ./target/release/dioramad --scene "$scene" --frame-out "$out" --time "$time" --pointer "$pointer" \
    --frame-size "$size" --clock "$clock" "${config_args[@]}" >/dev/null 2>&1
  echo -n "$frame $(basename "$scene"): "
  ./target/release/imgdiff "$caps/$frame.final.png" "$out" --side-by-side "$caps/$frame.${OUT:-diorama}.sbs.png"
done
