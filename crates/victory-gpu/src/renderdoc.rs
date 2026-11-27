//! `renderdoc_capture` (`VICTORY_MCP`): one frame capture through RenderDoc's
//! in-application API.
//!
//! Nothing is installed or downloaded. The API exists only when
//! `renderdoc.dll` is in the process before the D3D12 device is made: either
//! the game was started from RenderDoc (it injects the dll), or
//! `VICTORY_RENDERDOC=<path to renderdoc.dll>` names one, which [`load`]
//! loads first. Without it the tool answers with that instruction.

use std::ffi::{CString, c_char, c_void};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};

/// `eRENDERDOC_API_Version_1_1_2`: the table up to `TriggerCapture` has the
/// same layout in every version from 1.0 on.
const API_VERSION: u32 = 10102;

/// Slots of the API table (renderdoc_app.h) that are used.
const SET_CAPTURE_PATH_TEMPLATE: usize = 11;
const GET_NUM_CAPTURES: usize = 13;
const GET_CAPTURE: usize = 14;
const TRIGGER_CAPTURE: usize = 15;

type GetApi = unsafe extern "C" fn(u32, *mut *mut c_void) -> i32;
type GetNum = unsafe extern "C" fn() -> u32;
type GetCapture = unsafe extern "C" fn(u32, *mut c_char, *mut u32, *mut u64) -> u32;
type Trigger = unsafe extern "C" fn();
type SetTemplate = unsafe extern "C" fn(*const c_char);

struct Api {
    table: *const usize,
}

// SAFETY: the table is RenderDoc's own, read-only and lives as long as the process.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static API: OnceLock<Option<Api>> = OnceLock::new();

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Loads the dll `VICTORY_RENDERDOC` names (if any), finds the API of the one
/// in the process and registers the tool. Call before the D3D12 device is made.
pub(crate) fn load() {
    if !victory_mcp::enabled() {
        return;
    }
    let path = std::env::var("VICTORY_RENDERDOC").ok().filter(|p| !p.is_empty());
    let api = API.get_or_init(|| {
        let name = wide("renderdoc.dll");
        // SAFETY: NUL-terminated names; loading a dll the user named.
        let module = unsafe {
            let loaded = GetModuleHandleW(name.as_ptr());
            if !loaded.is_null() {
                loaded
            } else if let Some(path) = &path {
                let m = LoadLibraryW(wide(path).as_ptr());
                if m.is_null() {
                    tracing::warn!(%path, "VICTORY_RENDERDOC: the dll did not load");
                }
                m
            } else {
                std::ptr::null_mut()
            }
        };
        if module.is_null() {
            return None;
        }
        // SAFETY: a loaded module and a NUL-terminated export name.
        let get: GetApi = unsafe { std::mem::transmute(GetProcAddress(module, c"RENDERDOC_GetAPI".as_ptr().cast())?) };
        let mut table: *mut c_void = std::ptr::null_mut();
        // SAFETY: the export's documented signature.
        if unsafe { get(API_VERSION, &mut table) } != 1 || table.is_null() {
            tracing::warn!("RenderDoc is in the process but gave no API");
            return None;
        }
        tracing::info!("RenderDoc in-application API ready (renderdoc_capture)");
        Some(Api { table: table.cast() })
    });
    if api.is_some() {
        victory_mcp::set_renderdoc_hook(capture);
    }
}

fn slot<T: Copy>(api: &Api, at: usize) -> T {
    // SAFETY: `at` is a slot of the version's table; each is one function pointer.
    unsafe { std::mem::transmute_copy(&*api.table.add(at)) }
}

fn capture_path(api: &Api, index: u32) -> Option<String> {
    let get: GetCapture = slot(api, GET_CAPTURE);
    let mut len = 0u32;
    // SAFETY: a null buffer asks for the length.
    unsafe { get(index, std::ptr::null_mut(), &mut len, std::ptr::null_mut()) };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len as usize + 1];
    // SAFETY: the buffer holds `len` bytes and a NUL.
    let ok = unsafe { get(index, buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut()) };
    (ok != 0).then(|| String::from_utf8_lossy(&buf[..buf.iter().position(|b| *b == 0).unwrap_or(0)]).into_owned())
}

/// Triggers a capture of the next presented frame and waits for the file.
fn capture(wait: Duration, folder: Option<&str>) -> Result<String, String> {
    let Some(Some(api)) = API.get() else {
        return Err("RenderDoc is not loaded in this process: start the game from RenderDoc, or set VICTORY_RENDERDOC=<full path to renderdoc.dll> before it starts".to_owned());
    };
    if let Some(folder) = folder {
        let _ = std::fs::create_dir_all(folder);
        let template = CString::new(format!("{}/victory", folder.trim_end_matches(['/', '\\']))).map_err(|_| "bad folder")?;
        let set: SetTemplate = slot(api, SET_CAPTURE_PATH_TEMPLATE);
        // SAFETY: a NUL-terminated string RenderDoc copies.
        unsafe { set(template.as_ptr()) };
    }
    let count: GetNum = slot(api, GET_NUM_CAPTURES);
    let trigger: Trigger = slot(api, TRIGGER_CAPTURE);
    // SAFETY: plain calls into RenderDoc.
    let before = unsafe { count() };
    // SAFETY: as above.
    unsafe { trigger() };
    let began = Instant::now();
    while began.elapsed() < wait {
        std::thread::sleep(Duration::from_millis(100));
        // SAFETY: as above.
        let now = unsafe { count() };
        if now > before {
            return capture_path(api, now - 1).ok_or_else(|| "RenderDoc made a capture but gave no path".to_owned());
        }
    }
    Err("no capture was finished in time (is the game presenting frames?)".to_owned())
}
