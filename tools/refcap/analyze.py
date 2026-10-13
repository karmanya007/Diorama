"""Extract pass structure from RenderDoc captures of Wallpaper Engine.

Run inside RenderDoc's embedded Python:
    qrenderdoc.exe --python tools/refcap/analyze.py

Env:
    REFCAP_OUT_DIR  folder holding manifest.json + .rdc files (from capture.py)

Writes <capture>.json (actions, render targets, shaders, cbuffer values) and
<capture>.final.png (last presented image) next to each capture.
"""

import json
import math
import os
import time
import traceback

import renderdoc as rd

OUT_DIR = os.environ["REFCAP_OUT_DIR"]
LOG = os.path.join(OUT_DIR, "analyze.log")
MAX_CBUFFER_ARRAY = 16


def log(msg):
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(time.strftime("%H:%M:%S ") + msg + "\n")


def flatten(actions):
    for a in actions:
        yield a
        yield from flatten(a.children)


def var_value(v):
    """ShaderVariable -> JSON-friendly value."""
    if len(v.members) > 0:
        return {m.name: var_value(m) for m in list(v.members)[:MAX_CBUFFER_ARRAY]}
    count = v.rows * v.columns
    vals = list(v.value.f32v[:count])
    if v.type in (rd.VarType.SInt,):
        vals = list(v.value.s32v[:count])
    elif v.type in (rd.VarType.UInt, rd.VarType.Bool):
        vals = list(v.value.u32v[:count])
    # Uninitialised cbuffer memory can hold NaN/Inf, which is not valid JSON.
    return [
        (round(x, 6) if math.isfinite(x) else None) if isinstance(x, float) else x
        for x in vals
    ]


def cbuffers(controller, state, stage):
    out = {}
    refl = state.GetShaderReflection(stage)
    if refl is None:
        return out
    pipe = state.GetGraphicsPipelineObject()
    shader = state.GetShader(stage)
    entry = state.GetShaderEntryPoint(stage)
    for slot, block in enumerate(refl.constantBlocks):
        try:
            used = state.GetConstantBlock(stage, slot, 0)
            desc = used.descriptor
            vars_ = controller.GetCBufferVariableContents(
                pipe, shader, stage, entry, slot, desc.resource, desc.byteOffset, desc.byteSize
            )
            out[block.name] = {v.name: var_value(v) for v in vars_}
        except Exception as e:  # keep going; record what failed
            out[block.name] = {"_error": str(e)}
    return out


def analyze(path):
    cap = rd.OpenCaptureFile()
    status = cap.OpenFile(path, "", None)
    if status != rd.ResultCode.Succeeded:
        raise RuntimeError("open failed: %s" % status)
    status, controller = cap.OpenCapture(rd.ReplayOptions(), None)
    if status != rd.ResultCode.Succeeded:
        raise RuntimeError("replay failed: %s" % status)

    sfile = controller.GetStructuredFile()
    textures = {str(t.resourceId): t for t in controller.GetTextures()}
    names = {str(r.resourceId): r.name for r in controller.GetResources()}

    def tex_info(rid):
        t = textures.get(str(rid))
        if t is None:
            return {"id": str(rid), "name": names.get(str(rid), "")}
        return {
            "id": str(rid),
            "name": names.get(str(rid), ""),
            "size": [t.width, t.height],
            "format": t.format.Name(),
            "mips": t.mips,
            "msaa": t.msSamp,
        }

    actions_out = []
    last_present_tex = None
    for a in flatten(controller.GetRootActions()):
        flags = a.flags
        entry = {"eid": a.eventId, "name": a.GetName(sfile)}
        if flags & rd.ActionFlags.Present:
            entry["kind"] = "present"
            last_present_tex = a.copyDestination if a.copyDestination != rd.ResourceId.Null() else None
        elif flags & rd.ActionFlags.Clear:
            entry["kind"] = "clear"
        elif flags & rd.ActionFlags.Drawcall:
            entry["kind"] = "draw"
            controller.SetFrameEvent(a.eventId, False)
            state = controller.GetPipelineState()
            entry["vertices"] = a.numIndices
            entry["instances"] = a.numInstances
            entry["targets"] = [tex_info(r) for r in a.outputs if r != rd.ResourceId.Null()]
            entry["vs"] = str(state.GetShader(rd.ShaderStage.Vertex))
            entry["ps"] = str(state.GetShader(rd.ShaderStage.Pixel))
            entry["vs_cbuffers"] = cbuffers(controller, state, rd.ShaderStage.Vertex)
            entry["ps_cbuffers"] = cbuffers(controller, state, rd.ShaderStage.Pixel)
            try:
                entry["ps_textures"] = [
                    tex_info(u.descriptor.resource) for u in state.GetReadOnlyResources(rd.ShaderStage.Pixel)
                ]
            except Exception as e:
                entry["ps_textures"] = {"_error": str(e)}
            try:
                blend = state.GetColorBlends()
                entry["blend"] = [
                    {
                        "enabled": b.enabled,
                        "color": [str(b.colorBlend.source), str(b.colorBlend.operation), str(b.colorBlend.destination)],
                        "alpha": [str(b.alphaBlend.source), str(b.alphaBlend.operation), str(b.alphaBlend.destination)],
                    }
                    for b in blend[:1]
                ]
            except Exception as e:
                entry["blend"] = {"_error": str(e)}
        else:
            continue
        actions_out.append(entry)

    # Save the final presented image (backbuffer at the last action).
    final_png = None
    swap = [t for t in controller.GetTextures() if t.creationFlags & rd.TextureCategory.SwapBuffer]
    if swap:
        last_eid = actions_out[-1]["eid"] if actions_out else 0
        controller.SetFrameEvent(last_eid, True)
        save = rd.TextureSave()
        save.resourceId = swap[0].resourceId
        save.destType = rd.FileType.PNG
        final_png = path[:-4] + ".final.png"
        controller.SaveTexture(save, final_png)

    summary = {
        "capture": os.path.basename(path),
        "api": str(controller.GetAPIProperties().pipelineType),
        "swapchains": [tex_info(t.resourceId) for t in swap],
        "draws": sum(1 for a in actions_out if a.get("kind") == "draw"),
        "actions": actions_out,
        "final_png": final_png and os.path.basename(final_png),
    }
    with open(path[:-4] + ".json", "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=1)
    controller.Shutdown()
    cap.Shutdown()
    log("%s: %d draws" % (os.path.basename(path), summary["draws"]))


def main():
    with open(os.path.join(OUT_DIR, "manifest.json"), encoding="utf-8") as f:
        manifest = json.load(f)
    for item in manifest:
        if item.get("capture"):
            try:
                analyze(item["capture"])
            except Exception:
                log("FAILED %s\n%s" % (item["capture"], traceback.format_exc()))


try:
    main()
    log("done")
except Exception:
    log("FAILED\n" + traceback.format_exc())
finally:
    os._exit(0)
