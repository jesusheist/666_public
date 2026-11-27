//! Switches, group 5 (see ../known.rs).
//!
//! Not declared: `VICTORY_TRANSLATOR` (a build-time value), `VICTORY_WAVE_WIDE`
//! (unset means off at one read and on at another), `VICTORY_CARD_BUFFERS_MB`,
//! `VICTORY_CARD_BUFFERS_MIN_MB` and `VICTORY_CARD_WINDOWS_MB` (unset follows
//! the card's memory), and the free-text ones: `VICTORY_DLSS_JITTER`,
//! `VICTORY_DLSS_MV`, `VICTORY_STREAMLINE_LOG`, `VICTORY_TEXTURE_BACK`,
//! `VICTORY_BENCH_FILE`, `VICTORY_TIMELINE`, `VICTORY_WRITE_SITES`;
//! `VICTORY_TILES_TEST_CARD` is read only by tests.

#[allow(unused_imports)]
use crate::{tunable, tunable_list};

// Shader translation.
tunable!(pub BVH_FENCES: bool = true, "VICTORY_BVH_FENCES",
    "Buffer atomics of the ray-tracing structure builder stand between two fences (off: they do not)", live = Restart);
tunable!(pub FIRST_HALF: bool = true, "VICTORY_FIRST_HALF",
    "First halves of interpolations are evaluated as the pair they start (off: such programs are refused again)", live = Restart);
tunable!(pub HELPER_OWN: bool = true, "VICTORY_HELPER_OWN",
    "A pixel shader's helper lane reads its own value where the console reads the first lane's (off: as before)", live = Restart);
tunable!(pub LDS_INIT_HOIST: bool = true, "VICTORY_LDS_INIT_HOIST",
    "Shared-memory reduction identities are set up while every host lane is still active (off: left where they are)", live = Restart);
tunable!(pub THREAD_TICKET: bool = true, "VICTORY_THREAD_TICKET",
    "Console lanes in groups of several host waves take their thread numbers from a group counter (off: from the thread id)", live = Restart);
tunable!(pub WAVE_DEEP_SKIPS: bool = true, "VICTORY_WAVE_DEEP_SKIPS",
    "A wave program stopped by a wave operation is tried again with the skips the code after them cannot see left out (off: not tried)", live = Restart);
tunable!(pub WAVE_LOCAL: bool = true, "VICTORY_WAVE_LOCAL",
    "A program still stopped where not every lane runs is taken with each host wave as a wave of its own (off: refused)", live = Restart);
tunable!(pub WAVE_OWN_BITS: bool = true, "VICTORY_WAVE_OWN_BITS",
    "A wave OR where not every lane runs is taken with each host wave's own bits (off: refused as before)", live = Restart);
tunable!(pub F16_NEAREST: bool = false, "VICTORY_F16_NEAREST",
    "Results of 16-bit float operations are rounded to nearest in translated shaders", live = Restart);
tunable!(pub FILTER_3D: bool = false, "VICTORY_FILTER_3D",
    "Volume (3D) textures are filtered by the sampler in translated shaders, for checking what fails", live = Restart);
tunable!(pub FIRSTLANE_OWN: bool = false, "VICTORY_FIRSTLANE_OWN",
    "A first-lane read takes the host wave's own first lane", live = Restart);
tunable!(pub MESH_OUTPUT_STRICT: bool = false, "VICTORY_MESH_OUTPUT_STRICT",
    "Mesh shader outputs past the cap are refused (off: emitted with the lower maxima)", live = Restart);
tunable!(pub STRICT_JOINS: bool = false, "VICTORY_STRICT_JOINS",
    "A join that only another join reads is read wherever that one is, so a value with no form is not left silently zero", live = Restart);

// Shadow copies of title memory.
tunable!(pub CARD_BUFFERS: bool = true, "VICTORY_CARD_BUFFERS",
    "Title buffer copies go to the card memory", live = Restart);
tunable!(pub CARD_HASH_LEN: bool = true, "VICTORY_CARD_HASH_LEN",
    "A card buffer is hashed only over the title buffer's own length (off: the whole buffer)", live = Restart);
tunable!(pub CARD_ROOM_FOR_BUFFERS: bool = true, "VICTORY_CARD_ROOM_FOR_BUFFERS",
    "Keep room for card buffer copies when texture copies take card memory", live = Restart);
tunable!(pub EXACT_BACK: bool = true, "VICTORY_EXACT_BACK",
    "What shaders wrote goes into title memory run by run (off: the whole stretch from the first changed byte to the last)", live = Restart);
tunable!(pub LAND_ECHO: bool = true, "VICTORY_LAND_ECHO",
    "Pages a landing was noted in are compared 4 KiB at a time, so pages that did not change are not sent again", live = Restart);
tunable!(pub QUICK_FIND: bool = true, "VICTORY_QUICK_FIND",
    "The last answers of the buffer lookup are kept by a hash of the ask (off: not kept)", live = Restart);
tunable!(pub UNDER_WIDE: bool = true, "VICTORY_UNDER_WIDE",
    "Pages of the under-write merge are compared on several threads (off: on the submitting one)", live = Restart);
tunable!(pub KEEP_SNAPSHOTS: bool = false, "VICTORY_KEEP_SNAPSHOTS",
    "Snapshots of copies are kept in the PC's memory between uses", live = Restart);
tunable!(pub REFRESH_CHECK: bool = false, "VICTORY_REFRESH_CHECK",
    "Host copies are checked for stale bytes after their refresh (diagnostics, about 100 ms a frame)", live = Restart);
tunable!(pub WC_BAR: bool = false, "VICTORY_WC_BAR",
    "A title buffer in write-combined memory gets one copy in card memory the CPU maps (experiment)", live = Restart);
tunable!(pub WINDOWS_OWN: bool = false, "VICTORY_WINDOWS_OWN",
    "Windows take their own bytes again", live = Restart);
tunable!(pub WINDOW_TAILS: bool = false, "VICTORY_WINDOW_TAILS",
    "The window of an ask without a length begins at the ask instead of at the 4 MiB line below it (experiment)", live = Restart);
tunable!(pub KEEP_SNAPSHOTS_MB: i64 = 256, 0..=1000000, "VICTORY_KEEP_SNAPSHOTS_MB",
    "Most MB of snapshots kept in the PC's memory", live = Restart);
tunable!(pub MEMORY_CARD_MB: i64 = 4096, 0..=1000000, "VICTORY_MEMORY_CARD_MB",
    "Most MB of memory copies on the card's own memory", live = Restart);
tunable!(pub SNAPSHOT_POOL: i64 = 1024, 0..=1000000, "VICTORY_SNAPSHOT_POOL",
    "How many snapshot buffers are kept", live = Restart);
tunable!(pub WIDE_SPIN: i64 = 0, 0..=100000, "VICTORY_WIDE_SPIN",
    "Microseconds a thread waits awake before sleeping (0: sleeps at once)", live = Restart);
tunable!(pub WINDOWS_BAR_MB: i64 = 0, 0..=16777216, "VICTORY_WINDOWS_BAR_MB",
    "Most MB of windows in card memory the CPU maps (0: none)", live = Restart);
tunable!(pub WINDOW_KB: enum = "4096", ["4096", "2048", "1024", "512", "256", "128", "64"], "VICTORY_WINDOW_KB",
    "Size in KiB of the grid windows of title memory are cut on", live = Restart);

// Streamline (DLSS and frame generation).
tunable!(pub CHECK_TEARDOWN: bool = true, "VICTORY_CHECK_TEARDOWN",
    "Streamline is shut down properly before the process ends (off: left out)", live = Restart);
tunable!(pub DLSS_REACTIVE: bool = true, "VICTORY_DLSS_REACTIVE",
    "The reactive mask is given to DLSS", live = Restart);
tunable!(pub FRAMEGEN_HUDLESS: bool = true, "VICTORY_FRAMEGEN_HUDLESS",
    "With frame generation, the colour without the interface is copied and tagged (off: not)", live = Restart);
tunable!(pub STREAMLINE_PRESENT: bool = false, "VICTORY_STREAMLINE_PRESENT",
    "With DLSS and no frame generation, the swap chain is Streamline's proxy so it sees the presents", live = Restart);
tunable!(pub FRAMEGEN_CYCLE: i64 = 0, 0..=1000000, "VICTORY_FRAMEGEN_CYCLE",
    "Check run: the frame generation amount is asked anew every this many presents (0: off)", live = Restart);

// Textures.
tunable!(pub COMMITTED_QUERY: bool = false, "VICTORY_COMMITTED_QUERY",
    "Texture memory is checked as committed by a kernel query", live = Restart);
tunable!(pub EXACT_NO_WAIT: bool = true, "VICTORY_EXACT_NO_WAIT",
    "A texture read through the kernel waits for remapping only if a page is not there (off: always waits)", live = Restart);
tunable!(pub MAPPED_BY_PAGE: bool = false, "VICTORY_MAPPED_BY_PAGE",
    "Texture memory is checked as mapped by whole pages", live = Restart);
tunable!(pub MAPPED_BY_VIEWS: bool = true, "VICTORY_MAPPED_BY_VIEWS",
    "Streamed textures are checked as mapped by the views of physical pages (off: ask the host)", live = Restart);
tunable!(pub STAGED_ASK_ALL: bool = false, "VICTORY_STAGED_ASK_ALL",
    "Windows is asked about every staged page range instead of using the views of physical pages", live = Restart);
tunable!(pub TEXTURE_COPY: bool = false, "VICTORY_TEXTURE_COPY",
    "Texture bytes are read through a copy of title memory (off: read in place)", live = Restart);
tunable!(pub TEXTURE_FILL: bool = true, "VICTORY_TEXTURE_FILL",
    "A staging buffer that holds nothing yet is filled with what is there (off: left empty)", live = Restart);
tunable!(pub TEXTURE_GUARD: bool = true, "VICTORY_TEXTURE_GUARD",
    "The write guard tells a texture is unchanged without hashing (off: hash at every check)", live = Restart);
tunable!(pub TEXTURE_HOT: bool = true, "VICTORY_TEXTURE_HOT",
    "A texture watched by the write guard is looked at every frame (off: less often)", live = Restart);
tunable!(pub TEXTURE_ONCE: bool = true, "VICTORY_TEXTURE_ONCE",
    "A texture's staging buffer goes with its upload when nothing asks for a refresh (off: kept)", live = Restart);
tunable!(pub TEXTURE_SAME: bool = true, "VICTORY_TEXTURE_SAME",
    "A texture uploaded again with the same bytes is not uploaded", live = Restart);
tunable!(pub LEVELS_MIN_KB: i64 = 1024, 0..=1000000000, "VICTORY_LEVELS_MIN_KB",
    "Smallest texture made level by level, in KiB (at least 64)", live = Restart);

// Timeline recording (diagnostics).
tunable!(pub TIMELINE_AT: f64 = 60.0, 0.0..=1000000.0, "VICTORY_TIMELINE_AT",
    "Seconds into the run at which the timeline recording starts (only with VICTORY_TIMELINE set)", live = Restart);
tunable!(pub TIMELINE_FRAMES: i64 = 3, 1..=60, "VICTORY_TIMELINE_FRAMES",
    "Frames the timeline recording covers (only with VICTORY_TIMELINE set)", live = Restart);

// Title writes and the page watch.
tunable!(pub COPY_BESIDE: bool = false, "VICTORY_COPY_BESIDE",
    "The title's memory copies are done beside the guards (diagnostics builds can also switch it while running)", live = Restart);
tunable!(pub STORE_BESIDE: bool = false, "VICTORY_STORE_BESIDE",
    "The title's stores are done beside the guards", live = Restart);
tunable!(pub SAME_RECORDS: bool = true, "VICTORY_SAME_RECORDS",
    "A record the title copies is not written when what is there is the same (off: always written)", live = Restart);
tunable!(pub UPLOAD_LOW: bool = true, "VICTORY_UPLOAD_LOW",
    "Upload memory is taken in 64 KiB blocks from arenas at low addresses (off: from the process heap)", live = Restart);
tunable!(pub ASK_EARLY: f64 = 0.0, 0.0..=100000.0, "VICTORY_ASK_EARLY",
    "Milliseconds between early asking rounds of the written pages (0: off)", live = Restart);
tunable!(pub ASK_ONCE: bool = true, "VICTORY_ASK_ONCE",
    "Ask once for all the ranges of a submission (off: per range)", live = Restart);
tunable!(pub FAULT_UNLOCKED: bool = false, "VICTORY_FAULT_UNLOCKED",
    "A write fault opens its block without the guards' lock", live = Restart);

tunable_list!(pub(super) fn register:
    BVH_FENCES, FIRST_HALF, HELPER_OWN, LDS_INIT_HOIST, THREAD_TICKET, WAVE_DEEP_SKIPS, WAVE_LOCAL,
    WAVE_OWN_BITS, F16_NEAREST, FILTER_3D, FIRSTLANE_OWN, MESH_OUTPUT_STRICT, STRICT_JOINS,
    CARD_BUFFERS, CARD_HASH_LEN, CARD_ROOM_FOR_BUFFERS, EXACT_BACK, LAND_ECHO, QUICK_FIND, UNDER_WIDE,
    KEEP_SNAPSHOTS, REFRESH_CHECK, WC_BAR, WINDOWS_OWN, WINDOW_TAILS, KEEP_SNAPSHOTS_MB, MEMORY_CARD_MB,
    SNAPSHOT_POOL, WIDE_SPIN, WINDOWS_BAR_MB, WINDOW_KB,
    CHECK_TEARDOWN, DLSS_REACTIVE, FRAMEGEN_HUDLESS, STREAMLINE_PRESENT, FRAMEGEN_CYCLE,
    COMMITTED_QUERY, EXACT_NO_WAIT, MAPPED_BY_PAGE, MAPPED_BY_VIEWS, STAGED_ASK_ALL, TEXTURE_COPY,
    TEXTURE_FILL, TEXTURE_GUARD, TEXTURE_HOT, TEXTURE_ONCE, TEXTURE_SAME, LEVELS_MIN_KB,
    TIMELINE_AT, TIMELINE_FRAMES, COPY_BESIDE, STORE_BESIDE, SAME_RECORDS, UPLOAD_LOW, ASK_EARLY,
    ASK_ONCE, FAULT_UNLOCKED
);
