//! hle: audio, input/controls and the other console-library stand-ins.

use crate::{tunable, tunable_list};

// ---- hle: audio
tunable!(pub AUDIO_BOOST: bool = false, "VICTORY_AUDIO_BOOST",
    "XMA decoding thread at time-critical priority (MMCSS Pro Audio), with a counter line every 5 s", live = Restart);


// ---- hle: runtime, dstorage, xmem, kernel, input
tunable!(pub DRAG_FIX: bool = true, "VICTORY_DRAG_FIX",
    "Window drag and resize by the title bar keeps the game window under the cursor", live = Safe);
tunable!(pub DSTORAGE_HALF: bool = false, "VICTORY_DSTORAGE_HALF",
    "DirectStorage: submit half-filled batches as they come (diagnosis)", live = Restart);
tunable!(pub DSTORAGE_IO: enum = "handles", ["one", "handles", "overlapped", "direct"], "VICTORY_DSTORAGE_IO",
    "DirectStorage: how files are read (one handle, a handle per worker, overlapped, direct)", live = Restart);
tunable!(pub DSTORAGE_RETRIES: i64 = 12, 0..=1000000, "VICTORY_DSTORAGE_RETRIES",
    "DirectStorage: tries again for a destination's physical pages before a request fails", live = Safe);
tunable!(pub DSTORAGE_STAGING_MB: i64 = 256, 0..=1000000, "VICTORY_DSTORAGE_STAGING_MB",
    "DirectStorage: staging buffer floor in MB", live = Restart);
tunable!(pub DSTORAGE_TRACE: i64 = 0, 0..=255, "VICTORY_DSTORAGE_TRACE",
    "DirectStorage streaming trace: 0 off, 1 odd batches and requests, 2 every request", live = Safe);
tunable!(pub WALLS_TRACE: i64 = 0, 0..=255, "VICTORY_WALLS_TRACE",
    "Walls trace: 0 off, 1 failed read requests, 2 every request", live = Safe);
tunable!(pub EFFICIENCY_MODE: bool = false, "VICTORY_EFFICIENCY_MODE",
    "Leave Windows its efficiency mode choice for the process (default: execution speed throttling off)", live = Restart);
tunable!(pub MAP_LEAN: bool = true, "VICTORY_MAP_LEAN",
    "Lean page mapping calls for the title's streaming", live = Restart);
tunable!(pub SECTION_WINDOW: bool = true, "VICTORY_SECTION_WINDOW",
    "One kept view of a memory section instead of a view for each write", live = Restart);
tunable!(pub STREAM_NOTICE: bool = true, "VICTORY_STREAM_NOTICE",
    "Tell the GPU side where DirectStorage wrote title pages, so it copies them again", live = Safe);
tunable!(pub STREAM_PRIORITY: bool = true, "VICTORY_STREAM_PRIORITY",
    "Lift the streaming thread from below normal priority to above normal", live = Restart);
tunable!(pub TITLE_PRIORITIES: bool = true, "VICTORY_TITLE_PRIORITIES",
    "Remap the thread priorities the title sets (off: passed as set)", live = Restart);
tunable!(pub ONE_PAD: bool = true, "VICTORY_ONE_PAD",
    "The title sees one always-present pad fed by keyboard and every host pad", live = Restart);

tunable_list!(pub(super) fn register: AUDIO_BOOST, DRAG_FIX, DSTORAGE_HALF, DSTORAGE_IO, DSTORAGE_RETRIES, DSTORAGE_STAGING_MB, DSTORAGE_TRACE, WALLS_TRACE, EFFICIENCY_MODE, MAP_LEAN, SECTION_WINDOW, STREAM_NOTICE, STREAM_PRIORITY, TITLE_PRIORITIES, ONE_PAD);
