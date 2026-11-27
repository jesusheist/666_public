//! Switches, group 4 (see ../known.rs).

#[allow(unused_imports)]
use crate::{tunable, tunable_list};

tunable!(pub INPLACE: i64 = 0, 0..=255, "VICTORY_INPLACE",
    "Title memory in place: 0 off, 1 a window of title memory, 2 buffers too", live = Restart);
tunable!(pub INPLACE_MB: i64 = 4096, 0..=16777216, "VICTORY_INPLACE_MB",
    "Most MB of title memory kept in place at once", live = Restart);
tunable!(pub INPLACE_PROBE: i64 = 0, 0..=255, "VICTORY_INPLACE_PROBE",
    "In-place probe level: 0 off, 1 probe on memory of its own, 2 also opens part of the title's memory", live = Restart);
tunable!(pub INPLACE_PROBE_MB: i64 = 64, 0..=1024, "VICTORY_INPLACE_PROBE_MB",
    "MB the in-place probe uses (capped at 1024)", live = Restart);

tunable!(pub LAND: bool = true, "VICTORY_LAND",
    "The landing relay merges pieces (off: piece by piece)", live = Restart);
tunable!(pub LAND_ANYTIME: bool = true, "VICTORY_LAND_ANYTIME",
    "Landing may happen at any time (off: only at set points)", live = Restart);
tunable!(pub LAND_BESIDE: bool = true, "VICTORY_LAND_BESIDE",
    "Views of the title's pages are written through one of ours", live = Restart);
tunable!(pub LAND_SKIP: bool = false, "VICTORY_LAND_SKIP",
    "Pieces an earlier fetch holds as they are now are left out", live = Restart);
tunable!(pub LAND_TAKEN: bool = true, "VICTORY_LAND_TAKEN",
    "Blocks landed in are noted as up to date when nothing was written", live = Restart);
tunable!(pub UNDER_THREADS: i64 = 6, 1..=16, "VICTORY_UNDER_THREADS",
    "Threads kept for the title's bytes under flights' writes", live = Restart);

tunable!(pub COPY_OVER_TITLE: enum = "other", ["0", "title", "other"], "VICTORY_COPY_OVER_TITLE",
    "First copy into a title texture: 0 as before, title = title memory, other = zeros unless used", live = Restart);
tunable!(pub CUT_WRITES: bool = false, "VICTORY_CUT_WRITES",
    "Lists are cut at value writes too", live = Restart);
tunable!(pub LIST_POOL: bool = true, "VICTORY_LIST_POOL",
    "Host lists that have run are used again (off: each is made new)", live = Restart);
tunable!(pub READ_BACK: bool = true, "VICTORY_READ_BACK",
    "Texture copies out to buffers are made (off: left unmade)", live = Restart);
tunable!(pub WAIT_ASK: bool = false, "VICTORY_WAIT_ASK",
    "A wait for a memory value asks whether its page is committed first", live = Restart);
tunable!(pub THIN_THREADS: i64 = 8, 1..=32, "VICTORY_THIN_THREADS",
    "Threads for thin lists", live = Restart);

tunable!(pub BVH_SIZES_WAIT: bool = false, "VICTORY_BVH_SIZES_WAIT",
    "Wait for the ray-tracing structure sizes copy", live = Restart);
tunable!(pub BVH_TRACE: bool = false, "VICTORY_BVH_TRACE",
    "Trace the ray-tracing structure builder (diagnostics builds)", live = Restart);
tunable!(pub PICKED_MB: i64 = 32, 0..=65536, "VICTORY_PICKED_MB",
    "MB looked at per frame for picked-view writes", live = Restart);
tunable!(pub PICKED_WRITES: enum = "0", ["0", "1", "2"], "VICTORY_PICKED_WRITES",
    "Writes through views picked from the heap: 0 off, 1 declared, 2 landed too", live = Restart);
tunable!(pub BVH_QUARANTINE: bool = false, "VICTORY_BVH_QUARANTINE",
    "The ray-tracing builder's results stay in the host's copies (diagnostics builds)", live = Restart);

tunable!(pub ROOT_SRV_DIRECT: bool = false, "VICTORY_ROOT_SRV_DIRECT",
    "Root descriptors are used for read views again instead of bounded copies", live = Restart);
tunable!(pub EARLY_ALL: bool = false, "VICTORY_EARLY_ALL",
    "Every pipeline is requested at creation, used before or not", live = Restart);
tunable!(pub LIBRARY_HANDLE: bool = true, "VICTORY_LIBRARY_HANDLE",
    "The mapped library file's handle is kept open (off: closed after mapping)", live = Restart);
tunable!(pub LIBRARY_LOCK: bool = false, "VICTORY_LIBRARY_LOCK",
    "One lock is held around every library call", live = Restart);
tunable!(pub LIBRARY_MAP: bool = true, "VICTORY_LIBRARY_MAP",
    "The pipeline library file is mapped (off: read into buffers)", live = Restart);
tunable!(pub LIBRARY_STATIC_SAMPLERS: bool = false, "VICTORY_LIBRARY_STATIC_SAMPLERS",
    "Pipelines with static samplers are loaded from and stored in the library too", live = Restart);
tunable!(pub LIBRARY_TRIM: bool = true, "VICTORY_LIBRARY_TRIM",
    "The mapped library is trimmed when idle (off: pages stay until Windows takes them)", live = Restart);

tunable!(pub PIPELINE_LOWEST: bool = true, "VICTORY_PIPELINE_LOWEST",
    "Pipeline workers run at the lowest priority (off: level with the streaming thread)", live = Restart);
tunable!(pub PIPELINE_SLOW: bool = true, "VICTORY_PIPELINE_SLOW",
    "Slow pipelines are limited to a share of the workers (off: in the title's order)", live = Restart);
tunable!(pub PIPELINE_WAIT_MS: i64 = 500, 0..=600000, "VICTORY_PIPELINE_WAIT_MS",
    "Milliseconds per title frame the title may wait for pipelines", live = Restart);
tunable!(pub PIPELINE_WAIT_SHARED: bool = true, "VICTORY_PIPELINE_WAIT_SHARED",
    "The pipeline wait budget is shared over the title's threads (off: each counts alone)", live = Restart);
tunable!(pub SCREEN_WAIT_MS: i64 = 10000, 0..=600000, "VICTORY_SCREEN_WAIT_MS",
    "As the pipeline wait, in milliseconds, for passes over the screen", live = Restart);

tunable!(pub POP_ALL: bool = false, "VICTORY_POP_ALL",
    "Pop watch keeps every picture (diagnostics builds)", live = Restart);

tunable!(pub CUT_WAITS: bool = true, "VICTORY_CUT_WAITS",
    "Cut lists into host lists at their waits for memory values (off: whole lists)", live = Restart);
tunable!(pub GPU_LOAD_MS: i64 = 0, 0..=1000000, "VICTORY_GPU_LOAD_MS",
    "What the title's GPU time is made to look like, in ms (0: as it is)", live = Restart);
tunable!(pub QUEUE_WAIT: bool = true, "VICTORY_QUEUE_WAIT",
    "Queues wait for fences (off: every submission runs when it is made)", live = Restart);
tunable!(pub SPLIT_WAITS: bool = true, "VICTORY_SPLIT_WAITS",
    "Hold a submission list by list at its waits (off: whole submissions)", live = Restart);
tunable!(pub WAIT_FORCE: bool = true, "VICTORY_WAIT_FORCE",
    "A present runs what queues still hold (off: waits are kept across presents)", live = Restart);
tunable!(pub SUBMIT_DEPTH: i64 = 64, 1..=1000000, "VICTORY_SUBMIT_DEPTH",
    "How many submissions may be in the line when a submit call returns", live = Restart);
tunable!(pub SUBMIT_FRAMES: i64 = 1, 0..=1000000, "VICTORY_SUBMIT_FRAMES",
    "How many presents may be in the line when a present call returns", live = Restart);

tunable!(pub REPLAY_GBV: bool = false, "VICTORY_REPLAY_GBV",
    "Replay checks every shader access on the card, very slow (diagnostics builds)", live = Restart);
tunable!(pub REPLAY_PEEK: bool = false, "VICTORY_REPLAY_PEEK",
    "Replay peeks at every diverging command (diagnostics builds)", live = Restart);
tunable!(pub RECORD_ALWAYS: bool = false, "VICTORY_RECORD_ALWAYS",
    "Recording is kept every time, with VICTORY_RECORD set (diagnostics builds)", live = Restart);
tunable!(pub RECORD_FRAMES: i64 = 1, 1..=1000000, "VICTORY_RECORD_FRAMES",
    "Frames a recording holds, with VICTORY_RECORD set (diagnostics builds)", live = Restart);

tunable!(pub ALIAS_COPY: enum = "all", ["all", "0", "1"], "VICTORY_ALIAS_COPY",
    "Aliased texture copies found: all, 0 none, 1 only textures of one level and one layer", live = Restart);
tunable!(pub CARD_ROOM_FOR_TARGETS_MB: i64 = 3072, 0..=8192, "VICTORY_CARD_ROOM_FOR_TARGETS_MB",
    "MB of the card kept for render targets in the texture queue (0: none)", live = Restart);
tunable!(pub CARD_ROOM_FOR_TARGETS_SETTLE_MS: i64 = 20000, 0..=3600000, "VICTORY_CARD_ROOM_FOR_TARGETS_SETTLE_MS",
    "Milliseconds the render targets stay level before the reserve for them is let go", live = Restart);
tunable!(pub DEPTH_TWINS: enum = "on", ["on", "0", "stencil", "nodepth"], "VICTORY_DEPTH_TWINS",
    "Depth twins: on, 0 off, stencil only, nodepth", live = Restart);
tunable!(pub HELD_LEVELS: bool = true, "VICTORY_HELD_LEVELS",
    "Only held levels are uploaded again", live = Restart);
tunable!(pub LAZY_TEXTURES: bool = true, "VICTORY_LAZY_TEXTURES",
    "Host textures are made when first used", live = Restart);
tunable!(pub PLACED_FILLER: bool = false, "VICTORY_PLACED_FILLER",
    "Filler words stay in the placed-texture key", live = Restart);
tunable!(pub TEXTURE_SWEEP: enum = "on", ["on", "0", "whole"], "VICTORY_TEXTURE_SWEEP",
    "Idle texture sweep: on (a part each look), 0 lets nothing go, whole (all each time)", live = Restart);
tunable!(pub SAFE_COPY: bool = true, "VICTORY_SAFE_COPY",
    "Copies of the title's memory are made here and not by the kernel", live = Restart);
tunable!(pub BVH_BOUNDS: bool = true, "VICTORY_BVH_BOUNDS",
    "The structure builder's accesses stay near their root views (off: no bounds)", live = Restart);

tunable_list!(pub(super) fn register:
    INPLACE, INPLACE_MB, INPLACE_PROBE, INPLACE_PROBE_MB,
    LAND, LAND_ANYTIME, LAND_BESIDE, LAND_SKIP, LAND_TAKEN, UNDER_THREADS,
    COPY_OVER_TITLE, CUT_WRITES, LIST_POOL, READ_BACK, WAIT_ASK, THIN_THREADS,
    BVH_SIZES_WAIT, BVH_TRACE, PICKED_MB, PICKED_WRITES, BVH_QUARANTINE,
    ROOT_SRV_DIRECT, EARLY_ALL, LIBRARY_HANDLE, LIBRARY_LOCK, LIBRARY_MAP,
    LIBRARY_STATIC_SAMPLERS, LIBRARY_TRIM,
    PIPELINE_LOWEST, PIPELINE_SLOW, PIPELINE_WAIT_MS, PIPELINE_WAIT_SHARED, SCREEN_WAIT_MS,
    POP_ALL,
    CUT_WAITS, GPU_LOAD_MS, QUEUE_WAIT, SPLIT_WAITS, WAIT_FORCE, SUBMIT_DEPTH, SUBMIT_FRAMES,
    REPLAY_GBV, REPLAY_PEEK, RECORD_ALWAYS, RECORD_FRAMES,
    ALIAS_COPY, CARD_ROOM_FOR_TARGETS_MB, CARD_ROOM_FOR_TARGETS_SETTLE_MS, DEPTH_TWINS,
    HELD_LEVELS, LAZY_TEXTURES, PLACED_FILLER, TEXTURE_SWEEP, SAFE_COPY, BVH_BOUNDS
);
