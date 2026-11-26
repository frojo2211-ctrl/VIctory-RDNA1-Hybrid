//! Original GFX10 programs shared with the D3D12 integration fixture.

use super::container::SigElement;
use super::{HostShader, MeshFallback, MeshLayout, lower};
use crate::native::{
    ComputeIo, HwStage, MeshIo, NativeProgram, ResourceType, RootParam, RootType, StageIo, Usage, UsedResource, VsOutput,
};
use crate::{ShaderError, io, ir, resource, structure};

/// A wave32 mesh group that emits an indexed triangle and a material attribute.
pub fn mesh() -> Result<MeshFallback, ShaderError> {
    let mut native = program(false);
    let StageIo::Ms(info) = &mut native.io else { unreachable!() };
    // The original triangle spans clip space. Offset x by the mesh group ID
    // and scale it to make four distinct overlapping triangles per dispatch.
    // v_readfirstlane_b32 s12, v5; v_cvt_f32_u32 v9,s12;
    // v5 supplies group ID times max_primitives (3). Multiply it by
    // 1/16 so each group moves 3/16 NDC; center the four triangles.
    let offset = [
        vop1(0x02, 12, vgpr(5)), vop1(0x06, 9, 12),
        vop2(0x08, 9, LITERAL, 9), 0.0625f32.to_bits(),
        vop2(0x08, 7, LITERAL, 7), 0.5f32.to_bits(),
        vop2(0x03, 7, vgpr(9), 7),
        vop2(0x04, 7, LITERAL, 7), 0.28125f32.to_bits(),
    ];
    let at = native.code.len() - 12;
    native.code.splice(at..at, offset.into_iter().flat_map(u32::to_le_bytes));
    info.max_primitives = 3;
    let sig = outputs();
    let layout = MeshLayout {
        threads: info.threads, max_vertices: info.max_vertices,
        max_primitives: info.max_primitives, vertex_dwords: 4 * sig.len() as u32,
        primitive_dwords: 0,
    };
    let compute = lower_program(&native, sig.clone(), layout.max_primitives, true)?;
    Ok(MeshFallback {
        compute,
        vertex: super::fallback_vertex_shader(&sig, &[], &layout),
        expanded: false, layout,
    })
}

fn lower_program(
    native: &NativeProgram, outputs: Vec<SigElement>, mesh_stride: u32, mesh_as_compute: bool,
) -> Result<HostShader, ShaderError> {
    let p = ir::build(&native.code, native.wave64, native.vgprs)?;
    let st = structure::structure(&p)?;
    let res = resource::track(&p, &native.layout, native.stage, Some(&st))?;
    let io = io::raise(&p, native)?;
    let shader = lower::lower(&lower::Parts {
        native, p: &p, st: &st, io: &io, res: &res,
        inputs: vec![], outputs, prims: vec![], mesh_stride, mesh_as_compute,
    })?;
    Ok(HostShader {
        stage: native.stage, dxil: shader.dxil, user_data: shader.user_data,
        samplers: shader.samplers, static_samplers: vec![], root_cbvs: shader.root_cbvs,
        root_srvs: shader.root_srvs, root_uavs: vec![], root_indexed_views: vec![],
            space_shift: 0,
    })
}

/// Compute the base texture color and two scalar lighting factors.
/// The clamped sample chooses mip 1. The control sample uses explicit mip 1.
/// The scalar switches isolate each V5 addition on the V4 source.
pub fn material(clamp: bool, scalar: bool) -> Result<HostShader, ShaderError> {
    let load = |op: u32, dst: u32, base: u32, offset: u32|
        [0x3D << 26 | op << 18 | dst << 6 | base >> 1, 125 << 25 | offset];
    let mut words = vec![s_mov_b32(1, ZERO), s_mov_b32(3, ZERO)];
    words.extend(load(3, 8, 0, 0)); // image descriptor
    words.extend(load(2, 16, 2, 0)); // sampler
    words.extend(load(2, 20, 0, 32)); // output UAV
    let arguments: &[f32] = if clamp {
        &[0.00390625, 0.0, 0.0, 0.00390625, 0.25, 0.25, 1.0]
    } else {
        &[0.25, 0.25, 1.0]
    };
    for (i, &arg) in arguments.iter().enumerate() {
        words.extend([vop1(1, i as u8, LITERAL), arg.to_bits()]);
    }
    let opcode = if clamp { 0x23 } else { 0x24 }; // image_sample_d_cl / image_sample_l
    words.extend([0xF000_0F08 | opcode << 18, 4 << 21 | 2 << 16 | 20 << 8]);
    let mut roots = vec![(RootType::Table, 0, 0, 1), (RootType::Table, 2, 1, 1)];
    let mut used = vec![
        (ResourceType::Srv, RootType::Table, 0, 0, 0, 8),
        (ResourceType::Uav, RootType::Table, 0, 0, 8, 4),
        (ResourceType::Sampler, RootType::Table, 2, 1, 0, 4),
    ];
    if scalar {
        words.extend(load(0, 24, 4, 12)); // CBV
        words.extend(load(0, 25, 6, 12)); // SRV
        words.extend([vop1(1, 24, 24), vop1(1, 25, 25)]);
        roots.extend([(RootType::Descriptor, 4, 2, 2), (RootType::Descriptor, 6, 3, 2)]);
        used.extend([
            (ResourceType::Cbv, RootType::Descriptor, 4, 2, 0, 4),
            (ResourceType::Srv, RootType::Descriptor, 6, 3, 0, 4),
        ]);
    } else {
        words.extend([vop1(1, 24, ONE_F), vop1(1, 25, ONE_F)]);
    }
    for reg in 20..26u32 {
        words.extend([0x38 << 26 | 0x1C << 18 | ((reg - 20) * 4), 0x80 << 24 | 5 << 16 | reg << 8]);
    }
    words.push(S_ENDPGM);
    let native = NativeProgram {
        stage: HwStage::Cs, wave64: false, vgprs: 32,
        io: StageIo::Cs(ComputeIo { rsrc2: 3 << 1, threads: [1, 1, 1] }),
        layout: roots.into_iter().map(|(root, user_data, slot, sgprs)| RootParam {
            root, user_data, slot, sgprs, table_offset_dwords: 0, base_register: 0,
        }).collect(),
        used: used.into_iter().map(|(kind, root, user_data, slot, table_offset_dwords, dwords)| UsedResource {
            kind, root, user_data, slot, table_offset_dwords, dwords, lower_bound: 0, stride: 0,
        }).collect(),
        code: words.into_iter().flat_map(u32::to_le_bytes).collect(),
    };
    lower_program(&native, vec![], 0, false)
}


pub(super) const S_ENDPGM: u32 = 0xBF81_0000;
/// Inline constants as operands.
pub(super) const ZERO: u16 = 0x80;
pub(super) const ONE: u16 = 0x81;
pub(super) const MINUS_ONE: u16 = 0xC1;
pub(super) const ONE_F: u16 = 0xF2;
pub(super) const TWO_F: u16 = 0xF4;
/// A literal dword follows the instruction.
pub(super) const LITERAL: u16 = 0xFF;

/// SOP1 `s_mov_b32 sdst, src0`.
pub(super) const fn s_mov_b32(sdst: u8, src0: u16) -> u32 {
    0xBE80_0000 | (sdst as u32) << 16 | 0x03 << 8 | (src0 as u32 & 0xFF)
}

/// VOP1 `op vdst, src0` (`src0`: below 256 SGPRs and constants, 256 on VGPRs).
pub(super) const fn vop1(op: u16, vdst: u8, src0: u16) -> u32 {
    0x7E00_0000 | (vdst as u32) << 17 | (op as u32) << 9 | (src0 as u32 & 0x1FF)
}

/// VOP2 `op vdst, src0, vsrc1`.
pub(super) const fn vop2(op: u8, vdst: u8, src0: u16, vsrc1: u8) -> u32 {
    (op as u32) << 25 | (vdst as u32) << 17 | (vsrc1 as u32) << 9 | (src0 as u32 & 0x1FF)
}

/// VOP3 `op vdst, src0, src1, src2` (no modifiers).
pub(super) const fn vop3(op: u16, vdst: u8, src0: u16, src1: u16, src2: u16) -> [u32; 2] {
    [
        0x35 << 26 | (op as u32 & 0x3FF) << 16 | vdst as u32,
        (src0 as u32 & 0x1FF) | (src1 as u32 & 0x1FF) << 9 | (src2 as u32 & 0x1FF) << 18,
    ]
}

/// `exp target, v[srcs]` with the channels `srcs` names enabled.
pub(super) fn exp(target: u8, srcs: [Option<u8>; 4], done: bool) -> [u32; 2] {
    let mut en = 0;
    let mut w1 = 0;
    for (k, s) in srcs.iter().enumerate() {
        if let Some(v) = s {
            en |= 1 << k;
            w1 |= u32::from(*v) << (8 * k);
        }
    }
    [
        0xF800_0000 | u32::from(done) << 11 | u32::from(target) << 4 | en,
        w1,
    ]
}

pub(super) const fn vgpr(n: u8) -> u16 {
    256 + n as u16
}

/// One triangle per group from three lanes. Lane n exports vertex n at
/// (n - 1, 2 (n & 1) - 1, 0, 1) with `TEXCOORD0` (n, 1, 2, 0), and
/// primitive n: lane 0 the triangle (0, 1, 2), lanes 1 and 2 culled ones;
/// with `prims`, primitive n also has the per-primitive `COLOR0`
/// (1, 2, n, 0).
pub(super) fn program(prims: bool) -> NativeProgram {
    let mut w = Vec::new();
    w.extend(vop3(0x365, 0, MINUS_ONE, ZERO, 0)); // v_mbcnt_lo_u32_b32 v0, -1, 0: the lane
    w.push(vop1(0x006, 1, vgpr(0))); // v_cvt_f32_u32 v1, v0
    w.push(vop1(0x001, 2, TWO_F)); // v_mov_b32 v2, 2.0
    w.push(vop1(0x001, 3, ZERO)); // v_mov_b32 v3, 0
    w.push(vop1(0x001, 4, ONE_F)); // v_mov_b32 v4, 1.0
    w.push(vop2(0x05, 7, ONE_F, 1)); // v_subrev_f32 v7, 1.0, v1: x
    w.push(vop2(0x1b, 8, ONE, 0)); // v_and_b32 v8, 1, v0
    w.push(vop1(0x006, 8, vgpr(8))); // v_cvt_f32_u32 v8, v8
    w.push(vop2(0x08, 8, TWO_F, 8)); // v_mul_f32 v8, 2.0, v8
    w.push(vop2(0x05, 8, ONE_F, 8)); // v_subrev_f32 v8, 1.0, v8: y
    // v6 = 0x00200400 + lane * 0x7FDFFC00: lane 0 the indices 0, 1, 2;
    // lanes 1 and 2 have bit 31 set (culled).
    w.extend([s_mov_b32(10, LITERAL), 0x7FDF_FC00]);
    w.extend([s_mov_b32(11, LITERAL), 0x0020_0400]);
    w.extend(vop3(0x169, 6, vgpr(0), 10, 0)); // v_mul_lo_u32 v6, v0, s10
    w.push(vop2(0x25, 6, 11, 6)); // v_add_nc_u32 v6, s11, v6
    w.extend(exp(20, [Some(6), None, None, None], true)); // exp prim
    w.extend(exp(32, [Some(1), Some(4), Some(2), Some(3)], false)); // exp param0
    if prims {
        w.extend(exp(33, [Some(4), Some(2), Some(1), Some(3)], false)); // exp param1
    }
    w.extend(exp(12, [Some(7), Some(8), Some(3), Some(4)], true)); // exp pos0
    w.push(S_ENDPGM);
    let output = |row, param| VsOutput {
        usage: Usage::Generic,
        row,
        param,
        fp16: false,
        pack2: false,
    };
    let mut outputs = vec![output(1, 0)];
    if prims {
        // Bit 7: per-primitive, row 0 of the primitive signature.
        outputs.push(output(0, 0x81));
    }
    NativeProgram {
        stage: HwStage::Ms,
        wave64: false,
        vgprs: 12,
        layout: vec![],
        used: vec![],
        io: StageIo::Ms(MeshIo {
            outputs,
            clip_cntl: 0,
            threads: 3,
            max_vertices: 3,
            max_primitives: 3,
            topology: 2,
            lds_bytes: 0,
        }),
        code: w.iter().flat_map(|w| w.to_le_bytes()).collect(),
    }
}

pub(super) fn element(name: &str, kind: u8, interp: u8, row: u32) -> SigElement {
    SigElement {
        name: name.into(),
        index: 0,
        kind,
        comp: 9,
        interp,
        row: Some(row),
        col: 0,
        cols: 4,
        usage: 0,
        half: false,
    }
}

pub(super) fn outputs() -> Vec<SigElement> {
    vec![
        element("SV_Position", 3, 4, 0),
        element("TEXCOORD", 0, 2, 1),
    ]
}

