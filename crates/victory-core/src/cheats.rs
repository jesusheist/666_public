//! The title's own cheat functions: a small lock-free queue of cheat numbers.
//!
//! The menu and the debug server queue a number; the title's game thread (the
//! loader's tick hook) takes the numbers and calls the title's function for
//! each. Nothing here runs title code.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// How many cheat numbers the title's table holds.
pub const COUNT: usize = 16;
/// The most presses of one number kept waiting.
const DEPTH: u8 = 4;

static READY: AtomicBool = AtomicBool::new(false);
static PENDING: [AtomicU8; COUNT] = [const { AtomicU8::new(0) }; COUNT];

/// The loader says the table was verified and the tick hook is installed.
pub fn set_ready(ready: bool) {
    READY.store(ready, Ordering::Release);
}

/// Whether the title's cheats can be used (VICTORY_CHEATS=1 and a verified build).
pub fn ready() -> bool {
    READY.load(Ordering::Acquire)
}

/// Queues cheat `n` to run once on the title's game thread.
pub fn queue(n: u8) -> Result<(), &'static str> {
    let slot = PENDING.get(usize::from(n)).ok_or("There is no such cheat")?;
    if !ready() {
        return Err("Start with VICTORY_CHEATS=1");
    }
    slot.fetch_update(Ordering::AcqRel, Ordering::Acquire, |c| (c < DEPTH).then_some(c + 1))
        .map(drop)
        .map_err(|_| "Too many presses waiting")
}

/// Calls `run` for every waiting press, lowest number first, and empties the queue.
pub fn drain(mut run: impl FnMut(u8)) {
    for (n, slot) in PENDING.iter().enumerate() {
        for _ in 0..slot.swap(0, Ordering::AcqRel) {
            run(n as u8);
        }
    }
}

/// Gives the debug server its `cheat` tool.
pub(crate) fn register_mcp() {
    victory_mcp::set_cheat_hook(|n| {
        queue(n).map_err(str::to_owned)?;
        Ok(victory_mcp::Json::obj([("queued", victory_mcp::Json::Int(i64::from(n)))]))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // The queue is process-wide: one test walks all of it.
    #[test]
    fn the_queue_needs_ready_keeps_order_and_caps_each_number() {
        set_ready(false);
        assert_eq!(queue(3), Err("Start with VICTORY_CHEATS=1"));
        assert_eq!(queue(16), Err("There is no such cheat"));
        set_ready(true);
        assert_eq!(queue(9), Ok(()));
        assert_eq!(queue(2), Ok(()));
        assert_eq!(queue(2), Ok(()));
        for _ in 0..DEPTH {
            let _ = queue(15);
        }
        assert_eq!(queue(15), Err("Too many presses waiting"));
        let mut seen = Vec::new();
        drain(|n| seen.push(n));
        assert_eq!(&seen[..3], [2, 2, 9]);
        assert_eq!(seen.iter().filter(|&&n| n == 15).count(), usize::from(DEPTH));
        let mut again = 0;
        drain(|_| again += 1);
        assert_eq!(again, 0);
        set_ready(false);
    }
}
