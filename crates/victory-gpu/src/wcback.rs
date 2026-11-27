//! `VICTORY_SKIP_WC_BACK`: what shaders wrote into card copies of title
//! memory the title reserved write-combining is not fetched back into title
//! memory (the console's CPU does not read that memory back either). Copies
//! under the floor (`VICTORY_SKIP_WC_BACK_MIN`, KiB, default 1024) still come
//! back: they hold small results the title's CPU does read. Off by default.
//!
//! Where the title's CPU writes part of a 64 KiB piece the card wrote, the
//! next upload can send stale bytes: this is a speed trade, not exact.
//! Source: fxceless 7e64279d (switch), Peppacodes ca2432b (keep floor),
//! Yuriko 27c190a (floor by copy size, 1 MiB).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// The console protection bit for write-combined memory.
const WRITE_COMBINED: u32 = 0x400;

/// A reservation of `XMemVirtualAlloc` that asked for write-combining.
static RESERVED: Mutex<BTreeMap<usize, usize>> = Mutex::new(BTreeMap::new());

/// Pieces left on the card: count and bytes.
static SKIPPED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

fn on() -> bool {
    victory_core::tunables::known::SKIP_WC_BACK.get()
}

/// The runtime reserved `[at, at + len)` for the title with the console
/// protection `asked` (before the runtime cleans it). Kept only with the
/// switch on and only for write-combined memory.
pub fn title_memory_reserved(at: usize, len: usize, asked: u32) {
    if !on() || at == 0 || len == 0 || asked & WRITE_COMBINED == 0 {
        return;
    }
    RESERVED.lock().unwrap_or_else(|e| e.into_inner()).insert(at, len);
}

/// Whether a piece of `bytes` of the copy of the title buffer at `start`,
/// `len` bytes long, stays on the card (counted).
pub(crate) fn skip_back(start: u64, len: u64, bytes: u64) -> bool {
    if !on() || len < (victory_core::tunables::known::SKIP_WC_BACK_MIN.get().max(0) as u64) << 10 {
        return false;
    }
    let wc = RESERVED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .range(..=start as usize)
        .next_back()
        .is_some_and(|(base, l)| *base + *l > start as usize);
    if wc {
        let n = SKIPPED[0].fetch_add(1, Ordering::Relaxed) + 1;
        let b = SKIPPED[1].fetch_add(bytes, Ordering::Relaxed) + bytes;
        if n.is_power_of_two() {
            tracing::info!("VICTORY_SKIP_WC_BACK: {n} pieces ({} MB) of write-combining memory left on the card so far", b >> 20);
        }
    }
    wc
}
