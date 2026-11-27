//! Switches, group 3 (see ../known.rs).

#[allow(unused_imports)]
use crate::{tunable, tunable_list};

// Draw.
tunable!(pub REUPLOAD_WHEN_FULL: bool = true, "VICTORY_REUPLOAD_WHEN_FULL",
    "A texture with a host copy is still brought up to date while the card has no room for new ones (off: not)", live = Restart);
tunable!(pub THREAD_DIMENSIONS: bool = true, "VICTORY_THREAD_DIMENSIONS",
    "Dispatch counts of the thread-dimensions kind are taken as threads (off: taken for groups, as before)", live = Restart);
tunable!(pub WRITE_SKIP: bool = true, "VICTORY_WRITE_SKIP",
    "Write ranges that did not change are not declared (off: every range)", live = Restart);
tunable!(pub UNWATCHED_FRAMES: i64 = 1, 0..=1000000, "VICTORY_UNWATCHED_FRAMES",
    "Frames between refreshes of copies the write watch does not cover (1: every frame)", live = Restart);
tunable!(pub SCISSOR_TEST: enum = "off", ["off", "full"], "VICTORY_SCISSOR_TEST",
    "Experiment: full = every scissor rectangle the title sets is widened to the whole target", live = Restart);

// Display and frame.
tunable!(pub DISPLAY_REFRESH: i64 = 0, 0..=240, "VICTORY_DISPLAY_REFRESH",
    "A second refresh rate in Hz (61 to 240) listed to the title besides 60 (0: none)", live = Restart);
tunable!(pub LATE_WAITS: bool = false, "VICTORY_LATE_WAITS",
    "A frame wait for an event that already happened sleeps to the next tick (off: ends at once)", live = Restart);

// Host.
tunable!(pub COPY_ASK: bool = false, "VICTORY_COPY_ASK",
    "Ask Windows whether a title destination is mapped before each buffer copy (off: only when the copy fails)", live = Restart);
tunable!(pub D3D12_BACKGROUND: bool = true, "VICTORY_D3D12_BACKGROUND",
    "The D3D12 runtime's background shader work (off: switched off)", live = Restart);
tunable!(pub D3D12_DEBUG: bool = false, "VICTORY_D3D12_DEBUG",
    "D3D12 debug layer (diagnostics builds)", live = Restart);
tunable!(pub D3D12_DRED: enum = "0", ["0", "1", "light", "faults"], "VICTORY_D3D12_DRED",
    "Device-removed data: 1 full, light breadcrumbs without texts, faults the faulting address only (diagnostics builds)", live = Restart);
tunable!(pub D3D12_GBV: bool = false, "VICTORY_D3D12_GBV",
    "GPU-based validation of the D3D12 debug layer (diagnostics builds, with the debug layer)", live = Restart);
tunable!(pub D3D12_WARP: bool = false, "VICTORY_D3D12_WARP",
    "Use the software adapter (diagnostics builds)", live = Restart);
tunable!(pub DECLARE_ONCE: bool = true, "VICTORY_DECLARE_ONCE",
    "A range shaders write is declared once until the list next writes title memory (off: declared every time)", live = Restart);
tunable!(pub DIRECT_WRITES: bool = true, "VICTORY_DIRECT_WRITES",
    "Title memory is written with the kernel's own call (off: WriteProcessMemory)", live = Restart);
tunable!(pub FILL_TITLE: bool = false, "VICTORY_FILL_TITLE",
    "Small constant fills a flight's shader writes are read as the title's when the title also wrote them", live = Restart);
tunable!(pub FRESH_OPEN: bool = true, "VICTORY_FRESH_OPEN",
    "A submitted list's new host list is left open for the title's next reset (off: closed at once and reset again)", live = Restart);
tunable!(pub GPU_TIMING: bool = false, "VICTORY_GPU_TIMING",
    "Time the card's work (also turns the late-wait stages off)", live = Restart);
tunable!(pub INDIRECT_CARD_BUDGET: bool = true, "VICTORY_INDIRECT_CARD_BUDGET",
    "A list's indirect-work budget lives in the card's own memory (off: in memory the CPU maps)", live = Restart);
tunable!(pub JOIN_LISTS: i64 = 48, 1..=1000000, "VICTORY_JOIN_LISTS",
    "Most lists joined into one call of a submission (needs the writes-ahead path)", live = Restart);
tunable!(pub KEEP_FREED: bool = true, "VICTORY_KEEP_FREED",
    "Keep freed memory for reuse for a while (off: given back at once)", live = Restart);
tunable!(pub LATE_WAIT_LISTS: i64 = 256, 1..=1000000, "VICTORY_LATE_WAIT_LISTS",
    "Most lists of a frame that may be held in flight at once by the late wait", live = Restart);
tunable!(pub LATE_WAIT_WRITES: bool = true, "VICTORY_LATE_WAIT_WRITES",
    "A submission with bottom-of-pipe writes may fly (off: settled as soon as it is sent)", live = Restart);
tunable!(pub LAZY_PRELUDE: bool = true, "VICTORY_LAZY_PRELUDE",
    "A host list's prelude is made when the list first needs one (off: made with the list)", live = Restart);
tunable!(pub PRESENT_TIMING: bool = false, "VICTORY_PRESENT_TIMING",
    "Log when the swap chain accepted a present", live = Restart);
tunable!(pub REFRESH_AHEAD: bool = true, "VICTORY_REFRESH_AHEAD",
    "Copies a submission's shaders write are brought up to date ahead of the writes (off: after; also needs wide refresh and the light watch)", live = Restart);
tunable!(pub REFRESH_FRONT: bool = false, "VICTORY_REFRESH_FRONT",
    "Everything a joined part refreshes is brought up to date in front of the first part", live = Restart);
tunable!(pub SCRATCH_POOL: bool = true, "VICTORY_SCRATCH_POOL",
    "Scratch chunks go back to a pool when a list is reset (off: kept by their list)", live = Restart);
tunable!(pub SEND_CHANGED: bool = false, "VICTORY_SEND_CHANGED",
    "Send up only the bytes the title changed, not the whole range", live = Restart);
tunable!(pub SWEEP: bool = true, "VICTORY_SWEEP",
    "The once-a-second sweep that lets go of what nothing names any more (off: keeps everything)", live = Restart);
tunable!(pub TEXTURE_KEEP: enum = "on", ["on", "0"], "VICTORY_TEXTURE_KEEP",
    "Keep textures while the card has room (0: the idle-time rule only)", live = Restart);
tunable!(pub TRAIL: bool = true, "VICTORY_TRAIL",
    "Keep the trail of recent submissions (off: nothing kept, nothing said)", live = Restart);
tunable!(pub WAIT_RESIDENT: bool = true, "VICTORY_WAIT_RESIDENT",
    "Lists wait for new card memory to be mapped (off: they do not)", live = Restart);
tunable!(pub WALLS_TRACE: enum = "0", ["0", "1", "2"], "VICTORY_WALLS_TRACE",
    "Walls trace: 1 a line for every failed read request, 2 for every request", live = Restart);
tunable!(pub WIDE_FILLS: bool = true, "VICTORY_WIDE_FILLS",
    "Fills of upload memory are done together on several threads (off: one after the other; needs wide refresh)", live = Restart);
tunable!(pub WIDE_MERGE: bool = false, "VICTORY_WIDE_MERGE",
    "Pieces the card found differing are merged into title memory on several threads", live = Restart);
tunable!(pub WIDE_TEXTURES: bool = true, "VICTORY_WIDE_TEXTURES",
    "Texture refreshes are done on several threads (off: on the submitting thread; needs wide refresh)", live = Restart);
tunable!(pub HEAP_PASS_ASIDE: bool = true, "VICTORY_HEAP_PASS_ASIDE",
    "Heap passes are found aside on a thread of their own (off: inline)", live = Restart);
tunable!(pub ORDERED_FOOTPRINT_FILLS: bool = false, "VICTORY_ORDERED_FOOTPRINT_FILLS",
    "Texture footprint fills go to the graphics queue in order (needs card steps 1 and the relay)", live = Restart);
tunable!(pub PLACED_ORDER: bool = false, "VICTORY_PLACED_ORDER",
    "Constant-buffer fills that read the title's ring keep their place in the list", live = Restart);
tunable!(pub COPIES_IN_PLACE: bool = true, "VICTORY_COPIES_IN_PLACE",
    "Title buffer copies are done on the card at their place in the list (off: all at submission, on the CPU)", live = Restart);
tunable!(pub SEND_BAR: bool = false, "VICTORY_SEND_BAR",
    "Copies to the bus go on the card's drawing queue", live = Restart);
tunable!(pub SEND_OWN: bool = true, "VICTORY_SEND_OWN",
    "Send up what a submission prepared on its own (off: as before)", live = Restart);
tunable!(pub UNDER_WORDS: bool = true, "VICTORY_UNDER_WORDS",
    "Send the words under a flight's shaders by word (off: by the bytes that changed; needs SEND_OWN)", live = Restart);
tunable!(pub CARD_SPANS: bool = false, "VICTORY_CARD_SPANS",
    "Measure and report the spans of the card's work", live = Restart);
tunable!(pub FETCH_CHECK: bool = false, "VICTORY_FETCH_CHECK",
    "Hash fetched pieces to catch ones torn by the title's writes", live = Restart);
tunable!(pub ASYNC_QUEUE: enum = "0", ["0", "plan"], "VICTORY_ASYNC_QUEUE",
    "plan: plan submissions for a compute queue beside the direct queue; everything still runs on the direct queue", live = Restart);
tunable!(pub CARD_ALL: bool = false, "VICTORY_CARD_ALL",
    "Every title buffer copy goes to the card", live = Restart);
tunable!(pub UPCOPY_CHECK: bool = false, "VICTORY_UPCOPY_CHECK",
    "Check that a later send never puts an older value over a newer one (costs memory and time)", live = Restart);
tunable!(pub INDIRECT_REST: bool = false, "VICTORY_INDIRECT_REST",
    "An oversized indirect record takes the rest of its list's budget", live = Restart);
tunable!(pub INDIRECT_WIDE: bool = true, "VICTORY_INDIRECT_WIDE",
    "Dispatch counts up to 1<<20 on AMD cards (off: capped at 65,535)", live = Restart);
tunable!(pub INDIRECT_MERGE: bool = false, "VICTORY_INDIRECT_MERGE",
    "Indirect executions that can join the one before them are joined", live = Restart);

tunable_list!(pub(super) fn register:
    REUPLOAD_WHEN_FULL, THREAD_DIMENSIONS, WRITE_SKIP, UNWATCHED_FRAMES, SCISSOR_TEST,
    DISPLAY_REFRESH, LATE_WAITS,
    COPY_ASK, D3D12_BACKGROUND, D3D12_DEBUG, D3D12_DRED, D3D12_GBV, D3D12_WARP, DECLARE_ONCE,
    DIRECT_WRITES, FILL_TITLE, FRESH_OPEN, GPU_TIMING, INDIRECT_CARD_BUDGET, JOIN_LISTS, KEEP_FREED,
    LATE_WAIT_LISTS, LATE_WAIT_WRITES, LAZY_PRELUDE, PRESENT_TIMING, REFRESH_AHEAD, REFRESH_FRONT,
    SCRATCH_POOL, SEND_CHANGED, SWEEP, TEXTURE_KEEP, TRAIL, WAIT_RESIDENT, WALLS_TRACE, WIDE_FILLS,
    WIDE_MERGE, WIDE_TEXTURES, HEAP_PASS_ASIDE, ORDERED_FOOTPRINT_FILLS, PLACED_ORDER,
    COPIES_IN_PLACE, SEND_BAR, SEND_OWN, UNDER_WORDS, CARD_SPANS, FETCH_CHECK, ASYNC_QUEUE,
    CARD_ALL, UPCOPY_CHECK, INDIRECT_REST, INDIRECT_WIDE, INDIRECT_MERGE
);
