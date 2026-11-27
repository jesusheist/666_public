//! Switches, group 1 (see ../known.rs).

use crate::{tunable, tunable_list};

tunable!(pub CARD_AWAKE: enum = "0", ["0", "1", "turns"], "VICTORY_CARD_AWAKE",
    "Keep the card's clocks up between submissions (1), or on and off in turns of 20 s to compare (turns).", live = Restart);
tunable!(pub CARD_REHASH_RUNS: bool = false, "VICTORY_CARD_REHASH_RUNS",
    "Send the changed ranges of a card copy up as joined runs instead of one by one.", live = Restart);
tunable!(pub DLSS: bool = false, "VICTORY_DLSS",
    "Let DLSS take the title's upscale dispatches (needs Streamline).", live = Restart);
tunable!(pub FRAMEGEN: bool = false, "VICTORY_FRAMEGEN",
    "Turn on DLSS Frame Generation on the present (needs Streamline and the line of submissions).", live = Restart);
tunable!(pub FRAMEGEN_X: i64 = 2, 0..=6, "VICTORY_FRAMEGEN_X",
    "Frames shown for each one rendered with frame generation (2 to 6); 0 is ready but off until the menu asks.", live = Restart);
tunable!(pub FRAME_INTERVAL_US: i64 = 0, 0..=1000000, "VICTORY_FRAME_INTERVAL_US",
    "Begin frames every this many microseconds (1000 to 1000000); 0 leaves the title's own interval.", live = Restart);
tunable!(pub HEAP_SYNC_AT_SUBMIT: bool = false, "VICTORY_HEAP_SYNC_AT_SUBMIT",
    "Ask for the heaps' records at the title's submit instead of later.", live = Restart);
tunable!(pub HEAP_SYNC_FIRST: bool = false, "VICTORY_HEAP_SYNC_FIRST",
    "Begin the heaps' records as a submission takes the lock, before the flights are looked at.", live = Restart);
tunable!(pub INDIRECT_WINDOW: bool = false, "VICTORY_INDIRECT_WINDOW",
    "Clamp the arguments of indirect draws a window at a time.", live = Restart);
tunable!(pub JOIN_COPIES: bool = false, "VICTORY_JOIN_COPIES",
    "Make a copy in a list one step together with the copies after it.", live = Restart);
tunable!(pub JOIN_PARTS: i64 = 1, 1..=1000, "VICTORY_JOIN_PARTS",
    "Parts of submissions the line's thread gives to the host as one; 1 joins none.", live = Restart);
tunable!(pub LEVELS_ON_DEMAND: bool = false, "VICTORY_LEVELS_ON_DEMAND",
    "Host copies of title textures take the card's memory level by level, on demand.", live = Restart);
tunable!(pub MAP_THREADS: enum = "8", ["8", "auto", "1", "2", "3", "4", "5", "6", "7", "9", "10", "12", "14", "16", "20", "24", "28", "32"], "VICTORY_MAP_THREADS",
    "Threads that map a group of title pages side by side (1 to 32); 1 is the caller alone, auto goes by how much there is to map.", live = Restart);
tunable!(pub NATIVE_1080P: bool = false, "VICTORY_NATIVE_1080P",
    "Report a 1080p display to the title (needs the Series X profile).", live = Restart);
tunable!(pub NATIVE_1440P: bool = false, "VICTORY_NATIVE_1440P",
    "Report a 1440p display to the title (needs the Series X profile).", live = Restart);
tunable!(pub PIPELINE_BOOK: bool = false, "VICTORY_PIPELINE_BOOK",
    "Keep the pipelines the title creates, so the host pipelines can be made ahead of a later run.", live = Restart);
tunable!(pub POOL_SMALL: bool = true, "VICTORY_POOL_SMALL",
    "Cut small textures from a shared block where the host allows it; off gives each its own allocation.", live = Restart);
tunable!(pub PRELUDE_UPLOADS: bool = false, "VICTORY_PRELUDE_UPLOADS",
    "Decide whether to upload a texture when the list runs: upload it if it changed or was never uploaded.", live = Restart);
tunable!(pub QUEUE_HOST_TEXTURES: bool = true, "VICTORY_QUEUE_HOST_TEXTURES",
    "Keep host texture slots in the queue until their upload is done; off lets the slot leave the queue at once.", live = Restart);
tunable!(pub RELAY_COPY: bool = false, "VICTORY_RELAY_COPY",
    "Make the relay copy title buffers (used with the relay modes and the fetch checks).", live = Restart);
tunable!(pub RENDER_1080P: bool = false, "VICTORY_RENDER_1080P",
    "Render the scene at 1080p inside the 1440p output (left out when a DLSS mode sets the scene size).", live = Restart);
tunable!(pub RIPE_BLOCKS: i64 = 12, 0..=1000, "VICTORY_RIPE_BLOCKS",
    "Blocks of textures made ahead so one has settled before its first piece is taken; 0 uses new blocks at once.", live = Restart);
tunable!(pub SHOW_AFTER: bool = false, "VICTORY_SHOW_AFTER",
    "Show a slot only after the submission with its texture upload has finished.", live = Restart);
tunable!(pub SHOW_NEW_LATER: bool = false, "VICTORY_SHOW_NEW_LATER",
    "Show a new texture slot later instead of at once (also an A/B switch in diagnostics builds).", live = Restart);
tunable!(pub SPANS_QUICK: bool = false, "VICTORY_SPANS_QUICK",
    "Take the quick path for view slot writes that are not in a span (also an A/B switch in diagnostics builds).", live = Restart);
tunable!(pub SYNC_FINE: bool = false, "VICTORY_SYNC_FINE",
    "Give each thread smaller jobs of pages and records when syncing the heaps (also an A/B switch in diagnostics builds).", live = Restart);
tunable!(pub SYNC_IN_PLACE: bool = false, "VICTORY_SYNC_IN_PLACE",
    "Read the heaps' records where they are instead of into a copy of the page (also an A/B switch in diagnostics builds).", live = Restart);
tunable!(pub TABLED_REREAD: bool = false, "VICTORY_TABLED_REREAD",
    "Read the table of a texture slot again after the first read of the frame, not once a frame.", live = Restart);
tunable!(pub TEXTURE_CATCHUP: bool = true, "VICTORY_TEXTURE_CATCHUP",
    "Give a texture backlog three times the bytes and time per frame until it drains; off keeps the plain budget.", live = Restart);
tunable!(pub TEXTURE_POOL: bool = true, "VICTORY_TEXTURE_POOL",
    "Make textures from shared blocks of memory; off makes every one an allocation of its own.", live = Restart);
tunable!(pub TEXTURE_STAGE_MS: i64 = 250, 0..=100000, "VICTORY_TEXTURE_STAGE_MS",
    "Milliseconds after which a frame's texture making and staging pass stops and leaves the rest for the next frames.", live = Restart);
tunable!(pub THIN: bool = false, "VICTORY_THIN",
    "Only write down list calls and hand them to translating threads (turns on the line of submissions).", live = Restart);
tunable!(pub UPLOAD_AHEAD: bool = false, "VICTORY_UPLOAD_AHEAD",
    "Read, detile and copy texture uploads ahead on threads of ours (also an A/B switch in diagnostics builds).", live = Restart);
tunable!(pub WC_NO_BACK_MIN_KB: i64 = 0, 0..=100000000, "VICTORY_WC_NO_BACK_MIN_KB",
    "Smallest copy, in KB, whose shader writes are left on the card when write-combined copies are not read back; 0 is all.", live = Restart);
tunable!(pub WIDE_BACK: bool = true, "VICTORY_WIDE_BACK",
    "Snapshot and compare a submission's shader-written ranges on several threads; off uses one.", live = Restart);
tunable!(pub AUTO_VEHICLE: bool = false, "VICTORY_AUTO_VEHICLE",
    "Use the automatic vehicle control layout.", live = Restart);
tunable!(pub CHEATS: bool = false, "VICTORY_CHEATS",
    "Allow the title's built-in cheats to be queued.", live = Restart);
tunable!(pub FSR_PATH: bool = false, "VICTORY_FSR_PATH",
    "Use the title's own FidelityFX path at the scale of the DLSS mode.", live = Restart);
tunable!(pub PROMPTS: enum = "off", ["off", "keys"], "VICTORY_PROMPTS",
    "Show keyboard key prompts instead of pad prompts (keys).", live = Restart);
tunable!(pub SKIP_CPU_CHECK: bool = false, "VICTORY_SKIP_CPU_CHECK",
    "Skip the AVX2 check at start (for CPU emulation such as Intel SDE).", live = Restart);
tunable!(pub STORY_CLOCK: bool = false, "VICTORY_STORY_CLOCK",
    "Plug in the story clock so the title's clock cheats can be used.", live = Restart);
tunable!(pub TIMER_LOG: bool = false, "VICTORY_TIMER_LOG",
    "Log how even the frames are and the title's timers once a second.", live = Restart);
tunable!(pub VIEW_LIMIT: i64 = 0, 0..=4294967295, "VICTORY_VIEW_LIMIT",
    "Translate shaders as for a host whose view heap ends at this null slot (translate commands only); 0 is no limit.", live = Restart);
tunable!(pub HOST_WAVE: i64 = 0, 0..=128, "VICTORY_HOST_WAVE",
    "Translate shaders as for a GPU whose waves are at most this many lanes (translate commands only); 0 is not known.", live = Restart);

tunable_list!(pub(super) fn register: CARD_AWAKE, CARD_REHASH_RUNS, DLSS, FRAMEGEN, FRAMEGEN_X, FRAME_INTERVAL_US, HEAP_SYNC_AT_SUBMIT, HEAP_SYNC_FIRST, INDIRECT_WINDOW, JOIN_COPIES, JOIN_PARTS, LEVELS_ON_DEMAND, MAP_THREADS, NATIVE_1080P, NATIVE_1440P, PIPELINE_BOOK, POOL_SMALL, PRELUDE_UPLOADS, QUEUE_HOST_TEXTURES, RELAY_COPY, RENDER_1080P, RIPE_BLOCKS, SHOW_AFTER, SHOW_NEW_LATER, SPANS_QUICK, SYNC_FINE, SYNC_IN_PLACE, TABLED_REREAD, TEXTURE_CATCHUP, TEXTURE_POOL, TEXTURE_STAGE_MS, THIN, UPLOAD_AHEAD, WC_NO_BACK_MIN_KB, WIDE_BACK, AUTO_VEHICLE, CHEATS, FSR_PATH, PROMPTS, SKIP_CPU_CHECK, STORY_CLOCK, TIMER_LOG, VIEW_LIMIT, HOST_WAVE);
