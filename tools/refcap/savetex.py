"""Save textures from RenderDoc captures as PNG, as they are at given events.

Run inside RenderDoc's embedded Python:
    qrenderdoc.exe --python tools/refcap/savetex.py

Env:
    REFCAP_SAVE  ';'-separated `<capture.rdc>|<event id>|<resource id>` items

Writes `<capture>.ev<event>.res<resource>.png` next to each capture.
"""

import os
import time
import traceback

import renderdoc as rd

ITEMS = [i.split("|") for i in os.environ["REFCAP_SAVE"].split(";") if i]
LOG = os.path.join(os.path.dirname(ITEMS[0][0]), "savetex.log")


def log(msg):
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(time.strftime("%H:%M:%S ") + msg + "\n")


def save(path, items):
    cap = rd.OpenCaptureFile()
    if cap.OpenFile(path, "", None) != rd.ResultCode.Succeeded:
        raise RuntimeError("open failed")
    status, controller = cap.OpenCapture(rd.ReplayOptions(), None)
    if status != rd.ResultCode.Succeeded:
        raise RuntimeError("replay failed: %s" % status)
    textures = {str(t.resourceId).split("::")[-1]: t for t in controller.GetTextures()}
    for event, resource in items:
        controller.SetFrameEvent(int(event), True)
        tex = textures.get(resource)
        if tex is None:
            log("%s: no texture %s" % (path, resource))
            continue
        s = rd.TextureSave()
        s.resourceId = tex.resourceId
        s.destType = rd.FileType.PNG
        s.alpha = rd.AlphaMapping.Preserve
        out = "%s.ev%s.res%s.png" % (path[:-4], event, resource)
        controller.SaveTexture(s, out)
        log("saved " + out)
    controller.Shutdown()
    cap.Shutdown()


try:
    by_capture = {}
    for path, event, resource in ITEMS:
        by_capture.setdefault(path, []).append((event, resource))
    for path, items in by_capture.items():
        try:
            save(path, items)
        except Exception:
            log("FAILED %s\n%s" % (path, traceback.format_exc()))
    log("done")
except Exception:
    log("FAILED\n" + traceback.format_exc())
finally:
    os._exit(0)
