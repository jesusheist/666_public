//! gpu_draw tunables (see ../known.rs): draws, indirect clamps, pipelines, library, landing.

#[allow(unused_imports)]
use crate::{tunable, tunable_list};

tunable!(pub EARLY_TEXTURES_MS: i64 = 12, 0..=100000, "VICTORY_EARLY_TEXTURES_MS",
    "Milliseconds a frame may spend on textures early in loading (0: never)", live = Safe);
tunable!(pub BAKE_TEXTURES_MS: i64 = 1500, 0..=100000, "VICTORY_BAKE_TEXTURES_MS",
    "Milliseconds a frame may spend on textures during the title's bakes (0: nothing special)", live = Safe);
tunable!(pub BAKE_EXACT: bool = true, "VICTORY_BAKE_EXACT",
    "A bake's draw finds textures as title memory has them at that moment (off: as the pass leaves them)", live = Safe);
tunable!(pub RECHECK_SOON: bool = true, "VICTORY_RECHECK_SOON",
    "A changed texture with nothing to tell of it is looked at again next frame (off: after four)", live = Safe);
tunable!(pub WRITE_SKIP: bool = true, "VICTORY_WRITE_SKIP",
    "Write ranges that did not change are not declared (off: every range)", live = NextFrame);
tunable!(pub INDIRECT_WINDOW: bool = false, "VICTORY_INDIRECT_WINDOW",
    "Indirect records are clamped in windows", live = NextFrame);
tunable!(pub INDIRECT_BATCH: enum = "1", ["0", "1", "2"], "VICTORY_INDIRECT_BATCH",
    "Indirect executions clamped in batches: 0 = each alone, 1 = batches, 2 = batches that dispatches may break", live = NextFrame);
tunable!(pub SCISSOR_TEST: enum = "off", ["off", "full"], "VICTORY_SCISSOR_TEST",
    "Experiment: full = a scissor rectangle the title sets cuts the walls away", live = NextFrame);
tunable!(pub HEAP_SPREAD: bool = true, "VICTORY_HEAP_SPREAD",
    "The heap texture pass spreads its looks over frames (off: all slots at once)", live = NextFrame);
tunable!(pub INDIRECT_WIDE: bool = true, "VICTORY_INDIRECT_WIDE",
    "Dispatch counts up to 1<<20 on AMD cards (off: capped at 65,535)", live = Restart);
tunable!(pub INDIRECT_REST: bool = false, "VICTORY_INDIRECT_REST",
    "An oversized indirect record takes the rest of its list's budget", live = Restart);
tunable!(pub BVH_QUARANTINE: bool = false, "VICTORY_BVH_QUARANTINE",
    "The ray-tracing builder's results stay in the host's copies", live = Restart);
tunable!(pub BVH_BUILDER: bool = false, "VICTORY_BVH_BUILDER",
    "The ray-tracing structure builder runs", live = Restart);
tunable!(pub LIBRARY_MAP: bool = true, "VICTORY_LIBRARY_MAP",
    "The pipeline library file is mapped (off: buffers)", live = Restart);
tunable!(pub LIBRARY_HANDLE: bool = true, "VICTORY_LIBRARY_HANDLE",
    "The mapped library file's handle is kept open (off: closed after mapping)", live = Restart);
tunable!(pub LIBRARY_TRIM: bool = true, "VICTORY_LIBRARY_TRIM",
    "The mapped library is trimmed when memory is idle (off: kept)", live = Restart);
tunable!(pub LIBRARY_LOCK: bool = false, "VICTORY_LIBRARY_LOCK",
    "One lock around every library call", live = Restart);
tunable!(pub EARLY_ALL: bool = false, "VICTORY_EARLY_ALL",
    "Every pipeline is requested early, used before or not", live = Restart);
tunable!(pub RT_RESOLVE: bool = false, "VICTORY_RT_RESOLVE",
    "Without the structure builder, CS_RaytracingResolve shaders are made too", live = Restart);
tunable!(pub PIPELINE_SLOW: bool = true, "VICTORY_PIPELINE_SLOW",
    "Slow pipelines are limited to a share of the workers (off: in the title's order)", live = Restart);
tunable!(pub PIPELINE_WAIT_MS: i64 = 500, 0..=600000, "VICTORY_PIPELINE_WAIT_MS",
    "Milliseconds per title frame the title may wait for pipelines", live = Safe);
tunable!(pub SCREEN_WAIT_MS: i64 = 10000, 0..=600000, "VICTORY_SCREEN_WAIT_MS",
    "As PIPELINE_WAIT_MS for passes over the screen (0: like the others)", live = Restart);
tunable!(pub PIPELINE_WAIT_SHARED: bool = true, "VICTORY_PIPELINE_WAIT_SHARED",
    "The pipeline wait budget is shared over the title's threads (off: each counts alone)", live = Safe);
tunable!(pub PIPELINE_WORKERS: i64 = 0, 0..=64, "VICTORY_PIPELINE_WORKERS",
    "Pipeline worker threads (0: worked out from the processors)", live = Restart);
tunable!(pub PIPELINE_LOWEST: bool = true, "VICTORY_PIPELINE_LOWEST",
    "Pipeline workers run at the lowest priority (off: below normal)", live = Restart);
tunable!(pub LAND: bool = true, "VICTORY_LAND",
    "The landing relay merges pieces (off: piece by piece)", live = Restart);
tunable!(pub LAND_THREADS: i64 = 0, 0..=16, "VICTORY_LAND_THREADS",
    "Threads that land (0: a quarter of the processors, 1 to 4)", live = Restart);
tunable!(pub LAND_BESIDE: bool = true, "VICTORY_LAND_BESIDE",
    "Views of the title's pages are written through one of ours", live = Restart);
tunable!(pub LAND_SKIP: bool = false, "VICTORY_LAND_SKIP",
    "Pieces an earlier fetch holds as they are now are left out", live = NextFrame);
tunable!(pub UNDER_THREADS: i64 = 6, 1..=16, "VICTORY_UNDER_THREADS",
    "Threads kept for the title's bytes under flights' writes", live = Restart);
tunable!(pub LAND_ANYTIME: bool = true, "VICTORY_LAND_ANYTIME",
    "Landing may happen at any time (off: only at set points)", live = Restart);
tunable!(pub LAND_TAKEN: bool = true, "VICTORY_LAND_TAKEN",
    "Blocks landed in are noted as up to date when nothing was written", live = NextFrame);
tunable!(pub AMBIENT_AIR: f64 = 0.0, 0.0..=1000000.0, "VICTORY_AMBIENT_AIR",
    "Most the air scatters, as a multiple of the light around (0: what the title has)", live = Safe);

tunable!(pub AMBIENT_MOST: f64 = 4.0, 0.0..=1000.0, "VICTORY_AMBIENT_MOST",
    "Times the estimate the title's own light may keep (4: as before; lower tames bright rooms facing a window)", live = Safe);
tunable!(pub AMBIENT_SETTLE: f64 = 0.0, 0.0..=60000.0, "VICTORY_AMBIENT_SETTLE",
    "Milliseconds the slow room-light mean takes to follow (0: by frame, as before; 250 steadies flicker)", live = Safe);
tunable!(pub AMBIENT_SETTLE_JUMP: f64 = 3.0, 0.0..=1000.0, "VICTORY_AMBIENT_SETTLE_JUMP",
    "A room-light change by more than this factor is taken at once (0: never); used with AMBIENT_SETTLE", live = Safe);
tunable!(pub AMBIENT_MARCH: f64 = 0.0, 0.0..=1000.0, "VICTORY_AMBIENT_MARCH",
    "Most the ray-marched fog in-scatter holds, as a multiple of the room light (0: what the title has; 1 to start)", live = Restart);
tunable!(pub SKIP_HAIR: bool = false, "VICTORY_SKIP_HAIR",
    "Hair strand and similar passes that do not translate are left out (off: they run)", live = NextFrame);
tunable!(pub VOXEL_VIS_COPIES: bool = false, "VICTORY_VOXEL_VIS_COPIES",
    "Voxel visibility shaders that park a wave64 mask in scalar copies are recovered", live = Restart);

tunable_list!(pub(super) fn register: EARLY_TEXTURES_MS, BAKE_TEXTURES_MS, BAKE_EXACT, RECHECK_SOON, WRITE_SKIP, INDIRECT_WINDOW, INDIRECT_BATCH, SCISSOR_TEST, HEAP_SPREAD, INDIRECT_WIDE, INDIRECT_REST, BVH_QUARANTINE, BVH_BUILDER, LIBRARY_MAP, LIBRARY_HANDLE, LIBRARY_TRIM, LIBRARY_LOCK, EARLY_ALL, RT_RESOLVE, PIPELINE_SLOW, PIPELINE_WAIT_MS, SCREEN_WAIT_MS, PIPELINE_WAIT_SHARED, PIPELINE_WORKERS, PIPELINE_LOWEST, LAND, LAND_THREADS, LAND_BESIDE, LAND_SKIP, UNDER_THREADS, LAND_ANYTIME, LAND_TAKEN, AMBIENT_AIR, AMBIENT_MOST, AMBIENT_SETTLE, AMBIENT_SETTLE_JUMP, AMBIENT_MARCH, SKIP_HAIR, VOXEL_VIS_COPIES);
