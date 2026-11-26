//! Integrated RDNA1 graphics fixture. Every shader, texture and mesh is procedural.
use super::memory_scale::{check_limits, committed, footprint, location, peak};
use super::*;
use crate::memstat::{self, Kind};
use crate::mesh_indirect as mesh;
use crate::perf::{self, Span, Stage};
use victory_shader::emit::{HostShader, MESH_OUT_SPACE, MeshFallback, integration_fixture};

const TILE: u32 = 32;
const COLS: u32 = 4;
const ROWS: u32 = 3;
const WIDTH: u32 = TILE * COLS;
const HEIGHT: u32 = TILE * ROWS;
const FRAMES: u64 = 24;
const TEXTURES: usize = 256;

const PIXEL: &[u8] = br#"
    #define ROOT "UAV(u0,space=2),UAV(u1,space=2),UAV(u2,space=2)," \
        "SRV(t0,space=2),SRV(t1,space=2),SRV(t2,space=2)," \
        "SRV(t0,visibility=SHADER_VISIBILITY_PIXEL),RootConstants(num32BitConstants=1,b0,visibility=SHADER_VISIBILITY_PIXEL)"
    ByteAddressBuffer material : register(t0);
    cbuffer Layer : register(b0) { uint failed_depth; }
    [RootSignature(ROOT)]
    float4 main(float4 position:SV_Position, float4 uv:TEXCOORD0) : SV_Target {
        if (failed_depth != 0 || position.z >= 0.5) return float4(1,0,1,1);
        float3 albedo = asfloat(material.Load3(0));
        float2 intensity = asfloat(material.Load2(16));
        // The mesh exports y=1, z=2 for this planar surface normal.
        float lambert = saturate(dot(uv.yz, float2(0.5,0.25)));
        return float4(albedo * (lambert * intensity.x * intensity.y), 1);
    }
"#;

fn bytecode(bytes: &[u8]) -> D3D12_SHADER_BYTECODE {
    D3D12_SHADER_BYTECODE { pShaderBytecode: bytes.as_ptr().cast(), BytecodeLength: bytes.len() }
}

fn texture(w: u32, h: u32, mips: u16, format: DXGI_FORMAT, flags: D3D12_RESOURCE_FLAGS) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D, Width: u64::from(w), Height: h,
        DepthOrArraySize: 1, MipLevels: mips, Format: format, Flags: flags,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
    }
}

fn translated(what: &str, translate: impl FnOnce() -> Result<HostShader, victory_shader::ShaderError>) -> Option<HostShader> {
    let _span = Span::new(Stage::Translate);
    let now = std::time::Instant::now();
    let result = translate();
    eprintln!("translate {what}: {:?}, {:?}", now.elapsed(), result.as_ref().map(|s| s.dxil.len()));
    result.ok()
}

/// Two independent root tables plus optional scalar views; live heaps are
/// bound before this root signature, as required by SM 6.6.
fn material_pso(host: &Host, shader: &HostShader) -> (ID3D12RootSignature, ID3D12PipelineState) {
    let constants = D3D12_ROOT_PARAMETER1 {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
        Anonymous: D3D12_ROOT_PARAMETER1_0 { Constants: D3D12_ROOT_CONSTANTS {
            Num32BitValues: shader.user_data_dwords(), ..Default::default()
        }},
        ..Default::default()
    };
    let view = |kind, register| D3D12_ROOT_PARAMETER1 {
        ParameterType: kind,
        Anonymous: D3D12_ROOT_PARAMETER1_0 { Descriptor: D3D12_ROOT_DESCRIPTOR1 {
            ShaderRegister: register, RegisterSpace: 1, ..Default::default()
        }},
        ..Default::default()
    };
    let params = [
        constants, view(D3D12_ROOT_PARAMETER_TYPE_CBV, 2), view(D3D12_ROOT_PARAMETER_TYPE_SRV, 3),
    ];
    let desc = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
        Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
        Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 { Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
            NumParameters: params.len() as u32, pParameters: params.as_ptr(),
            Flags: D3D12_ROOT_SIGNATURE_FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED
                | D3D12_ROOT_SIGNATURE_FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED,
            ..Default::default()
        }},
    };
    let _span = Span::new(Stage::CompilePso);
    // SAFETY: the signature points to live parameters and the PSO borrows its code and root.
    unsafe {
        let mut blob: Option<ID3DBlob> = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut blob, None).unwrap();
        let blob = blob.unwrap();
        let bytes = std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
        let root: ID3D12RootSignature = host.device.CreateRootSignature(0, bytes).unwrap();
        let pso: ID3D12PipelineState = host.device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root), CS: bytecode(&shader.dxil), ..Default::default()
        }).unwrap_or_else(|e| panic!("{e}, {:?}", host.log_messages()));
        memstat::object(&pso, Kind::Pipeline);
        (root, pso)
    }
}

struct Geometry {
    root: ID3D12RootSignature,
    compute: ID3D12PipelineState,
    graphics: ID3D12PipelineState,
    signatures: mesh::Signatures,
    regions: mesh::Regions,
    fallback: MeshFallback,
    reuse_scratch: bool,
}

impl Geometry {
    fn new(host: &Host) -> Self {
        let fallback = integration_fixture::mesh().expect("translate GFX10 mesh");
        assert_eq!(MESH_OUT_SPACE, 2);
        let _span = Span::new(Stage::CompilePso);
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("mesh-scene.hlsl");
        let object = dir.path().join("mesh-scene.dxil");
        std::fs::write(&source, PIXEL).unwrap();
        let dxc = std::env::var_os("VICTORY_PC_DXC").unwrap_or_else(|| "dxc.exe".into());
        let output = std::process::Command::new(dxc)
            .args(["-E", "main", "-T", "ps_6_6", "-Fo"]).arg(&object).arg(&source)
            .output().expect("installed PC DXC");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let ps = std::fs::read(object).unwrap();
        // SAFETY: valid descriptors, stable shader bytecode, and outputs owned by Geometry.
        unsafe {
            let dev = &host.device;
            let root: ID3D12RootSignature = dev.CreateRootSignature(0, &ps).unwrap();
            let cs = D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root), CS: bytecode(&fallback.compute.dxil), ..Default::default()
            };
            let compute: ID3D12PipelineState = dev.CreateComputePipelineState(&cs).unwrap_or_else(|e| panic!("{e}, {:?}", host.log_messages()));
            let mut blend = D3D12_BLEND_DESC::default();
            blend.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
            let mut formats = [DXGI_FORMAT_UNKNOWN; 8];
            formats[0] = DXGI_FORMAT_R8G8B8A8_UNORM;
            let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                VS: bytecode(&fallback.vertex.dxil), PS: bytecode(&ps), BlendState: blend, SampleMask: u32::MAX,
                RasterizerState: D3D12_RASTERIZER_DESC {
                    FillMode: D3D12_FILL_MODE_SOLID, CullMode: D3D12_CULL_MODE_NONE,
                    DepthClipEnable: true.into(), ..Default::default()
                },
                DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                    DepthEnable: true.into(), DepthFunc: D3D12_COMPARISON_FUNC_LESS,
                    DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL, ..Default::default()
                },
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: 1, RTVFormats: formats, DSVFormat: DXGI_FORMAT_D32_FLOAT,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
            };
            let graphics: ID3D12PipelineState = dev.CreateGraphicsPipelineState(&desc).unwrap_or_else(|e| panic!("{e}, {:?}", host.log_messages()));
            memstat::object(&compute, Kind::Pipeline);
            memstat::object(&graphics, Kind::Pipeline);
            let signatures = mesh::signatures(dev, &root, [0,1,2], 3, fallback.expanded).unwrap();
            let regions = mesh::regions(&fallback.layout);
            let reuse_scratch = std::env::var_os("VICTORY_INTEGRATED_BASELINE").is_none();
            Self { root, compute, graphics, signatures, regions, fallback, reuse_scratch }
        }
    }

    fn record(&self, list: &HostList, args: &ID3D12Resource, material: &ID3D12Resource) {
        let _span = Span::new(Stage::MeshRecord);
        let data = if self.reuse_scratch {
            list.mesh_indirect_data(self.regions.bytes)
        } else {
            list.mesh_scratch(self.regions.bytes, ScratchPool::Data)
        }.expect("mesh data");
        let commands = list.mesh_scratch(512, ScratchPool::Commands).expect("mesh commands");
        let native = &list.list;
        let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
        let indirect = D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT;
        let read = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE | D3D12_RESOURCE_STATE_INDEX_BUFFER;
        // SAFETY: open list, live resources retained through the queue fence.
        unsafe {
            for region in [&data, &commands] {
                if region.first {
                    native.ResourceBarrier(&[transition(&region.buffer, D3D12_RESOURCE_STATE_COMMON, uav)]);
                }
            }
            let base = data.buffer.GetGPUVirtualAddress() + data.offset;
            let plan = mesh::plan((2, 12, false), &self.fallback.layout,
                &self.regions, false, base, 256);
            mesh::record(native, mesh::convert().unwrap(), &plan, args.GetGPUVirtualAddress(),
                None, commands.buffer.GetGPUVirtualAddress() + commands.offset);
            native.ResourceBarrier(&[transition(&commands.buffer, uav, indirect)]);
            native.SetComputeRootSignature(&self.root);
            native.SetPipelineState(&self.compute);
            list.note_op(|| "integrated mesh compute".into());
            native.ExecuteIndirect(&self.signatures.compute, 2, &commands.buffer, commands.offset, None, 0);
            native.ResourceBarrier(&[transition(&data.buffer, uav, read)]);
            native.SetGraphicsRootSignature(&self.root);
            native.SetPipelineState(&self.graphics);
            native.SetGraphicsRootShaderResourceView(6, material.GetGPUVirtualAddress());
            native.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            native.IASetIndexBuffer(Some(&D3D12_INDEX_BUFFER_VIEW {
                BufferLocation: base + self.regions.indices, SizeInBytes: self.regions.index_bytes as u32,
                Format: DXGI_FORMAT_R32_UINT,
            }));
            list.note_op(|| "integrated mesh raster".into());
            native.ExecuteIndirect(&self.signatures.draw, 2, &commands.buffer, commands.offset + 256, None, 0);
            native.ResourceBarrier(&[
                transition(&data.buffer, read, uav), transition(&commands.buffer, indirect, uav),
            ]);
        }
    }
}

fn mip_colors(frame: u64, material: usize) -> [u8;4] {
    if (frame as usize + material).is_multiple_of(2) { [192,64,0,255] } else { [64,192,64,255] }
}

fn stream(host: &Host, list: &HostList, texture: &ID3D12Resource, color: [u8;4], sampled: bool) {
    let _span = Span::new(Stage::UploadRecord);
    let read = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;
    let copy = D3D12_RESOURCE_STATE_COPY_DEST;
    // SAFETY: both footprints are within their reserved upload space, and
    // the consumer waits for the producer's fence before using the texture.
    unsafe {
        if sampled { list.list.ResourceBarrier(&[transition(texture, read, copy)]); }
        for (mip, texel) in [[32u8,32,255,255], color].into_iter().enumerate() {
            let side = 128u32 >> mip;
            let pitch = (side * 4).next_multiple_of(256);
            let (staging, offset, ptr) = list.upload(host, u64::from(side * pitch)).unwrap();
            std::slice::from_raw_parts_mut(ptr.cast::<[u8;4]>(), (side * pitch / 4) as usize).fill(texel);
            let src = location(&staging, Some(footprint(offset, side, pitch)));
            let mut dst = location(texture, None);
            dst.Anonymous.SubresourceIndex = mip as u32;
            list.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        }
        list.list.ResourceBarrier(&[transition(texture, copy, read)]);
    }
}

fn inspect_frame(frame: u64, pixels: &[u8], pitch: usize, full: bool) -> (usize, usize) {
    let mut checked = 0;
    let mut lit = 0;
    let mut edges = 0;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let point = [
                2.0 * (f64::from(x % TILE) + 0.5) / f64::from(TILE) - 1.0,
                1.0 - 2.0 * (f64::from(y % TILE) + 0.5) / f64::from(TILE),
            ];
            let half = (1.0 - point[1]) / 4.0;
            let margin = (0..4).map(|g| half - (point[0] - (f64::from(g)*0.1875 - 0.28125)).abs())
                .fold(f64::NEG_INFINITY, f64::max);
            let base = if margin > 0.0 {
                lit += 1;
                mip_colors(frame, ((x / TILE + y / TILE) % 2) as usize)
            } else {
                [0,0,0,255]
            };
            // A premultiplied blue overlay with alpha 64/255.
            let at = y as usize * pitch + x as usize * 4;
            let actual = &pixels[at..at+4];
            assert_eq!(actual[3], 255, "frame {frame} at {x},{y}");
            if margin.abs() < 0.000_000_1 {
                edges += 1;
                continue;
            }
            for channel in 0..3 {
                let light = if full { 0.375 } else { 1.0 };
                let scene = (f64::from(base[channel]) * light).round();
                let expected = (scene * (191.0/255.0)).round() as u8 + if channel == 2 {32} else {0};
                assert!(expected.abs_diff(actual[channel]) <= 1,
                    "frame {frame} ({x},{y}) material={base:?} margin={margin} expected channel {channel}={expected} actual={actual:?}");
            }
            checked += 1;
        }
    }
    eprintln!("frame={frame} checked={checked} lit={lit} raster_tie_pixels={edges}");
    (checked, lit)
}

#[test]
#[ignore = "explicit hardware fixture with streaming and memory guardrails"]
fn integrated_rdna1_frames() {
    assert!(memstat::enabled(), "set VICTORY_MEMORY_STATS=1");
    let host = host().expect("SM 6.6 device");
    let adapter = memstat::adapter(&host.device).unwrap();
    // SAFETY: query on the live adapter.
    let desc = unsafe { adapter.GetDesc1().unwrap() };
    let description = String::from_utf16_lossy(&desc.Description).trim_matches('\0').to_owned();
    assert!(description.contains("5700 XT"), "measured hardware: {description}");
    let initial = memstat::usage(&adapter).unwrap();
    check_limits(initial, initial);
    eprintln!("adapter={description} initial={initial:?}");

    let scalar = translated("CBV + SRV", || integration_fixture::material(false,true));
    let clamp = translated("SampleGrad clamp", || integration_fixture::material(true,false));
    let full = std::env::var_os("VICTORY_INTEGRATED_CONTROL").is_none();
    let shader = if full {
        if scalar.is_none() || clamp.is_none() {
            assert!(std::env::var_os("VICTORY_EXPECT_V4").is_some() && scalar.is_none() && clamp.is_none(),
                "required GFX10 material shaders");
            eprintln!("full scene blocked at GFX10 translation before GPU submission");
            return;
        }
        translated("combined material", || integration_fixture::material(true,true)).unwrap()
    } else {
        translated("common control: explicit mip", || integration_fixture::material(false,false)).unwrap()
    };
    assert_eq!(scalar.is_some(), std::env::var_os("VICTORY_EXPECT_V4").is_none());
    assert_eq!(clamp.is_some(), std::env::var_os("VICTORY_EXPECT_V4").is_none());
    let geo = Geometry::new(host);
    let (material_root, material_pso) = material_pso(host, &shader);
    let rt = D3D12_RESOURCE_STATE_RENDER_TARGET;
    let color_desc = texture(
        WIDTH, HEIGHT, 1, DXGI_FORMAT_R8G8B8A8_UNORM, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    );
    let scene = committed(host, D3D12_HEAP_TYPE_DEFAULT, &color_desc, rt, Kind::Texture);
    let overlay = committed(host, D3D12_HEAP_TYPE_DEFAULT, &color_desc, rt, Kind::Texture);
    let depth = committed(host, D3D12_HEAP_TYPE_DEFAULT,
        &texture(WIDTH, HEIGHT, 1, DXGI_FORMAT_D32_FLOAT, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL),
        D3D12_RESOURCE_STATE_DEPTH_WRITE, Kind::Texture);
    let textures: Vec<_> = (0..TEXTURES).map(|i| {
        if i.is_multiple_of(32) { check_limits(memstat::usage(&adapter).unwrap(), initial); }
        committed(host, D3D12_HEAP_TYPE_DEFAULT, &texture(128, 128, 2, DXGI_FORMAT_R8G8B8A8_UNORM, D3D12_RESOURCE_FLAG_NONE),
            D3D12_RESOURCE_STATE_COPY_DEST, Kind::Texture)
    }).collect();
    let (cb, cb_cpu) = host.upload_buffer(256).unwrap();
    let (srv, srv_cpu) = host.upload_buffer(256).unwrap();
    let (args, args_cpu) = host.shared_buffer(256).unwrap();
    let materials = [host.uav_buffer(256).unwrap(), host.uav_buffer(256).unwrap()];
    let upload = host.new_list().unwrap();
    let draw = host.new_list().unwrap();
    let timer = std::time::Instant::now();
    let mut high = initial;
    let mut total = (0,0);
    let mut used = [false; TEXTURES];
    let rtv = host.rtv(&scene, None);
    let overlay_rtv = host.rtv(&overlay, None);
    let dsv = host.dsv(&depth, None);
    // SAFETY: live resources and queues. Each descriptor update, reset, and
    // readback happens after completion of the preceding draw and producer.
    unsafe {
        cb_cpu.cast::<f32>().add(3).write(0.75);
        srv_cpu.cast::<f32>().add(3).write(0.5);
        std::ptr::copy_nonoverlapping([4u32,1,1,2,1,1].as_ptr(), args_cpu.cast(), 6);
        let dev = &host.device;
        let views: ID3D12DescriptorHeap = dev.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, NumDescriptors: 4,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, ..Default::default()
        }).unwrap();
        let samplers: ID3D12DescriptorHeap = dev.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER, NumDescriptors: 1,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, ..Default::default()
        }).unwrap();
        dev.CreateSampler(&D3D12_SAMPLER_DESC {
            Filter: D3D12_FILTER_MIN_MAG_MIP_POINT, AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP, AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            ComparisonFunc: D3D12_COMPARISON_FUNC_ALWAYS, MaxLOD: 1.0, ..Default::default()
        }, samplers.GetCPUDescriptorHandleForHeapStart());
        let handle = |i: usize| D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: views.GetCPUDescriptorHandleForHeapStart().ptr
                + i * dev.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) as usize,
        };
        for (i, buffer) in materials.iter().enumerate() {
            dev.CreateUnorderedAccessView(buffer, None, Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: DXGI_FORMAT_R32_TYPELESS, ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
                Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Buffer: D3D12_BUFFER_UAV {
                    NumElements: 64, Flags: D3D12_BUFFER_UAV_FLAG_RAW, ..Default::default()
                }},
            }), handle(2*i+1));
        }
        let producer: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default()
        }).unwrap();
        let fence: ID3D12Fence = dev.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        for frame in 1..=FRAMES {
            let frame_time = std::time::Instant::now();
            check_limits(memstat::usage(&adapter).unwrap(), initial);
            for i in 0..12 {
                let index = ((frame as usize - 1) * 12 + i) % TEXTURES;
                stream(host, &upload, &textures[index], mip_colors(frame,i), used[index]);
                used[index] = true;
                if i < 2 { dev.CreateShaderResourceView(&textures[index], None, handle(i*2)); }
            }
            upload.close();
            producer.ExecuteCommandLists(&[Some(upload.list.cast().unwrap())]);
            producer.Signal(&fence, frame).unwrap();
            host.queue.Wait(&fence, frame).unwrap();

            let l = &draw.list;
            l.ClearRenderTargetView(rtv, &[0.,0.,0.,1.], None);
            l.ClearRenderTargetView(overlay_rtv, &[0.,0.,32./255.,64./255.], None);
            l.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 1.0, 0, None);
            l.OMSetRenderTargets(1, Some(&rtv), false, Some(&dsv));
            l.SetDescriptorHeaps(&[Some(views.clone()),Some(samplers.clone())]);
            l.SetComputeRootSignature(&material_root);
            l.SetPipelineState(&material_pso);
            l.SetComputeRootConstantBufferView(1, cb.GetGPUVirtualAddress());
            l.SetComputeRootShaderResourceView(2, srv.GetGPUVirtualAddress());
            for (i, material) in materials.iter().enumerate() {
                let constants = [i as u32 * 64,0,0,0];
                draw.buffer_state(material, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
                l.SetComputeRoot32BitConstants(0, shader.user_data_dwords(), constants.as_ptr().cast(), 0);
                draw.note_op(|| "integrated material compute".into());
                l.Dispatch(1,1,1);
                draw.buffer_state(material, D3D12_RESOURCE_STATE_GENERIC_READ);
            }
            draw.buffer_state(&args, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
            l.SetGraphicsRootSignature(&geo.root);
            for y in 0..ROWS {
                for x in 0..COLS {
                    let left = x * TILE;
                    let top = y * TILE;
                    l.RSSetScissorRects(&[RECT { left: left as i32, top: top as i32,
                        right: (left+TILE) as i32, bottom: (top+TILE) as i32 }]);
                    for (marker, z) in [(0,0.25),(1,0.75)] {
                        l.RSSetViewports(&[D3D12_VIEWPORT {
                            TopLeftX: left as f32, TopLeftY: top as f32, Width: TILE as f32, Height: TILE as f32,
                            MinDepth: z, MaxDepth: z,
                        }]);
                        l.SetGraphicsRoot32BitConstant(7, marker, 0);
                        geo.record(&draw, &args, &materials[((x+y)%2) as usize]);
                    }
                }
            }
            draw.close();
            let recorded = memstat::usage(&adapter).unwrap();
            high = peak(high, recorded);
            check_limits(recorded, initial);
            assert_eq!(host.execute_and_report(&[&draw]), None, "scene submission frame {frame}");
            assert_eq!(fence.GetCompletedValue(), frame);
            high = peak(high, memstat::usage(&adapter).unwrap());
            upload.reset();
            draw.reset();
            let measured = host.present_with_compositor(&[(&scene, rt.0 as u32), (&overlay, rt.0 as u32)],
                frame, Compositor::new, |readback, footprint| {
                    let mut ptr = std::ptr::null_mut();
                    readback.Map(0,None,Some(&mut ptr)).unwrap();
                    let pitch = footprint.Footprint.RowPitch as usize;
                    let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), pitch * HEIGHT as usize);
                    crate::capture::save_fixture_frame(frame, WIDTH, HEIGHT, pitch, bytes);
                    let (checked, lit) = inspect_frame(frame, bytes, pitch, full);
                    total.0 += checked;
                    total.1 += lit;
                    readback.Unmap(0, None);
                });
            assert!(measured.is_some(), "PresentX frame {frame}");
            assert_eq!(host.log_messages(), None, "debug queue frame {frame}");
            eprintln!("frame={frame} host_time_us={} scratch_live={:?} idle={:?}",
                frame_time.elapsed().as_micros(), memstat::sample(Kind::Mesh), host.idle_scratch_bytes());
        }
    }
    assert_eq!(total.0, WIDTH as usize * HEIGHT as usize * FRAMES as usize);
    assert_eq!(total.1, 6240 * FRAMES as usize);
    eprintln!("frames={FRAMES} checked={} lit={} indirect_records={} streamed={} elapsed={:?} peak={high:?}",
        total.0, total.1, FRAMES * u64::from(COLS*ROWS) * 4, FRAMES * 12, timer.elapsed());
    for kind in [Kind::Texture, Kind::Upload, Kind::Mesh, Kind::Shared, Kind::Readback, Kind::Allocator, Kind::List, Kind::Pipeline, Kind::Signature] {
        eprintln!("objects {kind:?} {:?}", memstat::sample(kind));
    }
    // RDNA1-specific allocation-injection recovery hooks are not present in
    // CGBJ host; mesh scratch reuse is covered by CGBJ behavior and this test
    // keeps its pixel/geometry/memory-scale assertions above.
    eprintln!("skipping RDNA1 allocation-injection recovery subtest on CGBJ host");
    perf::fixture_report();
}

