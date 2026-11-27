//! "Find out what writes to this address": a hardware data breakpoint (debug
//! register 0, write only) on every thread of the process, and a vectored
//! exception handler that records which instruction wrote.
//!
//! The processor reports the trap after the writing instruction has run, so
//! the recorded RIP is the instruction AFTER the write. The handler only
//! uses atomics (no locks, no allocation), because it runs on whatever thread
//! wrote, and only consumes a SINGLE_STEP whose DR6 slot-0 bit is set and
//! whose DR0 is our address; every other exception goes on to the handlers
//! after it (guard faults and so on).

use crate::json::Json;
use crate::memscan::hex;

/// One recorded write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawHit {
    /// The instruction after the write.
    pub rip: u64,
    pub tid: u32,
    /// The value at the address after the write.
    pub value: u64,
}

/// Most hits one watch can record.
pub const HIT_CAPACITY: usize = 4096;
/// How many value changes the result lists.
pub const HISTORY_LIMIT: usize = 50;

/// An address as text: `img+<hex>` (from the image base), `0x<hex>` or bare hex.
pub fn parse_addr(text: &str, image_base: u64) -> Option<u64> {
    let t = text.trim();
    let hex_of = |h: &str| u64::from_str_radix(h.strip_prefix("0x").or_else(|| h.strip_prefix("0X")).unwrap_or(h), 16).ok();
    if t.len() >= 4 && t[..4].eq_ignore_ascii_case("img+") {
        if image_base == 0 {
            return None;
        }
        return image_base.checked_add(hex_of(t[4..].trim())?);
    }
    hex_of(t)
}

/// The DR7 value that turns slot 0 on as a write-only watch of `len` bytes,
/// or None if slot 0 is already in use or `len` is not 1, 2, 4 or 8.
pub fn dr7_arm(dr7: u64, len: usize) -> Option<u64> {
    if dr7 & 0b11 != 0 {
        return None;
    }
    let len_bits: u64 = match len {
        1 => 0b00,
        2 => 0b01,
        4 => 0b11,
        8 => 0b10,
        _ => return None,
    };
    // Local enable (bit 0), then RW (bits 16-17: 01 = write) and LEN (18-19).
    Some((dr7 & !(0xf << 16)) | 1 | ((0b01 | (len_bits << 2)) << 16))
}

/// DR7 with slot 0 off.
pub fn dr7_disarm(dr7: u64) -> u64 {
    dr7 & !(0b11 | (0xf << 16))
}

/// Where `addr` sits in the image, as `img+0x...`, if it is inside.
pub fn image_offset(addr: u64, base: u64, size: u64) -> Option<String> {
    (base != 0 && addr >= base && addr - base < size).then(|| format!("img+{:#x}", addr - base))
}

/// The bytes `code_read` returns: `[addr - before, addr + after)` clipped to
/// the image. Returns (start, length), or an error if `addr` is outside.
pub fn code_window(addr: u64, base: u64, size: u64, before: u64, after: u64) -> Result<(u64, usize), String> {
    if base == 0 || addr < base || addr - base >= size {
        return Err(format!("{addr:#x} is outside the title image ({base:#x} to {:#x})", base.wrapping_add(size)));
    }
    let start = addr.saturating_sub(before).max(base);
    let end = addr.saturating_add(after.max(1)).min(base + size);
    Ok((start, (end - start) as usize))
}

/// The result of a watch, as JSON.
pub fn shape(hits: &[RawHit], initial: u64, total: u64, base: u64, size: u64) -> Json {
    struct Writer {
        rip: u64,
        count: u64,
        last: u64,
        tids: Vec<u32>,
    }
    let mut writers: Vec<Writer> = Vec::new();
    for h in hits {
        match writers.iter_mut().find(|w| w.rip == h.rip) {
            Some(w) => {
                w.count += 1;
                w.last = h.value;
                if !w.tids.contains(&h.tid) {
                    w.tids.push(h.tid);
                }
            }
            None => writers.push(Writer { rip: h.rip, count: 1, last: h.value, tids: vec![h.tid] }),
        }
    }
    writers.sort_by(|a, b| b.count.cmp(&a.count).then(a.rip.cmp(&b.rip)));
    let writers = writers
        .iter()
        .map(|w| {
            let mut pairs = vec![("rip_after_write", hex(w.rip))];
            if let Some(o) = image_offset(w.rip, base, size) {
                pairs.push(("rip_image", Json::Str(o)));
            }
            pairs.push(("count", Json::uint(w.count)));
            pairs.push(("last_value", hex(w.last)));
            pairs.push(("thread_ids", Json::Arr(w.tids.iter().map(|t| Json::uint(u64::from(*t))).collect())));
            Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
        })
        .collect();
    let mut history = Vec::new();
    let mut prev = initial;
    for h in hits {
        if h.value != prev {
            if history.len() < HISTORY_LIMIT {
                let mut pairs = vec![("from", hex(prev)), ("to", hex(h.value)), ("rip_after_write", hex(h.rip))];
                if let Some(o) = image_offset(h.rip, base, size) {
                    pairs.push(("rip_image", Json::Str(o)));
                }
                pairs.push(("thread_id", Json::uint(u64::from(h.tid))));
                history.push(Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()));
            }
            prev = h.value;
        }
    }
    Json::obj([
        ("initial_value", hex(initial)),
        ("hits_total", Json::uint(total)),
        ("hits_recorded", Json::uint(hits.len() as u64)),
        ("writers", Json::Arr(writers)),
        ("value_history", Json::Arr(history)),
    ])
}

#[cfg(windows)]
pub use os::Session;

#[cfg(windows)]
mod os {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use std::time::Instant;

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, CONTEXT, CONTEXT_DEBUG_REGISTERS_AMD64, EXCEPTION_POINTERS, GetThreadContext, RemoveVectoredExceptionHandler,
        SetThreadContext,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
    use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread};

    use super::{HIT_CAPACITY, RawHit, dr7_arm, dr7_disarm};

    const THREAD_SUSPEND_RESUME: u32 = 0x0002;
    const THREAD_GET_CONTEXT: u32 = 0x0008;
    const THREAD_SET_CONTEXT: u32 = 0x0010;
    const SINGLE_STEP: u32 = 0x8000_0004;

    #[allow(clippy::declare_interior_mutable_const)]
    const Z64: AtomicU64 = AtomicU64::new(0);
    #[allow(clippy::declare_interior_mutable_const)]
    const Z32: AtomicU32 = AtomicU32::new(0);
    static RIPS: [AtomicU64; HIT_CAPACITY] = [Z64; HIT_CAPACITY];
    static VALUES: [AtomicU64; HIT_CAPACITY] = [Z64; HIT_CAPACITY];
    static TIDS: [AtomicU32; HIT_CAPACITY] = [Z32; HIT_CAPACITY];
    /// Hits seen (may pass the capacity; only the first ones are stored).
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    /// The watched address while a watch is up (0: none).
    static ADDR: AtomicU64 = AtomicU64::new(0);
    static LEN: AtomicUsize = AtomicUsize::new(1);
    static BUSY: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the system passes valid pointers to a handler.
        unsafe {
            let rec = (*info).ExceptionRecord;
            if rec.is_null() || (*rec).ExceptionCode as u32 != SINGLE_STEP {
                return 0;
            }
            let ctx = (*info).ContextRecord;
            if ctx.is_null() {
                return 0;
            }
            let addr = ADDR.load(Ordering::Acquire);
            if addr == 0 || (*ctx).Dr6 & 1 == 0 || (*ctx).Dr0 != addr {
                return 0;
            }
            let n = COUNT.fetch_add(1, Ordering::AcqRel);
            if n < HIT_CAPACITY {
                let value = match LEN.load(Ordering::Relaxed) {
                    1 => u64::from(std::ptr::read_volatile(addr as *const u8)),
                    2 => u64::from(std::ptr::read_volatile(addr as *const u16)),
                    4 => u64::from(std::ptr::read_volatile(addr as *const u32)),
                    _ => std::ptr::read_volatile(addr as *const u64),
                };
                RIPS[n].store((*ctx).Rip, Ordering::Relaxed);
                VALUES[n].store(value, Ordering::Relaxed);
                TIDS[n].store(GetCurrentThreadId(), Ordering::Relaxed);
            }
            (*ctx).Dr6 &= !0xf;
            -1 // EXCEPTION_CONTINUE_EXECUTION
        }
    }

    /// Thread ids of this process.
    fn thread_ids() -> Vec<u32> {
        let mut out = Vec::new();
        // SAFETY: a snapshot handle and a structure of ours.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap.is_null() || snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let pid = GetCurrentProcessId();
            let mut e: THREADENTRY32 = std::mem::zeroed();
            e.dwSize = size_of::<THREADENTRY32>() as u32;
            let mut ok = Thread32First(snap, &mut e);
            while ok != 0 {
                if e.th32OwnerProcessID == pid {
                    out.push(e.th32ThreadID);
                }
                ok = Thread32Next(snap, &mut e);
            }
            CloseHandle(snap);
        }
        out
    }

    enum Change {
        Done,
        /// Slot 0 is already used by someone else.
        Busy,
        Failed,
    }

    /// Sets or clears the watch on one thread. Nothing allocates while the
    /// thread is suspended (it might hold the heap lock).
    fn change(tid: u32, arm: bool, addr: u64, len: usize) -> Change {
        // SAFETY: a thread handle we open and close; a context of ours.
        unsafe {
            let h = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_SET_CONTEXT, 0, tid);
            if h.is_null() {
                return Change::Failed;
            }
            if SuspendThread(h) == u32::MAX {
                CloseHandle(h);
                return Change::Failed;
            }
            let mut ctx: CONTEXT = std::mem::zeroed();
            ctx.ContextFlags = CONTEXT_DEBUG_REGISTERS_AMD64;
            let mut result = Change::Failed;
            if GetThreadContext(h, &mut ctx) != 0 {
                if arm {
                    match dr7_arm(ctx.Dr7, len) {
                        None => result = Change::Busy,
                        Some(d7) => {
                            ctx.Dr0 = addr;
                            ctx.Dr6 = 0;
                            ctx.Dr7 = d7;
                            if SetThreadContext(h, &ctx) != 0 {
                                result = Change::Done;
                            }
                        }
                    }
                } else {
                    ctx.Dr0 = 0;
                    ctx.Dr6 = 0;
                    ctx.Dr7 = dr7_disarm(ctx.Dr7);
                    if SetThreadContext(h, &ctx) != 0 {
                        result = Change::Done;
                    }
                }
            }
            ResumeThread(h);
            CloseHandle(h);
            result
        }
    }

    /// Thread counts of a watch.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Stats {
        pub armed: u32,
        pub in_use_skipped: u32,
        pub failed: u32,
    }

    /// A running watch. Dropping it stops it.
    pub struct Session {
        addr: u64,
        len: usize,
        handle: *mut c_void,
        armed: Vec<u32>,
        tried: Vec<u32>,
        stats: Stats,
        stopped: bool,
        pub started: Instant,
    }

    // SAFETY: the handle is only used to remove the handler.
    unsafe impl Send for Session {}

    impl Session {
        /// Puts the watch on every thread but this one. Fails if the length
        /// or alignment is wrong or another watch is running.
        pub fn start(addr: u64, len: usize) -> Result<Session, String> {
            if !matches!(len, 1 | 2 | 4 | 8) {
                return Err("len must be 1, 2, 4 or 8".into());
            }
            if addr % len as u64 != 0 {
                return Err(format!("{addr:#x} is not aligned to {len} bytes"));
            }
            if BUSY.swap(true, Ordering::AcqRel) {
                return Err("another watch is already running".into());
            }
            COUNT.store(0, Ordering::Release);
            LEN.store(len, Ordering::Relaxed);
            ADDR.store(addr, Ordering::Release);
            // SAFETY: our handler has the right signature and lives forever.
            let handle = unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
            if handle.is_null() {
                ADDR.store(0, Ordering::Release);
                BUSY.store(false, Ordering::Release);
                return Err("the exception handler could not be installed".into());
            }
            let mut s = Session { addr, len, handle, armed: Vec::new(), tried: Vec::new(), stats: Stats::default(), stopped: false, started: Instant::now() };
            s.poll();
            Ok(s)
        }

        /// Arms threads that appeared since the last look.
        pub fn poll(&mut self) {
            let me = unsafe { GetCurrentThreadId() };
            for tid in thread_ids() {
                if tid == me || self.tried.contains(&tid) {
                    continue;
                }
                self.tried.push(tid);
                match change(tid, true, self.addr, self.len) {
                    Change::Done => {
                        self.armed.push(tid);
                        self.stats.armed += 1;
                    }
                    Change::Busy => self.stats.in_use_skipped += 1,
                    Change::Failed => self.stats.failed += 1,
                }
            }
        }

        /// Hits seen so far.
        pub fn hit_count(&self) -> u64 {
            COUNT.load(Ordering::Acquire) as u64
        }

        pub fn stats(&self) -> Stats {
            self.stats
        }

        fn stop(&mut self) {
            if self.stopped {
                return;
            }
            self.stopped = true;
            for &tid in &self.armed {
                change(tid, false, 0, self.len);
            }
            // The handler goes last; it swallows our own late traps until then.
            // SAFETY: the handle came from AddVectoredExceptionHandler.
            unsafe { RemoveVectoredExceptionHandler(self.handle) };
            BUSY.store(false, Ordering::Release);
        }

        /// Stops the watch and returns what it recorded and the total count.
        pub fn finish(mut self) -> (Vec<RawHit>, u64) {
            self.stop();
            let total = COUNT.load(Ordering::Acquire);
            ADDR.store(0, Ordering::Release);
            let hits = (0..total.min(HIT_CAPACITY))
                .map(|i| RawHit { rip: RIPS[i].load(Ordering::Relaxed), tid: TIDS[i].load(Ordering::Relaxed), value: VALUES[i].load(Ordering::Relaxed) })
                .collect();
            (hits, total as u64)
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            self.stop();
            ADDR.store(0, Ordering::Release);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::AtomicBool;
        use std::sync::mpsc;
        use std::time::Duration;

        static TARGET: AtomicU64 = AtomicU64::new(0);
        static GO: AtomicBool = AtomicBool::new(false);

        static HERE: AtomicU64 = AtomicU64::new(0);

        #[inline(never)]
        fn test_writer(v: u64) {
            let here: u64;
            // SAFETY: reads the instruction pointer into a register, nothing else.
            unsafe { std::arch::asm!("lea {}, [rip]", out(reg) here, options(nomem, nostack)) };
            HERE.store(here, Ordering::Relaxed);
            // A plain store in this very function (a debug build does not inline atomics).
            // SAFETY: writes 8 bytes to a static of ours.
            unsafe { std::arch::asm!("mov qword ptr [{0}], {1}", in(reg) TARGET.as_ptr(), in(reg) v, options(nostack)) };
        }

        #[test]
        fn a_write_from_another_thread_is_caught_inside_the_writer() {
            let addr = &TARGET as *const AtomicU64 as u64;
            let mut s = Session::start(addr, 8).expect("start");
            let (tx, rx) = mpsc::channel();
            let t = std::thread::spawn(move || {
                tx.send(()).unwrap();
                while !GO.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                test_writer(0x1234_5678_9abc);
            });
            rx.recv().unwrap();
            s.poll();
            assert!(s.stats().armed >= 1);
            GO.store(true, Ordering::Release);
            t.join().unwrap();
            let (hits, total) = s.finish();
            assert_eq!(total, 1, "{hits:?}");
            let here = HERE.load(Ordering::Relaxed);
            assert!(hits[0].rip > here && hits[0].rip - here < 128, "rip {:#x}, inside the writer near {here:#x}", hits[0].rip);
            assert_eq!(hits[0].value, 0x1234_5678_9abc);
            // The watch is gone: a later write is not recorded and does not crash.
            test_writer(1);
            let again = Session::start(addr, 8).expect("a second watch can start");
            drop(again);
        }

        #[test]
        fn bad_lengths_and_alignment_are_refused() {
            assert!(Session::start(0x1001, 2).is_err());
            assert!(Session::start(0x1000, 3).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_three_ways() {
        assert_eq!(parse_addr("img+74dfca5", 0x1000), Some(0x1000 + 0x74dfca5));
        assert_eq!(parse_addr("IMG+0x10", 0x1000), Some(0x1010));
        assert_eq!(parse_addr("0x2000", 0x1000), Some(0x2000));
        assert_eq!(parse_addr("2000", 0), Some(0x2000));
        assert_eq!(parse_addr("img+10", 0), None);
        assert_eq!(parse_addr("nope", 1), None);
    }

    #[test]
    fn debug_register_bits() {
        assert_eq!(dr7_arm(0, 1), Some(0x1_0001));
        assert_eq!(dr7_arm(0, 4), Some(0xd_0001));
        assert_eq!(dr7_arm(0, 8), Some(0x9_0001));
        assert_eq!(dr7_arm(0, 2), Some(0x5_0001));
        assert_eq!(dr7_arm(1, 1), None);
        assert_eq!(dr7_arm(0, 3), None);
        assert_eq!(dr7_disarm(dr7_arm(0x400, 8).unwrap()), 0x400);
    }

    #[test]
    fn code_window_clips_to_the_image() {
        assert_eq!(code_window(0x1100, 0x1000, 0x200, 64, 64), Ok((0x10c0, 128)));
        assert_eq!(code_window(0x1010, 0x1000, 0x200, 64, 64), Ok((0x1000, 0x10 + 64)));
        assert_eq!(code_window(0x11f0, 0x1000, 0x200, 0, 512), Ok((0x11f0, 0x10)));
        assert!(code_window(0x1200, 0x1000, 0x200, 1, 1).is_err());
        assert!(code_window(0x10, 0x1000, 0x200, 1, 1).is_err());
        assert!(code_window(0x10, 0, 0x200, 1, 1).is_err());
    }

    #[test]
    fn results_group_by_rip_and_list_changes() {
        let h = |rip, tid, value| RawHit { rip, tid, value };
        let hits = [h(0x1010, 1, 0), h(0x1010, 2, 1), h(0x9000, 1, 1), h(0x1010, 1, 0)];
        let j = shape(&hits, 0, 4, 0x1000, 0x100);
        let Some(Json::Arr(w)) = j.get("writers") else { panic!() };
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].get("rip_after_write").and_then(Json::as_str), Some("0x1010"));
        assert_eq!(w[0].get("rip_image").and_then(Json::as_str), Some("img+0x10"));
        assert_eq!(w[0].get("count").and_then(Json::as_i64), Some(3));
        assert!(w[1].get("rip_image").is_none());
        let Some(Json::Arr(hist)) = j.get("value_history") else { panic!() };
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].get("to").and_then(Json::as_str), Some("0x1"));
        assert_eq!(hist[1].get("to").and_then(Json::as_str), Some("0x0"));
    }
}
