//! gpu_ports tunables (see ../known.rs): renderer ports from other forks.

#[allow(unused_imports)]
use crate::{tunable, tunable_list};

tunable!(pub PORT_CONSTANTS: bool = false, "VICTORY_PORT_CONSTANTS",
    "Renderer port 1: the title's 32-bit graphics heap (the ring of constants) is read and written by the card where the title has it, no copy, upload, fetch or fill", live = Restart);
tunable!(pub PORT_CONSTANTS_ORDER: bool = true, "VICTORY_PORT_CONSTANTS_ORDER",
    "With PORT_CONSTANTS: indirect executions whose arguments the card reads in place are clamped right before they run (off: in batches and windows, as before)", live = Restart);
tunable!(pub PORT_CONSTANTS_ORDER_ALONE: bool = true, "VICTORY_PORT_CONSTANTS_ORDER_ALONE",
    "With PORT_CONSTANTS_ORDER: such executions never share a batch (off: in a batch opened after the last shader that may write buffers)", live = Restart);

tunable!(pub PORT_CBV: bool = false, "VICTORY_PORT_CBV",
    "Renderer port 5: translated shaders read constant buffers as raw bytes, the title's in place given to the card where they are, no copy at submission (changes shader translation and its cache key)", live = Restart);
tunable!(pub PORT_CBV_PLACE: bool = true, "VICTORY_PORT_CBV_PLACE",
    "With PORT_CBV: a table's constant buffer in title memory in place is read where it is (off: copied as before, read raw all the same)", live = Restart);

tunable!(pub PORT_GUARD: bool = false, "VICTORY_PORT_GUARD",
    "Renderer port 6: the write guards' lock shared by faults and refreshes, a claim per 64 KiB block, view memory not asked of the kernel", live = Restart);
tunable!(pub PORT_GUARD_PLAN: bool = true, "VICTORY_PORT_GUARD_PLAN",
    "With PORT_GUARD: a submission's planning of its refreshes is done on the threads beside the submitting one (off: on it, as before)", live = Restart);
tunable!(pub GUARD_HOT: i64 = 0, 0..=1000, "VICTORY_GUARD_HOT",
    "With PORT_GUARD: milliseconds a block the title writes again soon after each taking rests longer (0: off; measured worse)", live = Restart);
tunable!(pub PORT_JOURNAL: bool = false, "VICTORY_PORT_JOURNAL",
    "Renderer port 7: the heaps' records asked at the title's submit, the journal's notes in 64 parts with locks of their own", live = Restart);

tunable_list!(pub(super) fn register: PORT_CONSTANTS, PORT_CONSTANTS_ORDER, PORT_CONSTANTS_ORDER_ALONE, PORT_CBV, PORT_CBV_PLACE, PORT_GUARD, PORT_GUARD_PLAN, GUARD_HOT, PORT_JOURNAL);
