"""Dump Wallpaper Engine text (font shader) draws from RenderDoc captures.

Run inside RenderDoc's embedded Python:
    qrenderdoc.exe --python tools/refcap/textdraws.py

Env:
    REFCAP_CAPTURES  ';'-separated .rdc paths

Writes <capture>.text.json (per font draw: target, MVP, g_Color4, atlas,
post-VS glyph vertices as clip-space position + texcoord) and
<capture>.atlas_<id>.png for every glyph atlas, next to each capture.
Draws that sample a text render target (copy-background, effects,
composite) are listed under "consumers" with their post-VS vertices.
"""

import json
import os
import struct
import time
import traceback

import renderdoc as rd

CAPTURES = [p for p in os.environ["REFCAP_CAPTURES"].split(";") if p]
LOG = os.path.join(os.path.dirname(CAPTURES[0]), "textdraws.log")


def log(msg):
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(time.strftime("%H:%M:%S ") + msg + "\n")


def flatten(actions):
    for a in actions:
        yield a
        yield from flatten(a.children)


def is_font_shader(state):
    refl = state.GetShaderReflection(rd.ShaderStage.Pixel)
    if refl is None:
        return False
    for cb in refl.constantBlocks:
        for v in cb.variables:
            if v.name == "g_RenderVar0":
                return True
    return False


def cbuffer_values(controller, state, stage):
    out = {}
    refl = state.GetShaderReflection(stage)
    if refl is None:
        return out
    pipe = state.GetGraphicsPipelineObject()
    shader = state.GetShader(stage)
    entry = state.GetShaderEntryPoint(stage)
    for i, cb in enumerate(refl.constantBlocks):
        bound = state.GetConstantBlock(stage, i, 0).descriptor
        for v in controller.GetCBufferVariableContents(
            pipe, shader, stage, entry, i, bound.resource, bound.byteOffset, bound.byteSize
        ):
            n = v.rows * v.columns
            out[v.name] = [round(x, 6) for x in v.value.f32v[:n]]
    return out


def post_vs(controller):
    """Clip-space position + first texcoord per index, in draw order."""
    mesh = controller.GetPostVSData(0, 0, rd.MeshDataStage.VSOut)
    if mesh.vertexResourceId == rd.ResourceId.Null():
        return []
    stride = mesh.vertexByteStride
    data = controller.GetBufferData(mesh.vertexResourceId, mesh.vertexByteOffset, 0)
    if mesh.indexResourceId != rd.ResourceId.Null() and mesh.indexByteStride:
        ib = controller.GetBufferData(mesh.indexResourceId, mesh.indexByteOffset, 0)
        fmt = "<H" if mesh.indexByteStride == 2 else "<I"
        indices = [
            struct.unpack_from(fmt, ib, i * mesh.indexByteStride)[0] for i in range(mesh.numIndices)
        ]
    else:
        indices = list(range(mesh.numIndices))
    out = []
    for i in indices:
        off = i * stride
        if off + 24 > len(data):
            break
        x, y, z, w = struct.unpack_from("<4f", data, off)
        u, v = struct.unpack_from("<2f", data, off + 16)
        out.append([round(x / w, 6), round(y / w, 6), round(u, 6), round(v, 6)])
    return out


def analyze(path):
    cap = rd.OpenCaptureFile()
    if cap.OpenFile(path, "", None) != rd.ResultCode.Succeeded:
        raise RuntimeError("open failed")
    status, controller = cap.OpenCapture(rd.ReplayOptions(), None)
    if status != rd.ResultCode.Succeeded:
        raise RuntimeError("replay failed: %s" % status)
    textures = {str(t.resourceId): t for t in controller.GetTextures()}
    saved = set()
    draws = []
    actions = list(flatten(controller.GetRootActions()))
    for a in actions:
        if not (a.flags & rd.ActionFlags.Drawcall):
            continue
        controller.SetFrameEvent(a.eventId, True)
        state = controller.GetPipelineState()
        if not is_font_shader(state):
            continue
        target = [r for r in a.outputs if r != rd.ResourceId.Null()]
        tsize = None
        if target:
            t = textures.get(str(target[0]))
            tsize = t and [t.width, t.height]
        atlas = []
        for u in state.GetReadOnlyResources(rd.ShaderStage.Pixel):
            rid = u.descriptor.resource
            t = textures.get(str(rid))
            if t is None:
                continue
            key = str(rid).split("::")[-1]
            atlas.append({"id": key, "size": [t.width, t.height], "format": t.format.Name()})
            if key not in saved:
                saved.add(key)
                save = rd.TextureSave()
                save.resourceId = rid
                save.destType = rd.FileType.PNG
                controller.SaveTexture(save, path[:-4] + ".atlas_%s.png" % key)
        vp = state.GetViewport(0)
        draws.append(
            {
                "eid": a.eventId,
                "target": tsize,
                "target_id": target and str(target[0]).split("::")[-1],
                "viewport": [vp.x, vp.y, vp.width, vp.height],
                "vs": cbuffer_values(controller, state, rd.ShaderStage.Vertex),
                "ps": cbuffer_values(controller, state, rd.ShaderStage.Pixel),
                "atlas": atlas,
                "vertices": post_vs(controller),
            }
        )
    names = {str(r.resourceId).split("::")[-1]: r.name for r in controller.GetResources()}
    swap = {k for k, n in names.items() if n.startswith("Swapchain")}
    text_targets = {d["target_id"] for d in draws if d["target_id"] and d["target_id"] not in swap}
    consumers = []
    for a in actions:
        if not (a.flags & rd.ActionFlags.Drawcall):
            continue
        controller.SetFrameEvent(a.eventId, True)
        state = controller.GetPipelineState()
        reads = [str(u.descriptor.resource).split("::")[-1] for u in state.GetReadOnlyResources(rd.ShaderStage.Pixel)]
        target = [str(r).split("::")[-1] for r in a.outputs if r != rd.ResourceId.Null()]
        if not (set(reads) & text_targets or set(target) & text_targets):
            continue
        if is_font_shader(state):
            continue
        # Follow the layer through its effect chain to the final composite.
        if target and target[0] not in swap and set(reads) & text_targets:
            text_targets.add(target[0])
        t = textures.get(str([r for r in a.outputs if r != rd.ResourceId.Null()][0]))
        consumers.append(
            {
                "eid": a.eventId,
                "reads": reads,
                "target_id": target and target[0],
                "target": t and [t.width, t.height],
                "vs": cbuffer_values(controller, state, rd.ShaderStage.Vertex),
                "vertices": post_vs(controller),
            }
        )
    with open(path[:-4] + ".text.json", "w", encoding="utf-8") as f:
        json.dump({"draws": draws, "consumers": consumers}, f, indent=1)
    controller.Shutdown()
    cap.Shutdown()
    log("%s: %d font draws" % (os.path.basename(path), len(draws)))


try:
    for p in CAPTURES:
        try:
            analyze(p)
        except Exception:
            log("FAILED %s\n%s" % (p, traceback.format_exc()))
    log("done")
except Exception:
    log("FAILED\n" + traceback.format_exc())
finally:
    os._exit(0)
