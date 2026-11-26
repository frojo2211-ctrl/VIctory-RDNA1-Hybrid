use super::*;
use crate::memstat::{self, Kind, Usage};

const LISTS: usize = 128;
const BATCH: usize = 8;
const PER_LIST: usize = 16;
const SIDE: u32 = 128;
const ROW: u32 = SIDE * 4;
const TEX_BYTES: u64 = (ROW * SIDE) as u64;
const PIXEL_SLOT: u64 = 512;
const ROUNDS: usize = 3;

pub(super) fn committed(host: &Host, heap: D3D12_HEAP_TYPE, desc: &D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES, kind: Kind) -> ID3D12Resource {
    let mut resource = None;
    // SAFETY: valid descriptions and an output owned by this fixture.
    unsafe {
        host.device.CreateCommittedResource(&D3D12_HEAP_PROPERTIES { Type: heap, ..Default::default() },
            D3D12_HEAP_FLAG_NONE, desc, state, None, &mut resource).unwrap();
    }
    let resource = resource.unwrap();
    memstat::resource(&host.device, &resource, kind);
    resource
}

pub(super) fn check_limits(usage: Usage, initial: Usage) {
    assert!(usage.physical_free >= 4 << 30, "STOP: physical memory reserve {usage:?}");
    assert!(usage.commit <= usage.limit * 3 / 4, "STOP: commit reserve {usage:?}");
    assert!(usage.local_budget > 0 && usage.local < usage.local_budget * 3 / 5,
        "STOP: local memory budget {usage:?}");
    assert!(usage.nonlocal_budget > 0 && usage.nonlocal < usage.nonlocal_budget * 7 / 10,
        "STOP: non-local memory budget {usage:?}");
    assert!(usage.private.saturating_sub(initial.private) <= 3 << 30, "STOP: fixture commit ceiling {usage:?}");
}

pub(super) fn peak(mut peak: Usage, current: Usage) -> Usage {
    peak.private = peak.private.max(current.private);
    peak.working_set = peak.working_set.max(current.working_set);
    peak.commit = peak.commit.max(current.commit);
    peak.local = peak.local.max(current.local);
    peak.nonlocal = peak.nonlocal.max(current.nonlocal);
    peak.physical_free = peak.physical_free.min(current.physical_free);
    peak
}

pub(super) unsafe fn location(resource: &ID3D12Resource, footprint: Option<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: borrowed COM pointer for the copy, owned by the fixture.
        pResource: unsafe { std::mem::transmute_copy(resource) },
        Type: if footprint.is_some() { D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT } else { D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX },
        Anonymous: footprint.map_or(D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            |placed| D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: placed }),
    }
}

pub(super) fn footprint(offset: u64, side: u32, row: u32) -> D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT { Offset: offset, Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
        Format: DXGI_FORMAT_R8G8B8A8_UNORM, Width: side, Height: side, Depth: 1, RowPitch: row,
    } }
}

#[test]
#[ignore = "allocates thousands of native resources with memory limits"]
fn stream_textures_and_recycle_mesh_at_scale() {
    assert!(memstat::enabled(), "set VICTORY_MEMORY_STATS=1");
    let host = host().expect("RDNA1 D3D12 device");
    let adapter = memstat::adapter(&host.device).expect("DXGI adapter");
    let initial = memstat::usage(&adapter).unwrap();
    check_limits(initial, initial);
    let timer = std::time::Instant::now();
    // SAFETY: a plain adapter query.
    let desc = unsafe { adapter.GetDesc1().unwrap() };
    let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
    eprintln!("adapter={} rounds={ROUNDS} lists={LISTS} batch={BATCH} textures={} initial={initial:?}",
        String::from_utf16_lossy(&desc.Description[..end]), LISTS * PER_LIST);

    let tex_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D, Width: u64::from(SIDE), Height: SIDE,
        DepthOrArraySize: 1, MipLevels: 1, Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
    };
    let mut textures = Vec::new();
    for i in 0..LISTS * PER_LIST {
        if i % 128 == 0 { check_limits(memstat::usage(&adapter).unwrap(), initial); }
        textures.push(committed(host, D3D12_HEAP_TYPE_DEFAULT, &tex_desc,
            D3D12_RESOURCE_STATE_COPY_DEST, Kind::Texture));
    }
    let lists: Vec<_> = (0..LISTS).map(|_| host.new_list().unwrap()).collect();
    let readback = committed(host, D3D12_HEAP_TYPE_READBACK, &D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: PIXEL_SLOT * textures.len() as u64,
        Height: 1, DepthOrArraySize: 1, MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ..Default::default()
    }, D3D12_RESOURCE_STATE_COPY_DEST, Kind::Readback);

    let source = br#"
        RWByteAddressBuffer mesh : register(u0);
        cbuffer Bounds : register(b0) { uint last; uint tag; };
        [RootSignature("UAV(u0),RootConstants(num32BitConstants=2,b0)")]
        [numthreads(1,1,1)]
        void main() { mesh.Store(0,tag); mesh.Store(last,tag); }
    "#;
    // SAFETY: static source and shader entry points, outputs owned by the fixture.
    let (root, pipelines, second, fence) = unsafe {
        let mut code: Option<ID3DBlob> = None;
        D3DCompile(source.as_ptr().cast(), source.len(), s!("memory_scale"), None, None,
            s!("main"), s!("cs_5_1"), 0, 0, &mut code, None).unwrap();
        let code = code.unwrap();
        let bytes = std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize());
        let root: ID3D12RootSignature = host.device.CreateRootSignature(0, bytes).unwrap();
        let compute_desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE { pShaderBytecode: bytes.as_ptr().cast(), BytecodeLength: bytes.len() },
            ..Default::default()
        };
        let pipelines: Vec<ID3D12PipelineState> = (0..192).map(|_| {
            let pso: ID3D12PipelineState = host.device.CreateComputePipelineState(&compute_desc).unwrap();
            memstat::object(&pso, Kind::Pipeline);
            pso
        }).collect();
        let second: ID3D12CommandQueue = host.device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default()
        }).unwrap();
        let fence: ID3D12Fence = host.device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        (root, pipelines, second, fence)
    };
    let mut high = initial;
    let mut sequence = 0u64;
    let mut checked = 0usize;
    for round in 0..ROUNDS {
        for (batch_index, batch) in lists.as_chunks::<BATCH>().0.iter().enumerate() {
            let before = memstat::usage(&adapter).unwrap();
            check_limits(before, initial);
            for (offset, list) in batch.iter().enumerate() {
                let i = batch_index * BATCH + offset;
                let size = if i.is_multiple_of(4) { 32 << 20 } else { 1 << 20 };
                let mesh = list.mesh_scratch(size, ScratchPool::Data).expect("mesh scratch");
                let l = &list.list;
                // SAFETY: an open list, valid staging and mesh resources
                // retained until the fence, and non-overlapping copy ranges.
                unsafe {
                    if mesh.first {
                        l.ResourceBarrier(&[transition(&mesh.buffer, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)]);
                    }
                    l.SetComputeRootSignature(&root);
                    l.SetPipelineState(&pipelines[(round * LISTS + i) % pipelines.len()]);
                    l.SetComputeRootUnorderedAccessView(0, mesh.buffer.GetGPUVirtualAddress() + mesh.offset);
                    let constants = [(size - 4) as u32, i as u32 + 1];
                    l.SetComputeRoot32BitConstants(1, 2, constants.as_ptr().cast(), 0);
                    l.Dispatch(1, 1, 1);
                    let start = i * PER_LIST;
                    for (index, texture) in textures[start..start + PER_LIST].iter().enumerate() {
                        let t = start + index;
                        let color = 0xff00_0000 | ((round as u32 + 1) << 16) | t as u32;
                        let (buffer, at, mapped) = list.upload(host, TEX_BYTES).expect("texture upload");
                        std::slice::from_raw_parts_mut(mapped.cast::<u32>(), TEX_BYTES as usize / 4).fill(color);
                        let dst = location(texture, None);
                        let src = location(&buffer, Some(footprint(at, SIDE, ROW)));
                        l.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
                        l.ResourceBarrier(&[transition(texture, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
                        let dst = location(&readback, Some(footprint(t as u64 * PIXEL_SLOT, 1, 256)));
                        let center = SIDE / 2;
                        let rect = D3D12_BOX { left: center, top: center, front: 0, right: center+1, bottom: center+1, back: 1 };
                        l.CopyTextureRegion(&dst, 0, 0, 0, &location(texture, None), Some(&rect));
                        l.ResourceBarrier(&[transition(texture, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST)]);
                    }
                }
                list.close();
            }

            let after_record = memstat::usage(&adapter).unwrap();
            high = peak(high, after_record);
            check_limits(after_record, initial);
            sequence += 1;
            // SAFETY: the secondary queue runs the first closed list. The
            // primary waits for its signal before running the remaining lists.
            unsafe {
                second.ExecuteCommandLists(&[Some(batch[0].list.cast().unwrap())]);
                second.Signal(&fence, sequence).unwrap();
                host.queue.Wait(&fence, sequence).unwrap();
            }
            let runs: Vec<_> = batch[1..].iter().collect();
            assert_eq!(host.execute_and_report(&runs), None);
            // SAFETY: waiting for the primary fence also proved the secondary
            // completed; every sample lies within its own placement.
            unsafe {
                assert_eq!(fence.GetCompletedValue(), sequence);
                let mut data = std::ptr::null_mut();
                readback.Map(0, None, Some(&mut data)).unwrap();
                for t in batch_index * BATCH * PER_LIST..(batch_index + 1) * BATCH * PER_LIST {
                    let actual = data.cast::<u8>().add(t * PIXEL_SLOT as usize).cast::<u32>().read();
                    assert_eq!(actual, 0xff00_0000 | ((round as u32 + 1) << 16) | t as u32);
                    checked += 1;
                }
                readback.Unmap(0, None);
            }
            batch.iter().for_each(HostList::reset);
            high = peak(high, memstat::usage(&adapter).unwrap());
        }
        eprintln!("round={round} after_fences={:?} upload={:?} mesh={:?}",
            memstat::usage(&adapter).unwrap(), memstat::sample(Kind::Upload), memstat::sample(Kind::Mesh));
    }
    eprintln!("elapsed={:?} pixels={checked} peak={high:?}", timer.elapsed());
    for kind in [Kind::Texture, Kind::Upload, Kind::Mesh, Kind::Shared, Kind::Readback, Kind::Allocator, Kind::List, Kind::Pipeline, Kind::Signature] {
        eprintln!("objects {kind:?} (created, live, bytes, peak, total) {:?}", memstat::sample(kind));
    }
    drop(lists);
    drop(textures);
    drop(pipelines);
    drop(readback);
    eprintln!("released upload={:?} mesh={:?} texture={:?} usage={:?}",
        memstat::sample(Kind::Upload), memstat::sample(Kind::Mesh), memstat::sample(Kind::Texture),
        memstat::usage(&adapter).unwrap());
    assert_eq!(checked, ROUNDS * LISTS * PER_LIST);
    eprintln!("skipping RDNA1 allocation-injection recovery subtest on CGBJ host");
}
