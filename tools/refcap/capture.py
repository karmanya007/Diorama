"""Capture reference frames of Wallpaper Engine with RenderDoc.

Run inside RenderDoc's embedded Python:
    qrenderdoc.exe --python tools/refcap/capture.py

Configured through environment variables:
    REFCAP_WE_DIR    Wallpaper Engine install dir
    REFCAP_OUT_DIR   where .rdc captures and manifest.json go (keep out of git)
    REFCAP_SCENES    ';'-separated list of project.json paths
    REFCAP_CURSORS   optional ';'-separated "x,y" desktop points: each scene is
                     captured once per point with the mouse cursor moved there
                     (parallax/pointer experiments). Every capture logs the
                     cursor position either way.
    REFCAP_FRAMES    optional number of captures per scene/cursor (default 1),
                     REFCAP_FRAME_GAP_S apart (default 0.25): animation studies.

The script launches wallpaper64.exe injected with RenderDoc, switches wallpapers
with WE's own `-control openWallpaper` command line, captures one frame per
scene, writes manifest.json, and exits. Captures contain Workshop content:
never commit them.
"""

import ctypes
import json
import os
import subprocess
import time
import traceback

import renderdoc as rd

WE_DIR = os.environ["REFCAP_WE_DIR"]
OUT_DIR = os.environ["REFCAP_OUT_DIR"]
SCENES = [s for s in os.environ["REFCAP_SCENES"].split(";") if s]
CURSORS = [
    tuple(int(v) for v in p.split(","))
    for p in os.environ.get("REFCAP_CURSORS", "").split(";")
    if p
]
CURSOR_SETTLE_S = 4
FRAMES = int(os.environ.get("REFCAP_FRAMES", "1"))
FRAME_GAP_S = float(os.environ.get("REFCAP_FRAME_GAP_S", "0.25"))
LOG = os.path.join(OUT_DIR, "capture.log")

STARTUP_WAIT_S = 20
SCENE_SETTLE_S = 10
CAPTURE_TIMEOUT_S = 60


def log(msg):
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(time.strftime("%H:%M:%S ") + msg + "\n")


def pump(target, seconds, want_capture=False):
    """Keep the target connection alive; return the first new capture if asked."""
    deadline = time.time() + seconds
    while time.time() < deadline:
        msg = target.ReceiveMessage(None)
        if msg.type == rd.TargetControlMessageType.Disconnected:
            raise RuntimeError("target disconnected")
        if want_capture and msg.type == rd.TargetControlMessageType.NewCapture:
            return msg.newCapture
        time.sleep(0.05)
    return None


class POINT(ctypes.Structure):
    _fields_ = [("x", ctypes.c_long), ("y", ctypes.c_long)]


def cursor():
    pt = POINT()
    ctypes.windll.user32.GetCursorPos(ctypes.byref(pt))
    return [pt.x, pt.y]


def capture(target, scene, manifest):
    # Unix ms around the captured frame (lines frames up with other recordings,
    # e.g. loopback audio for the audio-spectrum calibration).
    trigger_ms = int(time.time() * 1000)
    target.TriggerCapture(1)
    cap = pump(target, CAPTURE_TIMEOUT_S, want_capture=True)
    done_ms = int(time.time() * 1000)
    pos = cursor()
    if cap is None:
        log("  no capture received")
        manifest.append({"scene": scene, "capture": None, "cursor": pos})
        return
    log("  captured %s frame=%d cursor=%s" % (cap.path, cap.frameNumber, pos))
    manifest.append({"scene": scene, "capture": cap.path, "frame": cap.frameNumber, "cursor": pos,
                     "trigger_ms": trigger_ms, "done_ms": done_ms})


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    exe = os.path.join(WE_DIR, "wallpaper64.exe")
    opts = rd.CaptureOptions()
    opts.captureCallstacks = False
    opts.apiValidation = False

    result = rd.ExecuteAndInject(exe, WE_DIR, "", [], os.path.join(OUT_DIR, "we"), opts, False)
    log("ExecuteAndInject: %s ident=%d" % (result.result, result.ident))
    if result.ident == 0:
        raise RuntimeError("injection failed")

    target = rd.CreateTargetControl("", result.ident, "lumen-refcap", True)
    if target is None or not target.Connected():
        raise RuntimeError("could not connect to target")
    log("connected to %s pid=%d" % (target.GetTarget(), target.GetPID()))
    pump(target, STARTUP_WAIT_S)

    manifest = []
    for scene in SCENES:
        log("opening " + scene)
        subprocess.run([exe, "-control", "openWallpaper", "-file", scene, "-monitor", "0"], cwd=WE_DIR, timeout=30)
        pump(target, SCENE_SETTLE_S)
        for point in CURSORS or [None]:
            if point is not None:
                ctypes.windll.user32.SetCursorPos(*point)
                pump(target, CURSOR_SETTLE_S)
            for i in range(FRAMES):
                if i:
                    pump(target, FRAME_GAP_S)
                capture(target, scene, manifest)

    with open(os.path.join(OUT_DIR, "manifest.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)
    target.Shutdown()


try:
    main()
    log("done")
except Exception:
    log("FAILED\n" + traceback.format_exc())
finally:
    os._exit(0)
