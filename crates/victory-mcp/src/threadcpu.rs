//! The `thread_cpu` tool: which of the process's threads burn CPU, grouped by
//! pool. Threads of ours carry a name (`victory-<pool>-<n>`, see
//! `victory_core::threads`), read from the thread's description; threads
//! without one (the title's own) are grouped by the module they start in.
//!
//! The grouping and the report are plain code (unit-tested); only the
//! sampling is Windows-specific.

use crate::json::Json;

/// The default and largest sample window, in milliseconds.
pub const DEFAULT_MS: i64 = 2000;
pub const MAX_MS: i64 = 10_000;
/// How many single threads the report lists.
pub const TOP_THREADS: usize = 15;

/// One thread's measurement over the window.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadSample {
    pub tid: u32,
    /// The thread's description (empty when it has none).
    pub name: String,
    /// Kernel plus user time spent in the window, in 100 ns units.
    pub busy_100ns: u64,
    pub priority: i32,
    /// Where an unnamed thread starts, as `module+0xoffset` (empty if unknown).
    pub start: String,
}

/// The pool a thread belongs to: its name without a trailing `-<n>` (or
/// `_<n>`, ` <n>`); for an unnamed thread, `unnamed <module>` from its start.
pub fn pool_of(name: &str, start: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        let module = start.split('+').next().filter(|m| !m.is_empty()).unwrap_or("?");
        return format!("unnamed {module}");
    }
    let digits = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if digits.len() < name.len() {
        if let Some(stem) = digits.strip_suffix(['-', '_', ' ']).filter(|s| !s.is_empty()) {
            return stem.to_owned();
        }
    }
    name.to_owned()
}

/// A thread's share of one core over the window, in percent.
fn percent(busy_100ns: u64, window_100ns: u64) -> f64 {
    if window_100ns == 0 { 0.0 } else { busy_100ns as f64 * 100.0 / window_100ns as f64 }
}

/// One pool in the report.
#[derive(Clone, Debug, PartialEq)]
pub struct PoolRow {
    pub pool: String,
    pub threads: usize,
    pub busy_cores: f64,
    pub top_thread_pct: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub window_ms: f64,
    pub threads: usize,
    pub total_busy_cores: f64,
    /// Busiest pool first.
    pub pools: Vec<PoolRow>,
    /// The busiest single threads, busiest first.
    pub top: Vec<ThreadSample>,
}

/// Groups the samples by pool. `window_100ns` is the real length of the window.
pub fn group(samples: &[ThreadSample], window_100ns: u64) -> Report {
    let mut pools: Vec<PoolRow> = Vec::new();
    for s in samples {
        let pool = pool_of(&s.name, &s.start);
        let pct = percent(s.busy_100ns, window_100ns);
        match pools.iter_mut().find(|p| p.pool == pool) {
            Some(p) => {
                p.threads += 1;
                p.busy_cores += pct / 100.0;
                p.top_thread_pct = p.top_thread_pct.max(pct);
            }
            None => pools.push(PoolRow { pool, threads: 1, busy_cores: pct / 100.0, top_thread_pct: pct }),
        }
    }
    pools.sort_by(|a, b| b.busy_cores.total_cmp(&a.busy_cores).then_with(|| a.pool.cmp(&b.pool)));
    let mut top = samples.to_vec();
    top.sort_by(|a, b| b.busy_100ns.cmp(&a.busy_100ns).then(a.tid.cmp(&b.tid)));
    top.truncate(TOP_THREADS);
    Report {
        window_ms: window_100ns as f64 / 10_000.0,
        threads: samples.len(),
        total_busy_cores: samples.iter().map(|s| percent(s.busy_100ns, window_100ns) / 100.0).sum(),
        pools,
        top,
    }
}

fn round(v: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (v * k).round() / k
}

/// The report as the tool returns it.
pub fn report_json(report: &Report, window_100ns: u64) -> Json {
    Json::obj([
        ("window_ms", Json::num(round(report.window_ms, 1))),
        ("threads", Json::uint(report.threads as u64)),
        ("total_busy_cores", Json::num(round(report.total_busy_cores, 2))),
        (
            "pools",
            Json::Arr(
                report
                    .pools
                    .iter()
                    .map(|p| {
                        Json::obj([
                            ("pool", Json::str(p.pool.clone())),
                            ("threads", Json::uint(p.threads as u64)),
                            ("busy_cores", Json::num(round(p.busy_cores, 2))),
                            ("top_thread_pct", Json::num(round(p.top_thread_pct, 1))),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "top_threads",
            Json::Arr(
                report
                    .top
                    .iter()
                    .map(|t| {
                        let name = if t.name.is_empty() { format!("(unnamed) {}", t.start) } else { t.name.clone() };
                        Json::obj([
                            ("tid", Json::uint(u64::from(t.tid))),
                            ("name", Json::str(name)),
                            ("pct", Json::num(round(percent(t.busy_100ns, window_100ns), 1))),
                            ("priority", Json::Int(i64::from(t.priority))),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// Samples this process's threads over `ms` milliseconds; the samples and
/// the real window length in 100 ns units.
#[cfg(windows)]
pub fn sample(ms: u64) -> Result<(Vec<ThreadSample>, u64), String> {
    win::sample(ms)
}

#[cfg(not(windows))]
pub fn sample(_ms: u64) -> Result<(Vec<ThreadSample>, u64), String> {
    Err("thread_cpu works on Windows only".into())
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, LocalFree};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetThreadDescription, GetThreadPriority, GetThreadTimes, OpenThread};

    use super::ThreadSample;

    const THREAD_QUERY_INFORMATION: u32 = 0x0040;
    const THREAD_QUERY_LIMITED_INFORMATION: u32 = 0x0800;
    /// `ThreadQuerySetWin32StartAddress`.
    const START_ADDRESS_CLASS: u32 = 9;

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryInformationThread(thread: HANDLE, class: u32, info: *mut c_void, len: u32, returned: *mut u32) -> i32;
    }

    struct Thread {
        tid: u32,
        handle: HANDLE,
        at_start: u64,
    }

    fn thread_ids() -> Vec<u32> {
        let mut ids = Vec::new();
        // SAFETY: plain snapshot calls; the entry is ours and sized.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap.is_null() || snap as isize == -1 {
                return ids;
            }
            let pid = GetCurrentProcessId();
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut more = Thread32First(snap, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    ids.push(entry.th32ThreadID);
                }
                more = Thread32Next(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
        }
        ids
    }

    fn open(tid: u32) -> Option<HANDLE> {
        // SAFETY: plain calls.
        unsafe {
            let h = OpenThread(THREAD_QUERY_INFORMATION | THREAD_QUERY_LIMITED_INFORMATION, 0, tid);
            if !h.is_null() {
                return Some(h);
            }
            let h = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, tid);
            (!h.is_null()).then_some(h)
        }
    }

    fn ticks(h: HANDLE) -> Option<u64> {
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: four valid out structures.
        let ok = unsafe { GetThreadTimes(h, &mut created, &mut exited, &mut kernel, &mut user) } != 0;
        let join = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
        ok.then(|| join(kernel) + join(user))
    }

    fn description(h: HANDLE) -> String {
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: on success the system gives a string it owns until LocalFree.
        unsafe {
            if GetThreadDescription(h, &mut text) < 0 || text.is_null() {
                return String::new();
            }
            let mut n = 0;
            while *text.add(n) != 0 {
                n += 1;
            }
            let name = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
            LocalFree(text.cast());
            name
        }
    }

    /// `module+0xoffset` of where the thread starts.
    fn start_place(h: HANDLE) -> String {
        let mut address = 0usize;
        // SAFETY: the out pointer is ours and the size matches.
        let status = unsafe {
            NtQueryInformationThread(h, START_ADDRESS_CLASS, (&raw mut address).cast(), std::mem::size_of::<usize>() as u32, std::ptr::null_mut())
        };
        if status < 0 || address == 0 {
            return String::new();
        }
        let mut module: *mut c_void = std::ptr::null_mut();
        // SAFETY: from-address lookup with an unchanged reference count; the name buffer is ours.
        unsafe {
            let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
            if GetModuleHandleExW(flags, address as *const u16, &mut module) == 0 || module.is_null() {
                return format!("unmapped+{address:#x}");
            }
            let mut buf = [0u16; 260];
            let n = GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) as usize;
            let path = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
            let file = path.rsplit(['\\', '/']).next().unwrap_or("?");
            format!("{file}+{:#x}", address - module as usize)
        }
    }

    pub fn sample(ms: u64) -> Result<(Vec<ThreadSample>, u64), String> {
        let began = Instant::now();
        let mut threads: Vec<Thread> = thread_ids()
            .into_iter()
            .filter_map(|tid| {
                let handle = open(tid)?;
                match ticks(handle) {
                    Some(at_start) => Some(Thread { tid, handle, at_start }),
                    None => {
                        // SAFETY: a handle opened above.
                        unsafe { CloseHandle(handle) };
                        None
                    }
                }
            })
            .collect();
        if threads.is_empty() {
            return Err("no thread of this process could be opened".into());
        }
        std::thread::sleep(Duration::from_millis(ms));
        // Threads born in the window start from zero.
        for tid in thread_ids() {
            if threads.iter().all(|t| t.tid != tid) {
                if let Some(handle) = open(tid) {
                    threads.push(Thread { tid, handle, at_start: 0 });
                }
            }
        }
        let window = (began.elapsed().as_nanos() / 100) as u64;
        let mut out = Vec::with_capacity(threads.len());
        for t in threads {
            if let Some(at_end) = ticks(t.handle) {
                let name = description(t.handle);
                let start = if name.is_empty() { start_place(t.handle) } else { String::new() };
                // SAFETY: a plain query on an open handle.
                let priority = unsafe { GetThreadPriority(t.handle) };
                out.push(ThreadSample { tid: t.tid, name, busy_100ns: at_end.saturating_sub(t.at_start), priority, start });
            }
            // SAFETY: a handle opened above.
            unsafe { CloseHandle(t.handle) };
        }
        Ok((out, window))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        #[test]
        fn a_named_thread_shows_its_name_and_a_busy_one_its_time() {
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let busy = std::thread::Builder::new()
                .name("victory-probe-1".into())
                .spawn(move || {
                    while !flag.load(Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                })
                .unwrap();
            let (samples, window) = sample(300).unwrap();
            stop.store(true, Ordering::Relaxed);
            busy.join().unwrap();
            let mine = samples.iter().find(|s| s.name == "victory-probe-1").expect("the description reaches Windows");
            assert!(mine.busy_100ns * 2 > window, "a spinning thread uses most of a core: {mine:?} in {window}");
            assert!(samples.iter().any(|s| s.name.is_empty() && s.start.contains('+')), "unnamed threads show where they start");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(tid: u32, name: &str, busy: u64, start: &str) -> ThreadSample {
        ThreadSample { tid, name: name.into(), busy_100ns: busy, priority: 0, start: start.into() }
    }

    #[test]
    fn the_number_at_the_end_is_not_part_of_the_pool() {
        assert_eq!(pool_of("victory-dstorage-3", ""), "victory-dstorage");
        assert_eq!(pool_of("victory-land-12", ""), "victory-land");
        assert_eq!(pool_of("worker 7", ""), "worker");
        assert_eq!(pool_of("victory-pad-poll", ""), "victory-pad-poll");
        assert_eq!(pool_of("x264", ""), "x264");
        assert_eq!(pool_of("-5", ""), "-5");
    }

    #[test]
    fn an_unnamed_thread_is_grouped_by_its_module() {
        assert_eq!(pool_of("", "game.dll+0x1234"), "unnamed game.dll");
        assert_eq!(pool_of("  ", ""), "unnamed ?");
    }

    #[test]
    fn busy_cores_add_up_per_pool_and_the_pools_sort_busiest_first() {
        let window = 10_000_000; // one second
        let samples = [
            t(1, "victory-a-0", 5_000_000, ""),
            t(2, "victory-a-1", 2_500_000, ""),
            t(3, "victory-b", 9_000_000, ""),
            t(4, "", 1_000_000, "game.dll+0x10"),
        ];
        let r = group(&samples, window);
        assert_eq!(r.threads, 4);
        assert!((r.total_busy_cores - 1.75).abs() < 1e-9);
        let names: Vec<_> = r.pools.iter().map(|p| p.pool.as_str()).collect();
        assert_eq!(names, ["victory-b", "victory-a", "unnamed game.dll"]);
        let a = &r.pools[1];
        assert_eq!(a.threads, 2);
        assert!((a.busy_cores - 0.75).abs() < 1e-9);
        assert!((a.top_thread_pct - 50.0).abs() < 1e-9);
        assert_eq!(r.top.iter().map(|s| s.tid).collect::<Vec<_>>(), [3, 1, 2, 4]);
    }

    #[test]
    fn only_the_busiest_fifteen_threads_are_listed() {
        let samples: Vec<_> = (0..40).map(|i| t(i, "victory-x", u64::from(i) * 10, "")).collect();
        let r = group(&samples, 1000);
        assert_eq!(r.top.len(), TOP_THREADS);
        assert_eq!(r.top[0].tid, 39);
        assert_eq!(r.pools[0].threads, 40);
    }

    #[test]
    fn an_empty_window_reports_zero_not_nan() {
        let r = group(&[t(1, "a", 5, "")], 0);
        assert_eq!(r.total_busy_cores, 0.0);
        assert_eq!(r.pools[0].top_thread_pct, 0.0);
    }

    #[test]
    fn the_json_has_the_documented_shape() {
        let r = group(&[t(1, "victory-a-0", 5_000_000, ""), t(2, "", 1_000_000, "game.dll+0x10")], 10_000_000);
        let j = report_json(&r, 10_000_000);
        for key in ["window_ms", "threads", "total_busy_cores", "pools", "top_threads"] {
            assert!(j.get(key).is_some(), "{key}");
        }
        let text = j.text();
        assert!(text.contains("\"pool\":\"victory-a\""), "{text}");
        assert!(text.contains("(unnamed) game.dll+0x10"), "{text}");
    }
}
