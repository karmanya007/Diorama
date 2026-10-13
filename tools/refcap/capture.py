"""Capture reference frames of Wallpaper Engine with RenderDoc.

Run inside RenderDoc's embedded Python:
    qrenderdoc.exe --python tools/refcap/capture.py

Configured through environment variables:
    REFCAP_WE_DIR    Wallpaper Engine install dir
    REFCAP_OUT_DIR   where .rdc captures and manifest.json go (keep out of git)
    REFCAP_SCENES    ';'-separated list of project.json paths

The script launches wallpaper64.exe injected with RenderDoc, switches wallpapers
with WE's own `-control openWallpaper` command line, captures one frame per
scene, writes manifest.json, and exits. Captures contain Workshop content:
never commit them.
"""

import json
import os
import subprocess
import time
import traceback

import renderdoc as rd

WE_DIR = os.environ["REFCAP_WE_DIR"]
OUT_DIR = os.environ["REFCAP_OUT_DIR"]
SCENES = [s for s in os.environ["REFCAP_SCENES"].split(";") if s]
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
        target.TriggerCapture(1)
        cap = pump(target, CAPTURE_TIMEOUT_S, want_capture=True)
        if cap is None:
            log("  no capture received")
            manifest.append({"scene": scene, "capture": None})
            continue
        log("  captured %s frame=%d" % (cap.path, cap.frameNumber))
        manifest.append({"scene": scene, "capture": cap.path, "frame": cap.frameNumber})

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
