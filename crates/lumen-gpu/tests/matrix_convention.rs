//! Pins how a WE-style `mul(v, M)` behaves after HLSL → DXC → naga → wgpu, so
//! renderers can upload captured matrices verbatim. Skips without DXC or GPU.

#![allow(clippy::unwrap_used, reason = "test")]

use lumen_gpu::shader::{ShaderCompiler, Stage, UniformLayout};
use lumen_gpu::{Gpu, GpuOptions, wgpu};

const HLSL: &str = r"
cbuffer U : register(b1, space0) { float4x4 M; };
float4 vs(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * 2.0 - 1.0, 0.0, 1.0);
}
float4 ps() : SV_Target0 { return mul(float4(1.0, 10.0, 100.0, 1000.0), M); }
";

#[test]
fn naga_columns_are_hlsl_columns() {
    let Ok(compiler) = ShaderCompiler::locate(None) else {
        eprintln!("no DXC; skipping");
        return;
    };
    let Ok(gpu) = Gpu::new(GpuOptions::default()) else {
        eprintln!("no GPU; skipping");
        return;
    };
    let vs = compiler.compile(HLSL, "vs", Stage::Vertex).unwrap();
    let fs = compiler.compile(HLSL, "ps", Stage::Fragment).unwrap();
    let layout = UniformLayout::reflect(&fs, 0, 1).unwrap();

    // Values 0..15 in naga-column order.
    let mut data = vec![0u8; layout.size as usize];
    let values: Vec<f32> = (0..16).map(|i| i as f32).collect();
    assert!(layout.write(&mut data, "M", &values));

    let d = &gpu.device;
    let buf = wgpu::util::DeviceExt::create_buffer_init(
        d,
        &wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &data,
            usage: wgpu::BufferUsages::UNIFORM,
        },
    );
    let bgl = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bg = d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 1,
            resource: buf.as_entire_binding(),
        }],
    });
    let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let vsm = d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(vs)),
    });
    let fsm = d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(fs)),
    });
    let format = wgpu::TextureFormat::Rgba32Float;
    let pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module: &vsm,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &fsm,
            entry_point: Some("ps"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let target = d.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let readback = d.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = d.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }
    enc.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([enc.finish()]);
    readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    d.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let bytes = readback.slice(..16).get_mapped_range().unwrap().to_vec();
    let out: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    eprintln!("mul((1,10,100,1000), M) = {out:?}");
    // If naga column j holds HLSL row j: out.x = 1*0 + 10*4 + 100*8 + 1000*12.
    let rows_in_columns = [12840.0, 13951.0, 15062.0, 16173.0];
    // If naga column j holds HLSL column j: out.x = dot(v, (0,1,2,3)).
    let columns_in_columns = [3210.0, 7654.0, 12098.0, 16542.0];
    assert!(
        out == rows_in_columns || out == columns_in_columns,
        "unexpected {out:?}"
    );
    // So a WE matrix captured as rows (translation in row 3) uploads transposed.
    assert_eq!(
        out, columns_in_columns,
        "convention changed; update renderers"
    );
}
