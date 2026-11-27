//! other tunables (see ../known.rs).

#[allow(unused_imports)]
use crate::{tunable, tunable_list};


// ---- loader: console instruction handling
tunable!(pub CLZERO_ASK: bool = false, "VICTORY_CLZERO_ASK",
    "Ask Windows about memory at every emulated instruction (off: answers are remembered per thread)", live = Restart);
tunable!(pub PATCH_WAITS: bool = true, "VICTORY_PATCH_WAITS",
    "Replace monitorx and mwaitx in the title's code the first time they fault", live = Restart);
tunable!(pub PATCH_CLZERO: bool = true, "VICTORY_PATCH_CLZERO",
    "Replace clzero loops by plain stores the first time they fault", live = Restart);

// ---- cheap ports from DoubleGhosty/cool_fox (all off by default)
tunable!(pub LIBRARY_STORE_LATER: bool = false, "VICTORY_LIBRARY_STORE_LATER",
    "A pipeline made while the library is saving is handed out at once and stored by the save", live = Safe);
tunable!(pub PIPELINE_COMPUTE_WAIT: bool = false, "VICTORY_PIPELINE_COMPUTE_WAIT",
    "A compute dispatch waits until its pipeline is made, whatever the frame's budget", live = Safe);
tunable!(pub READ_ZERO_GAPS: bool = false, "VICTORY_READ_ZERO_GAPS",
    "Title memory reads zero only the bytes no mapped run covers, not the whole buffer first", live = Safe);
tunable!(pub TEXTURE_ROW_RUNS: bool = false, "VICTORY_TEXTURE_ROW_RUNS",
    "Tiled texture mips are copied through shared run offsets per row (same bytes, fewer address lookups)", live = Restart);

// ---- prompts (from avatarmanz/VIctory, imjustprism's key caps)
tunable!(pub PROMPTS: enum = "gamepad", ["gamepad", "keys"], "VICTORY_PROMPTS",
    "keys: the title's button prompts show the keys of the [controls] layout in force (off: its gamepad pictures)", live = Restart);

tunable_list!(pub(super) fn register: CLZERO_ASK, PATCH_WAITS, PATCH_CLZERO, LIBRARY_STORE_LATER, PIPELINE_COMPUTE_WAIT, READ_ZERO_GAPS, TEXTURE_ROW_RUNS, PROMPTS);
