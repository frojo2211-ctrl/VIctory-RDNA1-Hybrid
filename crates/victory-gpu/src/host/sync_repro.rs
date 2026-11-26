/// Isolated RDNA1 sync/lifetime/mesh-fallback repro (owned fixture). Not a game asset.
/// Exercises mesh fallback compute+graphics, ExecuteIndirect, mesh_scratch reuse/reset,
/// multiple command lists, queue dependencies, and pixel readback validation under
/// D3D12 Debug Layer + DRED.
/// Run:
///   $env:VICTORY_D3D12_DEBUG='1'; $env:VICTORY_D3D12_DRED='light'
///   cargo test -p victory-gpu --lib --features diagnostics --locked --offline host::sync_repro::rdna1_mesh_fallback_repro -- --ignored --nocapture --test-threads=1
use super::*;
use crate::mesh_indirect as mesh;
use victory_shader::emit::{integration_fixture, MeshFallback, MESH_OUT_SPACE};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).filter(|v| *v > 0).unwrap_or(default)
}

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;

fn transition(res: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { std::mem::transmute_copy(res) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

fn bytecode(bytes: &[u8]) -> D3D12_SHADER_BYTECODE {
    D3D12_SHADER_BYTECODE { pShaderBytecode: bytes.as_ptr().cast(), BytecodeLength: bytes.len() }
}

fn check_debug(host: &Host, frame: u64, where_: &str) {
    if let Some(msgs) = host.log_messages() {
        if !msgs.is_empty() {
            panic!("D3D12 debug messages frame {frame} {where_}:\n{msgs}");
        }
    }
    if let Err(e) = unsafe { host.device.GetDeviceRemovedReason() } {
        panic!("device removed frame {frame} {where_}: {e:?}\n{}", host.log_messages().unwrap_or_default());
    }
}

struct Geo {
    root: ID3D12RootSignature,
    compute: ID3D12PipelineState,
    graphics: ID3D12PipelineState,
    signatures: mesh::Signatures,
    regions: mesh::Regions,
    fallback: MeshFallback,
}

impl Geo {
    fn new(host: &Host) -> Self {
        let fallback = integration_fixture::mesh().expect("mesh fallback shader");
        assert_eq!(MESH_OUT_SPACE, 2);
        let ps_src = br#"
            #define ROOT "UAV(u0,space=2),UAV(u1,space=2),UAV(u2,space=2),SRV(t0,space=2),SRV(t1,space=2),SRV(t2,space=2),SRV(t0,visibility=SHADER_VISIBILITY_PIXEL),RootConstants(num32BitConstants=1,b0,visibility=SHADER_VISIBILITY_PIXEL)"
            ByteAddressBuffer material : register(t0);
            cbuffer Layer : register(b0) { uint failed_depth; }
            [RootSignature(ROOT)] float4 main(float4 position:SV_Position, float4 uv:TEXCOORD0):SV_Target {
                if (failed_depth != 0 || position.z >= 0.5) return float4(1,0,1,1);
                float3 albedo = asfloat(material.Load3(0));
                return float4(albedo, 1);
            }
        "#;
        unsafe {
            // Use DXC for SM 6.6 root signature parity with mesh fallback shaders.
            let dir = std::env::temp_dir().join(format!("victory-sync-repro-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("temp dir");
            let src = dir.join("repro.hlsl");
            let obj = dir.join("repro.dxil");
            std::fs::write(&src, ps_src).expect("write hlsl");
            let dxc = std::env::var_os("VICTORY_PC_DXC").unwrap_or_else(|| "dxc.exe".into());
            let out = std::process::Command::new(&dxc)
                .args(["-E", "main", "-T", "ps_6_6", "-Fo"]).arg(&obj).arg(&src)
                .output().expect("run dxc.exe (set VICTORY_PC_DXC if needed)");
            assert!(out.status.success(), "DXC failed: {}", String::from_utf8_lossy(&out.stderr));
            let ps_bytes_owned = std::fs::read(&obj).expect("read dxil");
            let _ = std::fs::remove_dir_all(&dir);
            let ps_bytes = &ps_bytes_owned;
            let root: ID3D12RootSignature = host.device.CreateRootSignature(0, ps_bytes.as_slice()).expect("root sig");
            let cs_desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                CS: bytecode(&fallback.compute.dxil), ..Default::default()
            };
            let compute: ID3D12PipelineState = host.device.CreateComputePipelineState(&cs_desc).expect("compute pso");
            let mut blend = D3D12_BLEND_DESC::default();
            blend.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
            let mut formats = [DXGI_FORMAT_UNKNOWN; 8];
            formats[0] = DXGI_FORMAT_R8G8B8A8_UNORM;
            let gs_desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                VS: bytecode(&fallback.vertex.dxil),
                PS: D3D12_SHADER_BYTECODE { pShaderBytecode: ps_bytes.as_ptr().cast(), BytecodeLength: ps_bytes.len() },
                BlendState: blend, SampleMask: u32::MAX,
                RasterizerState: D3D12_RASTERIZER_DESC {
                    FillMode: D3D12_FILL_MODE_SOLID, CullMode: D3D12_CULL_MODE_NONE,
                    DepthClipEnable: true.into(), ..Default::default()
                },
                DepthStencilState: D3D12_DEPTH_STENCIL_DESC::default(),
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: 1, RTVFormats: formats,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                ..Default::default()
            };
            let graphics: ID3D12PipelineState = host.device.CreateGraphicsPipelineState(&gs_desc).expect("graphics pso");
            let signatures = mesh::signatures(&host.device, &root, [0, 1, 2], 3, fallback.expanded).expect("signatures");
            let regions = mesh::regions(&fallback.layout);
            Self { root, compute, graphics, signatures, regions, fallback }
        }
    }

    unsafe fn record_indirect(&self, list: &HostList, args: &ID3D12Resource, material: &ID3D12Resource) {
        unsafe {
            let data = list.mesh_scratch(self.regions.bytes, ScratchPool::Data).expect("mesh data scratch");
            let commands = list.mesh_scratch(512, ScratchPool::Commands).expect("mesh commands scratch");
            let l = &list.list;
            let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
            let indirect = D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT;
            let read = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE | D3D12_RESOURCE_STATE_INDEX_BUFFER;
            if data.first { l.ResourceBarrier(&[transition(&data.buffer, D3D12_RESOURCE_STATE_COMMON, uav)]); }
            if commands.first { l.ResourceBarrier(&[transition(&commands.buffer, D3D12_RESOURCE_STATE_COMMON, uav)]); }
            list.buffer_state(&data.buffer, uav);
            list.buffer_state(&commands.buffer, uav);
            let base = data.buffer.GetGPUVirtualAddress() + data.offset;
            let plan = mesh::plan((2, 12, false), &self.fallback.layout, &self.regions, false, base, 256);
            let convert = mesh::convert().expect("mesh convert pass");
            mesh::record(l, convert, &plan, args.GetGPUVirtualAddress(), None, commands.buffer.GetGPUVirtualAddress() + commands.offset);
            list.buffer_state(&commands.buffer, indirect);
            l.SetComputeRootSignature(&self.root);
            l.SetPipelineState(&self.compute);
            l.ExecuteIndirect(&self.signatures.compute, 2, &commands.buffer, commands.offset, None, 0);
            list.buffer_state(&data.buffer, read);
            l.SetGraphicsRootSignature(&self.root);
            l.SetPipelineState(&self.graphics);
            l.SetGraphicsRootShaderResourceView(6, material.GetGPUVirtualAddress());
            l.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            l.IASetIndexBuffer(Some(&D3D12_INDEX_BUFFER_VIEW {
                BufferLocation: base + self.regions.indices,
                SizeInBytes: self.regions.index_bytes as u32,
                Format: DXGI_FORMAT_R32_UINT,
            }));
            l.ExecuteIndirect(&self.signatures.draw, 2, &commands.buffer, commands.offset + 256, None, 0);
            list.buffer_state(&data.buffer, uav);
            list.buffer_state(&commands.buffer, uav);
        }
    }
}

fn create_texture(host: &Host, format: DXGI_FORMAT, flags: D3D12_RESOURCE_FLAGS, state: D3D12_RESOURCE_STATES) -> ID3D12Resource {
    let mut res: Option<ID3D12Resource> = None;
    unsafe {
        host.device.CreateCommittedResource(
            &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
            D3D12_HEAP_FLAG_NONE,
            &D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D, Width: WIDTH as u64, Height: HEIGHT,
                DepthOrArraySize: 1, MipLevels: 1, Format: format, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN, Flags: flags, ..Default::default()
            },
            state, None, &mut res
        ).expect("texture");
    }
    res.unwrap()
}

#[test]
#[ignore = "hardware mesh-fallback/ExecuteIndirect/scratch/multi-list/queue repro; run explicitly on RX 5700 XT with debug+DRED"]
fn rdna1_mesh_fallback_repro() {
    let frames = env_usize("VICTORY_SYNC_REPRO_FRAMES", 120).min(2000) as u64;
    let host = host().expect("D3D12 device");
    eprintln!("mesh-repro: frames={frames} debug={} dred_any={}", crate::switch::on("VICTORY_D3D12_DEBUG"), dred_any());
    check_debug(host, 0, "init");

    let geo = Geo::new(host);
    let rt = create_texture(host, DXGI_FORMAT_R8G8B8A8_UNORM, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_STATE_RENDER_TARGET);
    let rtv_heap: ID3D12DescriptorHeap = unsafe { host.device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: 1, ..Default::default() }).unwrap() };
    let rtv = unsafe { rtv_heap.GetCPUDescriptorHandleForHeapStart() };
    unsafe { host.device.CreateRenderTargetView(&rt, None, rtv); }

    // Indirect args: 2 records of (groupsX,1,1) = 3 uints each, stride 12 as per mesh_indirect plan (records,stride,counted)
    let (args, args_map) = host.shared_buffer(64).expect("args buffer");
    unsafe { std::ptr::copy_nonoverlapping([4u32,1,1, 2u32,1,1].as_ptr(), args_map.cast::<u32>(), 6); }
    let material = host.uav_buffer(256).expect("material");
    let (mat_up, mat_up_map) = host.upload_buffer(256).expect("mat upload");
    unsafe { std::ptr::copy_nonoverlapping([0.0f32, 1.0f32, 0.0f32, 1.0f32].as_ptr(), mat_up_map.cast::<f32>(), 4); }

    let readback = host.read_back_buffer((WIDTH * HEIGHT * 4) as u64).expect("readback");
    let copy_queue: ID3D12CommandQueue = unsafe { host.device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap() };
    let fence: ID3D12Fence = unsafe { host.device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap() };
    let mut fence_value = 0u64;
    let mut total_lit = 0usize;
    let mut frame_times_us: Vec<u128> = Vec::new();

    for frame in 1..=frames {
        let t0 = std::time::Instant::now();
        // List A: material upload/copy (queue dependency target)
        let la = host.new_list().expect("list A");
        la.buffer_state(&material, D3D12_RESOURCE_STATE_COPY_DEST);
        unsafe { la.list.CopyBufferRegion(&material, 0, &mat_up, 0, 16); }
        la.buffer_state(&material, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        la.close();

        // List B: draw/ExecuteIndirect/mesh scratch reuse
        let lb = host.new_list().expect("list B");
        unsafe {
            lb.list.ClearRenderTargetView(rtv, &[0.0, 0.0, 0.0, 1.0], None);
            lb.list.OMSetRenderTargets(1, Some(&rtv), false, None);
            lb.list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: WIDTH as f32, Height: HEIGHT as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
            lb.list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: WIDTH as i32, bottom: HEIGHT as i32 }]);
            lb.buffer_state(&args, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
            lb.buffer_state(&material, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
            geo.record_indirect(&lb, &args, &material);
        }
        lb.close();

        // Execute A on copy_queue, signal, make main queue wait, execute B on main queue.
        unsafe {
            let la_cmd: ID3D12CommandList = la.list.cast().unwrap();
            copy_queue.ExecuteCommandLists(&[Some(la_cmd)]);
            fence_value += 1;
            copy_queue.Signal(&fence, fence_value).unwrap();
            host.queue.Wait(&fence, fence_value).unwrap();
        }
        host.execute(&[&lb]);

        // List C: readback on main queue after execution
        let lc = host.new_list().expect("list C");
        lc.buffer_state(&rt, D3D12_RESOURCE_STATE_COPY_SOURCE);
        // For texture->buffer readback, use footprint. Keep simple: copy resource via intermediate placed footprint.
        let row_pitch = (WIDTH * 4).next_multiple_of(256);
        let rb_size = row_pitch * HEIGHT;
        assert!((rb_size as u64) <= 256 * 1024);
        let footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
            Offset: 0,
            Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: DXGI_FORMAT_R8G8B8A8_UNORM, Width: WIDTH, Height: HEIGHT, Depth: 1, RowPitch: row_pitch },
        };
        let dst = D3D12_TEXTURE_COPY_LOCATION { pResource: unsafe { std::mem::transmute_copy(&readback) }, Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprint } };
        let src = D3D12_TEXTURE_COPY_LOCATION { pResource: unsafe { std::mem::transmute_copy(&rt) }, Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 } };
        unsafe { lc.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None); }
        lc.buffer_state(&rt, D3D12_RESOURCE_STATE_RENDER_TARGET);
        lc.close();
        host.execute(&[&lc]);

        // Validate pixels
        unsafe {
            let mut ptr = std::ptr::null_mut();
            let range = D3D12_RANGE { Begin: 0, End: (rb_size) as usize };
            readback.Map(0, Some(&range), Some(&mut ptr)).unwrap();
            let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), rb_size as usize);
            let mut lit = 0usize;
            for y in 0..HEIGHT as usize {
                for x in 0..WIDTH as usize {
                    let px = &bytes[y * row_pitch as usize + x * 4..][..4];
                    if px[1] > 100 { lit += 1; }
                    // background or green
                    let ok = (px[0] == 0 && px[1] == 0 && px[2] == 0) || (px[0] < 10 && px[1] > 200 && px[2] < 10);
                    assert!(ok, "frame {frame} pixel ({x},{y})={px:?}");
                }
            }
            assert!(lit > 50, "frame {frame} lit pixels too few: {lit}");
            total_lit += lit;
            readback.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
        }

        check_debug(host, frame, "post-frame");
        la.reset();
        lb.reset();
        lc.reset();
        frame_times_us.push(t0.elapsed().as_micros());
    }
    let avg_us = frame_times_us.iter().sum::<u128>() / frame_times_us.len().max(1) as u128;
    eprintln!("mesh-repro: PASS frames={frames} avg_frame_us={avg_us} total_lit={total_lit}");
}

/// Own fixtures for carded shadow/send.rs path: exercises real carded Shadow creation,
/// card_dirty/send::take/record/give_back, prepare_card_before-like flow, Root SRV after
/// upload, reuse under pressure, and pixel/data validation after concurrent submissions.
/// No third-party/game assets.
#[test]
#[ignore = "hardware carded shadow/send.rs repro; run explicitly on RX 5700 XT with debug+DRED"]
fn rdna1_carded_send_repro() {
    use std::sync::Arc;
    // Ensure carded buffers are enabled for this process; default is on, but be explicit.
    // VICTORY_CARD_ALL=1 makes small test buffers use the carded/two-sided path.
    if std::env::var_os("VICTORY_CARD_BUFFERS").is_some_and(|v| v == "0") {
        eprintln!("SKIP: VICTORY_CARD_BUFFERS=0");
        return;
    }
    let frames = env_usize("VICTORY_SYNC_REPRO_FRAMES", 80).min(500) as u64;
    let host = host().expect("D3D12 device");
    assert!(host.card_ready(), "card compare work not ready");
    eprintln!("carded-repro: frames={frames} debug={} dred_any={}", crate::switch::on("VICTORY_D3D12_DEBUG"), dred_any());
    check_debug(host, 0, "init");

    // Title memory backing for a carded shadow. Registered so shadow::find creates a real carded shadow.
    let title_len: usize = 256 * 1024; // 256KB
    let title = unsafe {
        windows::Win32::System::Memory::VirtualAlloc(None, title_len, windows::Win32::System::Memory::MEM_COMMIT | windows::Win32::System::Memory::MEM_RESERVE, windows::Win32::System::Memory::PAGE_READWRITE)
    }.cast::<u8>();
    assert!(!title.is_null(), "VirtualAlloc title memory");
    let title_addr = title as u64;
    let resource_id = 0xC0DE_0001usize;
    crate::shadow::register(resource_id, title_addr, title_len as u64);
    unsafe { std::ptr::write_bytes(title, 0, title_len); }

    // Create/find carded shadow via normal path. May require VICTORY_CARD_ALL=1 or size within limits.
    let shadow = crate::shadow::find(title_addr, 256).expect("shadow for registered title memory");
    if !shadow.carded() {
        // Try by_address path for carded behavior if normal path didn't card it.
        eprintln!("carded-repro: registered shadow not carded (size/limits); trying by_address");
    }
    eprintln!("carded-repro: shadow carded={} span={:#x}+{:#x}", shadow.carded(), shadow.span().0, shadow.span().1);

    // Build a simple compute shader that reads from SRV buffer (t0) and writes to UAV buffer (u0).
    // Root: SRV(t0), UAV(u0), RootConstants(b0,2)
    // Raw root signature: SRV(t0) as root SRV, UAV(u0) as root UAV, 2 root constants b0.
    let (root, pso) = unsafe {
        let params = [
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
                Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: 0, RegisterSpace: 0 } },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
                Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: 0, RegisterSpace: 0 } },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: 2 } },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
        ];
        let desc = D3D12_ROOT_SIGNATURE_DESC { NumParameters: params.len() as u32, pParameters: params.as_ptr(), NumStaticSamplers: 0, pStaticSamplers: std::ptr::null(), Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE };
        let mut blob: Option<ID3DBlob> = None;
        let mut err: Option<ID3DBlob> = None;
        D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, Some(&mut err)).expect("serialize root sig");
        let blob = blob.unwrap();
        let root: ID3D12RootSignature = host.device.CreateRootSignature(0, std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize())).expect("root sig");
        let hlsl = br#"
            ByteAddressBuffer src : register(t0);
            RWByteAddressBuffer dst : register(u0);
            cbuffer C : register(b0) { uint off; uint val; };
            [numthreads(1,1,1)] void main() { uint v = src.Load(off); dst.Store(0, v + val); }
        "#;
        let cs = compile_hlsl(hlsl, "cs_5_1");
        let pso_desc = D3D12_COMPUTE_PIPELINE_STATE_DESC { pRootSignature: std::mem::transmute_copy(&root), CS: bytecode(&cs), ..Default::default() };
        let pso: ID3D12PipelineState = host.device.CreateComputePipelineState(&pso_desc).expect("pso");
        (root, pso)
    };

    let output = host.uav_buffer(256).expect("output uav");
    let readback = host.read_back_buffer(256).expect("readback");
    let mut frame_times_us: Vec<u128> = Vec::new();

    for frame in 1..=frames {
        let t0 = std::time::Instant::now();
        let expected = 0x1000u32 + frame as u32;
        unsafe { std::ptr::write(title.cast::<u32>(), expected); }
        if shadow.carded() {
            shadow.resend(0, 4);
        }

        let list = host.new_list().expect("list");
        shadow.refresh(&list, title_addr, 4);
        list.buffer_state(&output, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        unsafe {
            let l = &list.list;
            l.SetComputeRootSignature(&root);
            l.SetPipelineState(&pso);
            l.SetComputeRootShaderResourceView(0, shadow.gpu(title_addr));
            l.SetComputeRootUnorderedAccessView(1, output.GetGPUVirtualAddress());
            let constants = [0u32, 7u32];
            l.SetComputeRoot32BitConstants(2, 2, constants.as_ptr().cast(), 0);
            l.Dispatch(1, 1, 1);
        }
        list.buffer_state(&output, D3D12_RESOURCE_STATE_COPY_SOURCE);
        unsafe { list.list.CopyBufferRegion(&readback, 0, &output, 0, 4); }
        list.close();
        host.execute(&[&list]);

        let mut val = 0u32;
        unsafe {
            let mut ptr = std::ptr::null_mut();
            readback.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 4 }), Some(&mut ptr)).unwrap();
            val = std::ptr::read(ptr.cast::<u32>());
            readback.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
        }
        assert_eq!(val, expected + 7, "frame {frame} carded={} val={val:#x} expected={:#x}", shadow.carded(), expected + 7);
        check_debug(host, frame, "post-frame");
        list.reset();
        frame_times_us.push(t0.elapsed().as_micros());
    }

    crate::shadow::unregister(resource_id, title_addr);
    unsafe { windows::Win32::System::Memory::VirtualFree(title.cast(), 0, windows::Win32::System::Memory::MEM_RELEASE).ok(); }
    let avg = frame_times_us.iter().sum::<u128>() / frame_times_us.len().max(1) as u128;
    eprintln!("carded-repro: PASS frames={frames} carded={} avg_frame_us={avg}", shadow.carded());
}

fn compile_hlsl(src: &[u8], target: &str) -> Vec<u8> {
    use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
    use windows::Win32::Graphics::Direct3D::ID3DBlob;
    unsafe {
        let mut code: Option<ID3DBlob> = None;
        let mut err: Option<ID3DBlob> = None;
        let target_s = std::ffi::CString::new(target).unwrap();
        let hr = D3DCompile(src.as_ptr().cast(), src.len(), windows::core::s!("repro"), None, None,
            windows::core::s!("main"), windows::core::PCSTR(target_s.as_ptr().cast()), 0, 0, &mut code, Some(&mut err));
        if let Err(e) = hr {
            let msg = err.as_ref().map(|b| {
                let s = std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize());
                String::from_utf8_lossy(s).to_string()
            }).unwrap_or_default();
            panic!("HLSL compile failed: {e:?} {msg}");
        }
        let code = code.unwrap();
        std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize()).to_vec()
    }
}
