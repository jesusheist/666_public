//! The numbers behind the memory line of `memory.log` (`VICTORY_LOG_DIR`):
//! gathered once a second by the frame meter and handed to
//! `victory_core::logging`, which writes a line every 5 s and one when the
//! host device is found removed. Nothing here runs without the log files.

use std::sync::atomic::Ordering::Relaxed;

/// The process's memory in bytes: working set, and committed (private bytes).
fn process_memory() -> [u64; 2] {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: our own process and a sized structure.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    [c.WorkingSetSize as u64, c.PagefileUsage as u64]
}

/// The hook for more memory numbers: named raw values, written at the end of
/// the memory line as `name value`: the card accounting, the texture pool
/// setting and the eviction totals ([`crate::vram::memory_extra`]).
pub(crate) fn memory_extra() -> Vec<(&'static str, u64)> {
    crate::vram::memory_extra()
}

/// Once a second, from the frame meter.
pub(crate) fn tick() {
    if !victory_core::logging::files_on() {
        return;
    }
    let groups = crate::host::host().and_then(|h| h.video_memory_groups());
    let mut extra = vec![("texture host copy count", crate::resource::KINDS[0].load(Relaxed))];
    extra.extend(memory_extra());
    victory_core::logging::publish_memory(victory_core::logging::MemSample {
        gpu_local: groups.map(|[own, _]| [own[0], own[1]]),
        gpu_non_local: groups.map(|[_, shared]| [shared[0], shared[1]]),
        process: Some(process_memory()),
        bytes: vec![
            ("texture host copies", crate::resource::KINDS[1].load(Relaxed)),
            ("tiles given", crate::tiles::given_total()),
            ("pool free texture room", crate::pool::texture_room()),
            ("card copies and windows", crate::shadow::live_span_bytes()),
        ],
        extra,
    });
}
