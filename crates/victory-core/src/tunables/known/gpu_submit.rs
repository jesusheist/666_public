//! gpu_submit tunables (see ../known.rs): submission, queues, lists, the host's
//! per-frame upkeep.

#[allow(unused_imports)]
use crate::{tunable, tunable_list};

tunable!(pub QUEUE_WAIT: bool = true, "VICTORY_QUEUE_WAIT",
    "Queues wait for fences (off: every submission runs when it is made)", live = NextFrame);
tunable!(pub SPLIT_WAITS: bool = true, "VICTORY_SPLIT_WAITS",
    "Hold a submission list by list at its waits (off: whole submissions)", live = NextFrame);
tunable!(pub CUT_WAITS: bool = true, "VICTORY_CUT_WAITS",
    "Cut lists into host lists at their waits for memory values (off: whole lists)", live = NextFrame);
tunable!(pub WAIT_FORCE: bool = true, "VICTORY_WAIT_FORCE",
    "A present runs what queues still hold (off: waits are kept across presents)", live = NextFrame);
tunable!(pub GPU_LOAD_MS: i64 = 0, 0..=1000000, "VICTORY_GPU_LOAD_MS",
    "What the title's GPU time is made to look like, in ms (0: as it is)", live = Safe);
tunable!(pub LIST_POOL: bool = true, "VICTORY_LIST_POOL",
    "Host lists that have run are used again (off: each is made new)", live = Restart);
tunable!(pub READ_BACK: bool = true, "VICTORY_READ_BACK",
    "Texture copies out to buffers are made (off: left unmade)", live = NextFrame);
tunable!(pub WAIT_ASK: bool = false, "VICTORY_WAIT_ASK",
    "A wait for a memory value asks whether its page is committed first", live = NextFrame);
tunable!(pub CUT_WRITES: bool = false, "VICTORY_CUT_WRITES",
    "Lists are cut at value writes too", live = NextFrame);
tunable!(pub COPY_OVER_TITLE: enum = "other", ["0", "title", "other"], "VICTORY_COPY_OVER_TITLE",
    "First copy into a title texture: 0 = as before, title = title memory, other = zeros unless used", live = Safe);
tunable!(pub SEND_OWN: bool = true, "VICTORY_SEND_OWN",
    "Send up what a submission prepared on its own (off: as before)", live = NextFrame);
tunable!(pub UNDER_WORDS: bool = true, "VICTORY_UNDER_WORDS",
    "Send the words under a flight's shaders by word (off: by the bytes that changed)", live = Safe);
tunable!(pub SEND_BAR: bool = false, "VICTORY_SEND_BAR",
    "Copies to the bus on the card's drawing queue", live = Restart);
tunable!(pub TEXTURE_KEEP: bool = true, "VICTORY_TEXTURE_KEEP",
    "Keep textures while the card has room (off: 5 s idle rule)", live = Safe);
tunable!(pub WAIT_RESIDENT: bool = true, "VICTORY_WAIT_RESIDENT",
    "Wait for new card memory to be mapped", live = NextFrame);
tunable!(pub KEEP_FREED: bool = true, "VICTORY_KEEP_FREED",
    "Keep freed memory for reuse (off: given back at once)", live = NextFrame);
tunable!(pub SWEEP: bool = true, "VICTORY_SWEEP",
    "The per-frame sweep of idle objects (off: keeps everything)", live = NextFrame);
tunable!(pub BURY_LAG: i64 = -1, -1..=1000000000000, "VICTORY_BURY_LAG",
    "Frames objects let go wait before they are dropped (-1: 64, or 16 when host RAM is limited)", live = Safe);
tunable!(pub TRAIL: bool = true, "VICTORY_TRAIL",
    "Keep the trail of recent submissions (off: nothing kept, nothing said)", live = Safe);
tunable!(pub D3D12_GBV: bool = false, "VICTORY_D3D12_GBV",
    "GPU-based validation of the D3D12 debug layer", live = Restart);
tunable!(pub D3D12_WARP: bool = false, "VICTORY_D3D12_WARP",
    "Use the software adapter (diagnostics builds)", live = Restart);
tunable!(pub D3D12_BACKGROUND: bool = true, "VICTORY_D3D12_BACKGROUND",
    "The runtime's background shader work (off: switched off)", live = Restart);

tunable_list!(pub(super) fn register:
    QUEUE_WAIT, SPLIT_WAITS, CUT_WAITS, WAIT_FORCE, GPU_LOAD_MS, LIST_POOL, READ_BACK, WAIT_ASK,
    CUT_WRITES, COPY_OVER_TITLE, SEND_OWN, UNDER_WORDS, SEND_BAR, TEXTURE_KEEP, WAIT_RESIDENT,
    KEEP_FREED, SWEEP, BURY_LAG, TRAIL, D3D12_GBV, D3D12_WARP, D3D12_BACKGROUND);
