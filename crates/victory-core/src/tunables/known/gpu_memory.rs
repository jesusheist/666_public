//! gpu: write guards, shadow copies, wide preparing, submission, card memory, eviction.

use crate::{tunable, tunable_list};

// ---- gpu: watch (write guards)
tunable!(pub VIEW_GUARD: bool = true, "VICTORY_VIEW_GUARD",
    "Guard all the other views of a page together (off: each address for itself)", live = Safe);

// ---- gpu: shadow (wide preparing)
tunable!(pub WIDE_SPIN: i64 = 0, 0..=100000, "VICTORY_WIDE_SPIN",
    "Microseconds a wide-preparing thread waits awake before sleeping (0: sleeps at once)", live = Safe);
tunable!(pub WIDE_THREADS: i64 = 0, 0..=24, "VICTORY_WIDE_THREADS",
    "Threads kept for wide preparing (0: a quarter of the logical processors, 2 to 6)", live = Restart);

// ---- gpu: write guards (watch.rs)
tunable!(pub PAGE_PARTS: bool = true, "VICTORY_PAGE_PARTS",
    "Check only the pages the kernel says were written, not the 64 KiB around each", live = NextFrame);
tunable!(pub ASK_EARLY: f64 = 0.0, 0.0..=100000.0, "VICTORY_ASK_EARLY",
    "Milliseconds between early asking rounds of the written pages (0: off)", live = Safe);
tunable!(pub ASK_ONCE: bool = true, "VICTORY_ASK_ONCE",
    "Ask once for all the ranges of a submission (off: per range)", live = NextFrame);
tunable!(pub LIGHT_REFRESH: bool = true, "VICTORY_LIGHT_REFRESH",
    "Light refresh of view memory under the guards", live = NextFrame);
tunable!(pub GUARD_KNOWN_BITS: bool = false, "VICTORY_GUARD_KNOWN_BITS",
    "Keep a bit per block for blocks known absent (kept from the start)", live = Restart);
tunable!(pub KNOWN_HOLES: bool = true, "VICTORY_KNOWN_HOLES",
    "Remember the holes of placeholders instead of asking Windows each time", live = NextFrame);
tunable!(pub GUARD_ASK: bool = false, "VICTORY_GUARD_ASK",
    "Ask Windows about every range, even a known one", live = NextFrame);
tunable!(pub GUARD_HOLES: bool = true, "VICTORY_GUARD_HOLES",
    "Look past the first address of a range when asking if it is a view", live = NextFrame);
tunable!(pub PAGE_WRITES: enum = "normal", ["normal", "0", "1", "slots", "all"], "VICTORY_PAGE_WRITES",
    "Writes through the runtime own views: normal (default), 0, 1, slots, all", live = NextFrame);

// ---- gpu: shadow (card buffers and windows)
tunable!(pub CARD_BUFFERS: bool = true, "VICTORY_CARD_BUFFERS",
    "Title buffer copies go to the card memory", live = Restart);
tunable!(pub CARD_BUFFERS_MIN_MB: i64 = -1, -1..=1000000000, "VICTORY_CARD_BUFFERS_MIN_MB",
    "Smallest title buffer whose copy goes to the card, MB (-1: from the VRAM budget)", live = Restart);
tunable!(pub CARD_BUFFERS_MB: i64 = -1, -1..=1000000000, "VICTORY_CARD_BUFFERS_MB",
    "Most bytes of card buffer copies, MB (-1: from the VRAM budget)", live = Restart);
tunable!(pub CARD_BUFFERS_MIN_KB: i64 = 0, 0..=1000000000, "VICTORY_CARD_BUFFERS_MIN_KB",
    "Smallest title buffer whose copy goes to the card, KiB (0: not set)", live = Restart);
tunable!(pub CARD_ROOM_FOR_BUFFERS: bool = true, "VICTORY_CARD_ROOM_FOR_BUFFERS",
    "Keep room for card buffer copies in the texture queue", live = Restart);
tunable!(pub CARD_ALL: bool = false, "VICTORY_CARD_ALL",
    "Every title buffer copy goes to the card", live = Restart);
tunable!(pub CARD_WINDOWS_MB: i64 = 0, 0..=16777216, "VICTORY_CARD_WINDOWS_MB",
    "Most MB of windows whose copy is in the card memory (0: none)", live = Restart);
tunable!(pub WINDOWS_BAR_MB: i64 = 0, 0..=16777216, "VICTORY_WINDOWS_BAR_MB",
    "Most MB of windows in card memory the CPU maps (0: none)", live = Restart);
tunable!(pub WINDOWS_OWN: bool = false, "VICTORY_WINDOWS_OWN",
    "Windows take their own bytes again", live = Restart);

// ---- gpu: card memory, pool, in place, tiles, pop watch, copies
tunable!(pub CARD_ROOM_FOR_TARGETS_MB: i64 = 3072, 0..=8192, "VICTORY_CARD_ROOM_FOR_TARGETS_MB",
    "MB of the card kept for render targets in the texture queue (0: none)", live = Safe);
tunable!(pub CARD_STEPS: enum = "0", ["0", "1", "2"], "VICTORY_CARD_STEPS",
    "Steps done on the card instead of waiting for flights: 0, 1, 2", live = NextFrame);
tunable!(pub BOOK: bool = true, "VICTORY_BOOK",
    "Keep the book of writes (when device-removed tracking or slot checks are on)", live = Restart);
tunable!(pub SLOT_CHECK: bool = false, "VICTORY_SLOT_CHECK",
    "Check texture slots", live = NextFrame);
tunable!(pub COPY_MODE: enum = "normal", ["normal", "plain", "none"], "VICTORY_COPY_MODE",
    "How copies run: normal (guarded), plain, none", live = NextFrame);
tunable!(pub BARRIER_TEST: enum = "off", ["off", "groups", "indirect", "both"], "VICTORY_BARRIER_TEST",
    "Timing test without waits: off, groups, indirect, both", live = NextFrame);
tunable!(pub BVH_SIZES_WAIT: bool = false, "VICTORY_BVH_SIZES_WAIT",
    "Wait for the BVH sizes copy", live = Restart);
tunable!(pub BVH_TRACE: bool = false, "VICTORY_BVH_TRACE",
    "Trace the BVH builder", live = Restart);
tunable!(pub PICKED_WRITES: enum = "0", ["0", "1", "2"], "VICTORY_PICKED_WRITES",
    "Writes through views picked from the heap: 0 off, 1 declared, 2 landed too", live = NextFrame);
tunable!(pub PICKED_MB: i64 = 32, 0..=65536, "VICTORY_PICKED_MB",
    "MB looked at per frame for picked-view writes", live = Safe);
tunable!(pub TEXTURE_POOL: bool = true, "VICTORY_TEXTURE_POOL",
    "Textures made in blocks (a pool)", live = Restart);
tunable!(pub RIPE_BLOCKS: i64 = 12, 0..=1024, "VICTORY_RIPE_BLOCKS",
    "Blocks of textures made ahead (0: none)", live = Safe);
tunable!(pub INPLACE_PROBE: i64 = 0, 0..=255, "VICTORY_INPLACE_PROBE",
    "In-place probe level: 0 off, 1, 2", live = Restart);
tunable!(pub INPLACE: i64 = 0, 0..=255, "VICTORY_INPLACE",
    "Title memory in place: 0 off, 1 windows, 2 buffers too", live = Restart);
tunable!(pub INPLACE_MB: i64 = 4096, 0..=16777216, "VICTORY_INPLACE_MB",
    "Most MB of title memory in place at once", live = Restart);
tunable!(pub INPLACE_PROBE_MB: i64 = 64, 0..=1024, "VICTORY_INPLACE_PROBE_MB",
    "MB the in-place probe uses (capped at 1024)", live = Restart);
tunable!(pub THIN: bool = false, "VICTORY_THIN",
    "List calls written down and translated by threads of ours", live = Restart);
tunable!(pub THIN_THREADS: i64 = 8, 1..=32, "VICTORY_THIN_THREADS",
    "Threads for thin lists", live = Restart);
tunable!(pub POP_ALL: bool = false, "VICTORY_POP_ALL",
    "Pop watch keeps every picture", live = Restart);
tunable!(pub EVICT_MIN_FRAMES: i64 = 120, 0..=1000000, "VICTORY_EVICT_MIN_FRAMES",
    "Frames a host copy goes unused before it may be evicted", live = Safe);
tunable!(pub RESIDENCY_PRIORITY: bool = false, "VICTORY_RESIDENCY_PRIORITY",
    "Tell the system the residency priorities of host copies", live = Restart);
tunable!(pub LEVELS_ON_DEMAND: bool = false, "VICTORY_LEVELS_ON_DEMAND",
    "Host copies of title textures take card memory level by level", live = Restart);
tunable!(pub LEVELS_MIN_KB: i64 = 1024, 0..=1000000000000, "VICTORY_LEVELS_MIN_KB",
    "Smallest texture made level by level, KiB (at least 64)", live = Restart);
tunable!(pub DEPTH_TWINS: enum = "on", ["on", "0", "stencil", "nodepth"], "VICTORY_DEPTH_TWINS",
    "Depth twins: on (default), 0, stencil, nodepth", live = Restart);
tunable!(pub HELD_LEVELS: bool = true, "VICTORY_HELD_LEVELS",
    "Only held levels are uploaded again", live = Restart);
tunable!(pub ALIAS_COPY: enum = "all", ["all", "0", "1"], "VICTORY_ALIAS_COPY",
    "Aliased texture copies found: all (default), 0 none, 1 one level and layer only", live = NextFrame);
tunable!(pub LAZY_TEXTURES: bool = true, "VICTORY_LAZY_TEXTURES",
    "Host textures made when first used", live = Restart);
tunable!(pub PLACED_FILLER: bool = false, "VICTORY_PLACED_FILLER",
    "Filler words stay in the placed-texture key", live = Restart);
tunable!(pub TEXTURE_SWEEP: enum = "on", ["on", "0", "whole"], "VICTORY_TEXTURE_SWEEP",
    "Idle texture sweep: on (default), 0, whole", live = Safe);
tunable!(pub SKIP_WC_BACK: bool = false, "VICTORY_SKIP_WC_BACK",
    "What shaders write into card copies of title memory reserved write-combining is not fetched back (speed trade: stale bytes possible where the title's CPU writes part of a piece)", live = Restart);
tunable!(pub SKIP_WC_BACK_MIN: i64 = 1024, 0..=1000000000, "VICTORY_SKIP_WC_BACK_MIN",
    "With SKIP_WC_BACK: copies smaller than this, KiB, still come back (default 1024, the small ones hold what the title's CPU reads)", live = Restart);

tunable_list!(pub(super) fn register: VIEW_GUARD, WIDE_SPIN, WIDE_THREADS, PAGE_PARTS, ASK_EARLY, ASK_ONCE, LIGHT_REFRESH, GUARD_KNOWN_BITS, KNOWN_HOLES, GUARD_ASK, GUARD_HOLES, PAGE_WRITES, CARD_BUFFERS, CARD_BUFFERS_MIN_MB, CARD_BUFFERS_MB, CARD_BUFFERS_MIN_KB, CARD_ROOM_FOR_BUFFERS, CARD_ALL, CARD_WINDOWS_MB, WINDOWS_BAR_MB, WINDOWS_OWN, CARD_ROOM_FOR_TARGETS_MB, CARD_STEPS, BOOK, SLOT_CHECK, COPY_MODE, BARRIER_TEST, BVH_SIZES_WAIT, BVH_TRACE, PICKED_WRITES, PICKED_MB, TEXTURE_POOL, RIPE_BLOCKS, INPLACE_PROBE, INPLACE, INPLACE_MB, INPLACE_PROBE_MB, THIN, THIN_THREADS, POP_ALL, EVICT_MIN_FRAMES, RESIDENCY_PRIORITY, LEVELS_ON_DEMAND, LEVELS_MIN_KB, DEPTH_TWINS, HELD_LEVELS, ALIAS_COPY, LAZY_TEXTURES, PLACED_FILLER, TEXTURE_SWEEP, SKIP_WC_BACK, SKIP_WC_BACK_MIN);
