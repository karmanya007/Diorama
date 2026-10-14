//! Golden tests for the scene renderer on synthetic scenes (no Wallpaper
//! Engine content): direct and target layers, effect ping-pong, FBOs,
//! blending, parallax. Exact colours at known pixels. Skipped without DXC
//! (`LUMEN_DXC`, see `scripts/fetch-dxc.ps1`) or without any GPU adapter.

#![allow(clippy::unwrap_used, reason = "test")]

use std::collections::BTreeMap;

use lumen_gpu::shader::ShaderCompiler;
use lumen_gpu::{Gpu, GpuOptions};
use lumen_render::scene::{FrameInputs, SceneRenderer};
use lumen_scene::{
    Attribute, Blend, Effect, Fbo, ImageLayer, LayerKind, Parallax, Pass, ProgramId, Scene,
    ShaderProgram, Texture, TextureBinding, TextureId, affine,
};

/// Textured quad through the engine MVP, tinted by g_Color4 (like genericimage).
const IMAGE: &str = r"
cbuffer WeVertexUniforms : register(b0, space0) { float4x4 g_ModelViewProjectionMatrix; };
cbuffer WeFragmentUniforms : register(b1, space0) { float4 g_Color4; };
Texture2D g_Texture0 : register(t0, space1);
SamplerState g_Texture0SamplerState : register(s1, space1);
struct VsIn { [[vk::location(0)]] float3 pos : ATTRIBUTE0; [[vk::location(1)]] float2 uv : ATTRIBUTE1; };
struct VsOut { float4 position : SV_Position; [[vk::location(0)]] float2 uv : TEXCOORD0; };
VsOut vs(VsIn i) { VsOut o; o.position = mul(float4(i.pos, 1.0), g_ModelViewProjectionMatrix); o.uv = i.uv; return o; }
float4 ps(VsOut i) : SV_Target0 { return g_Texture0.Sample(g_Texture0SamplerState, i.uv) * g_Color4; }
";

/// Effect pass: 1 - previous (alpha kept).
const INVERT: &str = r"
Texture2D g_Texture0 : register(t0, space1);
SamplerState g_Texture0SamplerState : register(s1, space1);
struct VsIn { [[vk::location(0)]] float3 pos : ATTRIBUTE0; [[vk::location(1)]] float2 uv : ATTRIBUTE1; };
struct VsOut { float4 position : SV_Position; [[vk::location(0)]] float2 uv : TEXCOORD0; };
VsOut vs(VsIn i) { VsOut o; o.position = float4(i.pos, 1.0); o.uv = i.uv; return o; }
float4 ps(VsOut i) : SV_Target0 { float4 c = g_Texture0.Sample(g_Texture0SamplerState, i.uv); return float4(1.0 - c.rgb, c.a); }
";

/// Effect pass: plain copy of slot 0.
const COPY: &str = r"
Texture2D g_Texture0 : register(t0, space1);
SamplerState g_Texture0SamplerState : register(s1, space1);
struct VsIn { [[vk::location(0)]] float3 pos : ATTRIBUTE0; [[vk::location(1)]] float2 uv : ATTRIBUTE1; };
struct VsOut { float4 position : SV_Position; [[vk::location(0)]] float2 uv : TEXCOORD0; };
VsOut vs(VsIn i) { VsOut o; o.position = float4(i.pos, 1.0); o.uv = i.uv; return o; }
float4 ps(VsOut i) : SV_Target0 { return g_Texture0.Sample(g_Texture0SamplerState, i.uv); }
";

fn program(name: &str, src: &str) -> ShaderProgram {
    ShaderProgram {
        name: name.into(),
        vertex_hlsl: src.into(),
        fragment_hlsl: src.into(),
        vertex_entry: "vs".into(),
        fragment_entry: "ps".into(),
        attributes: vec![
            Attribute {
                name: "a_Position".into(),
                location: 0,
                components: 3,
            },
            Attribute {
                name: "a_TexCoord".into(),
                location: 1,
                components: 2,
            },
        ],
        texture_bindings: BTreeMap::from([(0, 0)]),
    }
}

fn solid(name: &str, rgba: [u8; 4]) -> Texture {
    Texture {
        name: name.into(),
        width: 2,
        height: 2,
        image_width: 2,
        image_height: 2,
        rgba: rgba.repeat(4),
        mips: vec![rgba.to_vec()],
        clamp: true,
        nearest: true,
    }
}

fn pass(program: usize, slot0: TextureBinding, blend: Blend) -> Pass {
    Pass {
        program: ProgramId(program),
        textures: BTreeMap::from([(0, slot0)]),
        target: None,
        constants: BTreeMap::new(),
        blend,
    }
}

fn layer(name: &str, origin: [f32; 2], texture: usize, alpha: f32) -> ImageLayer {
    ImageLayer {
        name: name.into(),
        size: [32, 32],
        transform: affine(origin, 0.0, [1.0, 1.0]),
        visible: true,
        parallax_depth: [1.0, 1.0],
        kind: LayerKind::Image,
        color: [1.0, 1.0, 1.0, alpha],
        blend: Blend::Translucent,
        albedo: pass(0, TextureBinding::Asset(TextureId(texture)), Blend::Opaque),
        composite: None,
        effects: Vec::new(),
    }
}

/// 64×32 scene: red layer on the left (drawn directly), green layer on the
/// right with effects (invert, then copy through a half-size FBO).
fn scene() -> Scene {
    let mut right = layer("right", [48.0, 16.0], 1, 1.0);
    let to_fbo = Pass {
        target: Some("_rt_half".into()),
        ..pass(2, TextureBinding::Previous, Blend::Opaque)
    };
    right.effects = vec![
        Effect {
            name: "invert".into(),
            fbos: Vec::new(),
            passes: vec![pass(1, TextureBinding::Previous, Blend::Opaque)],
        },
        Effect {
            name: "fbo round trip".into(),
            fbos: vec![Fbo {
                name: "_rt_half".into(),
                scale_div: 2,
            }],
            passes: vec![
                to_fbo,
                pass(2, TextureBinding::Fbo("_rt_half".into()), Blend::Opaque),
            ],
        },
    ];
    Scene {
        width: 64,
        height: 32,
        clear_color: [0.0, 0.0, 0.0, 1.0],
        layers: vec![layer("left", [16.0, 16.0], 0, 1.0), right],
        textures: vec![
            solid("red", [255, 0, 0, 255]),
            solid("green", [0, 255, 0, 255]),
        ],
        programs: vec![
            program("image", IMAGE),
            program("invert", INVERT),
            program("copy", COPY),
        ],
        ..Scene::default()
    }
}

fn setup() -> Option<(Gpu, ShaderCompiler)> {
    let Ok(compiler) = ShaderCompiler::locate(None) else {
        eprintln!("no DXC; skipping");
        return None;
    };
    let Ok(gpu) = Gpu::new(GpuOptions::default()) else {
        eprintln!("no GPU; skipping");
        return None;
    };
    Some((gpu, compiler))
}

fn render(gpu: &Gpu, compiler: &ShaderCompiler, scene: &Scene, pointer: [f32; 2]) -> Vec<u8> {
    let mut r = SceneRenderer::new(gpu, scene, compiler, None).unwrap();
    r.render(
        gpu,
        &FrameInputs {
            time: 1.0,
            pointer,
            ..FrameInputs::default()
        },
    );
    r.read_output(gpu).unwrap()
}

/// RGBA at (x, y) with y down, as in the PNG.
fn px(img: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    let i = (y * width + x) * 4;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

#[test]
fn layers_effects_and_fbos() {
    let Some((gpu, compiler)) = setup() else {
        return;
    };
    let img = render(&gpu, &compiler, &scene(), [0.5, 0.5]);
    assert_eq!(px(&img, 64, 16, 16), [255, 0, 0, 255], "direct layer");
    // Green inverted is magenta; the FBO round trip keeps it.
    assert_eq!(px(&img, 64, 48, 16), [255, 0, 255, 255], "effect chain");
}

#[test]
fn translucent_layers_blend_with_alpha() {
    let Some((gpu, compiler)) = setup() else {
        return;
    };
    let mut s = scene();
    s.clear_color = [1.0, 1.0, 1.0, 1.0];
    s.layers[0].color[3] = 0.5;
    let img = render(&gpu, &compiler, &s, [0.5, 0.5]);
    let [r, g, b, _] = px(&img, 64, 16, 16);
    assert_eq!(r, 255);
    assert!(
        (126..=129).contains(&g) && (126..=129).contains(&b),
        "{g} {b}"
    );
}

#[test]
fn parallax_moves_layers_away_from_the_cursor() {
    let Some((gpu, compiler)) = setup() else {
        return;
    };
    let mut s = scene();
    s.layers.truncate(1);
    s.parallax = Some(Parallax {
        amount: 0.25,
        mouse_influence: 1.0,
        delay: 0.0,
    });
    // Cursor at the left edge: shift = 0.25 · 64 · (0.5 − 0) = +8 px.
    let img = render(&gpu, &compiler, &s, [0.0, 0.5]);
    assert_eq!(px(&img, 64, 1, 16), [0, 0, 0, 255], "left edge uncovered");
    assert_eq!(
        px(&img, 64, 39, 16),
        [255, 0, 0, 255],
        "right edge moved in"
    );
}
