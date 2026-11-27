//! Switches, group 6 (see ../known.rs).

#[allow(unused_imports)]
use crate::{tunable, tunable_list};

tunable!(pub GUARD_ASK: bool = false, "VICTORY_GUARD_ASK", "Ask the guards about every placeholder each time instead of remembering the ones already known", live = Restart);
tunable!(pub GUARD_HOLES: bool = true, "VICTORY_GUARD_HOLES", "Look at every address of a guarded block, not only the first one", live = Restart);
tunable!(pub GUARD_HOT: f64 = 0.0, 0.0..=1000000.0, "VICTORY_GUARD_HOT", "Milliseconds a block the title writes again and again is left open after a write fault (0 is off)", live = Restart);
tunable!(pub KNOWN_HOLES: bool = true, "VICTORY_KNOWN_HOLES", "Know placeholders for what they are, so filling one is not asked about every time", live = Restart);
tunable!(pub LIGHT_REFRESH: bool = true, "VICTORY_LIGHT_REFRESH", "Refresh only view memory that something wrote since the last frame", live = Restart);
tunable!(pub OPEN_ONE_VIEW: bool = true, "VICTORY_OPEN_ONE_VIEW", "Open only the one view of a guarded block that is needed, not all of them", live = Restart);
tunable!(pub PAGE_PARTS: bool = true, "VICTORY_PAGE_PARTS", "Copy only the parts of a page that were written, not the whole 64 KiB page", live = Restart);
tunable!(pub PAGE_WRITES: enum = "guards", ["guards", "0", "1", "slots", "all"], "VICTORY_PAGE_WRITES", "What to do with title memory writes the guards do not see: guards opens guarded blocks, 0 only counts, 1 also marks texture slots due, slots marks texture slots only, all does everything", live = Restart);
tunable!(pub TAKE_PARTS: bool = true, "VICTORY_TAKE_PARTS", "Take only the written parts of a page when copying, not the whole page", live = Restart);
tunable!(pub VIEW_GUARD: bool = true, "VICTORY_VIEW_GUARD", "Guard the other views of a page, not only the address that was written", live = Restart);
tunable!(pub KEYS_AS_PAD: bool = true, "VICTORY_KEYS_AS_PAD", "Let the keyboard and mouse act as a game pad", live = Restart);
tunable!(pub DSTORAGE_HALF: bool = false, "VICTORY_DSTORAGE_HALF", "A DirectStorage queue submits itself once half of it is enqueued, as the PC version does", live = Restart);
tunable!(pub DSTORAGE_IO: enum = "handles", ["handles", "one", "overlapped", "direct"], "VICTORY_DSTORAGE_IO", "How DirectStorage files are read: a handle per worker, one shared handle, one handle with overlapped reads, or a handle per worker past the file cache", live = Restart);
tunable!(pub DSTORAGE_RETRIES: i64 = 12, 0..=100000, "VICTORY_DSTORAGE_RETRIES", "How many times a DirectStorage request is tried again before it fails (12 is about 1.5 s)", live = Restart);
tunable!(pub DSTORAGE_STAGING_MB: i64 = 256, 0..=65536, "VICTORY_DSTORAGE_STAGING_MB", "Least DirectStorage staging buffer, in MiB, whatever size the title asks for", live = Restart);
tunable!(pub DSTORAGE_TRACE: i64 = 0, 0..=2, "VICTORY_DSTORAGE_TRACE", "Streaming trace: 0 off, 1 a line per batch and per odd request, 2 a line per request", live = Restart);
tunable!(pub ONE_PAD: bool = true, "VICTORY_ONE_PAD", "Show the title one pad made of the keyboard and every host pad", live = Restart);
tunable!(pub TITLE_PRIORITIES: bool = true, "VICTORY_TITLE_PRIORITIES", "Set thread priorities as the runtime chooses; off keeps them as the title sets them", live = Restart);
tunable!(pub MOUSE_SENSITIVITY: f64 = 0.0, 0.0..=100.0, "VICTORY_MOUSE_SENSITIVITY", "Mouse sensitivity for the keyboard and mouse controls (0 uses mouse_sensitivity of the controls file)", live = Safe);
tunable!(pub MOUSE_INVERT_Y: enum = "file", ["file", "off", "on"], "VICTORY_MOUSE_INVERT_Y", "Invert the mouse's up and down (file uses mouse_invert_y of the controls file)", live = Safe);
tunable!(pub VEHICLE_FLAG: enum = "off", ["off", "img+74dfca5"], "VICTORY_VEHICLE_FLAG", "Switch to the vehicle keys by itself from the title's own in-a-vehicle byte (img+74dfca5 is game 1.0.2.1; any <address|img+offset>[:width][:value] can be set in the environment; off falls back to VICTORY_AUTO_VEHICLE)", live = Restart);
tunable!(pub DRAG_FIX: bool = true, "VICTORY_DRAG_FIX", "Move and size the window by its title bar and borders without the system's modal loop, so the title keeps running while it is dragged", live = Safe);
tunable!(pub MOUSE_WARP: bool = false, "VICTORY_MOUSE_WARP", "Move the mouse pointer back to the window center after each mouse sample", live = Restart);
tunable!(pub LAZY_VIEWS: bool = false, "VICTORY_LAZY_VIEWS", "Map title pages on the host when first touched instead of at once", live = Restart);
tunable!(pub MAP_LEAN: bool = true, "VICTORY_MAP_LEAN", "Map only the pages that are committed", live = Restart);
tunable!(pub MAP_RUNS: bool = false, "VICTORY_MAP_RUNS", "Make one view for a run of pages instead of one view per page", live = Restart);
tunable!(pub SECTION_WINDOW: bool = true, "VICTORY_SECTION_WINDOW", "Use one view window of a section for writes instead of a view for each write", live = Restart);
tunable!(pub STREAM_NOTICE: bool = true, "VICTORY_STREAM_NOTICE", "Tell the GPU side where streamed bytes landed so it copies them again", live = Restart);
tunable!(pub STREAM_PRIORITY: bool = true, "VICTORY_STREAM_PRIORITY", "Raise a streaming thread that is below normal priority to above normal", live = Restart);
tunable!(pub EFFICIENCY_MODE: bool = false, "VICTORY_EFFICIENCY_MODE", "Leave the system its choice of efficiency mode for the process when its window is not in front", live = Restart);
tunable!(pub PROCESS_PRIORITY: enum = "above", ["above", "normal"], "VICTORY_PROCESS_PRIORITY", "Process priority: above runs above normal, normal leaves it as started", live = Restart);
tunable!(pub CLZERO_ASK: bool = false, "VICTORY_CLZERO_ASK", "Ask Windows about the region for every clzero instead of remembering the answer", live = Restart);
tunable!(pub PATCH_CLZERO: bool = true, "VICTORY_PATCH_CLZERO", "Patch clzero instructions in place instead of emulating each time", live = Restart);
tunable!(pub PATCH_WAITS: bool = true, "VICTORY_PATCH_WAITS", "Patch wait loops in place instead of emulating each time", live = Restart);
tunable!(pub WARP: bool = false, "VICTORY_WARP", "Draw shader tests on the WARP software adapter (host and test builds)", live = Restart);
tunable!(pub DESC_WHY: bool = false, "VICTORY_DESC_WHY", "Print a line for each descriptor decision made while translating shaders", live = Restart);
tunable!(pub WAVE_WHY: bool = false, "VICTORY_WAVE_WHY", "Print why a shader program keeps 64 lane waves, one line per reason", live = Restart);
tunable!(pub GROUP_SYNC: enum = "2", ["2", "1", "0"], "VICTORY_GROUP_SYNC", "Which group barriers wait for the whole group: 2 every barrier, 1 the title's barriers in safe blocks only, 0 none", live = Restart);
tunable!(pub RDNA_FLOAT: i64 = 15, 0..=4294967295, "VICTORY_RDNA_FLOAT", "On AMD RDNA hosts, which console float instructions use the host's own operation, as bits (15 is all, 0 none)", live = Restart);
tunable!(pub SHADER_LOOP_LIMIT: i64 = 1048576, 1..=4294967295, "VICTORY_SHADER_LOOP_LIMIT", "Iteration count after which a translated loop is left (diagnostics builds)", live = Restart);
tunable!(pub SHADER_OPT: i64 = 63, 0..=127, "VICTORY_SHADER_OPT", "Which shader clean-up passes run, as bits (63 is the default set, 0 none, 127 all)", live = Restart);

tunable_list!(pub(super) fn register: GUARD_ASK, GUARD_HOLES, GUARD_HOT, KNOWN_HOLES, LIGHT_REFRESH, OPEN_ONE_VIEW, PAGE_PARTS, PAGE_WRITES, TAKE_PARTS, VIEW_GUARD, KEYS_AS_PAD, DSTORAGE_HALF, DSTORAGE_IO, DSTORAGE_RETRIES, DSTORAGE_STAGING_MB, DSTORAGE_TRACE, ONE_PAD, MOUSE_SENSITIVITY, MOUSE_INVERT_Y, VEHICLE_FLAG, DRAG_FIX, TITLE_PRIORITIES, MOUSE_WARP, LAZY_VIEWS, MAP_LEAN, MAP_RUNS, SECTION_WINDOW, STREAM_NOTICE, STREAM_PRIORITY, EFFICIENCY_MODE, PROCESS_PRIORITY, CLZERO_ASK, PATCH_CLZERO, PATCH_WAITS, WARP, DESC_WHY, WAVE_WHY, GROUP_SYNC, RDNA_FLOAT, SHADER_LOOP_LIMIT, SHADER_OPT);
