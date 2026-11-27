//! The title's life cycle. A console title is never told it is closing:
//! quitting it or launching a new game first constrains it (its
//! constrained-change routines run), then suspends it (its suspend routines
//! run, and it has a second to save) and then ends the process. Closing the
//! title's window does the same here; a new launch then starts `victory`
//! again, which waits for this process to be gone.
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
use windows_sys::Win32::UI::WindowsAndMessaging::{self as wm, HMENU};
use windows_sys::core::{PCSTR, PCWSTR, w};

use crate::{HleModule, hle_export};

/// A suspend routine (`quiesced`) or a constrained-change routine
/// (`constrained`): both take a flag and the title's context.
type Routine = extern "system" fn(flag: u8, context: *mut c_void);

/// Registration id, routine and context.
type Registration = (usize, Routine, usize);

/// Each suspend routine.
static ROUTINES: Mutex<Vec<Registration>> = Mutex::new(Vec::new());

/// Each constrained-change routine.
static CONSTRAINED: Mutex<Vec<Registration>> = Mutex::new(Vec::new());

fn routines() -> MutexGuard<'static, Vec<Registration>> {
    ROUTINES.lock().unwrap_or_else(|e| e.into_inner())
}

fn constrained() -> MutexGuard<'static, Vec<Registration>> {
    CONSTRAINED.lock().unwrap_or_else(|e| e.into_inner())
}

/// How long the console lets suspend routines run before it ends the title.
const SUSPEND_DEADLINE: Duration = Duration::from_secs(1);

const ERROR_INVALID_PARAMETER: u32 = 87;

/// Adds `routine` to `list` and writes its registration id.
fn register(
    mut list: MutexGuard<'static, Vec<Registration>>,
    routine: Option<Routine>,
    context: *mut c_void,
    registration: *mut usize,
) -> u32 {
    let Some(routine) = routine else {
        return ERROR_INVALID_PARAMETER;
    };
    if registration.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    list.push((id, routine, context as usize));
    // SAFETY: checked non-null; the title passes room for its registration.
    unsafe { registration.write(id) };
    0
}

pub extern "system" fn RegisterAppStateChangeNotification(
    routine: Option<Routine>,
    context: *mut c_void,
    registration: *mut usize,
) -> u32 {
    register(routines(), routine, context, registration)
}

pub extern "system" fn UnregisterAppStateChangeNotification(registration: usize) {
    routines().retain(|&(id, ..)| id != registration);
}

/// The host's own version never calls the routine (a desktop app is never
/// constrained), so a title waiting for it would never suspend.
pub extern "system" fn RegisterAppConstrainedChangeNotification(
    routine: Option<Routine>,
    context: *mut c_void,
    registration: *mut usize,
) -> u32 {
    register(constrained(), routine, context, registration)
}

pub extern "system" fn UnregisterAppConstrainedChangeNotification(registration: usize) {
    constrained().retain(|&(id, ..)| id != registration);
}

/// The window procedure a title window had before ours.
const TITLE_PROC: PCWSTR = w!("victory.wndproc");
const SYNC_CURSOR: u32 = wm::WM_APP + 0x315;
const CURSOR_TOKEN: usize = 0x564b_424d;

/// Fullscreen, the English keyboard and the mouse capture are asked for
/// (`=1`, or the F1 menu): left unset, the window and input stay as before.
pub(super) fn enabled(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| v == "1")
}

fn fullscreen(hwnd: HWND) {
    if !enabled("VICTORY_FULLSCREEN") {
        return;
    }
    // SAFETY: this thread owns the newly created top-level window.
    unsafe {
        let (width, height) = std::env::var("VICTORY_FULLSCREEN_RES")
            .ok().as_deref().and_then(parse_window_limit)
            .unwrap_or_else(|| (wm::GetSystemMetrics(wm::SM_CXSCREEN), wm::GetSystemMetrics(wm::SM_CYSCREEN)));
        let style = wm::GetWindowLongW(hwnd, wm::GWL_STYLE) as u32;
        let ex = wm::GetWindowLongW(hwnd, wm::GWL_EXSTYLE) as u32;
        wm::SetWindowLongW(hwnd, wm::GWL_STYLE, ((style & !wm::WS_OVERLAPPEDWINDOW) | wm::WS_POPUP) as i32);
        wm::SetWindowLongW(hwnd, wm::GWL_EXSTYLE, (ex & !(wm::WS_EX_CLIENTEDGE | wm::WS_EX_WINDOWEDGE | wm::WS_EX_DLGMODALFRAME | wm::WS_EX_STATICEDGE)) as i32);
        wm::SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, width, height,
            wm::SWP_FRAMECHANGED | wm::SWP_NOZORDER | wm::SWP_NOACTIVATE);
    }
}

fn english_input() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse as kb;
    if !enabled("VICTORY_IME_ENGLISH") {
        return;
    }
    // SAFETY: enumerate loaded layouts; activate only on the window's thread.
    unsafe {
        static ENGLISH: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let layout = *ENGLISH.get_or_init(|| {
            let mut layouts = vec![std::ptr::null_mut(); kb::GetKeyboardLayoutList(0, std::ptr::null_mut()).max(0) as usize];
            let count = kb::GetKeyboardLayoutList(layouts.len() as i32, layouts.as_mut_ptr()).max(0) as usize;
            let found = layouts.iter().take(count).find(|&&h| h as usize & 0xffff_ffff == 0x0409_0409).copied();
            if found.is_none() { tracing::warn!("en-US keyboard layout unavailable; keeping current input language"); }
            found.unwrap_or(std::ptr::null_mut()) as usize
        });
        if layout != 0 { kb::ActivateKeyboardLayout(layout as kb::HKL, 0); }
    }
}

#[derive(Default)]
struct MouseCapture {
    window: usize,
    center: (i32, i32),
    reference: Option<(i32, i32)>,
    hidden: i32,
}
static MOUSE_CAPTURE: std::sync::LazyLock<Mutex<MouseCapture>> =
    std::sync::LazyLock::new(|| Mutex::new(MouseCapture::default()));

fn capture_mouse(hwnd: HWND, active: bool) {
    if super::controls::mapped_input_enabled() {
        return;
    }
    if !enabled("VICTORY_MOUSE_CAPTURE") { return; }
    let mut state = MOUSE_CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: runs on the window thread; local geometry, balanced cursor count.
    unsafe {
        if active && !wm::GetParent(hwnd).is_null() { return; }
        let active = active && wm::GetForegroundWindow() == hwnd && wm::IsIconic(hwnd) == 0;
        if !active {
            if state.window == hwnd as usize {
                wm::ClipCursor(std::ptr::null());
                for _ in 0..state.hidden { wm::ShowCursor(1); }
                *state = MouseCapture::default();
            }
            return;
        }
        if state.window != 0 && state.window != hwnd as usize { return; }
        let (mut area, mut origin) = (RECT::default(), POINT::default());
        if wm::GetClientRect(hwnd, &mut area) == 0 || ClientToScreen(hwnd, &mut origin) == 0
            || area.right <= 0 || area.bottom <= 0 { return; }
        let center = (origin.x + area.right / 2, origin.y + area.bottom / 2);
        area.left += origin.x; area.right += origin.x;
        area.top += origin.y; area.bottom += origin.y;
        if wm::ClipCursor(&area) == 0 { return; }
        if state.window == 0 {
            state.hidden = 1 + (wm::ShowCursor(0) + 1).max(0);
            for _ in 1..state.hidden { wm::ShowCursor(0); }
        }
        if state.center != center { state.reference = None; }
        state.window = hwnd as usize;
        state.center = center;
    }
}

pub(super) fn captured_mouse_delta() -> Option<(i64, i64)> {
    if super::controls::mapped_input_enabled() {
        return None;
    }
    if !enabled("VICTORY_MOUSE_CAPTURE") { return None; }
    let mut state = MOUSE_CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: sampling and warping serialize with capture/focus messages.
    unsafe {
        if state.window == 0 || wm::GetForegroundWindow() as usize != state.window { return None; }
        let mut point = POINT::default();
        if wm::GetCursorPos(&mut point) == 0 { state.reference = None; return None; }
        let point = (point.x, point.y);
        let warp = std::env::var_os("VICTORY_MOUSE_WARP").is_some_and(|v| v == "1");
        let warped = (warp && wm::SetCursorPos(state.center.0, state.center.1) != 0)
            .then_some(state.center);
        Some(mouse_motion(&mut state.reference, point, warped))
    }
}

fn mouse_motion(reference: &mut Option<(i32, i32)>, point: (i32, i32), warped: Option<(i32, i32)>) -> (i64, i64) {
    let delta = reference.map_or((0, 0), |(x, y)|
        (i64::from(point.0) - i64::from(x), i64::from(point.1) - i64::from(y)));
    *reference = Some(warped.unwrap_or(point));
    delta
}

pub extern "system" fn CreateWindowExW(
    ex_style: u32,
    class: PCWSTR,
    name: PCWSTR,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: HWND,
    menu: HMENU,
    instance: HINSTANCE,
    param: *const c_void,
) -> HWND {
    let (x, y, width, height) = fit(parent, x, y, width, height);
    // SAFETY: the title's own arguments, forwarded (the size fitted).
    let hwnd = unsafe {
        wm::CreateWindowExW(
            ex_style, class, name, style, x, y, width, height, parent, menu, instance, param,
        )
    };
    hook(hwnd);
    if parent.is_null() {
        set_icon(hwnd);
    }
    hwnd
}

/// See [`CreateWindowExW`].
pub extern "system" fn CreateWindowExA(
    ex_style: u32,
    class: PCSTR,
    name: PCSTR,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: HWND,
    menu: HMENU,
    instance: HINSTANCE,
    param: *const c_void,
) -> HWND {
    let (x, y, width, height) = fit(parent, x, y, width, height);
    // SAFETY: the title's own arguments, forwarded (the size fitted).
    let hwnd = unsafe {
        wm::CreateWindowExA(
            ex_style, class, name, style, x, y, width, height, parent, menu, instance, param,
        )
    };
    hook(hwnd);
    if parent.is_null() {
        set_icon(hwnd);
    }
    hwnd
}

/// Gives a top-level window of the title the port's icon, big for the taskbar
/// and small for the title bar: the title's window class has none, and Windows
/// would show its default. The icon file is built into the program
/// (`assets/gtavi.ico`) and made in memory: inside a title run the resource
/// functions find no icon group in our exe (seen in run logs), so no resource
/// lookup is used.
fn set_icon(hwnd: HWND) {
    if hwnd.is_null() {
        return;
    }
    let mut set = 0;
    // SAFETY: plain calls on our own window with bytes that live for the program.
    unsafe {
        for (which, cx, cy) in [
            (wm::ICON_BIG, wm::SM_CXICON, wm::SM_CYICON),
            (wm::ICON_SMALL, wm::SM_CXSMICON, wm::SM_CYSMICON),
        ] {
            let (w, h) = (wm::GetSystemMetrics(cx), wm::GetSystemMetrics(cy));
            let Some(bits) = icon_image(ICON_FILE, w.max(h) as u32) else {
                tracing::warn!("window icon: no image in the built-in icon file");
                return;
            };
            let icon = wm::CreateIconFromResourceEx(bits.as_ptr(), bits.len() as u32, 1, 0x0003_0000, w, h, wm::LR_DEFAULTCOLOR);
            if !icon.is_null() {
                // The class icon too: shown whenever the window's own is unset.
                let slot = if which == wm::ICON_BIG { wm::GCLP_HICON } else { wm::GCLP_HICONSM };
                wm::SetClassLongPtrW(hwnd, slot, icon as isize);
                wm::SendMessageW(hwnd, wm::WM_SETICON, which as WPARAM, icon as LPARAM);
                set += 1;
            }
        }
        let now = wm::SendMessageW(hwnd, wm::WM_GETICON, wm::ICON_BIG as WPARAM, 0);
        tracing::warn!(hwnd = hwnd as usize, set, now, "window icon applied");
    }
}

/// The port's icon (the GTA VI logo, several sizes), as an .ico file.
const ICON_FILE: &[u8] = include_bytes!("../../assets/gtavi.ico");

/// The image of an .ico file closest to `size` pixels (the smallest one at
/// least that big, else the biggest): its bytes, as `CreateIconFromResourceEx` takes them.
fn icon_image(file: &[u8], size: u32) -> Option<&[u8]> {
    let word = |at: usize| Some(u16::from_le_bytes(file.get(at..at + 2)?.try_into().ok()?));
    let dword = |at: usize| Some(u32::from_le_bytes(file.get(at..at + 4)?.try_into().ok()?));
    if word(0)? != 0 || word(2)? != 1 {
        return None;
    }
    let mut best: Option<(u32, usize, usize)> = None;
    for k in 0..usize::from(word(4)?) {
        let entry = 6 + 16 * k;
        let side = match *file.get(entry)? {
            0 => 256,
            n => u32::from(n),
        };
        let (len, at) = (dword(entry + 8)? as usize, dword(entry + 12)? as usize);
        let better = match best {
            None => true,
            Some((b, _, _)) => (side >= size && (b < size || side < b)) || (b < size && side > b),
        };
        if better {
            best = Some((side, at, len));
        }
    }
    let (_, at, len) = best?;
    file.get(at..at + len)
}

#[cfg(test)]
#[test]
fn the_built_in_icon_has_an_image_near_each_size() {
    for size in [16, 24, 32, 48, 64, 256] {
        let bits = icon_image(ICON_FILE, size).expect("an image");
        assert!(bits.len() > 1000, "size {size}");
    }
    assert!(icon_image(&[0; 4], 32).is_none());
}

/// The title's top-level window still has no icon (it was made before ours
/// could be set, or the title cleared it): sets it, at most a few times.
fn icon_missing(hwnd: HWND) -> bool {
    static TRIES: AtomicUsize = AtomicUsize::new(0);
    if TRIES.load(Ordering::Relaxed) >= 8 {
        return false;
    }
    // SAFETY: plain queries on a window of ours.
    let missing = unsafe {
        wm::GetParent(hwnd).is_null() && wm::SendMessageW(hwnd, wm::WM_GETICON, wm::ICON_BIG as WPARAM, 0) == 0
    };
    if missing {
        TRIES.fetch_add(1, Ordering::Relaxed);
    }
    missing
}

/// A console title sizes its window to its render target (
/// the title: 3840x2160 on a Series X), which can exceed the host's screen.
/// A top-level window larger than the work area is shrunk to fit, keeping
/// its proportions, and centred; what it shows is scaled to it (the swap
/// chain stretches). `VICTORY_WINDOW_MAX=WIDTHxHEIGHT` applies a smaller
/// limit without changing the title's render targets.
fn fit(parent: HWND, x: i32, y: i32, width: i32, height: i32) -> (i32, i32, i32, i32) {
    let mut area = RECT::default();
    // SAFETY: `area` receives the primary monitor's work area.
    let known =
        unsafe { wm::SystemParametersInfoW(wm::SPI_GETWORKAREA, 0, (&raw mut area).cast(), 0) };
    let limit = window_limit();
    let fitted = fit_with_limit(&area, width, height, limit);
    if !parent.is_null() || known == 0 || fitted == (width, height) {
        return (x, y, width, height);
    }
    tracing::info!(width, height, to = ?fitted, ?limit, "title window fitted to the screen");
    let (w, h) = fitted;
    let left = area.left + (area.right - area.left - w) / 2;
    let top = area.top + (area.bottom - area.top - h) / 2;
    (left, top, w, h)
}

fn parse_window_limit(value: &str) -> Option<(i32, i32)> {
    let (width, height) = value.trim().split_once(['x', 'X'])?;
    let (width, height) = (width.trim().parse().ok()?, height.trim().parse().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

fn window_limit() -> Option<(i32, i32)> {
    static LIMIT: std::sync::OnceLock<Option<(i32, i32)>> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        let value = std::env::var_os("VICTORY_WINDOW_MAX")?;
        let parsed = value.to_str().and_then(parse_window_limit);
        if parsed.is_none() {
            tracing::warn!(
                ?value,
                "ignoring invalid VICTORY_WINDOW_MAX; expected positive WIDTHxHEIGHT"
            );
        }
        parsed
    })
}

fn fit_with_limit(
    area: &RECT,
    width: i32,
    height: i32,
    limit: Option<(i32, i32)>,
) -> (i32, i32) {
    let Some((max_width, max_height)) = limit else {
        return fit_into(area, width, height);
    };
    let bounded = RECT {
        left: 0,
        top: 0,
        right: (area.right - area.left).min(max_width),
        bottom: (area.bottom - area.top).min(max_height),
    };
    fit_into(&bounded, width, height)
}

/// `width` x `height` scaled down to fit `area`, or unchanged if it fits
/// (or is `CW_USEDEFAULT`).
fn fit_into(area: &RECT, width: i32, height: i32) -> (i32, i32) {
    let (aw, ah) = (area.right - area.left, area.bottom - area.top);
    if width <= 0 || height <= 0 || aw <= 0 || ah <= 0 || (width <= aw && height <= ah) {
        return (width, height);
    }
    let scale = f64::min(
        f64::from(aw) / f64::from(width),
        f64::from(ah) / f64::from(height),
    );
    (
        (f64::from(width) * scale) as i32,
        (f64::from(height) * scale) as i32,
    )
}

/// Installs the lifecycle and input procedure once on a window in our process.
pub(crate) fn hook(hwnd: HWND) {
    if hwnd.is_null() {
        return;
    }
    static INSTALL: Mutex<()> = Mutex::new(());
    let _install = INSTALL.lock().unwrap_or_else(|err| err.into_inner());
    let procedure = window_proc as *const () as isize;
    // SAFETY: the caller has checked ownership or just created the window.
    // Its previous procedure is stored before ours can be called.
    unsafe {
        let previous = wm::GetWindowLongPtrW(hwnd, wm::GWLP_WNDPROC);
        if previous == procedure || previous == 0 {
            return;
        }
        // Already in the chain under another subclass (ScriptHook VI's overlay hooks the window after us): a
        // second install would store that subclass as the title's procedure while it calls ours, and the two
        // would call each other until the stack runs out.
        if !wm::GetPropW(hwnd, TITLE_PROC).is_null() {
            return;
        }
        if wm::SetPropW(hwnd, TITLE_PROC, previous as *mut c_void) == 0 {
            tracing::warn!("could not store the game window procedure");
            return;
        }
        if wm::SetWindowLongPtrW(hwnd, wm::GWLP_WNDPROC, procedure) == 0 {
            wm::RemovePropW(hwnd, TITLE_PROC);
            tracing::warn!("could not attach the game window input handler");
            return;
        }
        if wm::GetParent(hwnd).is_null() {
            fullscreen(hwnd);
            english_input();
            capture_mouse(hwnd, true);
        }
    }
}

/// Refreshes the visible cursor on the window's own UI thread.
pub(crate) fn sync_cursor(hwnd: HWND) {
    // SAFETY: the private message carries only an integer token.
    unsafe { wm::PostMessageW(hwnd, SYNC_CURSOR, CURSOR_TOKEN, 0) };
}

extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the foreground-window query has no arguments.
    if message == wm::WM_KEYDOWN && unsafe { wm::GetForegroundWindow() == hwnd }
        && victory_gpu::menu_key_down(wparam as u16, lparam & (1 << 30) != 0)
    {
        return 0;
    }
    if message == wm::WM_SETFOCUS {
        english_input();
    }
    match message {
        wm::WM_KILLFOCUS | wm::WM_NCDESTROY => capture_mouse(hwnd, false),
        wm::WM_ACTIVATE => capture_mouse(hwnd, wparam & 0xffff != wm::WA_INACTIVE as usize),
        wm::WM_SETFOCUS | wm::WM_MOVE | wm::WM_SIZE => capture_mouse(hwnd, true),
        _ => {}
    }
    if message == SYNC_CURSOR && wparam == CURSOR_TOKEN {
        // SAFETY: the foreground window owns the visible cursor shape.
        unsafe {
            if wm::GetForegroundWindow() == hwnd {
                let captured = super::gameinput::cursor::captured();
                let cursor = if captured {
                    std::ptr::null_mut()
                } else {
                    wm::LoadCursorW(std::ptr::null_mut(), wm::IDC_ARROW)
                };
                wm::SetCursor(cursor);
                tracing::debug!(captured, "cursor applied on game window thread");
            }
        }
        return 0;
    }
    // A user close (close button, Alt+F4, window or taskbar menu); a
    // console title never sees one, nor a WM_CLOSE from it.
    if message == wm::WM_SYSCOMMAND && wparam & 0xFFF0 == wm::SC_CLOSE as usize {
        end_title("window closed", Box::new(|| {}));
        return 0;
    }
    // Alt or F10 puts a window into menu mode: the system runs a modal menu
    // loop on the thread that owns the window, which is the title's. The
    // title then stops (no frames, no input) until a key ends the loop, and
    // nothing shows it, since the window has no menu bar;
    // the title aborts itself about half a minute later. A console has no window
    // menu. The mouse still opens it from the title bar.
    if message == wm::WM_SYSCOMMAND && wparam & 0xFFF0 == wm::SC_KEYMENU as usize {
        return 0;
    }
    if drag::handle(hwnd, message, wparam) {
        return 0;
    }
    // The mouse cursor for keyboard and mouse play (`gameinput::cursor`):
    // hidden over the window while captured, captured by a click, freed when
    // the window loses focus. The title still sees every message.
    match message {
        wm::WM_SETCURSOR
            if super::gameinput::cursor::captured() && lparam as u32 & 0xFFFF == wm::HTCLIENT =>
        {
            // SAFETY: no cursor while it is over our client area.
            unsafe { wm::SetCursor(std::ptr::null_mut()) };
            return 1;
        }
        wm::WM_LBUTTONDOWN | wm::WM_RBUTTONDOWN | wm::WM_MBUTTONDOWN => {
            super::gameinput::cursor::click();
        }
        wm::WM_KILLFOCUS => super::gameinput::cursor::release(),
        // Sizing or moving the window: the cursor is the user's until the
        // loop ends (a clip following a shrinking client area collapses it).
        wm::WM_ENTERSIZEMOVE => super::gameinput::cursor::size_move(true),
        wm::WM_EXITSIZEMOVE => super::gameinput::cursor::size_move(false),
        wm::WM_SHOWWINDOW | wm::WM_ACTIVATE | wm::WM_ACTIVATEAPP | wm::WM_SIZE | wm::WM_PAINT if icon_missing(hwnd) => set_icon(hwnd),
        _ => {}
    }
    // SAFETY: the procedure `hook` stored, called the way the system would.
    unsafe {
        let title = wm::GetPropW(hwnd, TITLE_PROC);
        if message == wm::WM_NCDESTROY {
            wm::RemovePropW(hwnd, TITLE_PROC);
        }
        let title = std::mem::transmute::<*mut c_void, wm::WNDPROC>(title);
        wm::CallWindowProcW(title, hwnd, message, wparam, lparam)
    }
}

/// Moving and sizing the window by its frame without the system's modal
/// loop. A click on the title bar or a border makes `DefWindowProc` run a
/// loop on the window's thread, which is the title's: the title gets no
/// frames, input or audio pump until the button is released. Here the
/// press starts a drag that is only state; the mouse messages that follow
/// (the window holds the mouse capture) move or size the window one
/// `SetWindowPos` at a time, and the title's own loop keeps running between
/// them. `VICTORY_DRAG_FIX=0` leaves the system's loop in place.
mod drag {
    use std::sync::Mutex;

    use windows_sys::Win32::Foundation::{HWND, POINT, RECT, WPARAM};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
    use windows_sys::Win32::UI::WindowsAndMessaging::{self as wm};

    /// A drag in progress: the window, the hit-test code that started it,
    /// and where the cursor and the window were then.
    #[derive(Clone, Copy)]
    struct Drag {
        hwnd: usize,
        hit: u32,
        cursor: POINT,
        rect: RECT,
    }

    /// The left button flag in a mouse message's `wparam`.
    const MK_LBUTTON: usize = 0x0001;

    static ACTIVE: Mutex<Option<Drag>> = Mutex::new(None);

    fn enabled() -> bool {
        victory_gpu::tunables::known::DRAG_FIX.get()
    }

    fn active() -> std::sync::MutexGuard<'static, Option<Drag>> {
        ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn sizing(hit: u32) -> bool {
        (wm::HTLEFT..=wm::HTBOTTOMRIGHT).contains(&hit)
    }

    /// The window's rectangle after the cursor moved by (`dx`, `dy`) in a
    /// drag begun on `hit`; a size never goes below `min`.
    pub(super) fn dragged_rect(hit: u32, start: &RECT, dx: i32, dy: i32, min: (i32, i32)) -> RECT {
        let mut r = *start;
        if hit == wm::HTCAPTION {
            r.left += dx;
            r.right += dx;
            r.top += dy;
            r.bottom += dy;
            return r;
        }
        let left = matches!(hit, wm::HTLEFT | wm::HTTOPLEFT | wm::HTBOTTOMLEFT);
        let right = matches!(hit, wm::HTRIGHT | wm::HTTOPRIGHT | wm::HTBOTTOMRIGHT);
        let top = matches!(hit, wm::HTTOP | wm::HTTOPLEFT | wm::HTTOPRIGHT);
        let bottom = matches!(hit, wm::HTBOTTOM | wm::HTBOTTOMLEFT | wm::HTBOTTOMRIGHT);
        if left {
            r.left = (start.left + dx).min(start.right - min.0);
        }
        if right {
            r.right = (start.right + dx).max(start.left + min.0);
        }
        if top {
            r.top = (start.top + dy).min(start.bottom - min.1);
        }
        if bottom {
            r.bottom = (start.bottom + dy).max(start.top + min.1);
        }
        r
    }

    fn end(hwnd: HWND) {
        *active() = None;
        // SAFETY: gives back the capture this window took (a no-op without).
        unsafe {
            if GetCapture() == hwnd {
                ReleaseCapture();
            }
        }
    }

    /// True when the message was ours and the title must not see it.
    pub(super) fn handle(hwnd: HWND, message: u32, wparam: WPARAM) -> bool {
        if !enabled() {
            return false;
        }
        match message {
            wm::WM_NCLBUTTONDOWN => {
                let hit = wparam as u32;
                if hit != wm::HTCAPTION && !sizing(hit) {
                    return false;
                }
                let mut cursor = POINT { x: 0, y: 0 };
                let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                // SAFETY: plain queries and the capture for this window.
                unsafe {
                    // A maximized window is restored by the system's own
                    // handling of the drag, and borders do nothing then.
                    if wm::IsZoomed(hwnd) != 0
                        || wm::GetCursorPos(&mut cursor) == 0
                        || wm::GetWindowRect(hwnd, &mut rect) == 0
                    {
                        return false;
                    }
                    *active() = Some(Drag { hwnd: hwnd as usize, hit, cursor, rect });
                    SetCapture(hwnd);
                }
                true
            }
            wm::WM_MOUSEMOVE => {
                let Some(drag) = active().filter(|d| d.hwnd == hwnd as usize) else {
                    return false;
                };
                if wparam & MK_LBUTTON == 0 {
                    end(hwnd);
                    return true;
                }
                let mut now = POINT { x: 0, y: 0 };
                // SAFETY: a query, then one move or size of our window.
                unsafe {
                    if wm::GetCursorPos(&mut now) == 0 {
                        return true;
                    }
                    let min = (
                        wm::GetSystemMetrics(wm::SM_CXMINTRACK),
                        wm::GetSystemMetrics(wm::SM_CYMINTRACK),
                    );
                    let r = dragged_rect(
                        drag.hit,
                        &drag.rect,
                        now.x - drag.cursor.x,
                        now.y - drag.cursor.y,
                        min,
                    );
                    wm::SetWindowPos(
                        hwnd,
                        std::ptr::null_mut(),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        wm::SWP_NOZORDER | wm::SWP_NOACTIVATE,
                    );
                }
                true
            }
            wm::WM_LBUTTONUP | wm::WM_CANCELMODE | wm::WM_CAPTURECHANGED => {
                if active().is_some_and(|d| d.hwnd == hwnd as usize) {
                    end(hwnd);
                    return message != wm::WM_CAPTURECHANGED;
                }
                false
            }
            _ => false,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const START: RECT = RECT { left: 100, top: 100, right: 500, bottom: 400 };
        const MIN: (i32, i32) = (136, 39);

        #[test]
        fn a_caption_drag_moves_without_sizing() {
            let r = dragged_rect(wm::HTCAPTION, &START, 30, -20, MIN);
            assert_eq!((r.left, r.top, r.right, r.bottom), (130, 80, 530, 380));
        }

        #[test]
        fn borders_move_only_their_own_edges() {
            let r = dragged_rect(wm::HTBOTTOMRIGHT, &START, 50, 60, MIN);
            assert_eq!((r.left, r.top, r.right, r.bottom), (100, 100, 550, 460));
            let r = dragged_rect(wm::HTTOPLEFT, &START, 10, 20, MIN);
            assert_eq!((r.left, r.top, r.right, r.bottom), (110, 120, 500, 400));
            let r = dragged_rect(wm::HTRIGHT, &START, -20, 99, MIN);
            assert_eq!((r.left, r.top, r.right, r.bottom), (100, 100, 480, 400));
        }

        #[test]
        fn a_size_never_goes_below_the_minimum_and_the_far_edge_stays() {
            let r = dragged_rect(wm::HTLEFT, &START, 900, 0, MIN);
            assert_eq!((r.left, r.right), (500 - 136, 500));
            let r = dragged_rect(wm::HTBOTTOM, &START, 0, -900, MIN);
            assert_eq!((r.top, r.bottom), (100, 100 + 39));
        }
    }
}

/// Suspends the title as the console does when it ends it, runs `then`
/// (once), then ends the process. Returns at once; later calls do nothing.
pub(crate) fn end_title(why: &'static str, then: Box<dyn FnOnce() + Send>) {
    static ENDING: AtomicBool = AtomicBool::new(false);
    if ENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let constrain = constrained().clone();
    let suspend = routines().clone();
    tracing::info!(
        constrained = constrain.len(),
        routines = suspend.len(),
        "{why}: suspending the title"
    );
    // Whoever comes first (the routines or the deadline) runs `then`; the
    // other waits for it, so the process never ends halfway through.
    let then = std::sync::Arc::new(Mutex::new(Some(then)));
    let finish = move |how: &str| {
        let mut then = then.lock().unwrap_or_else(|e| e.into_inner());
        tracing::info!("{how}; ending the title");
        if let Some(then) = then.take() {
            then();
        }
        end();
    };
    let late = finish.clone();
    // Not on the calling thread: titles hand the suspend to their window
    // thread and wait for it inside the routine.
    victory_core::threads::spawn("victory-suspend", move || {
        constrain_then_suspend(&constrain, &suspend);
        finish("title suspended");
    });
    victory_core::threads::spawn("victory-suspend-wait", move || {
        std::thread::sleep(SUSPEND_DEADLINE);
        late("title did not finish suspending in time");
    });
}

/// Runs the constrained-change routines (constrained), then the suspend
/// routines (quiesced), as the console does when it suspends a title.
/// Grand Theft Auto VI's suspend routine waits for a state that only its
/// constrained routine brings about; called alone it never returns.
fn constrain_then_suspend(constrain: &[Registration], suspend: &[Registration]) {
    for &(_, routine, context) in constrain.iter().chain(suspend) {
        routine(1, context as *mut c_void);
    }
}

/// Set in the environment of a relaunched `victory`: an inherited handle
/// of the process it replaces.
const RELAUNCHED_FROM: &str = "VICTORY_RELAUNCHED_FROM";

/// Starts `victory` again with `args` (after the title has suspended); the
/// new process waits until this one has ended.
pub(crate) fn relaunch(args: Vec<std::ffi::OsString>) {
    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
    let mut me: HANDLE = std::ptr::null_mut();
    // SAFETY: an inheritable duplicate of our own process handle.
    let duplicated = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            GetCurrentProcess(),
            GetCurrentProcess(),
            &mut me,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    } != 0;
    let spawned = std::env::current_exe().and_then(|exe| {
        let mut command = std::process::Command::new(exe);
        command.args(&args);
        if duplicated {
            command.env(RELAUNCHED_FROM, (me as usize).to_string());
        }
        command.spawn()
    });
    match spawned {
        Ok(child) => tracing::info!(pid = child.id(), ?args, "relaunched"),
        Err(e) => tracing::warn!(%e, "relaunch failed; the title just ends"),
    }
}

/// In a relaunched `victory`: waits (30 s at most) for the process it
/// replaces to be gone, so the two never share the title's files.
pub fn wait_for_previous() {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    let Some(handle) = std::env::var(RELAUNCHED_FROM)
        .ok()
        .and_then(|h| h.parse::<usize>().ok())
    else {
        return;
    };
    // SAFETY: removing our own variable before any other thread reads it.
    unsafe { std::env::remove_var(RELAUNCHED_FROM) };
    let handle = handle as HANDLE;
    // SAFETY: the inherited process handle the previous process gave us.
    unsafe {
        WaitForSingleObject(handle, 30_000);
        CloseHandle(handle);
    }
}

fn end() {
    // Compiled host pipelines kept for the next run.
    #[cfg(windows)]
    victory_gpu::pipeline::save_pipeline_cache();
    // SAFETY: ends this process, as the console ends a suspended title.
    unsafe { TerminateProcess(GetCurrentProcess(), 0) };
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "lifecycle",
        dlls: &[
            "api-ms-win-core-psm-appnotify-*",
            "ext-ms-win-rtcore-ntuser-window-*",
        ],
        exports: vec![
            hle_export!(RegisterAppStateChangeNotification),
            hle_export!(UnregisterAppStateChangeNotification),
            hle_export!(RegisterAppConstrainedChangeNotification),
            hle_export!(UnregisterAppConstrainedChangeNotification),
            hle_export!(CreateWindowExW),
            hle_export!(CreateWindowExA),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_reference_handles_focus_reset_and_warp() {
        if !enabled("VICTORY_MOUSE_CAPTURE") { assert_eq!(captured_mouse_delta(), None); }
        let mut reference = None;
        assert_eq!(mouse_motion(&mut reference, (480, 390), None), (0, 0));
        assert_eq!(mouse_motion(&mut reference, (500, 400), None), (20, 10));
        reference = None; // Refocus never reports motion from another application.
        assert_eq!(mouse_motion(&mut reference, (500, 400), Some((960, 540))), (0, 0));
        assert_eq!(mouse_motion(&mut reference, (970, 535), Some((960, 540))), (10, -5));
        assert_eq!(mouse_motion(&mut reference, (960, 540), None), (0, 0));
        assert_eq!(reference, Some((960, 540))); // Failed/disabled warp uses the sample.
    }

    extern "system" fn noop(_: u8, _: *mut c_void) {}

    #[test]
    fn registrations_come_and_go() {
        let (mut a, mut b) = (0, 0);
        assert_eq!(
            RegisterAppStateChangeNotification(Some(noop), std::ptr::null_mut(), &mut a),
            0
        );
        assert_eq!(
            RegisterAppStateChangeNotification(Some(noop), std::ptr::null_mut(), &mut b),
            0
        );
        UnregisterAppStateChangeNotification(a);
        let left: Vec<_> = routines().iter().map(|r| r.0).collect();
        assert_eq!(left, [b]);
    }

    #[test]
    fn constrained_registrations_come_and_go() {
        let (mut a, mut b) = (0, 0);
        assert_eq!(
            RegisterAppConstrainedChangeNotification(Some(noop), std::ptr::null_mut(), &mut a),
            0
        );
        assert_eq!(
            RegisterAppConstrainedChangeNotification(Some(noop), std::ptr::null_mut(), &mut b),
            0
        );
        assert_eq!(
            RegisterAppConstrainedChangeNotification(None, std::ptr::null_mut(), &mut a),
            ERROR_INVALID_PARAMETER
        );
        UnregisterAppConstrainedChangeNotification(a);
        let left: Vec<_> = constrained().iter().map(|r| r.0).collect();
        assert_eq!(left, [b]);
    }

    /// What `recording_routine` saw: (context, flag) per call, in order.
    static CALLS: Mutex<Vec<(usize, u8)>> = Mutex::new(Vec::new());

    extern "system" fn recording_routine(flag: u8, context: *mut c_void) {
        CALLS.lock().unwrap().push((context as usize, flag));
    }

    #[test]
    fn constrained_routines_run_before_suspend_routines() {
        let r: Routine = recording_routine;
        constrain_then_suspend(&[(1, r, 10), (2, r, 11)], &[(3, r, 20), (4, r, 21)]);
        assert_eq!(*CALLS.lock().unwrap(), [(10, 1), (11, 1), (20, 1), (21, 1)]);
    }

    #[test]
    fn oversized_windows_shrink_to_the_screen() {
        let area = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        assert_eq!(fit_into(&area, 3840, 2160), (1848, 1040));
        assert_eq!(fit_into(&area, 1280, 720), (1280, 720));
        assert_eq!(
            fit_into(&area, wm::CW_USEDEFAULT, 0),
            (wm::CW_USEDEFAULT, 0)
        );
    }

    #[test]
    fn configured_window_limit_fits_4k_into_1080p() {
        let area = RECT {
            left: 100,
            top: 50,
            right: 2660,
            bottom: 1490,
        };
        assert_eq!(fit_with_limit(&area, 3840, 2160, Some((1920, 1080))), (1920, 1080));
        assert_eq!(fit_with_limit(&area, 640, 480, Some((1920, 1080))), (640, 480));
        assert_eq!(fit_with_limit(&area, 3840, 2160, None), (2560, 1440));
    }

    #[test]
    fn configured_window_limit_still_respects_the_work_area() {
        let area = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        assert_eq!(fit_with_limit(&area, 3840, 2160, Some((2560, 1440))), (1848, 1040));
        assert_eq!(
            fit_with_limit(&area, wm::CW_USEDEFAULT, 0, Some((1920, 1080))),
            (wm::CW_USEDEFAULT, 0)
        );
    }

    #[test]
    fn configured_window_limit_preserves_non_widescreen_proportions() {
        let area = RECT {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1440,
        };
        assert_eq!(fit_with_limit(&area, 1600, 1200, Some((1920, 1080))), (1440, 1080));
    }

    #[test]
    fn window_limit_requires_two_positive_dimensions() {
        assert_eq!(parse_window_limit("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_window_limit(" 1920 X 1080 "), Some((1920, 1080)));
        for bad in [
            "",
            "1920",
            "0x1080",
            "1920x0",
            "-1x1080",
            "1920x-1",
            "1920x1080x2",
            "2147483648x1080",
        ] {
            assert_eq!(parse_window_limit(bad), None, "{bad}");
        }
    }

    #[test]
    fn hooked_windows_still_reach_the_title_procedure() {
        let null = std::ptr::null_mut();
        let hwnd = CreateWindowExW(
            0,
            w!("STATIC"),
            w!("before"),
            0,
            0,
            0,
            10,
            10,
            null,
            null,
            null,
            std::ptr::null(),
        );
        assert!(!hwnd.is_null());
        let mut text = [0u16; 16];
        // SAFETY: our window; the buffer holds 16 characters.
        unsafe {
            assert_eq!(wm::GetWindowLongW(hwnd, wm::GWL_STYLE) as u32 & wm::WS_POPUP != 0, enabled("VICTORY_FULLSCREEN"));
            wm::SetWindowTextW(hwnd, w!("after"));
            let n = wm::GetWindowTextW(hwnd, text.as_mut_ptr(), 16);
            assert_eq!(String::from_utf16_lossy(&text[..n as usize]), "after");
            assert!(!wm::GetPropW(hwnd, TITLE_PROC).is_null());
            wm::DestroyWindow(hwnd);
        }
    }

    /// Another subclass on top of ours (ScriptHook VI's overlay), then a second `hook` (focus regained): ours must
    /// not go on top again, or the two procedures would call each other until the stack overflows.
    #[test]
    fn a_second_hook_under_another_subclass_does_not_loop() {
        static OUTER_NEXT: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
        unsafe extern "system" fn outer(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            // SAFETY: the procedure stored below, called the way the system would.
            unsafe {
                let next = std::mem::transmute::<isize, wm::WNDPROC>(OUTER_NEXT.load(std::sync::atomic::Ordering::SeqCst));
                wm::CallWindowProcW(next, hwnd, msg, w, l)
            }
        }
        let null = std::ptr::null_mut();
        let hwnd = CreateWindowExW(0, w!("STATIC"), w!("before"), 0, 0, 0, 10, 10, null, null, null, std::ptr::null());
        assert!(!hwnd.is_null());
        // SAFETY: our window; `outer` forwards to the procedure it replaced.
        unsafe {
            let ours = wm::GetWindowLongPtrW(hwnd, wm::GWLP_WNDPROC);
            OUTER_NEXT.store(ours, std::sync::atomic::Ordering::SeqCst);
            wm::SetWindowLongPtrW(hwnd, wm::GWLP_WNDPROC, outer as *const () as isize);
            hook(hwnd);
            assert_eq!(wm::GetWindowLongPtrW(hwnd, wm::GWLP_WNDPROC), outer as *const () as isize);
            wm::SetWindowTextW(hwnd, w!("after"));
            let mut text = [0u16; 16];
            let n = wm::GetWindowTextW(hwnd, text.as_mut_ptr(), 16);
            assert_eq!(String::from_utf16_lossy(&text[..n as usize]), "after");
            wm::DestroyWindow(hwnd);
        }
    }

    thread_local! {
        /// The system commands `recording` received, low four bits cleared.
        static COMMANDS: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Stands in for a title's procedure: notes system commands and acts
    /// on none of them (no menu loop, no minimising).
    extern "system" fn recording(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == wm::WM_SYSCOMMAND {
            COMMANDS.with(|c| c.borrow_mut().push(wparam & 0xFFF0));
            return 0;
        }
        // SAFETY: the system's default procedure, called as the system would.
        unsafe { wm::DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    #[test]
    fn the_keyboard_menu_never_reaches_the_title() {
        let null = std::ptr::null_mut();
        let hwnd = CreateWindowExW(
            0,
            w!("STATIC"),
            w!("menu"),
            0,
            0,
            0,
            10,
            10,
            null,
            null,
            null,
            std::ptr::null(),
        );
        assert!(!hwnd.is_null());
        // SAFETY: our window, owned by this thread, so each message is
        // handled before SendMessageW returns. `recording` stands in for the
        // title's procedure and the original is put back before the window
        // is destroyed.
        unsafe {
            let title = wm::GetPropW(hwnd, TITLE_PROC);
            wm::SetPropW(hwnd, TITLE_PROC, recording as *const () as *mut c_void);
            let command = |c: u32, key: u8| {
                wm::SendMessageW(hwnd, wm::WM_SYSCOMMAND, c as usize, LPARAM::from(key))
            };
            command(wm::SC_KEYMENU, 0); // Alt or F10
            command(wm::SC_KEYMENU, b' '); // Alt+Space
            command(wm::SC_KEYMENU | 1, 0); // the low bits are the system's
            command(wm::SC_MINIMIZE, 0);
            wm::SetPropW(hwnd, TITLE_PROC, title);
            wm::DestroyWindow(hwnd);
        }
        let seen = COMMANDS.with(|c| c.borrow().clone());
        assert_eq!(seen, [wm::SC_MINIMIZE as usize]);
    }
}
