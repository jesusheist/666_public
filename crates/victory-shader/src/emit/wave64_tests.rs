//! A console wave64 compute program on a host of 32-lane waves: a wave reduction inside a
//! per-lane branch, with that branch taken wave-wide ([`crate::structure::structure_wide`])
//! so that the two host waves can exchange their halves inside it. Lowered, validated by
//! `dxil.dll` when present, and run on the host GPU when it takes waves of 32.

use std::collections::HashSet;

use super::host::{Dispatch, Host, Root, View};
use super::{HostShader, lower, validator};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{ShaderError, io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;

/// 64 threads in one console wave. Each thread t holds the bit `1 << (t >> 1)`; the threads
/// below s0 (a root constant) OR theirs over the wave inside `if (t < s0) { ... }` and store
/// the result to dword 0 of the raw UAV.
/// `skip`: the branch past the body when no thread is below s0: `s_cbranch_execz`, or
/// `s_cbranch_scc0` on the SCC `s_and_saveexec` sets ("some lane is left").
fn program(skip: u32) -> NativeProgram {
    let mut words = vec![
        0x7E0A_0281, // v_mov_b32 v5, 1
        0x2C0C_0081, // v_lshrrev_b32 v6, 1, v0
        0x340E_0B06, // v_lshlrev_b32 v7, v6, v5
        0x7D88_0000, // v_cmp_gt_u32 vcc, s0, v0
        0xBE8C_246A, // s_and_saveexec_b64 s[12:13], vcc
        skip | 23,   // the branch to the restore of EXEC
    ];
    words.extend_from_slice(&super::wave::WAVE_OR_V7_TO_S7);
    words.extend_from_slice(&[
        0x7E00_0207, // v_mov_b32 v0, s7
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
        0xBEFE_040C, // s_mov_b64 exec, s[12:13]
    ]);
    native(&words)
}

/// A wave64 compute program of 64 threads (one console wave; the thread index in v0) with 4
/// root constants in s0..s3 and a raw UAV's descriptor in s[8:11], then `body`.
fn native(body: &[u32]) -> NativeProgram {
    native_of(body, true, 64)
}

/// The same for a console of 32-lane waves when `wave64` is not set, with `threads` threads.
fn native_of(body: &[u32], wave64: bool, threads: u32) -> NativeProgram {
    native_lds(body, wave64, threads, 0)
}

/// [`native_of`] with `granules` x 512 bytes of LDS.
fn native_lds(body: &[u32], wave64: bool, threads: u32, granules: u32) -> NativeProgram {
    let mut words = vec![
        0xBE85_0380, // s_mov_b32 s5, 0
        0x3D << 26 | 0x02 << 18 | 8 << 6 | 2,
        125 << 25, // s_load_dwordx4 s[8:11], s[4:5], 0
    ];
    words.extend_from_slice(body);
    words.push(S_ENDPGM);
    NativeProgram {
        stage: HwStage::Cs,
        wave64,
        vgprs: 16,
        layout: vec![
            RootParam {
                root: RootType::Constants,
                table_offset_dwords: 0,
                sgprs: 4,
                base_register: 0,
                user_data: 0,
                slot: 0,
            },
            RootParam {
                root: RootType::Table,
                table_offset_dwords: 0,
                sgprs: 1,
                base_register: 0,
                user_data: 4,
                slot: 1,
            },
        ],
        used: vec![UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 4,
            slot: 1,
        }],
        io: StageIo::Cs(ComputeIo {
            rsrc2: 5 << 1 | granules << 15, // 5 user SGPRs: s0..s4
            threads: [threads, 1, 1],
        }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    }
}

/// `program()` for a host of 32-lane waves, with the branches closing `wide` taken wave-wide.
fn lowered(native: &NativeProgram, p: &ir::Program, wide: &HashSet<usize>) -> Result<HostShader, ShaderError> {
    lowered_on(native, p, wide, 32)
}

/// [`lowered`] for a host whose waves have at most `host_wave` lanes.
fn lowered_on(
    native: &NativeProgram,
    p: &ir::Program,
    wide: &HashSet<usize>,
    host_wave: u32,
) -> Result<HostShader, ShaderError> {
    lowered_counted(native, p, wide, host_wave, false)
}

fn lowered_counted(
    native: &NativeProgram,
    p: &ir::Program,
    wide: &HashSet<usize>,
    host_wave: u32,
    counted: bool,
) -> Result<HostShader, ShaderError> {
    lowered_with(native, p, wide, host_wave, counted, None).map(|(shader, _)| shader)
}

/// [`lowered_counted`], and the wave size the shader asks of the host; `forced`: what
/// `VICTORY_WAVE_WIDE=1` is for the lowering (None: as the process has it).
fn lowered_with(
    native: &NativeProgram,
    p: &ir::Program,
    wide: &HashSet<usize>,
    host_wave: u32,
    counted: bool,
    forced: Option<bool>,
) -> Result<(HostShader, Option<u32>), ShaderError> {
    let st = structure::structure_wide(p, wide).expect("structure");
    let res = resource::track(p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(p, native).expect("io::raise");
    let was = lower::TEST_HOST_WAVE.replace(Some(host_wave));
    let was_wide = lower::TEST_WIDE.replace(forced);
    let lowered = lower::lower_counted(&lower::Parts {
        native,
        p,
        st: &st,
        io: &io,
        res: &res,
        inputs: Vec::new(),
        outputs: Vec::new(),
        prims: Vec::new(),
        mesh_stride: 0,
    }, counted);
    lower::TEST_HOST_WAVE.set(was);
    lower::TEST_WIDE.set(was_wide);
    let lowered = lowered?;
    let wave = lowered.wave_size;
    Ok((HostShader {
        stage: HwStage::Cs,
        dxil: lowered.dxil,
        user_data: lowered.user_data,
        samplers: lowered.samplers,
        static_samplers: Vec::new(),
        root_cbvs: lowered.root_cbvs,
        root_srvs: lowered.root_srvs,
        root_uavs: lowered.root_uavs,
        root_indexed_views: res.root_indexed_views,
        half_views: Vec::new(),
    }, wave))
}

#[test]
fn a_sum_of_halves_over_the_wave_is_the_consoles_tree_bit_for_bit() {
    // thread t holds the 16-bit float with bits 0x3C00 + 37 t (1.0 and up, with odd
    // mantissas, so that additions round); all of them store the wave's sum
    let mut words = vec![
        0x340C_0085, // v_lshlrev_b32 v6, 5, v0
        0x340E_0082, // v_lshlrev_b32 v7, 2, v0
        0x4A0E_0F06, // v_add_nc_u32 v7, v6, v7
        0x4A0E_0F00, // v_add_nc_u32 v7, v0, v7
        0x4A0E_0EFF,
        0x0000_3C00, // v_add_nc_u32 v7, 0x3c00, v7
    ];
    // (the wave OR's words with v_add_f16 for v_or_b32: opcode 50 for 28)
    words.extend(super::wave::WAVE_OR_V7_TO_S7.iter().map(|&w| if w >> 25 == 28 { w & 0x01FF_FFFF | 50 << 25 } else { w }));
    words.extend_from_slice(&[
        0x7E00_0207, // v_mov_b32 v0, s7
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
    ]);
    let native = native(&words);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    assert_eq!(p.blocks.iter().flat_map(|b| &b.insts).filter(|i| i.name() == "v_add_f16").count(), 6);
    let shader = lowered(&native, &p, &HashSet::new()).expect("lower");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    // the console's sum: lanes 2k and 2k+1, then pairs, ... each addition rounded to 16 bits
    // (to nearest, ties to even); values here are normal numbers
    let wide = |h: u16| f32::from_bits((u32::from(h >> 10) + 112) << 23 | u32::from(h & 0x3FF) << 13);
    let narrow = |f: f32| {
        let b = f.to_bits();
        let h = ((b >> 23) - 112) << 10 | (b >> 13 & 0x3FF);
        let rest = b & 0x1FFF;
        (h + u32::from(rest > 0x1000 || rest == 0x1000 && h & 1 == 1)) as u16
    };
    let add = |a: u16, b: u16| narrow(wide(a) + wide(b));
    let terms: Vec<u16> = (0..64u16).map(|t| 0x3C00 + 37 * t).collect();
    let mut tree = terms.clone();
    while tree.len() > 1 {
        tree = tree.chunks(2).map(|c| add(c[0], c[1])).collect();
    }
    let in_a_row = terms.iter().copied().reduce(add).unwrap();
    assert_ne!(tree[0], in_a_row, "these terms tell the tree from another order");
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return;
    };
    let (root, pso) = match host.compute_pipeline(&shader) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("the hardware device does not take this program (waves of 32?): {e}; not run");
            return;
        }
    };
    let ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(1)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
    // (the high half is lane 0's term's: zero)
    assert_eq!(out[0][0], u32::from(tree[0]), "tree {:#x}, in a row {in_a_row:#x}", tree[0]);
    eprintln!("ran on the hardware device: the sum {:#x} of the console's tree", tree[0]);
}

/// "Some lane is left" of a mask operation as a number: an upscaler's programs wait for
/// other groups in a loop that ends when every lane saw its flag, or after enough rounds:
/// both tests become -1 or 0 and SCC of `timeout | ~waiting` ends it.
#[test]
fn some_lane_of_a_mask_is_a_number_the_whole_wave_agrees_on() {
    let native = native(&[
        0xD765_0001,
        0x0001_00C1, // v_mbcnt_lo_u32_b32 v1, -1, 0
        0xD766_0001,
        0x0002_02C1, // v_mbcnt_hi_u32_b32 v1, -1, v1
        0x7D84_0200, // v_cmp_eq_u32 vcc, s0, v1
        0x8792_7E6A, // s_and_b64 s[18:19], vcc, exec
        0x8592_80C1, // s_cselect_b64 s[18:19], -1, 0
        0xBF09_8501, // s_cmp_ge_u32 s1, 5
        0x8594_80C1, // s_cselect_b64 s[20:21], -1, 0
        0x8B92_1214, // s_orn2_b64 s[18:19], s[20:21], s[18:19]
        0x8506_8081, // s_cselect_b32 s6, 1, 0
        0x7E04_0206, // v_mov_b32 v2, s6
        0xE070_0000,
        0x8002_0200, // buffer_store_dword v2, off, s[8:11], 0
    ]);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let shader = on_64_lanes(&native, &p);
    // (lane s0 waits, if there is one; s1 rounds)
    for (wait, rounds, done) in [(0, 0, 0), (37, 4, 0), (63, 4, 0), (64, 0, 1), (37, 5, 1), (64, 9, 1)] {
        let Some(out) = dispatched(&shader, &[wait, rounds]) else {
            return;
        };
        assert_eq!(out, done, "lane {wait} after {rounds} rounds");
    }
}

/// `native` for a host of 64-lane waves.
fn on_64_lanes(native: &NativeProgram, p: &ir::Program) -> HostShader {
    let st = structure::structure(p).expect("structure");
    let res = resource::track(p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(p, native).expect("io::raise");
    let was = lower::TEST_HOST_WAVE.replace(Some(64));
    let lowered = lower::lower(&lower::Parts {
        native,
        p,
        st: &st,
        io: &io,
        res: &res,
        inputs: Vec::new(),
        outputs: Vec::new(),
        prims: Vec::new(),
        mesh_stride: 0,
    });
    lower::TEST_HOST_WAVE.set(was);
    let lowered = lowered.expect("lower");
    assert_eq!(lowered.wave_size, Some(64));
    let shader = HostShader {
        stage: HwStage::Cs,
        dxil: lowered.dxil,
        user_data: lowered.user_data,
        samplers: lowered.samplers,
        static_samplers: Vec::new(),
        root_cbvs: lowered.root_cbvs,
        root_srvs: lowered.root_srvs,
        root_uavs: lowered.root_uavs,
        root_indexed_views: res.root_indexed_views,
        half_views: Vec::new(),
    };
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    shader
}

/// Dword 0 of the UAV after `shader` ran with root constants `constants`; none without a
/// device that takes it.
fn dispatched(shader: &HostShader, constants: &[u32]) -> Option<u32> {
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return None;
    };
    let (root, pso) = match host.compute_pipeline(shader) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("the hardware device does not take this program (waves of 64?): {e}; not run");
            return None;
        }
    };
    let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    for u in &shader.user_data {
        if u.root == RootType::Constants {
            ud[u.index as usize] = constants.get(u.dword as usize).copied().unwrap_or(0);
        }
    }
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(1)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
    Some(out[0][0])
}

/// The same sum inside `if (t >= s0) { ... }` on a host of 64-lane waves: the lanes below s0
/// are elsewhere, and count as +0 in the console's tree.
#[test]
fn a_sum_of_halves_where_not_every_lane_runs_on_a_wave_of_64() {
    let mut words = vec![
        0x340C_0085, // v_lshlrev_b32 v6, 5, v0
        0x340E_0082, // v_lshlrev_b32 v7, 2, v0
        0x4A0E_0F06, // v_add_nc_u32 v7, v6, v7
        0x4A0E_0F00, // v_add_nc_u32 v7, v0, v7
        0x4A0E_0EFF,
        0x0000_3C00, // v_add_nc_u32 v7, 0x3c00, v7
        0x7D86_0000, // v_cmp_le_u32 vcc, s0, v0
        0xBE8C_246A, // s_and_saveexec_b64 s[12:13], vcc
        0xBF88_0000 | 23, // s_cbranch_execz to the restore of EXEC
    ];
    words.extend(super::wave::WAVE_OR_V7_TO_S7.iter().map(|&w| if w >> 25 == 28 { w & 0x01FF_FFFF | 50 << 25 } else { w }));
    words.extend_from_slice(&[
        0x7E00_0207, // v_mov_b32 v0, s7
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
        0xBEFE_040C, // s_mov_b64 exec, s[12:13]
    ]);
    let native = native(&words);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let shader = on_64_lanes(&native, &p);
    let wide = |h: u16| match h {
        0 => 0.0,
        h => f32::from_bits((u32::from(h >> 10) + 112) << 23 | u32::from(h & 0x3FF) << 13),
    };
    let narrow = |f: f32| {
        if f == 0.0 {
            return 0;
        }
        let b = f.to_bits();
        let h = ((b >> 23) - 112) << 10 | (b >> 13 & 0x3FF);
        let rest = b & 0x1FFF;
        (h + u32::from(rest > 0x1000 || rest == 0x1000 && h & 1 == 1)) as u16
    };
    for from in [0u32, 1, 5, 31, 32, 37, 63] {
        let mut tree: Vec<u16> = (0..64u16).map(|t| if u32::from(t) < from { 0 } else { 0x3C00 + 37 * t }).collect();
        while tree.len() > 1 {
            tree = tree.chunks(2).map(|c| narrow(wide(c[0]) + wide(c[1]))).collect();
        }
        let Some(out) = dispatched(&shader, &[from]) else {
            return;
        };
        assert_eq!(out, u32::from(tree[0]), "lanes from {from}");
    }
}

/// The console's 16-bit wave sum where the lanes at or past s0 have left the program (a
/// thread past the end of the data): on the console they add +0, and after each step of the
/// tree they hold their run's partial sum all the same. On a host of 64-lane waves (AMD) the
/// lanes that are here read each partner run's sum from a lane of it that is here.
#[test]
fn a_sum_of_halves_where_lanes_have_left_is_the_consoles_tree_bit_for_bit() {
    let mut words = vec![
        0x7DA8_0000, // v_cmpx_gt_u32 exec, s0, v0: the lanes below s0 stay
        0, // s_cbranch_execz to the end (filled in below)
        0x340C_0085, // v_lshlrev_b32 v6, 5, v0
        0x340E_0082, // v_lshlrev_b32 v7, 2, v0
        0x4A0E_0F06, // v_add_nc_u32 v7, v6, v7
        0x4A0E_0F00, // v_add_nc_u32 v7, v0, v7
        0x4A0E_0EFF,
        0x0000_3C00, // v_add_nc_u32 v7, 0x3c00, v7
    ];
    words.extend(super::wave::WAVE_OR_V7_TO_S7.iter().map(|&w| if w >> 25 == 28 { w & 0x01FF_FFFF | 50 << 25 } else { w }));
    words.extend_from_slice(&[
        0x7E00_0207, // v_mov_b32 v0, s7
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
    ]);
    // past every word after it, to the program's end
    words[1] = 0xBF88_0000 | (words.len() as u32 - 2);
    let native = native(&words);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let shader = lowered_on(&native, &p, &HashSet::new(), 64).expect("lower");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    let wide = |h: u16| {
        if h == 0 {
            0.0
        } else {
            f32::from_bits((u32::from(h >> 10) + 112) << 23 | u32::from(h & 0x3FF) << 13)
        }
    };
    let narrow = |f: f32| {
        if f == 0.0 {
            return 0;
        }
        let b = f.to_bits();
        let h = ((b >> 23) - 112) << 10 | (b >> 13 & 0x3FF);
        let rest = b & 0x1FFF;
        (h + u32::from(rest > 0x1000 || rest == 0x1000 && h & 1 == 1)) as u16
    };
    let add = |a: u16, b: u16| narrow(wide(a) + wide(b));
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut ran = false;
    for stay in [64u32, 63, 40, 33, 32, 17, 5, 1] {
        let terms: Vec<u16> = (0..64u32)
            .map(|t| if t < stay { (0x3C00 + 37 * t) as u16 } else { 0 })
            .collect();
        let mut tree = terms.clone();
        while tree.len() > 1 {
            tree = tree.chunks(2).map(|c| add(c[0], c[1])).collect();
        }
        let Some(got) = on_the_card(&shader, [stay, 0]) else {
            return;
        };
        ran = true;
        // (the high half is lane 0's term's: zero)
        assert_eq!(got, u32::from(tree[0]), "{stay} lanes stay: got {got:#x}, the console's tree {:#x}", tree[0]);
    }
    if ran {
        eprintln!("ran on the hardware device: the console's tree with lanes gone, in every case");
    }
}

/// One `v_add_f16` whose exact result lies between two halves: 1 + 3/1024 (0x3C03) plus 2
/// (0x4000) is 3 + 1.5 steps of that range, which the console rounds up to 0x4202. The
/// host's `f32tof16` may give 0x4201 (toward zero); with `VICTORY_F16_NEAREST=1` the
/// translation gives the console's whatever the host does.
#[test]
fn a_16_bit_sum_rounds_to_nearest_when_asked_to() {
    let words = [
        0x7E0E_02FF,
        0x0000_3C03, // v_mov_b32 v7, 0x3c03
        0x7E0C_02FF,
        0x0000_4000, // v_mov_b32 v6, 0x4000
        0x6400_0D07, // v_add_f16 v0, v7, v6
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
    ];
    let native = native(&words);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    assert!(p.blocks.iter().flat_map(|b| &b.insts).any(|i| i.name() == "v_add_f16"));
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for nearest in [false, true] {
        let was = lower::TEST_F16_NEAREST.replace(Some(nearest));
        let shader = lowered(&native, &p, &HashSet::new());
        lower::TEST_F16_NEAREST.set(was);
        let shader = shader.expect("lower");
        if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
            let mut copy = shader.dxil.clone();
            v.validate(&mut copy).expect("DXIL validation failed");
        }
        let Ok(host) = Host::new(false) else {
            eprintln!("no SM 6.6 hardware device; not run");
            return;
        };
        let Ok((root, pso)) = host.compute_pipeline(&shader) else {
            eprintln!("the hardware device does not take this program; not run");
            return;
        };
        let ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
        let out = host
            .dispatch(
                &root,
                &pso,
                &Dispatch {
                    roots: vec![(0, Root::Constants(ud))],
                    views: &[View::RawUav(1)],
                    samplers: &[],
                    groups: [1, 1, 1],
                },
            )
            .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
        let got = out[0][0] & 0xFFFF;
        eprintln!("v_add_f16 0x3c03 + 0x4000 on the hardware device, nearest {nearest}: {got:#x}");
        if nearest {
            assert_eq!(got, 0x4202);
        } else {
            assert!(got == 0x4201 || got == 0x4202, "{got:#x}");
        }
    }
}

#[test]
fn a_wave_or_inside_a_lane_branch_needs_the_branch_wave_wide() {
    for (skip, name) in [(0xBF88_0000, "s_cbranch_execz"), (0xBF84_0000, "s_cbranch_scc0")] {
        a_wave_or_inside(skip, name);
    }
}

fn a_wave_or_inside(skip: u32, name: &str) {
    let native = program(skip);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let branch = (0..p.blocks.len())
        .find(|&b| p.blocks[b].insts.last().is_some_and(|i| i.name() == name))
        .expect("the branch");
    // per lane, the two host waves cannot meet inside the branch
    lower::take_needs();
    let per_lane = lowered(&native, &p, &HashSet::new());
    assert!(
        matches!(&per_lane, Err(ShaderError::Unsupported { what, .. }) if *what == "a wave reduction where not every lane runs"),
        "{:?}",
        per_lane.err()
    );
    // ... and the lowering names the block, around which that branch is the one to widen
    let needs = lower::take_needs();
    let st = structure::structure(&p).expect("structure");
    assert_eq!(lower::branches_around(&st, &needs), Some(vec![branch]));
    // (taken, past the body, when every lane's bit is clear)
    assert_eq!(st.wave_any[branch], Some(false));
    // wave-wide, every lane runs the reduction (EXEC stays a guard on what it does there)
    let wide = HashSet::from([branch]);
    let st = structure::structure_wide(&p, &wide).expect("structure");
    let inside = match p.blocks[branch].exit {
        ir::Exit::Branch { not_taken, .. } => not_taken,
        _ => unreachable!(),
    };
    assert!(st.guards[inside].iter().any(Option::is_some));
    let shader = lowered(&native, &p, &wide).expect("lower");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    // On a GPU that takes waves of 32 (two host waves for the console's one): the OR is of
    // both halves, whichever half's lanes take the branch.
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return;
    };
    let (root, pso) = match host.compute_pipeline(&shader) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("the hardware device does not take this program (waves of 32?): {e}; not run");
            return;
        }
    };
    // (threads below s0 take the branch; thread t gives bit t >> 1)
    for (below, expected) in [(64u32, u32::MAX), (40, 0x000F_FFFF), (33, 0x0001_FFFF), (10, 0x1F), (1, 1)] {
        let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
        for u in &shader.user_data {
            if let (RootType::Constants, 0, 0) = (u.root, u.slot, u.dword) {
                ud[u.index as usize] = below;
            }
        }
        let out = host
            .dispatch(
                &root,
                &pso,
                &Dispatch {
                    roots: vec![(0, Root::Constants(ud))],
                    views: &[View::RawUav(1)],
                    samplers: &[],
                    groups: [1, 1, 1],
                },
            )
            .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
        assert_eq!(out[0][0], expected, "{name}: {below} threads in the branch");
    }
    eprintln!("ran on the hardware device ({name}): the OR of both halves in every case");
}

/// Runs `shader` on the hardware device with the root constants `s0`, `s1` and gives dword 0
/// of the UAV; None (and a line) without a device that takes it.
fn on_the_card(shader: &HostShader, constants: [u32; 2]) -> Option<u32> {
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return None;
    };
    let Ok((root, pso)) = host.compute_pipeline(shader) else {
        eprintln!("the hardware device does not take this program; not run");
        return None;
    };
    let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    for u in &shader.user_data {
        if let (RootType::Constants, 0, d @ 0..=1) = (u.root, u.slot, u.dword) {
            ud[u.index as usize] = constants[d as usize];
        }
    }
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(1)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
    Some(out[0][0])
}

/// A 32-lane console wave. A register holds a lane mask on one path (the lanes below s0)
/// and a number on the other, and is read as a number after the join, as the trace programs
/// keep their lanes' masks and their node numbers on one stack
/// (`deferred_tiled:CS_LaserLightShadowRaytracing`, s46 at 0x104, 0x75c and 0x188). The
/// mask arrives as the ballot of its lanes.
#[test]
fn a_lane_mask_joined_with_a_number_is_read_as_the_number_it_is() {
    let words = [
        0x7D88_0000, // v_cmp_gt_u32 vcc_lo, s0, v0
        0xBE94_036A, // s_mov_b32 s20, vcc_lo
        0xBF06_8001, // s_cmp_eq_u32 s1, 0
        0xBF85_0001, // s_cbranch_scc1 over the next
        0xBE94_0385, // s_mov_b32 s20, 5
        0x7E00_0214, // v_mov_b32 v0, s20
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
    ];
    let native = native_of(&words, false, 32);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let shader = lowered(&native, &p, &HashSet::new()).expect("lower");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for (below, other, expected) in [(32u32, 0u32, u32::MAX), (10, 0, 0x3FF), (0, 0, 0), (10, 1, 5)] {
        let Some(got) = on_the_card(&shader, [below, other]) else { return };
        assert_eq!(got, expected, "{below} lanes in the mask, s1 = {other}");
    }
    eprintln!("ran on the hardware device: the mask's bits, or the number");
}

/// A 32-lane console wave. `if (t < s0) { ... SCC = (s1 == 7) }` and a branch on SCC after
/// the join: the skip brings the SCC of `s_and_saveexec` ("some lane is left") along
/// (`rtdirectionalshadows:CS_RTShadowTrace32HQ_TiledFarOnly`, 0x360-0x39c). On the console
/// the skip is taken by the wave, so with some lane inside every lane sees the inner test.
/// A lane that skipped on its own would see "some lane is left" instead: refused per lane,
/// right with the branch taken wave-wide. Lane 31 stores what it saw.
#[test]
fn scc_of_a_mask_operation_joined_with_a_scalar_test_needs_the_skip_wave_wide() {
    let words = [
        0x7D88_0000, // v_cmp_gt_u32 vcc_lo, s0, v0
        0xBE8D_3C6A, // s_and_saveexec_b32 s13, vcc_lo
        0xBF88_0001, // s_cbranch_execz over the next
        0xBF06_8701, // s_cmp_eq_u32 s1, 7
        0xBEFE_030D, // s_mov_b32 exec_lo, s13
        0xBF85_0002, // s_cbranch_scc1 to "seven"
        0x7E0A_0281, // v_mov_b32 v5, 1
        0xBF82_0001, // s_branch over the next
        0x7E0A_0282, // seven: v_mov_b32 v5, 2
        0x7DA4_009F, // v_cmpx_eq_u32 exec_lo, 31, v0
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16 | 5 << 8, // buffer_store_dword v5, off, s[8:11], 0
    ];
    let native = native_of(&words, false, 32);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let branch = (0..p.blocks.len())
        .find(|&b| p.blocks[b].insts.last().is_some_and(|i| i.name() == "s_cbranch_execz"))
        .expect("the skip");
    lower::take_needs();
    let per_lane = lowered(&native, &p, &HashSet::new());
    assert!(
        matches!(&per_lane, Err(ShaderError::Unsupported { what, .. }) if *what == "a lane mask read as data"),
        "{:?}",
        per_lane.err()
    );
    let st = structure::structure(&p).expect("structure");
    assert_eq!(lower::branches_around(&st, &lower::take_needs()), Some(vec![branch]));
    let shader = lowered(&native, &p, &HashSet::from([branch])).expect("lower");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    // (lanes below s0 inside, s1) -> what lane 31 stores: 2 if SCC was set at the branch
    for (below, s1, expected) in [(0u32, 7u32, 1u32), (10, 7, 2), (10, 3, 1), (32, 3, 1), (32, 7, 2)] {
        let Some(got) = on_the_card(&shader, [below, s1]) else { return };
        assert_eq!(got, expected, "{below} lanes inside, s1 = {s1}");
    }
    eprintln!("ran on the hardware device: the inner test's SCC for every lane once some lane is inside");
}

/// An `s_cbranch_execz` guarding a loop whose exit is `v_cmp_eq_u32` + `s_cbranch_vccz`
/// must not be stripped by `ir::without_skips` (bbfe4261 of the upstream): when no lane
/// enters the loop (`EXEC == 0`), `v_cmp` always writes 0 to `vcc`, so `s_cbranch_vccz` loops
/// until the synthetic loop limit and aborts the shader before the store after the loop runs.
#[test]
fn without_skips_keeps_execz_guard_over_vcc_loop() {
    // if (tid < s0) { loop { v5 += 1; if (v5 == 3) break; } }  store(7 + v5)
    let words = [
        0x7E0A_0280, // v_mov_b32 v5, 0
        0x7D88_0000, // v_cmp_gt_u32 vcc, s0, v0  (active iff tid < s0)
        0xBE8C_246A, // s_and_saveexec_b64 s[12:13], vcc
        0xBF88_0005, // s_cbranch_execz +5 -> after_loop
        // loop:
        0x4A0A_0A81, // v_add_nc_u32 v5, 1, v5
        0x7D84_0A83, // v_cmp_eq_u32 vcc, 3, v5
        0xBF86_0001, // s_cbranch_vccz +1 -> latch
        0xBF82_0001, // s_branch +1 -> after_loop
        // latch:
        0xBF82_FFFB, // s_branch -5 -> loop
        // after_loop:
        0xBEFE_040C, // s_mov_b64 exec, s[12:13]
        0x7DA4_0001, // v_cmpx_eq_u32 exec, s1, v0 (only lane s1 stores)
        0x4A00_0A87, // v_add_nc_u32 v0, 7, v5
        0x38 << 26 | 0x1C << 18,
        0x80 << 24 | 2 << 16, // buffer_store_dword v0, off, s[8:11], 0
    ];
    let native = native(&words);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let stripped = ir::without_skips(&p, true);
    assert!(stripped.is_none(), "without_skips must keep s_cbranch_execz guarding a vcc loop");
    let wide_blocks: HashSet<usize> = (0..p.blocks.len())
        .filter(|&b| {
            p.blocks[b]
                .insts
                .last()
                .is_some_and(|i| matches!(i.name(), "s_cbranch_execz" | "s_cbranch_vccz"))
        })
        .collect();
    assert_eq!(wide_blocks.len(), 2, "expected execz skip and vccz loop branch");
    let cases = [
        // (s0 = active count, s1 = lane that stores, expected value)
        (0u32, 0u32, 7u32), // empty wave skips loop -> v5 stays 0 -> stores 7
        (10, 0, 10),        // active lane runs 3 iterations -> stores 10
        (10, 35, 7),        // inactive lane in second host wave -> stores 7
        (64, 63, 10),       // full wave runs 3 iterations -> stores 10
    ];
    for (label, wide) in [("per-lane", HashSet::new()), ("wave-wide", wide_blocks)] {
        let shader = lowered(&native, &p, &wide).expect("lower");
        if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
            let mut copy = shader.dxil.clone();
            v.validate(&mut copy).expect("DXIL validation failed");
        }
        let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        for (below, lane, expected) in cases {
            let Some(got) = on_the_card(&shader, [below, lane]) else { return };
            assert_eq!(got, expected, "{label}: s0 = {below}, lane {lane}");
        }
    }
}

#[path = "wave64_after_loop.rs"]
mod after_loop;

/// A dispatch counted in threads: of the last group only the lanes below the
/// count store (after DexNom/VIctory bc3cd4b0).
#[test]
fn the_lanes_past_a_count_of_threads_do_not_run() {
    // Each lane that runs adds 1 to counter[local x].
    let native = native(&[
        0x3402_0082, // v_lshlrev_b32 v1, 2, v0
        0x7e04_0281, // v_mov_b32 v2, 1
        0xe0c8_1000, 0x8002_0201, // buffer_atomic_add v2, v1, s[8:11], 0 offen
    ]);
    let p = ir::build(&native.code, native.wave64, native.vgprs).unwrap();
    let shader = lowered_counted(&native, &p, &HashSet::new(), 64, true).unwrap();
    let limit = shader
        .user_data
        .iter()
        .find(|u| u.root == RootType::Other(super::USER_DISPATCH_THREADS_X))
        .unwrap();
    assert_eq!((limit.slot, limit.dword), (0, 64));
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(true) else { return };
    let Ok((root, pso)) = host.compute_pipeline(&shader) else { return };
    for count in [0u32, 1, 7, 63, 64, 65, 74, 77, 127, 128] {
        let mut constants = vec![0; shader.user_data_dwords() as usize];
        constants[limit.index as usize] = count;
        let result = host
            .dispatch(&root, &pso, &Dispatch {
                roots: vec![(0, Root::Constants(constants))],
                views: &[View::RawUav(68)],
                samplers: &[],
                groups: [2, 1, 1],
            })
            .unwrap();
        let active = if count == 0 { 128 } else { count };
        for (i, &word) in result[0].iter().enumerate().take(64) {
            let i = i as u32;
            assert_eq!(word, u32::from(i < active) + u32::from(i + 64 < active), "count {count}, word {i}");
        }
    }
}

// ---- thread groups of several console waves ----

/// The words `v_lshrrev_b32 v9, 6, v0` and `v_lshlrev_b32 v9, 2, v9`: v9 = (thread / 64) * 4, the
/// byte offset of the thread's console wave's dword.
const WAVE_SLOT_V9: [u32; 2] = [0x2C12_0086, 0x3412_1282];
/// `v_lshlrev_b32 v9, 2, v0`: v9 = the thread's own dword.
const THREAD_SLOT_V9: u32 = 0x3412_0082;
/// `v_add_nc_u32 v2, 1000, v0` (the literal after it): v2 = thread + 1000.
const V2_THREAD_PLUS_1000: [u32; 2] = [0x4A04_00FF, 1000];
/// `buffer_store_dword v<data>, v9, s[8:11], 0 offen`.
const fn store_v9(data: u32) -> [u32; 2] {
    [0xE070_1000, 0x8002_0009 | data << 8]
}

/// `if (thread < s0) { inner }`, as the console compiles it: `s_and_saveexec_b64` and a skip
/// past the body when no lane is left, EXEC restored after it.
fn branch_around(inner: &[u32]) -> Vec<u32> {
    let mut words = vec![
        0x7D88_0000,                      // v_cmp_gt_u32 vcc, s0, v0
        0xBE8C_246A,                      // s_and_saveexec_b64 s[12:13], vcc
        0xBF88_0000 | inner.len() as u32, // s_cbranch_execz past the body
    ];
    words.extend_from_slice(inner);
    words.push(0xBEFE_040C); // s_mov_b64 exec, s[12:13]
    words
}

/// The one `s_cbranch_execz` of `p`.
fn the_skip(p: &ir::Program) -> usize {
    (0..p.blocks.len())
        .find(|&b| p.blocks[b].insts.last().is_some_and(|i| i.name() == "s_cbranch_execz"))
        .expect("the skip")
}

/// Runs `shader` (one group) on the hardware device with the root constants `s0`, `s1` and gives
/// the first `words` dwords of the UAV; None (and a line) without a device that takes it.
fn on_the_card_all(shader: &HostShader, constants: [u32; 2], words: u32) -> Option<Vec<u32>> {
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return None;
    };
    let Ok((root, pso)) = host.compute_pipeline(shader) else {
        eprintln!("the hardware device does not take this program; not run");
        return None;
    };
    let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    for u in &shader.user_data {
        if let (RootType::Constants, 0, d @ 0..=1) = (u.root, u.slot, u.dword) {
            ud[u.index as usize] = constants[d as usize];
        }
    }
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(words)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        )
        .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
    Some(out[0].clone())
}

/// One program of a thread group of `threads` threads (`inner` words behind `prologue`, inside
/// `if (thread < s0)`) taken wave-wide for the group, as `widen` does it, and what it must
/// store for each `s0` in `cases` (`expect(s0)`: the first `words` dwords of the UAV).
/// Checked: the branch alone is refused (a block the host waves must meet in is behind it),
/// taken wave-wide it lowers to waves of 32 with one decision for the group (the group
/// barriers inside are reached by every thread whichever console waves have a lane there), the
/// validator takes it, and on the hardware device the results are the console's, bit for bit.
fn wide_for_the_group(
    prologue: &[u32],
    inner: &[u32],
    threads: u32,
    granules: u32,
    words: u32,
    cases: &[u32],
    expect: &dyn Fn(u32) -> Vec<u32>,
) {
    let mut code = prologue.to_vec();
    code.extend(branch_around(inner));
    wide_program(&code, threads, granules, words, cases, expect);
}

/// [`wide_for_the_group`] for the whole program `code` (its one `s_cbranch_execz` taken
/// wave-wide).
fn wide_program(
    code: &[u32],
    threads: u32,
    granules: u32,
    words: u32,
    cases: &[u32],
    expect: &dyn Fn(u32) -> Vec<u32>,
) {
    let native = native_lds(code, true, threads, granules);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let branch = the_skip(&p);
    // per lane: the host waves cannot meet inside the branch, and the lowering names where
    lower::take_needs();
    let _ = lowered_with(&native, &p, &HashSet::new(), 32, false, Some(true));
    let needs = lower::take_needs();
    let st = structure::structure(&p).expect("structure");
    assert_eq!(lower::branches_around(&st, &needs), Some(vec![branch]), "{threads} threads");
    // wave-wide: one decision for the group
    let wide = HashSet::from([branch]);
    lower::GROUP_DECISIONS.set(0);
    let (shader, wave) = lowered_with(&native, &p, &wide, 32, false, Some(true)).expect("lower");
    assert_eq!(wave, Some(32), "{threads} threads: waves of 32");
    assert_eq!(lower::GROUP_DECISIONS.get(), 1, "{threads} threads: one decision for the group");
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for &s0 in cases {
        let Some(got) = on_the_card_all(&shader, [s0, 0], words) else { return };
        assert_eq!(got, expect(s0), "{threads} threads, {s0} in the branch");
    }
    eprintln!("ran on the hardware device ({threads} threads): the console's results for {cases:?}");
}

/// Of the console wave `k` (64 threads) the lanes below `s0` (`s0` counts threads of the
/// group): how many there are.
fn lanes_in(k: u32, s0: u32) -> u32 {
    s0.saturating_sub(64 * k).min(64)
}

/// The group sizes: two, four and sixteen console waves.
const GROUPS: [u32; 3] = [128, 256, 1024];

/// The `s0` cases of a group size: some console waves have all their lanes in the branch, some
/// have a few, and some none (those would have skipped it).
fn group_cases(threads: u32) -> Vec<u32> {
    match threads {
        128 => vec![128, 64, 40, 70, 1, 0],
        256 => vec![256, 130, 64, 200, 193, 7],
        _ => vec![1024, 64, 700, 65, 513, 0],
    }
}

/// A wave OR in the branch: the console waves that have a lane there OR their own bits, and
/// the ones that have none store nothing - whether or not the console wave before them did.
#[test]
fn a_wave_or_in_a_wide_branch_of_a_group_of_several_console_waves() {
    let mut prologue = vec![
        0x7E0A_0281, // v_mov_b32 v5, 1
        0x2C0C_0081, // v_lshrrev_b32 v6, 1, v0
        0x340E_0B06, // v_lshlrev_b32 v7, v6, v5
    ];
    prologue.extend(WAVE_SLOT_V9);
    let mut inner = super::wave::WAVE_OR_V7_TO_S7.to_vec();
    inner.push(0x7E00_0207); // v_mov_b32 v0, s7
    inner.extend(store_v9(0));
    for threads in GROUPS {
        wide_for_the_group(&prologue, &inner, threads, 0, 16, &group_cases(threads), &|s0| {
            // lane l holds bit l >> 1
            let mut want = vec![0; 16];
            for k in 0..threads / 64 {
                let m = lanes_in(k, s0);
                want[k as usize] = if m == 0 { 0 } else { ((1u64 << ((m - 1) / 2 + 1)) - 1) as u32 };
            }
            want
        });
    }
}

/// `v_readlane_b32` of a lane in the upper half of the console wave, from the wide branch's
/// lanes of every console wave of the group (the lane need not be in the branch itself).
#[test]
fn a_lane_read_across_the_console_wave_in_a_wide_branch_of_a_group_of_several_waves() {
    let mut prologue = V2_THREAD_PLUS_1000.to_vec();
    prologue.extend(WAVE_SLOT_V9);
    let mut inner = vec![
        0xD760_000C,
        0x0001_5102, // v_readlane_b32 s12, v2, 40
        0x7E00_020C, // v_mov_b32 v0, s12
    ];
    inner.extend(store_v9(0));
    for threads in GROUPS {
        wide_for_the_group(&prologue, &inner, threads, 0, 16, &group_cases(threads), &|s0| {
            let mut want = vec![0; 16];
            for k in 0..threads / 64 {
                if lanes_in(k, s0) > 0 {
                    want[k as usize] = 64 * k + 40 + 1000;
                }
            }
            want
        });
    }
}

/// `v_readfirstlane_b32` where only the upper half of the console wave has lanes (the lanes
/// above 34 of those below `s0`): the first active lane is in the other host wave than lane 0.
#[test]
fn a_first_lane_read_across_the_console_wave_in_a_wide_branch_of_a_group_of_several_waves() {
    let mut prologue = V2_THREAD_PLUS_1000.to_vec();
    prologue.push(0x3614_00BF); // v_and_b32 v10, 63, v0
    // (a lane-crossing instruction, so that the program is one the host waves exchange in)
    prologue.extend([0xD765_000C, 0x0001_00C1]); // v_mbcnt_lo_u32_b32 v12, -1, 0
    prologue.extend(WAVE_SLOT_V9);
    // (inside `if (thread < s0)`, EXEC is narrowed again to the lanes above 34)
    let mut inner = vec![
        0x7DA2_14A2, // v_cmpx_lt_u32 exec, 34, v10
        0x7E1C_0502, // v_readfirstlane_b32 s14, v2
        0x7E00_020E, // v_mov_b32 v0, s14
    ];
    inner.extend(store_v9(0));
    for threads in GROUPS {
        wide_for_the_group(&prologue, &inner, threads, 0, 16, &group_cases(threads), &|s0| {
            let mut want = vec![0; 16];
            for k in 0..threads / 64 {
                if lanes_in(k, s0) > 35 {
                    want[k as usize] = 64 * k + 35 + 1000;
                }
            }
            want
        });
    }
}

/// `v_mbcnt_lo` and `v_mbcnt_hi` of a 64-bit lane mask (EXEC, the even lanes of the branch):
/// the number of lanes of the mask below this one, which for the upper half counts the lower
/// half's too.
#[test]
fn v_mbcnt_of_a_64_bit_mask_in_a_wide_branch_of_a_group_of_several_console_waves() {
    let mut prologue = vec![0x3616_0081]; // v_and_b32 v11, 1, v0
    prologue.push(THREAD_SLOT_V9);
    let mut inner = vec![
        0x7DA4_1680, // v_cmpx_eq_u32 exec, 0, v11
        0xBE90_047E, // s_mov_b64 s[16:17], exec
        0xD765_0003,
        0x0001_0010, // v_mbcnt_lo_u32_b32 v3, s16, 0
        0xD766_0003,
        0x0002_0611, // v_mbcnt_hi_u32_b32 v3, s17, v3
    ];
    inner.extend(store_v9(3));
    for threads in GROUPS {
        wide_for_the_group(&prologue, &inner, threads, 0, 1024, &group_cases(threads), &|s0| {
            let mut want = vec![0; 1024];
            for t in 0..threads {
                if t % 2 == 0 && t < s0 {
                    want[t as usize] = (t % 64) / 2;
                }
            }
            want
        });
    }
}

/// A lane mask as data: the number of lanes of the console wave in the branch (`s_bcnt1_i32_b64`
/// of EXEC kept as a number), which counts both host waves' lanes.
#[test]
fn a_lane_mask_as_data_in_a_wide_branch_of_a_group_of_several_console_waves() {
    let prologue = WAVE_SLOT_V9.to_vec();
    let mut inner = vec![
        0xBE90_047E, // s_mov_b64 s[16:17], exec
        0xBE92_1010, // s_bcnt1_i32_b64 s18, s[16:17]
        0x7E00_0212, // v_mov_b32 v0, s18
    ];
    inner.extend(store_v9(0));
    for threads in GROUPS {
        wide_for_the_group(&prologue, &inner, threads, 0, 16, &group_cases(threads), &|s0| {
            let mut want = vec![0; 16];
            for k in 0..threads / 64 {
                want[k as usize] = lanes_in(k, s0);
            }
            want
        });
    }
}

/// LDS written and read again in one lane-branch region, as the console's one wave does it
/// without a barrier: each thread writes its own dword and reads its partner's (the other half
/// of its console wave). Whole console waves in the branch; the others have skipped it.
#[test]
fn lds_written_then_read_in_a_wide_branch_of_a_group_of_several_console_waves() {
    let mut prologue = V2_THREAD_PLUS_1000.to_vec();
    prologue.push(THREAD_SLOT_V9);
    prologue.extend([0x3A14_12FF, 0x80]); // v_xor_b32 v10, 0x80, v9
    // (a lane-crossing instruction, so that the program is one the host waves exchange in)
    prologue.extend([0xD765_000C, 0x0001_00C1]); // v_mbcnt_lo_u32_b32 v12, -1, 0
    let mut inner = vec![
        0xD834_0000,
        0x0000_0209, // ds_write_b32 v9, v2
        0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
        0xD8D8_0000,
        0x0300_000A, // ds_read_b32 v3, v10
        0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
    ];
    inner.extend(store_v9(3));
    let cases = |threads: u32| match threads {
        128 => vec![128, 64, 0],
        256 => vec![256, 192, 64, 0],
        _ => vec![1024, 64, 512, 0],
    };
    for threads in GROUPS {
        wide_for_the_group(&prologue, &inner, threads, 8, 1024, &cases(threads), &|s0| {
            let mut want = vec![0; 1024];
            for t in 0..threads.min(s0) {
                want[t as usize] = (t ^ 32) + 1000;
            }
            want
        });
    }
}

/// A group of one console wave decides for itself: its wave-wide branch has no word of the
/// group's (it is the group).
#[test]
fn a_group_of_one_console_wave_decides_for_itself() {
    let native = program(0xBF88_0000);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let wide = HashSet::from([the_skip(&p)]);
    lower::GROUP_DECISIONS.set(0);
    let (_, wave) = lowered_with(&native, &p, &wide, 32, false, Some(true)).expect("lower");
    assert_eq!(wave, Some(32));
    assert_eq!(lower::GROUP_DECISIONS.get(), 0);
}

/// What the group plan says of the skip: its body is the side `s_cbranch_execz` does not take,
/// and every block of that body is guarded by the branch's own decision.
#[test]
fn the_group_plan_guards_the_body_of_a_wide_branch_by_the_console_waves_own_decision() {
    let native = native_lds(&branch_around(&[0x7E00_0207, 0xE070_1000, 0x8002_0009]), true, 128, 0);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let skip = the_skip(&p);
    let st = structure::structure_wide(&p, &HashSet::from([skip])).expect("structure");
    let plan = super::wide::plan(&p, &st).expect("plan");
    let ir::Exit::Branch { taken, not_taken } = p.blocks[skip].exit else { unreachable!() };
    // (taken past the body, to the restore of EXEC)
    assert_eq!(plan.body_taken[&skip], false);
    assert_eq!(plan.region[not_taken], vec![skip]);
    assert!(plan.region[taken].is_empty() && plan.region[skip].is_empty());
    assert!(plan.headers.is_empty());
}

/// `if (thread < s0) { wave OR; store it } else { store 0x1234 }` (the else is for the console
/// waves that have no lane below `s0`: the branch is `s_cbranch_execz`): both sides do something,
/// and the else holds nothing the host waves must meet in. A console wave with a lane below `s0`
/// takes the first side, one without takes the second - in one group, whichever waves they are -
/// and the group runs the first side for all of them, then the second for the waves that took it.
fn if_else(then: &[u32]) -> Vec<u32> {
    let mut words = vec![
        0x7D88_0000, // v_cmp_gt_u32 vcc, s0, v0
        0xBE8C_246A, // s_and_saveexec_b64 s[12:13], vcc
        0xBF88_0000 | (then.len() + 1) as u32, // s_cbranch_execz to the else
    ];
    words.extend_from_slice(then);
    words.extend([
        0xBF82_0005, // s_branch past the else
        0xBEFE_040C, // s_mov_b64 exec, s[12:13]
        0x7E00_02FF,
        0x1234, // v_mov_b32 v0, 0x1234
        0xE070_1040,
        0x8002_0009, // buffer_store_dword v0, v9, s[8:11], 0 offen offset:64
        0xBEFE_040C, // s_mov_b64 exec, s[12:13]
    ]);
    words
}

#[test]
fn the_two_sides_of_a_wide_branch_in_a_group_of_several_console_waves() {
    let mut code = vec![
        0x7E0A_0281, // v_mov_b32 v5, 1
        0x2C0C_0081, // v_lshrrev_b32 v6, 1, v0
        0x340E_0B06, // v_lshlrev_b32 v7, v6, v5
    ];
    code.extend(WAVE_SLOT_V9);
    let mut then = super::wave::WAVE_OR_V7_TO_S7.to_vec();
    then.push(0x7E00_0207); // v_mov_b32 v0, s7
    then.extend(store_v9(0));
    code.extend(if_else(&then));
    for threads in GROUPS {
        let waves = threads / 64;
        wide_program(&code, threads, 0, 32, &group_cases(threads), &|s0| {
            let mut want = vec![0; 32];
            for k in 0..waves {
                let m = lanes_in(k, s0);
                if m > 0 {
                    want[k as usize] = ((1u64 << ((m - 1) / 2 + 1)) - 1) as u32;
                } else {
                    want[16 + k as usize] = 0x1234;
                }
            }
            want
        });
    }
}

/// With a wave OR on both sides neither can run alone: refused, by name.
#[test]
fn a_wide_branch_whose_two_sides_both_need_the_host_waves_is_refused_for_a_group_of_several_waves() {
    let mut then = super::wave::WAVE_OR_V7_TO_S7.to_vec();
    then.push(0x7E00_0207);
    let mut code = vec![0x7E0A_0281, 0x2C0C_0081, 0x340E_0B06];
    code.extend(WAVE_SLOT_V9);
    let mut words = vec![
        0x7D88_0000,
        0xBE8C_246A,
        0xBF88_0000 | (then.len() + 1) as u32,
    ];
    words.extend_from_slice(&then);
    words.push(0xBF82_0000 | then.len() as u32 + 1); // s_branch past the else
    words.extend_from_slice(&then);
    words.push(0xBEFE_040C);
    code.extend(words);
    let native = native_lds(&code, true, 128, 0);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let wide = HashSet::from([the_skip(&p)]);
    let r = lowered_with(&native, &p, &wide, 32, false, Some(true));
    assert!(
        matches!(&r, Err(ShaderError::Unsupported { what, .. }) if *what == "a wave-wide branch whose both sides do something"),
        "{:?}",
        r.err()
    );
}

/// A loop closed by a plain jump that the lanes leave on their own (the lane that never leaves
/// is cut by the iteration limit): the lane at the limit used to leave the program, and the
/// group barrier the translation puts before the LDS access after the loop waited for it.
/// Now it leaves by the loop's exit branch and goes on with the others, so every thread reaches
/// that barrier. Lane t >= 1 leaves after t rounds; lane 0 never does.
#[test]
fn a_lane_cut_by_the_loop_limit_goes_on_to_the_barrier_after_the_loop() {
    let code = [
        THREAD_SLOT_V9,
        0x7E02_0280, // v_mov_b32 v1, 0
        0x7E08_02C1, // v_mov_b32 v4, -1
        0x7D84_0080, // v_cmp_eq_u32 vcc, 0, v0
        0x0204_0900, // v_cndmask_b32 v2, v0, v4, vcc: the limit is the thread's, lane 0's is none
        0xBE8C_047E, // s_mov_b64 s[12:13], exec
        // the loop
        0x4A02_0281, // v_add_nc_u32 v1, 1, v1
        0x7DA2_0501, // v_cmpx_lt_u32 exec, v1, v2
        0xBF88_0001, // s_cbranch_execz past the jump
        0xBF82_FFFC, // s_branch to the loop
        // after it
        0xBEFE_040C, // s_mov_b64 exec, s[12:13]
        0xD834_0000,
        0x0000_0109, // ds_write_b32 v9, v1
        0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
        0xD8D8_0000,
        0x0300_0009, // ds_read_b32 v3, v9
        0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
        0xE070_1000,
        0x8002_0309, // buffer_store_dword v3, v9, s[8:11], 0 offen
    ];
    let native = native_lds(&code, true, 64, 1);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let none = HashSet::new();
    // as it was: a block that leaves the program
    lower::ABORT_BLOCKS.set(0);
    lowered_with(&native, &p, &none, 32, false, Some(false)).expect("lower");
    assert_eq!(lower::ABORT_BLOCKS.get(), 1);
    // with `VICTORY_WAVE_WIDE=1`: none, every lane leaves the loop by its exit branch
    lower::ABORT_BLOCKS.set(0);
    let (shader, wave) = lowered_with(&native, &p, &none, 32, false, Some(true)).expect("lower");
    assert_eq!(wave, Some(32));
    assert_eq!(lower::ABORT_BLOCKS.get(), 0);
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(got) = on_the_card_all(&shader, [0, 0], 64) else { return };
    for t in 1..64 {
        assert_eq!(got[t], t as u32, "lane {t}");
    }
    // (lane 0 went round until the limit: 4,096 rounds, and a round or two more or less)
    assert!((4090..=4100).contains(&got[0]), "lane 0: {}", got[0]);
}

/// The exchange dwords and the counter of the host waves come out of the 32 KB a thread group
/// has: a program that leaves no room for them stays with waves of 64 (the group of 1024 has
/// sixteen console waves, so 33 dwords).
#[test]
fn the_exchange_slots_of_a_big_group_must_fit_the_32_kb_of_a_thread_group() {
    let mut inner = super::wave::WAVE_OR_V7_TO_S7.to_vec();
    inner.push(0x7E00_0207);
    inner.extend(store_v9(0));
    let mut code = vec![0x7E0A_0281, 0x2C0C_0081, 0x340E_0B06];
    code.extend(WAVE_SLOT_V9);
    code.extend(branch_around(&inner));
    // (LDS in 512-byte granules; the slots and the counter are 132 bytes)
    for (granules, wave) in [(8u32, Some(32)), (63, Some(32)), (64, Some(64))] {
        let native = native_lds(&code, true, 1024, granules);
        let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
        let wide = HashSet::from([the_skip(&p)]);
        let r = lowered_with(&native, &p, &wide, 32, false, Some(true));
        match r {
            Ok((_, got)) => assert_eq!(got, wave, "{granules} x 512 bytes"),
            Err(e) => panic!("{granules} x 512 bytes: {e:?}"),
        }
    }
}

/// `if (thread < s0) { loop { v5 += 1; wave OR (a group barrier); if (v5 == 3) break; } }`, then
/// every thread stores v5, in a group of several console waves with the skip and the loop's
/// exit branch taken wave-wide. A console wave with no lane below `s0` is a ghost: it must not
/// speak for another round of the loop (its `EXEC` is 0, so `v_cmp` is 0 and `s_cbranch_vccz`
/// would keep it going to the iteration limit, thousands of barriers), yet it meets every group
/// barrier the others do. Waves with a lane go round three times and the ghosts' v5 stays 0.
#[test]
fn an_empty_console_wave_does_not_keep_a_wide_loop_with_barriers_going() {
    let mut code = vec![
        0x7E0A_0280, // v_mov_b32 v5, 0
        0x7E0E_0280, // v_mov_b32 v7, 0 (what the wave OR takes: 0 in every lane)
        0x3416_0082, // v_lshlrev_b32 v11, 2, v0
    ];
    code.extend(WAVE_SLOT_V9);
    code.extend([
        0x7D88_0000, // v_cmp_gt_u32 vcc, s0, v0
        0xBE8C_246A, // s_and_saveexec_b64 s[12:13], vcc
        0xBF88_001B, // s_cbranch_execz +27 -> after the loop
        0x4A0A_0A81, // loop: v_add_nc_u32 v5, 1, v5
    ]);
    code.extend(super::wave::WAVE_OR_V7_TO_S7);
    code.extend([
        0x7E10_0207, // v_mov_b32 v8, s7
        0x4A0A_0B08, // v_add_nc_u32 v5, v8, v5
        0x7D84_0A83, // v_cmp_eq_u32 vcc, 3, v5
        0xBF86_0001, // s_cbranch_vccz +1 -> the jump back
        0xBF82_0001, // s_branch +1 -> after the loop
        0xBF82_FFE5, // s_branch -27 -> loop
        0xBEFE_040C, // s_mov_b64 exec, s[12:13]
        0xE070_1000,
        0x8002_050B, // buffer_store_dword v5, v11, s[8:11], 0 offen
    ]);
    for threads in [128u32, 256] {
        let native = native_lds(&code, true, threads, 8);
        let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
        let stripped = ir::without_skips(&p, true);
        assert!(stripped.is_none(), "the skip over the loop stays");
        let wide: HashSet<usize> = (0..p.blocks.len())
            .filter(|&b| {
                p.blocks[b]
                    .insts
                    .last()
                    .is_some_and(|i| matches!(i.name(), "s_cbranch_execz" | "s_cbranch_vccz"))
            })
            .collect();
        assert_eq!(wide.len(), 2, "the skip and the loop's exit branch");
        let st = structure::structure_wide(&p, &wide).expect("structure");
        let plan = super::wide::plan(&p, &st).expect("plan");
        let latch = *wide
            .iter()
            .find(|&&b| p.blocks[b].insts.last().is_some_and(|i| i.name() == "s_cbranch_vccz"))
            .expect("the latch");
        // the loop's own decision is the group's: the body side is the back edge, and the loop
        // carries the flag that tells a wave has left it
        assert!(plan.body_taken[&latch], "vccz: taken (no lane has the bit yet) goes round again");
        assert_eq!(plan.headers.len(), 1, "{threads} threads: one loop with a wide branch inside");
        lower::GROUP_DECISIONS.set(0);
        let (shader, wave) = lowered_with(&native, &p, &wide, 32, false, Some(true)).expect("lower");
        assert_eq!(wave, Some(32), "{threads} threads: waves of 32");
        assert_eq!(lower::GROUP_DECISIONS.get(), 2, "{threads} threads: the skip's and the loop's decisions");
        if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
            let mut copy = shader.dxil.clone();
            v.validate(&mut copy).expect("DXIL validation failed");
        }
        let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let cases: &[u32] = if threads == 128 { &[128, 64, 70, 1, 0] } else { &[256, 130, 64, 7, 0] };
        for &s0 in cases {
            let Some(got) = on_the_card_all(&shader, [s0, 0], threads) else { return };
            for t in 0..threads {
                assert_eq!(got[t as usize], if t < s0 { 3 } else { 0 }, "{threads} threads, s0 = {s0}, thread {t}");
            }
        }
    }
}

/// A waterfall loop in a group of several console waves: `if (thread < s0) { do { first = readfirstlane(v1);
/// the lanes with v1 == first add 1 to v5; } while (some lane is left) }`, then every thread
/// stores v5. v1 is 0 in the lower and 1 in the upper host wave of every console wave, so the
/// lower half is done after one round and the upper after two. The loop is a single block whose
/// own branch (`s_cbranch_execnz`) is per-lane, and the first-lane read needs the other host
/// wave (an exchange of two group barriers): each host wave leaving at its own round left its
/// partner reading the stale dwords of the round before, and the lanes of the upper half never
/// matched (4,096 rounds of four barriers per console wave, the device removal of
/// `firevbuffer:CS_EntityFireResolve_Scalarized`). The loop's branch must be one of the wide
/// branches (the decision is the group's, an empty console wave votes for nothing and goes
/// round with the others), and every lane in `thread < s0` ends with 1.
#[test]
fn a_waterfall_loop_that_needs_the_other_host_wave_leaves_together_with_an_empty_console_wave() {
    let code = [
        0x7E0A_0280, // v_mov_b32 v5, 0
        0x3416_0082, // v_lshlrev_b32 v11, 2, v0
        0x2C02_0085, // v_lshrrev_b32 v1, 5, v0
        0x3602_0281, // v_and_b32 v1, 1, v1
        // (a lane-crossing instruction, so that the program is one the host waves exchange in)
        0xD765_000C,
        0x0001_00C1, // v_mbcnt_lo_u32_b32 v12, -1, 0
        0x7D88_0000, // v_cmp_gt_u32 vcc, s0, v0
        0xBE8C_246A, // s_and_saveexec_b64 s[12:13], vcc
        0xBF88_0006, // s_cbranch_execz past the loop
        // the loop (one block)
        0x7E0C_0501, // v_readfirstlane_b32 s6, v1
        0x7D84_0206, // v_cmp_eq_u32 vcc, s6, v1
        0xBE8E_246A, // s_and_saveexec_b64 s[14:15], vcc
        0x4A0A_0A81, // v_add_nc_u32 v5, 1, v5
        0x89FE_0E7E, // s_xor_b64 exec, exec, s[14:15]
        0xBF89_FFFA, // s_cbranch_execnz to the loop
        // after it
        0xBEFE_040C, // s_mov_b64 exec, s[12:13]
        0xE070_1000,
        0x8002_050B, // buffer_store_dword v5, v11, s[8:11], 0 offen
    ];
    for threads in [128u32, 256] {
        let native = native_lds(&code, true, threads, 8);
        let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
        let skip = the_skip(&p);
        let latch = (0..p.blocks.len())
            .find(|&b| p.blocks[b].insts.last().is_some_and(|i| i.name() == "s_cbranch_execnz"))
            .expect("the loop's branch");
        // the way `widen` goes: what the lowering says needs the other host wave is taken wave-wide
        let mut wide = HashSet::new();
        let mut lowered_shader = None;
        for _ in 0..4 {
            lower::take_needs();
            let r = lowered_with(&native, &p, &wide, 32, false, Some(true));
            let needs = lower::take_needs();
            let st = structure::structure_wide(&p, &wide).expect("structure");
            let was = lower::TEST_WIDE.replace(Some(true));
            let around = lower::branches_around(&st, &needs);
            lower::TEST_WIDE.set(was);
            let more: Vec<usize> = around.unwrap_or_default().into_iter().filter(|b| !wide.contains(b)).collect();
            if more.is_empty() {
                lowered_shader = Some(r.expect("lower"));
                break;
            }
            wide.extend(more);
        }
        let (shader, wave) = lowered_shader.expect("the branches settle");
        assert_eq!(wave, Some(32), "{threads} threads: waves of 32");
        assert!(wide.contains(&latch), "{threads} threads: the loop's own branch is wave-wide: {wide:?}");
        assert!(wide.contains(&skip), "{threads} threads: and so is the skip around it: {wide:?}");
        if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
            let mut copy = shader.dxil.clone();
            v.validate(&mut copy).expect("DXIL validation failed");
        }
        let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let cases: &[u32] = if threads == 128 { &[128, 64, 40, 70, 0] } else { &[256, 130, 64, 7, 0] };
        for &s0 in cases {
            let Some(got) = on_the_card_all(&shader, [s0, 0], threads) else { return };
            for t in 0..threads {
                assert_eq!(got[t as usize], u32::from(t < s0), "{threads} threads, s0 = {s0}, thread {t}");
            }
        }
    }
}
