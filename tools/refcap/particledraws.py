"""Dump Wallpaper Engine particle draws from RenderDoc captures.

Run inside RenderDoc's embedded Python:
    qrenderdoc.exe --python tools/refcap/particledraws.py

Env:
    REFCAP_CAPTURES  ';'-separated .rdc paths

WE draws particles as point lists expanded by a geometry shader, so the
vertex-shader output is the particle itself: position xyz and size in
SV_Position, then rotation and colour. Writes <capture>.particles.json with,
per draw: event id, blend, VS/PS constants, textures, and every point's
output attributes.
"""

import json
import os
import struct
import time
import traceback

import renderdoc as rd

CAPTURES = [p for p in os.environ["REFCAP_CAPTURES"].split(";") if p]
LOG = os.path.join(os.path.dirname(CAPTURES[0]), "particledraws.log")
MAX_POINTS = 2000


def log(msg):
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(time.strftime("%H:%M:%S ") + msg + "\n")


def flatten(actions):
    for a in actions:
        yield a
        yield from flatten(a.children)


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


def outputs(controller, state):
    """Every VS output float per point (SV_Position first)."""
    mesh = controller.GetPostVSData(0, 0, rd.MeshDataStage.VSOut)
    if mesh.vertexResourceId == rd.ResourceId.Null():
        return []
    stride = mesh.vertexByteStride
    data = controller.GetBufferData(mesh.vertexResourceId, mesh.vertexByteOffset, 0)
    count = min(mesh.numIndices, MAX_POINTS)
    floats = stride // 4
    return [
        [round(x, 4) for x in struct.unpack_from("<%df" % floats, data, i * stride)]
        for i in range(count)
        if (i + 1) * stride <= len(data)
    ]


def analyze(path):
    cap = rd.OpenCaptureFile()
    if cap.OpenFile(path, "", None) != rd.ResultCode.Succeeded:
        raise RuntimeError("open failed")
    status, controller = cap.OpenCapture(rd.ReplayOptions(), None)
    if status != rd.ResultCode.Succeeded:
        raise RuntimeError("replay failed: %s" % status)
    textures = {str(t.resourceId): t for t in controller.GetTextures()}
    draws = []
    for a in flatten(controller.GetRootActions()):
        if not (a.flags & rd.ActionFlags.Drawcall):
            continue
        controller.SetFrameEvent(a.eventId, True)
        state = controller.GetPipelineState()
        if state.GetShader(rd.ShaderStage.Geometry) == rd.ResourceId.Null():
            continue
        tex = []
        for u in state.GetReadOnlyResources(rd.ShaderStage.Pixel):
            t = textures.get(str(u.descriptor.resource))
            if t is not None:
                tex.append({"size": [t.width, t.height], "format": t.format.Name()})
        blend = state.GetColorBlends()[0]
        draws.append(
            {
                "eid": a.eventId,
                "points": a.numIndices,
                "blend": [str(blend.colorBlend.source), str(blend.colorBlend.destination)],
                "vs": cbuffer_values(controller, state, rd.ShaderStage.Vertex),
                "gs": cbuffer_values(controller, state, rd.ShaderStage.Geometry),
                "ps": cbuffer_values(controller, state, rd.ShaderStage.Pixel),
                "textures": tex,
                "outputs": outputs(controller, state),
            }
        )
    with open(path[:-4] + ".particles.json", "w", encoding="utf-8") as f:
        json.dump(draws, f)
    controller.Shutdown()
    cap.Shutdown()
    log("%s: %d particle draws" % (os.path.basename(path), len(draws)))


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
