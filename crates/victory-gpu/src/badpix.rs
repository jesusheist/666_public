//! The debug server's `find_bad_pixels` (`VICTORY_MCP`): a one-frame capture
//! that scans, around every draw and dispatch recorded in the frame, the
//! float targets it writes for NaN and Inf, so the first command that
//! produces them can be named.
//!
//! How it works. The present path begins a capture at one present and ends
//! it at the next ([`victory_mcp::scan_edge`]); the commands recorded in
//! between are the frame. Each of them asks [`recording`] (one relaxed load
//! of an atomic, false whenever no capture is asked for) and, if it is true,
//! brings a ticket ([`begin`]): every render target the draw writes (and,
//! with `include_uav`, every float texture its shaders write through an
//! unordered access view) is scanned right before the command and again
//! right after it ([`end`]), so a target that was already bad is told from
//! one the command made bad.
//!
//! A scan is, on the host list the command is recorded on: the target moved
//! to the copy-source state through the list's own state tracking
//! ([`crate::list::ensure_state`], which is what the title's barriers and
//! the prelude use, so the states stay consistent across lists), a copy of
//! the subresource into a scratch buffer, and three small compute
//! dispatches that need no descriptor heap: root constants, the scratch
//! buffer as a root SRV and the results buffer as a root UAV. (A texture
//! cannot be read through a root descriptor, and a descriptor heap cannot be
//! set in the middle of a list without undoing the title's tables; the copy
//! is what keeps the scan clear of both.) The scratch buffer is moved with
//! explicit barriers from the common state and back to it, so every scan
//! starts and ends in the state the next list expects, whatever order the
//! lists run in.
//!
//! The dispatches (HLSL, `SOURCE`): `init` zeroes the command's result slot
//! and stamps it with a counter (the order the scans ran in on the card);
//! `scan` visits every pixel once, keeps counts in group shared memory and
//! writes one atomic per group; `locate` reads the first bad pixel and the
//! followed pixel back out of the scratch copy. Every number is found from
//! the bits of the values, so no comparison involves a NaN; the work is a
//! fixed grid over the subresource, never a loop that depends on data, so it
//! cannot hang the card. Nothing the scan writes is a game resource: only
//! the scratch buffer (a copy destination) and the results buffer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use victory_mcp::badpix::{Counts, DrawScan, ScanParams, ScanReport, TargetScan};
use victory_mcp::{ScanEdge, ScanRequest};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Interface, s};

use crate::host::HostList;
use crate::object::This;
use crate::pipeline::host::HostPipeline;

/// Dwords in the header of the results buffer (word 0 counts the scans that
/// ran, for their order) and in each result slot.
pub(crate) const HEADER_DWORDS: u32 = 16;
pub(crate) const SLOT_DWORDS: u32 = 16;
/// Result slots a capture has: two for each target of each command.
pub(crate) const SLOTS: u32 = 32_768;
/// Largest side of a subresource that is scanned (a fixed grid of groups).
const MAX_SIDE: u32 = 16_384;
/// The host state of a texture a copy reads.
const COPY_SOURCE: u32 = 0x800;
const NONE: u32 = 0xffff_ffff;

/// Slot words: 0 NaN pixels, 1 Inf pixels, 2 bits of the largest finite
/// absolute value, 3 first bad pixel as y << 16 | x (all ones: none), 4 the
/// order the scan ran in, 5 pixels scanned (0: the scan has not run), 6
/// unused, 7..=10 the followed pixel's float bits, 11..=14 the first bad
/// pixel's, 15 the followed pixel was read.
const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=8, b0), SRV(t0), UAV(u0)"
cbuffer C : register(b0) { uint W; uint H; uint PITCH; uint KIND; uint BASE; uint PX; uint PY; uint PAD; };
ByteAddressBuffer src : register(t0);
RWByteAddressBuffer res : register(u0);

uint ld16(uint at) { uint w = src.Load(at & ~3u); return (at & 2u) ? (w >> 16) : (w & 0xffffu); }
uint half_bits(uint h) { return asuint(f16tof32(h)); }

// An 11 or 10 bit unsigned float (5 exponent bits, mbits mantissa bits) as
// the bits of a 32 bit float; the all-ones exponent stays Inf or NaN.
uint small_float(uint e, uint m, uint mbits) {
    if (e == 31u) { return m == 0u ? 0x7f800000u : 0x7fc00000u; }
    float mf = float(m) / float(1u << mbits);
    float v = (e == 0u) ? mf * exp2(-14.0) : (1.0 + mf) * exp2(float(e) - 15.0);
    return asuint(v);
}

uint channels() {
    if (KIND == 1u || KIND == 5u) return 4u;
    if (KIND == 2u || KIND == 8u) return 3u;
    if (KIND == 3u || KIND == 6u) return 2u;
    return 1u;
}

// The float bits of the pixel's channels (the unused ones are zero).
uint4 load_pixel(uint x, uint y) {
    uint at = y * PITCH;
    uint4 v = uint4(0, 0, 0, 0);
    if (KIND == 1u) { v = src.Load4(at + x * 16u); }
    else if (KIND == 2u) { v.xyz = src.Load3(at + x * 12u); }
    else if (KIND == 3u) { v.xy = src.Load2(at + x * 8u); }
    else if (KIND == 4u) { v.x = src.Load(at + x * 4u); }
    else if (KIND == 5u) {
        uint2 w = src.Load2(at + x * 8u);
        v = uint4(half_bits(w.x & 0xffffu), half_bits(w.x >> 16), half_bits(w.y & 0xffffu), half_bits(w.y >> 16));
    }
    else if (KIND == 6u) {
        uint w = src.Load(at + x * 4u);
        v.xy = uint2(half_bits(w & 0xffffu), half_bits(w >> 16));
    }
    else if (KIND == 7u) { v.x = half_bits(ld16(at + x * 2u)); }
    else {
        uint w = src.Load(at + x * 4u);
        v.x = small_float((w >> 6) & 31u, w & 63u, 6u);
        v.y = small_float((w >> 17) & 31u, (w >> 11) & 63u, 6u);
        v.z = small_float((w >> 27) & 31u, (w >> 22) & 31u, 5u);
    }
    return v;
}

groupshared uint g_max;
groupshared uint g_nan;
groupshared uint g_inf;
groupshared uint g_first;

[RootSignature(RS)]
[numthreads(1, 1, 1)]
void cs_init() {
    uint order;
    res.InterlockedAdd(0, 1, order);
    res.Store4((BASE + 0u) * 4u, uint4(0u, 0u, 0u, 0xffffffffu));
    res.Store4((BASE + 4u) * 4u, uint4(order, 0u, 0u, 0u));
    res.Store4((BASE + 8u) * 4u, uint4(0u, 0u, 0u, 0u));
    res.Store4((BASE + 12u) * 4u, uint4(0u, 0u, 0u, 0u));
}

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void cs_scan(uint3 id : SV_DispatchThreadID, uint3 gid : SV_GroupID, uint gi : SV_GroupIndex) {
    if (gi == 0u) { g_max = 0u; g_nan = 0u; g_inf = 0u; g_first = 0xffffffffu; }
    GroupMemoryBarrierWithGroupSync();
    if (id.x < W && id.y < H) {
        uint4 v = load_pixel(id.x, id.y);
        uint n = channels();
        bool has_nan = false;
        bool has_inf = false;
        uint biggest = 0u;
        for (uint i = 0u; i < 4u; i++) {
            if (i < n) {
                uint b = v[i];
                if (((b >> 23) & 0xffu) == 0xffu) {
                    if ((b & 0x7fffffu) != 0u) { has_nan = true; } else { has_inf = true; }
                } else {
                    biggest = max(biggest, b & 0x7fffffffu);
                }
            }
        }
        uint old;
        InterlockedMax(g_max, biggest, old);
        if (has_nan) { InterlockedAdd(g_nan, 1u, old); }
        if (has_inf) { InterlockedAdd(g_inf, 1u, old); }
        if (has_nan || has_inf) { InterlockedMin(g_first, (id.y << 16) | id.x, old); }
    }
    GroupMemoryBarrierWithGroupSync();
    if (gi == 0u) {
        uint old;
        if (g_nan != 0u) { res.InterlockedAdd((BASE + 0u) * 4u, g_nan, old); }
        if (g_inf != 0u) { res.InterlockedAdd((BASE + 1u) * 4u, g_inf, old); }
        if (g_max != 0u) { res.InterlockedMax((BASE + 2u) * 4u, g_max, old); }
        if (g_first != 0xffffffffu) { res.InterlockedMin((BASE + 3u) * 4u, g_first, old); }
        if (gid.x == 0u && gid.y == 0u) { res.Store((BASE + 5u) * 4u, W * H); }
    }
}

[RootSignature(RS)]
[numthreads(1, 1, 1)]
void cs_locate() {
    uint first = res.Load((BASE + 3u) * 4u);
    if (first != 0xffffffffu) {
        res.Store4((BASE + 11u) * 4u, load_pixel(first & 0xffffu, first >> 16));
    }
    if (PX < W && PY < H) {
        res.Store4((BASE + 7u) * 4u, load_pixel(PX, PY));
        res.Store((BASE + 15u) * 4u, 1u);
    }
}
"#;

/// The scan's pipelines, made once per device.
pub(crate) struct Scanner {
    root: ID3D12RootSignature,
    init: ID3D12PipelineState,
    scan: ID3D12PipelineState,
    locate: ID3D12PipelineState,
}

fn compile(entry: windows::core::PCSTR) -> Result<ID3DBlob, String> {
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the source and names live through the call; the out pointers
    // are ours.
    let made = unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            s!("badpix"),
            None,
            None,
            entry,
            s!("cs_5_1"),
            0,
            0,
            &mut code,
            Some(&raw mut errors),
        )
    };
    if let Err(e) = made {
        let text = errors
            .map(|b| {
                // SAFETY: the blob's own bytes.
                let bytes = unsafe { std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize()) };
                String::from_utf8_lossy(bytes).into_owned()
            })
            .unwrap_or_default();
        return Err(format!("the scan shader did not compile: {e} {text}"));
    }
    code.ok_or_else(|| "the scan shader compiled to nothing".to_owned())
}

impl Scanner {
    pub(crate) fn new(device: &ID3D12Device) -> Result<Scanner, String> {
        let init = compile(s!("cs_init"))?;
        let scan = compile(s!("cs_scan"))?;
        let locate = compile(s!("cs_locate"))?;
        let bytes = |b: &ID3DBlob| {
            // SAFETY: the blob's own bytes, which live as long as the blob.
            unsafe { std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize()) }
        };
        // SAFETY: the blobs hold the shaders and the root signature inside them.
        unsafe {
            let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes(&init)).map_err(|e| format!("scan root signature: {e}"))?;
            let make = |code: &ID3DBlob| -> Result<ID3D12PipelineState, String> {
                let code = bytes(code);
                device
                    .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                        pRootSignature: std::mem::transmute_copy(&root),
                        CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() },
                        ..Default::default()
                    })
                    .map_err(|e| format!("scan pipeline: {e}"))
            };
            Ok(Scanner { init: make(&init)?, scan: make(&scan)?, locate: make(&locate)?, root })
        }
    }

    /// Records the scan of `subresource` of `texture`, in the copy-source
    /// state on entry and on exit, into result slot `slot` of `results`
    /// (a buffer in the unordered access state, on entry and on exit).
    /// `scratch` is a buffer in the common state, left in it, big enough for
    /// the subresource ([`footprint`]). `probe`: a pixel whose value is also
    /// stored.
    pub(crate) fn record(
        &self,
        list: &ID3D12GraphicsCommandList,
        texture: &ID3D12Resource,
        subresource: u32,
        kind: u32,
        footprint: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
        scratch: &ID3D12Resource,
        results: &ID3D12Resource,
        slot: u32,
        probe: Option<(u32, u32)>,
    ) {
        let f = footprint.Footprint;
        let (px, py) = probe.unwrap_or((NONE, NONE));
        let base = HEADER_DWORDS + slot * SLOT_DWORDS;
        let words = [f.Width, f.Height, f.RowPitch, kind, base, px, py, 0u32];
        let barrier = |state: (D3D12_RESOURCE_STATES, D3D12_RESOURCE_STATES)| crate::host::transition(scratch, state.0, state.1);
        // SAFETY: an open list; live resources that the caller keeps alive
        // until the list has run; the scratch buffer holds the footprint.
        unsafe {
            list.SetComputeRootSignature(&self.root);
            list.SetComputeRoot32BitConstants(0, 8, words.as_ptr().cast(), 0);
            list.SetComputeRootUnorderedAccessView(2, results.GetGPUVirtualAddress());
            list.SetPipelineState(&self.init);
            list.Dispatch(1, 1, 1);
            uav_barrier(list);
            list.ResourceBarrier(&[barrier((D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST))]);
            let to = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(scratch),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: *footprint },
            };
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(texture),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: subresource },
            };
            list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
            list.ResourceBarrier(&[barrier((D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE))]);
            list.SetComputeRootShaderResourceView(1, scratch.GetGPUVirtualAddress());
            list.SetPipelineState(&self.scan);
            list.Dispatch(f.Width.div_ceil(8), f.Height.div_ceil(8), 1);
            uav_barrier(list);
            list.SetPipelineState(&self.locate);
            list.Dispatch(1, 1, 1);
            list.ResourceBarrier(&[barrier((D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COMMON))]);
            uav_barrier(list);
        }
    }
}

fn uav_barrier(list: &ID3D12GraphicsCommandList) {
    let barrier = D3D12_RESOURCE_BARRIER { Type: D3D12_RESOURCE_BARRIER_TYPE_UAV, ..Default::default() };
    // SAFETY: an open list; the barrier names no resource.
    unsafe { list.ResourceBarrier(&[barrier]) };
}

/// The scan kind and bytes per pixel of a float format (the numbers
/// `SOURCE` decodes), `None` for any other.
pub(crate) fn float_kind(format: u32) -> Option<(u32, u32)> {
    Some(match format {
        2 => (1, 16),
        6 => (2, 12),
        16 => (3, 8),
        41 => (4, 4),
        10 => (5, 8),
        34 => (6, 4),
        54 => (7, 2),
        26 => (8, 4),
        _ => return None,
    })
}

/// Bytes per pixel of a float format or of its typeless family (a texture
/// made typeless is written through a float view).
fn family_bytes(format: u32) -> Option<u32> {
    match format {
        1 | 2 => Some(16),
        5 | 6 => Some(12),
        15 | 16 | 9 | 10 => Some(8),
        26 | 33 | 34 | 39 | 41 => Some(4),
        53 | 54 => Some(2),
        _ => None,
    }
}

fn is_typeless(format: u32) -> bool {
    matches!(format, 1 | 5 | 9 | 15 | 33 | 39 | 53)
}

/// The scan kind for a texture of `resource_format` written through a view
/// of `view_format`, or why it is not scanned.
pub(crate) fn kind_for(resource_format: u32, view_format: u32) -> Result<u32, String> {
    let Some((kind, bytes)) = float_kind(view_format) else {
        return Err(format!("{} is not a float format", victory_mcp::badpix::format_name(view_format)));
    };
    match family_bytes(resource_format) {
        Some(b) if b == bytes && (is_typeless(resource_format) || resource_format == view_format) => Ok(kind),
        _ => Err(format!(
            "the texture is {}, not a float texture the view {} reads",
            victory_mcp::badpix::format_name(resource_format),
            victory_mcp::badpix::format_name(view_format)
        )),
    }
}

/// The footprint of one subresource as a placed copy at offset 0, and the
/// bytes it takes.
pub(crate) fn footprint(device: &ID3D12Device, desc: &D3D12_RESOURCE_DESC, subresource: u32) -> (D3D12_PLACED_SUBRESOURCE_FOOTPRINT, u64) {
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total = 0;
    // SAFETY: outputs for one subresource.
    unsafe { device.GetCopyableFootprints(desc, subresource, 1, 0, Some(&mut footprint), None, None, Some(&mut total)) };
    (footprint, total)
}

fn buffer_desc(size: u64, flags: D3D12_RESOURCE_FLAGS) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: flags,
        ..Default::default()
    }
}

/// A committed buffer.
pub(crate) fn buffer(
    device: &ID3D12Device,
    heap: D3D12_HEAP_TYPE,
    size: u64,
    flags: D3D12_RESOURCE_FLAGS,
    state: D3D12_RESOURCE_STATES,
) -> Result<ID3D12Resource, String> {
    let properties = D3D12_HEAP_PROPERTIES { Type: heap, ..Default::default() };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: plain creation.
    let result = unsafe { device.CreateCommittedResource(&properties, D3D12_HEAP_FLAG_NONE, &buffer_desc(size, flags), state, None, &mut made) };
    made.filter(|_| result.is_ok()).ok_or_else(|| format!("no buffer of {size} bytes: {result:?}"))
}

/// The results buffer, in the unordered access state (zeroed by creation).
pub(crate) fn results_buffer(device: &ID3D12Device) -> Result<ID3D12Resource, String> {
    buffer(
        device,
        D3D12_HEAP_TYPE_DEFAULT,
        results_bytes(),
        D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
    )
}

pub(crate) fn results_bytes() -> u64 {
    u64::from(HEADER_DWORDS + SLOTS * SLOT_DWORDS) * 4
}

/// What a slot's sixteen words say.
pub(crate) fn parse_slot(w: &[u32]) -> Counts {
    let first = (w[3] != NONE).then(|| (w[3] & 0xffff, w[3] >> 16));
    Counts {
        ran: w[5] != 0,
        nan: w[0],
        inf: w[1],
        max_bits: w[2],
        order: w[4],
        first,
        first_value: first.map(|_| [w[11], w[12], w[13], w[14]]),
        pixel: (w[15] == 1).then(|| [w[7], w[8], w[9], w[10]]),
    }
}

/// The slot `slot` of the results as bytes read back.
pub(crate) fn slot_words(bytes: &[u8], slot: u32) -> Option<[u32; SLOT_DWORDS as usize]> {
    let at = (HEADER_DWORDS + slot * SLOT_DWORDS) as usize * 4;
    let chunk = bytes.get(at..at + SLOT_DWORDS as usize * 4)?;
    let mut words = [0u32; SLOT_DWORDS as usize];
    for (w, b) in words.iter_mut().zip(chunk.chunks_exact(4)) {
        *w = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    }
    Some(words)
}

// ---- the capture ----

/// A target of a command, as the command's state names it.
pub(crate) struct Source {
    /// The title resource.
    pub(crate) resource: usize,
    /// `rt0`, ... or `uav`.
    pub(crate) role: String,
    /// The format the command writes it through (0: the texture's own).
    pub(crate) view_format: u32,
    pub(crate) subresource: u32,
}

struct TargetRec {
    role: String,
    resource: u64,
    format: u32,
    width: u32,
    height: u32,
    subresource: u32,
    pre: Option<u32>,
    post: Option<u32>,
    skipped: Option<String>,
}

struct EntryRec {
    index: u32,
    kind: &'static str,
    pipeline: String,
    shaders: Vec<(String, String)>,
    targets: Vec<TargetRec>,
}

struct Scratch {
    buffers: Vec<ID3D12Resource>,
    size: u64,
}

struct Capture {
    request: Mutex<Option<ScanRequest>>,
    params: ScanParams,
    scanner: Arc<Scanner>,
    device: ID3D12Device,
    results: ID3D12Resource,
    scratch: Mutex<Scratch>,
    next_slot: AtomicU32,
    /// Commands that got an entry.
    entries_made: AtomicU32,
    over_cap: AtomicU32,
    over_slots: AtomicU32,
    /// Commands that wrote no float target.
    no_float: AtomicU32,
    /// Subresources too big, multisampled or of another kind, by reason.
    skips: Mutex<Vec<(String, u32)>>,
    entries: Mutex<Vec<EntryRec>>,
    back: (u32, u32),
    /// The command lists that got scans, by the list's own object (a host
    /// list moves when the title submits it): whether each has been given
    /// to the card since its last scan was recorded. With the lists
    /// translated by threads of ours (`VICTORY_THIN`) the title records
    /// ahead of the present, so scans recorded in the capture's window sit
    /// in lists that run after the window's end.
    listed: Mutex<HashMap<usize, bool>>,
    /// Presents that found lists with scans not yet submitted, after the
    /// capture stopped recording.
    waited: AtomicU32,
    /// Lists with scans that were still not submitted when the results were
    /// read back anyway (given up waiting).
    unsent: AtomicU32,
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

static CURRENT: Mutex<Option<Arc<Capture>>> = Mutex::new(None);
/// A capture has stopped recording and waits for the lists holding its scans
/// to be submitted ([`due`]).
static ENDING: AtomicBool = AtomicBool::new(false);
/// A capture has lists with scans ([`submitted`] has something to note).
static LISTED: AtomicBool = AtomicBool::new(false);
/// Presents a stopped capture waits for its lists to be submitted before the
/// results are read back as they are.
const MAX_WAITED: u32 = 600;
static SCANNER: OnceLock<Option<Arc<Scanner>>> = OnceLock::new();

/// Whether a capture is recording: one relaxed load of an atomic, false
/// whenever no capture is asked for.
#[inline]
pub(crate) fn recording() -> bool {
    victory_mcp::scan_recording()
}

/// Whether the capture asked for the texture UAVs too.
pub(crate) fn wants_uav() -> bool {
    locked(&CURRENT).as_ref().is_some_and(|c| c.params.include_uav)
}

impl Capture {
    /// A scratch buffer of at least `bytes`, made bigger when needed (the
    /// older ones stay until the capture ends: lists recorded earlier use them).
    fn scratch(&self, bytes: u64) -> Option<ID3D12Resource> {
        let mut s = locked(&self.scratch);
        if s.size < bytes || s.buffers.is_empty() {
            let size = bytes.max(64 << 20).next_multiple_of(1 << 20);
            let made = buffer(&self.device, D3D12_HEAP_TYPE_DEFAULT, size, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COMMON);
            match made {
                Ok(b) => {
                    s.buffers.push(b);
                    s.size = size;
                }
                Err(why) => {
                    tracing::warn!(%why, "no scratch buffer for the bad-pixel scan");
                    return None;
                }
            }
        }
        s.buffers.last().cloned()
    }

    fn skip(&self, why: &str) {
        let mut skips = locked(&self.skips);
        match skips.iter_mut().find(|(w, _)| w == why) {
            Some((_, n)) => *n += 1,
            None => skips.push((why.to_owned(), 1)),
        }
    }
}

/// What a scan of one target needs, found when the command begins.
struct Planned {
    resource: usize,
    subresource: u32,
    kind: u32,
    probe: Option<(u32, u32)>,
    pre: u32,
    post: u32,
    /// The target's place in the entry.
    at: usize,
}

/// A command being scanned: the pre-scans are recorded, the post-scans wait
/// for [`end`].
pub(crate) struct Ticket {
    capture: Arc<Capture>,
    entry: usize,
    planned: Vec<Planned>,
}

fn scanner(device: &ID3D12Device) -> Result<Arc<Scanner>, String> {
    match SCANNER.get_or_init(|| match Scanner::new(device) {
        Ok(s) => Some(Arc::new(s)),
        Err(why) => {
            tracing::warn!(%why, "the bad-pixel scan cannot be made");
            None
        }
    }) {
        Some(s) => Ok(s.clone()),
        None => Err("the scan shader could not be made (see the log)".to_owned()),
    }
}

/// Starts or finishes the capture at a present. `list` is the present's
/// open list, which gets the copy of the results at the end.
pub(crate) fn present(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    edge: ScanEdge,
    size: (u32, u32),
) -> Option<Pending> {
    match edge {
        ScanEdge::Begin(request) => {
            let made = scanner(device).and_then(|scanner| {
                let results = results_buffer(device)?;
                Ok((scanner, results))
            });
            match made {
                Ok((scanner, results)) => {
                    let params = request.params.clone();
                    *locked(&CURRENT) = Some(Arc::new(Capture {
                        request: Mutex::new(Some(request)),
                        params,
                        scanner,
                        device: device.clone(),
                        results,
                        scratch: Mutex::new(Scratch { buffers: Vec::new(), size: 0 }),
                        next_slot: AtomicU32::new(0),
                        entries_made: AtomicU32::new(0),
                        over_cap: AtomicU32::new(0),
                        over_slots: AtomicU32::new(0),
                        no_float: AtomicU32::new(0),
                        skips: Mutex::new(Vec::new()),
                        entries: Mutex::new(Vec::new()),
                        back: size,
                        listed: Mutex::new(HashMap::new()),
                        waited: AtomicU32::new(0),
                        unsent: AtomicU32::new(0),
                    }));
                    ENDING.store(false, Ordering::Release);
                    LISTED.store(false, Ordering::Release);
                }
                Err(why) => request.fail(why),
            }
            None
        }
        ScanEdge::Cancel => {
            ENDING.store(false, Ordering::Release);
            LISTED.store(false, Ordering::Release);
            if let Some(capture) = locked(&CURRENT).take()
                && let Some(request) = locked(&capture.request).take()
            {
                request.fail("the capture was cancelled");
            }
            None
        }
        ScanEdge::End => {
            let capture = locked(&CURRENT).take()?;
            ENDING.store(false, Ordering::Release);
            LISTED.store(false, Ordering::Release);
            capture.unsent.store(capture.unsubmitted(), Ordering::Relaxed);
            let bytes = u64::from(HEADER_DWORDS + capture.next_slot.load(Ordering::Relaxed).min(SLOTS) * SLOT_DWORDS) * 4;
            let readback = buffer(device, D3D12_HEAP_TYPE_READBACK, bytes, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST);
            let readback = match readback {
                Ok(b) => b,
                Err(why) => {
                    if let Some(request) = locked(&capture.request).take() {
                        request.fail(why);
                    }
                    return None;
                }
            };
            // SAFETY: an open list; the results are in the unordered access
            // state (every scan leaves them there), and go back to it.
            unsafe {
                list.ResourceBarrier(&[crate::host::transition(
                    &capture.results,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                )]);
                list.CopyBufferRegion(&readback, 0, &capture.results, 0, bytes);
                list.ResourceBarrier(&[crate::host::transition(
                    &capture.results,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                )]);
            }
            Some(Pending { capture, readback, bytes })
        }
    }
}

impl Capture {
    /// How many lists with scans have not been given to the card.
    fn unsubmitted(&self) -> u32 {
        locked(&self.listed).values().filter(|sent| !**sent).count() as u32
    }
}

/// The capture stopped recording at this present (the title's lists for the
/// frame were translated by now, perhaps ahead of the lists that ran): the
/// results are read back at the first present from which every list with
/// scans has been given to the card ([`due`]).
pub(crate) fn stop_recording() {
    if locked(&CURRENT).is_some() {
        ENDING.store(true, Ordering::Release);
    }
}

/// Whether a capture that stopped recording can have its results read back at
/// this present: every list that holds a scan of it has been submitted (the
/// present's own list runs behind them), or it waited long enough.
#[inline]
pub(crate) fn due() -> bool {
    ENDING.load(Ordering::Acquire) && due_now()
}

#[cold]
#[inline(never)]
fn due_now() -> bool {
    let Some(capture) = locked(&CURRENT).clone() else {
        ENDING.store(false, Ordering::Release);
        return false;
    };
    capture.unsubmitted() == 0 || capture.waited.fetch_add(1, Ordering::Relaxed) >= MAX_WAITED
}

/// `list` is being given to the card: its scans run before whatever is
/// submitted after it.
#[inline]
pub(crate) fn submitted(list: &HostList) {
    if LISTED.load(Ordering::Acquire) {
        submitted_listed(list);
    }
}

#[cold]
#[inline(never)]
fn submitted_listed(list: &HostList) {
    let Some(capture) = locked(&CURRENT).clone() else { return };
    if let Some(sent) = locked(&capture.listed).get_mut(&(list.list.as_raw() as usize)) {
        *sent = true;
    }
}

/// A capture whose results are being copied by the present's list.
pub(crate) struct Pending {
    capture: Arc<Capture>,
    readback: ID3D12Resource,
    bytes: u64,
}

impl Pending {
    /// Once the present's list has run (and everything before it): reads
    /// the results and answers the request.
    pub(crate) fn deliver(self, frame: u64) {
        let Some(request) = locked(&self.capture.request).take() else { return };
        let mut data = std::ptr::null_mut();
        // SAFETY: the copy has finished; the mapping spans the buffer.
        let bytes = unsafe {
            if self.readback.Map(0, None, Some(&mut data)).is_err() || data.is_null() {
                request.fail("the results buffer could not be mapped");
                return;
            }
            let bytes = std::slice::from_raw_parts(data.cast::<u8>(), self.bytes as usize).to_vec();
            self.readback.Unmap(0, None);
            bytes
        };
        request.deliver(report(&self.capture, &bytes, frame));
    }
}

fn report(capture: &Capture, bytes: &[u8], frame: u64) -> ScanReport {
    let counts = |slot: Option<u32>| slot.and_then(|s| slot_words(bytes, s)).map(|w| parse_slot(&w));
    let entries = locked(&capture.entries);
    let draws = entries
        .iter()
        .map(|e| DrawScan {
            index: e.index,
            kind: e.kind.to_owned(),
            pipeline: e.pipeline.clone(),
            shaders: e.shaders.clone(),
            targets: e
                .targets
                .iter()
                .map(|t| TargetScan {
                    role: t.role.clone(),
                    resource: t.resource,
                    format: t.format,
                    width: t.width,
                    height: t.height,
                    subresource: t.subresource,
                    pre: counts(t.pre),
                    post: counts(t.post),
                    skipped: t.skipped.clone(),
                })
                .collect(),
        })
        .collect();
    let mut notes = Vec::new();
    let no_float = capture.no_float.load(Ordering::Relaxed);
    if no_float > 0 {
        notes.push(format!("{no_float} draws and dispatches wrote no float target and were not scanned"));
    }
    for (why, n) in locked(&capture.skips).iter() {
        notes.push(format!("{n} targets not scanned: {why}"));
    }
    let unsent = capture.unsent.load(Ordering::Relaxed);
    if unsent > 0 {
        notes.push(format!(
            "{unsent} command lists with scans were never given to the card (the title reset or dropped them); their scans did not run"
        ));
    }
    if !capture.params.include_uav {
        notes.push("texture UAVs were not scanned (include_uav is false)".to_owned());
    }
    notes.push(
        "UAV textures are found through descriptor tables of the compute or graphics roots; views reached by a computed \
         index, root UAV one-view tables and buffers are not scanned"
            .to_owned(),
    );
    ScanReport {
        frame,
        back_buffer: capture.back,
        params: capture.params.clone(),
        draws,
        slots_used: capture.next_slot.load(Ordering::Relaxed).min(SLOTS),
        slots_total: SLOTS,
        over_cap: capture.over_cap.load(Ordering::Relaxed),
        over_slots: capture.over_slots.load(Ordering::Relaxed),
        notes,
    }
}

/// Plans the scans of a command's `sources`; `None` if it gets none (no
/// float target, over the cap, or no slots left).
#[cold]
#[inline(never)]
pub(crate) fn begin(list: &HostList, kind: &'static str, pipeline: Option<&HostPipeline>, sources: &[Source]) -> Option<Ticket> {
    let capture = locked(&CURRENT).clone()?;
    let mut recs = Vec::new();
    let mut planned = Vec::new();
    for source in sources {
        let Some(host) = crate::resource::host_resource(source.resource as This) else {
            continue;
        };
        // SAFETY: a live resource.
        let desc = unsafe { host.GetDesc() };
        let own = desc.Format.0 as u32;
        let view = if source.view_format != 0 { source.view_format } else { own };
        let mut rec = TargetRec {
            role: source.role.clone(),
            resource: source.resource as u64,
            format: view,
            width: 0,
            height: 0,
            subresource: source.subresource,
            pre: None,
            post: None,
            skipped: None,
        };
        let verdict = (|| {
            let kind = kind_for(own, view)?;
            if desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D {
                return Err("not a 2D texture".to_owned());
            }
            if desc.SampleDesc.Count > 1 {
                return Err("multisampled".to_owned());
            }
            let subs = u32::from(desc.MipLevels) * u32::from(desc.DepthOrArraySize);
            if source.subresource >= subs {
                return Err("subresource out of range".to_owned());
            }
            let mip = source.subresource % u32::from(desc.MipLevels.max(1));
            let (w, h) = ((desc.Width >> mip).max(1) as u32, (desc.Height >> mip).max(1));
            if w > MAX_SIDE || h > MAX_SIDE {
                return Err(format!("{w}x{h} is bigger than {MAX_SIDE}"));
            }
            Ok((kind, w, h))
        })();
        match verdict {
            Ok((scan_kind, w, h)) => {
                (rec.width, rec.height) = (w, h);
                // The followed pixel is shown on the textures of the back buffer's size.
                let probe = capture.params.pixel.filter(|_| source.subresource == 0 && (w, h) == capture.back);
                planned.push(Planned { resource: source.resource, subresource: source.subresource, kind: scan_kind, probe, pre: 0, post: 0, at: recs.len() });
            }
            Err(why) => {
                capture.skip(&why);
                rec.skipped = Some(why);
            }
        }
        recs.push(rec);
    }
    if planned.is_empty() {
        capture.no_float.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    if capture.entries_made.fetch_add(1, Ordering::Relaxed) >= capture.params.max_draws {
        capture.entries_made.fetch_sub(1, Ordering::Relaxed);
        capture.over_cap.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    let needed = planned.len() as u32 * 2;
    let first = capture.next_slot.fetch_add(needed, Ordering::Relaxed);
    if first.saturating_add(needed) > SLOTS {
        capture.next_slot.store(SLOTS, Ordering::Relaxed);
        capture.entries_made.fetch_sub(1, Ordering::Relaxed);
        capture.over_slots.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    for (i, p) in planned.iter_mut().enumerate() {
        p.pre = first + i as u32 * 2;
        p.post = p.pre + 1;
        recs[p.at].pre = Some(p.pre);
        recs[p.at].post = Some(p.post);
    }
    let entry = {
        let mut entries = locked(&capture.entries);
        let index = entries.len() as u32;
        entries.push(EntryRec {
            index,
            kind,
            pipeline: pipeline.map_or_else(String::new, |p| p.name.clone()),
            shaders: pipeline.map_or_else(Vec::new, |p| p.stages.iter().map(|s| (s.shader.stage.to_owned(), s.shader.hash.clone())).collect()),
            targets: recs,
        });
        entries.len() - 1
    };
    let ticket = Ticket { capture, entry, planned };
    for p in &ticket.planned {
        scan(list, &ticket.capture, p, p.pre);
    }
    Some(ticket)
}

/// Records the scans after the command.
#[cold]
#[inline(never)]
pub(crate) fn end(list: &HostList, ticket: Ticket) {
    for p in &ticket.planned {
        scan(list, &ticket.capture, p, p.post);
    }
}

/// The command was dropped by the host: nothing was written after the
/// pre-scans.
#[cold]
#[inline(never)]
pub(crate) fn dropped(ticket: Ticket) {
    let mut entries = locked(&ticket.capture.entries);
    for p in &ticket.planned {
        let t = &mut entries[ticket.entry].targets[p.at];
        t.post = None;
        t.skipped = Some("the host dropped this command (nothing was drawn)".to_owned());
    }
}

/// Records one scan of the planned target into `slot` on `list`, moving the
/// texture to the copy-source state and back.
fn scan(list: &HostList, capture: &Capture, p: &Planned, slot: u32) {
    let resource = p.resource as This;
    let Some(host) = crate::resource::host_resource(resource) else { return };
    // SAFETY: a live resource.
    let desc = unsafe { host.GetDesc() };
    let (footprint, total) = footprint(&capture.device, &desc, p.subresource);
    let Some(scratch) = capture.scratch(total) else { return };
    let before = list.texture_state(resource);
    crate::list::ensure_state(list, resource, COPY_SOURCE);
    capture.scanner.record(&list.list, host, p.subresource, p.kind, &footprint, &scratch, &capture.results, slot, p.probe);
    if let Some(state) = before {
        crate::list::ensure_state(list, resource, state);
    }
    // The buffers stay alive until the list has run, whatever happens to the capture.
    list.keep(&scratch);
    list.keep(&capture.results);
    // The readback waits for this list to be submitted.
    locked(&capture.listed).insert(list.list.as_raw() as usize, false);
    LISTED.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::HANDLE;
    use windows::core::Interface;
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    /// A device, queue, list and fence of the test's own; `None` without a
    /// hardware adapter.
    struct Rig {
        device: ID3D12Device,
        queue: ID3D12CommandQueue,
        allocator: ID3D12CommandAllocator,
        list: ID3D12GraphicsCommandList,
        fence: ID3D12Fence,
        done: u64,
        event: HANDLE,
    }

    impl Rig {
        fn new() -> Option<Rig> {
            // SAFETY: plain object creation on a device of our own.
            unsafe {
                let mut device: Option<ID3D12Device> = None;
                D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device).ok()?;
                let device = device?;
                let desc = CreateDXGIFactory2::<IDXGIFactory4>(DXGI_CREATE_FACTORY_FLAGS(0))
                    .and_then(|factory| factory.EnumAdapterByLuid::<IDXGIAdapter3>(device.GetAdapterLuid()))
                    .ok()?
                    .cast::<IDXGIAdapter1>()
                    .ok()?
                    .GetDesc1()
                    .ok()?;
                if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                    return None;
                }
                let queue: ID3D12CommandQueue =
                    device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).ok()?;
                let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).ok()?;
                let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None).ok()?;
                list.Close().ok()?;
                let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).ok()?;
                let event = CreateEventW(None, false, false, None).ok()?;
                Some(Rig { device, queue, allocator, list, fence, done: 0, event })
            }
        }

        fn run(&mut self, record: impl FnOnce(&ID3D12GraphicsCommandList)) {
            // SAFETY: our own list and queue, one run at a time.
            unsafe {
                self.allocator.Reset().unwrap();
                self.list.Reset(&self.allocator, None).unwrap();
                record(&self.list);
                self.list.Close().unwrap();
                self.queue.ExecuteCommandLists(&[Some(self.list.cast().unwrap())]);
                self.done += 1;
                self.queue.Signal(&self.fence, self.done).unwrap();
                self.fence.SetEventOnCompletion(self.done, self.event).unwrap();
                assert_eq!(WaitForSingleObject(self.event, 20_000).0, 0, "the card did not finish");
                self.device.GetDeviceRemovedReason().unwrap();
            }
        }

        /// A texture of `format` and `size` holding `bytes` (rows of
        /// `row_bytes`), left in the copy-source state.
        fn texture(&mut self, format: DXGI_FORMAT, size: (u32, u32), row_bytes: usize, bytes: &[u8]) -> ID3D12Resource {
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: u64::from(size.0),
                Height: size.1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                ..Default::default()
            };
            let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
            let mut texture: Option<ID3D12Resource> = None;
            // SAFETY: plain creation.
            unsafe {
                self.device
                    .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut texture)
                    .unwrap()
            };
            let texture = texture.unwrap();
            let (fp, total) = footprint(&self.device, &desc, 0);
            let upload = buffer(&self.device, D3D12_HEAP_TYPE_UPLOAD, total, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_GENERIC_READ).unwrap();
            let mut data = std::ptr::null_mut();
            // SAFETY: the mapping holds `total` bytes; rows are copied into their pitch.
            unsafe {
                upload.Map(0, None, Some(&mut data)).unwrap();
                for y in 0..size.1 as usize {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr().add(y * row_bytes), data.cast::<u8>().add(y * fp.Footprint.RowPitch as usize), row_bytes);
                }
                upload.Unmap(0, None);
            }
            self.run(|list| {
                // SAFETY: live resources on an open list.
                unsafe {
                    let to = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(&texture),
                        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
                    };
                    let from = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(&upload),
                        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: fp },
                    };
                    list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
                    list.ResourceBarrier(&[crate::host::transition(&texture, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
                }
            });
            texture
        }

        /// Scans `texture` into the given slots of a fresh results buffer
        /// (`scans`: slot and probe) and reads the whole buffer back.
        fn scan(&mut self, scanner: &Scanner, texture: &ID3D12Resource, kind: u32, scans: &[(u32, Option<(u32, u32)>)]) -> Vec<u8> {
            let results = results_buffer(&self.device).unwrap();
            let desc = unsafe { texture.GetDesc() };
            let (fp, total) = footprint(&self.device, &desc, 0);
            let scratch = buffer(&self.device, D3D12_HEAP_TYPE_DEFAULT, total.max(1 << 20), D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COMMON).unwrap();
            let readback = buffer(&self.device, D3D12_HEAP_TYPE_READBACK, results_bytes(), D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST).unwrap();
            self.run(|list| {
                for &(slot, probe) in scans {
                    scanner.record(list, texture, 0, kind, &fp, &scratch, &results, slot, probe);
                }
                // SAFETY: live buffers on an open list.
                unsafe {
                    list.ResourceBarrier(&[crate::host::transition(&results, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
                    list.CopyBufferRegion(&readback, 0, &results, 0, results_bytes());
                }
            });
            let mut data = std::ptr::null_mut();
            // SAFETY: the copy has finished.
            unsafe {
                readback.Map(0, None, Some(&mut data)).unwrap();
                let bytes = std::slice::from_raw_parts(data.cast::<u8>(), results_bytes() as usize).to_vec();
                readback.Unmap(0, None);
                bytes
            }
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            // SAFETY: our event.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(self.event);
            }
        }
    }

    fn half(v: f32) -> u16 {
        // The values the tests use are exact in half precision.
        let b = v.to_bits();
        let sign = (b >> 16) as u16 & 0x8000;
        if v == 0.0 {
            return sign;
        }
        let e = ((b >> 23) & 0xff) as i32 - 127 + 15;
        sign | (e as u16) << 10 | ((b >> 13) & 0x3ff) as u16
    }

    const HALF_NAN: u16 = 0x7e00;
    const HALF_INF: u16 = 0x7c00;
    const HALF_NEG_INF: u16 = 0xfc00;

    fn rgba16(pixels: &[[u16; 4]]) -> Vec<u8> {
        pixels.iter().flatten().flat_map(|h| h.to_le_bytes()).collect()
    }

    #[test]
    fn the_scan_counts_nan_inf_and_finds_the_first_pixel_on_the_card() {
        let Some(mut rig) = Rig::new() else {
            eprintln!("no hardware adapter: the scan test is skipped");
            return;
        };
        let scanner = Scanner::new(&rig.device).expect("the scan shader compiles and its pipelines are made");
        // 19x11: not a multiple of the 8x8 group, so edge groups are covered.
        let (w, h) = (19usize, 11usize);
        let mut pixels = vec![[half(0.5), half(-2.0), half(4.0), half(1.0)]; w * h];
        // Largest finite value 1024 at (2, 9).
        pixels[9 * w + 2][2] = half(1024.0);
        // NaN in alpha at (7, 3) and in red at (18, 10); Inf at (5, 3) (positive,
        // in green), at (0, 6) (negative, in blue); both in one pixel at (12, 8).
        pixels[3 * w + 7][3] = HALF_NAN;
        pixels[10 * w + 18][0] = HALF_NAN;
        pixels[3 * w + 5][1] = HALF_INF;
        pixels[6 * w][2] = HALF_NEG_INF;
        pixels[8 * w + 12][0] = HALF_NAN;
        pixels[8 * w + 12][1] = HALF_INF;
        let texture = rig.texture(DXGI_FORMAT_R16G16B16A16_FLOAT, (w as u32, h as u32), w * 8, &rgba16(&pixels));
        let kind = kind_for(10, 10).unwrap();
        let bytes = rig.scan(&scanner, &texture, kind, &[(0, Some((12, 8))), (3, None)]);
        let first = parse_slot(&slot_words(&bytes, 0).unwrap());
        assert!(first.ran);
        // Pixels with a NaN: (7,3), (18,10), (12,8). With an Inf: (5,3), (0,6), (12,8).
        assert_eq!((first.nan, first.inf), (3, 3));
        assert_eq!(first.max_finite(), 1024.0);
        // Row order: the first bad pixel is on row 3, at x = 5.
        assert_eq!(first.first, Some((5, 3)));
        // Its value: (0.5, +Inf, 4.0, 1.0).
        let value = first.first_value.unwrap().map(f32::from_bits);
        assert_eq!((value[0], value[1], value[2], value[3]), (0.5, f32::INFINITY, 4.0, 1.0));
        // The followed pixel (12, 8): NaN, Inf, 4.0, 1.0.
        let followed = first.pixel.unwrap().map(f32::from_bits);
        assert!(followed[0].is_nan() && followed[1] == f32::INFINITY && followed[2] == 4.0 && followed[3] == 1.0);
        // Two scans in one run keep their own slots and are stamped in order.
        let second = parse_slot(&slot_words(&bytes, 3).unwrap());
        assert_eq!((second.nan, second.inf, second.first, second.pixel), (3, 3, Some((5, 3)), None));
        assert_eq!((first.order, second.order), (0, 1));
        // A slot nobody scanned says so.
        assert!(!parse_slot(&slot_words(&bytes, 1).unwrap()).ran);
    }

    #[test]
    fn a_clean_texture_has_no_bad_pixels_and_no_first() {
        let Some(mut rig) = Rig::new() else {
            eprintln!("no hardware adapter: the scan test is skipped");
            return;
        };
        let scanner = Scanner::new(&rig.device).unwrap();
        let (w, h) = (16usize, 16usize);
        let mut pixels = vec![[half(0.25), half(0.0), half(-8.0), half(1.0)]; w * h];
        pixels[0][0] = half(1023.5);
        let texture = rig.texture(DXGI_FORMAT_R16G16B16A16_FLOAT, (16, 16), w * 8, &rgba16(&pixels));
        let bytes = rig.scan(&scanner, &texture, 5, &[(0, None)]);
        let c = parse_slot(&slot_words(&bytes, 0).unwrap());
        assert_eq!((c.ran, c.nan, c.inf, c.first, c.first_value, c.pixel), (true, 0, 0, None, None, None));
        assert_eq!(c.max_finite(), 1023.5);
    }

    #[test]
    fn single_channel_32_bit_and_packed_formats_are_decoded_too() {
        let Some(mut rig) = Rig::new() else {
            eprintln!("no hardware adapter: the scan test is skipped");
            return;
        };
        let scanner = Scanner::new(&rig.device).unwrap();
        // R32_FLOAT, 4x2: a NaN at (3, 0) and an Inf at (1, 1), the largest finite 3.5.
        let mut r32 = vec![1.0f32.to_bits(); 8];
        r32[3] = f32::NAN.to_bits();
        r32[5] = f32::NEG_INFINITY.to_bits();
        r32[6] = 3.5f32.to_bits();
        let bytes32: Vec<u8> = r32.iter().flat_map(|v| v.to_le_bytes()).collect();
        let texture = rig.texture(DXGI_FORMAT_R32_FLOAT, (4, 2), 16, &bytes32);
        let bytes = rig.scan(&scanner, &texture, kind_for(41, 41).unwrap(), &[(0, None)]);
        let c = parse_slot(&slot_words(&bytes, 0).unwrap());
        assert_eq!((c.nan, c.inf, c.first, c.max_finite()), (1, 1, Some((3, 0)), 3.5));
        // R11G11B10_FLOAT, 2x1: (1.0, 1.0, 1.0) is clean; red exponent all ones with a
        // mantissa is NaN, blue exponent all ones with none is Inf: 0x7c0 | ... below.
        let clean = 0x3c0u32 | (0x3c0 << 11) | (0x1e0 << 22);
        let nan_red = 0x7c1u32 | (0x3c0 << 11) | (0x1e0 << 22);
        let inf_blue = 0x3c0u32 | (0x3c0 << 11) | (0x3e0 << 22);
        let packed: Vec<u8> = [clean, nan_red, inf_blue, clean].iter().flat_map(|v| v.to_le_bytes()).collect();
        let texture = rig.texture(DXGI_FORMAT_R11G11B10_FLOAT, (4, 1), 16, &packed);
        let bytes = rig.scan(&scanner, &texture, kind_for(26, 26).unwrap(), &[(0, None)]);
        let c = parse_slot(&slot_words(&bytes, 0).unwrap());
        assert_eq!((c.nan, c.inf, c.first), (1, 1, Some((1, 0))));
        assert_eq!(c.max_finite(), 1.0);
    }

    #[test]
    fn formats_with_no_nan_are_refused_and_typeless_textures_follow_their_view() {
        assert_eq!(kind_for(10, 10), Ok(5));
        assert_eq!(kind_for(9, 10), Ok(5));
        assert_eq!(kind_for(39, 41), Ok(4));
        assert_eq!(kind_for(26, 26), Ok(8));
        assert!(kind_for(28, 28).unwrap_err().contains("not a float format"));
        // A 32-bit float view of a 16-bit float texture, or a typed texture of another format.
        assert!(kind_for(10, 2).is_err());
        assert!(kind_for(11, 10).is_err());
        assert!(kind_for(41, 34).is_err());
    }

    #[test]
    fn slots_are_parsed_and_unscanned_slots_are_not_ran() {
        let mut words = [0u32; 16];
        words[3] = NONE;
        assert_eq!(parse_slot(&words), Counts { first: None, ..Counts::default() });
        words[5] = 64;
        words[0] = 2;
        words[3] = 5 << 16 | 7;
        words[4] = 9;
        words[11..15].copy_from_slice(&[1, 2, 3, 4]);
        words[15] = 1;
        words[7..11].copy_from_slice(&[5, 6, 7, 8]);
        let c = parse_slot(&words);
        assert_eq!((c.ran, c.nan, c.first, c.order), (true, 2, Some((7, 5)), 9));
        assert_eq!((c.first_value, c.pixel), (Some([1, 2, 3, 4]), Some([5, 6, 7, 8])));
        let bytes: Vec<u8> = (0..(HEADER_DWORDS + 2 * SLOT_DWORDS)).flat_map(|i| i.to_le_bytes()).collect();
        assert_eq!(slot_words(&bytes, 1).unwrap()[0], HEADER_DWORDS + SLOT_DWORDS);
        assert!(slot_words(&bytes, 2).is_none());
    }

    #[test]
    fn the_readback_waits_for_every_list_with_scans_to_be_submitted() {
        let Some(rig) = Rig::new() else {
            eprintln!("no hardware adapter; skipped");
            return;
        };
        let scanner = Arc::new(Scanner::new(&rig.device).unwrap());
        let capture = Arc::new(Capture {
            request: Mutex::new(None),
            params: ScanParams::default(),
            scanner,
            device: rig.device.clone(),
            results: results_buffer(&rig.device).unwrap(),
            scratch: Mutex::new(Scratch { buffers: Vec::new(), size: 0 }),
            next_slot: AtomicU32::new(0),
            entries_made: AtomicU32::new(0),
            over_cap: AtomicU32::new(0),
            over_slots: AtomicU32::new(0),
            no_float: AtomicU32::new(0),
            skips: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
            back: (4, 4),
            listed: Mutex::new(HashMap::new()),
            waited: AtomicU32::new(0),
            unsent: AtomicU32::new(0),
        });
        assert!(!due(), "no capture is ending");
        *locked(&CURRENT) = Some(capture.clone());
        // Two lists hold scans, translated ahead of the present, not run yet.
        locked(&capture.listed).insert(1, false);
        locked(&capture.listed).insert(2, false);
        LISTED.store(true, Ordering::Release);
        assert!(!due(), "recording has not stopped");
        stop_recording();
        assert!(!due(), "both lists are still to be submitted");
        assert_eq!(capture.unsubmitted(), 2);
        locked(&capture.listed).insert(1, true);
        assert!(!due(), "one list is still to be submitted");
        locked(&capture.listed).insert(2, true);
        assert!(due(), "every list holding scans has been submitted");
        // Given up after a long wait.
        locked(&capture.listed).insert(3, false);
        capture.waited.store(MAX_WAITED, Ordering::Relaxed);
        assert!(due());
        assert_eq!(capture.unsubmitted(), 1);
        *locked(&CURRENT) = None;
        ENDING.store(false, Ordering::Release);
        LISTED.store(false, Ordering::Release);
    }
}
