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
/// the memory line as `name value`.
pub(crate) fn memory_extra() -> Vec<(&'static str, u64)> {
    Vec::new()
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

/// Gives the debug server (`VICTORY_MCP`) its memory tools: `get_memory` is the
/// numbers of the memory line, and `get_settings` / `set_setting` are the
/// tunables ([`victory_core::tunables`]). Once is enough.
pub(crate) fn register_mcp() {
    use victory_mcp::Json;
    fn mb(b: u64) -> Json {
        Json::uint(b >> 20)
    }
    fn memory() -> Json {
        let groups = crate::host::host().and_then(|h| h.video_memory_groups());
        let group = |g: [u64; 4]| Json::obj([("budget_mb", mb(g[0])), ("in_use_mb", mb(g[1]))]);
        let [ws, committed] = process_memory();
        Json::obj([
            ("gpu_local", groups.map_or(Json::Null, |[own, _]| group(own))),
            ("gpu_shared", groups.map_or(Json::Null, |[_, shared]| group(shared))),
            ("process", Json::obj([("working_set_mb", mb(ws)), ("committed_mb", mb(committed))])),
            ("texture_host_copies_mb", mb(crate::resource::KINDS[1].load(Relaxed))),
            ("tiles_given_mb", mb(crate::tiles::given_total())),
            ("pool_free_texture_room_mb", mb(crate::pool::texture_room())),
            ("card_copies_and_windows_mb", mb(crate::shadow::live_span_bytes())),
        ])
    }
    victory_mcp::set_memory_hooks(victory_mcp::MemoryHooks {
        memory,
        settings: || victory_core::tunables::list_json(""),
        set: victory_core::tunables::set_json,
    });
}
