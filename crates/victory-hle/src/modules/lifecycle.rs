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

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
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
    let (x, y, width, height) = fit(parent, x, y, width, height, style, ex_style, !menu.is_null());
    let ex_style = ex_style | no_activate(parent);
    // SAFETY: the title's own arguments, forwarded (the size fitted).
    let hwnd = unsafe {
        wm::CreateWindowExW(
            ex_style, class, name, style, x, y, width, height, parent, menu, instance, param,
        )
    };
    hook(hwnd);
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
    let (x, y, width, height) = fit(parent, x, y, width, height, style, ex_style, !menu.is_null());
    let ex_style = ex_style | no_activate(parent);
    // SAFETY: the title's own arguments, forwarded (the size fitted).
    let hwnd = unsafe {
        wm::CreateWindowExA(
            ex_style, class, name, style, x, y, width, height, parent, menu, instance, param,
        )
    };
    hook(hwnd);
    hwnd
}

/// `VICTORY_NO_FOCUS_STEAL=1`: a top-level window of the title is made with
/// `WS_EX_NOACTIVATE`, so showing it (or a click on it) never takes the
/// keyboard focus from the window the user is working in. The window is
/// otherwise a normal one: it can be moved and sized by other tools.
fn no_activate(parent: HWND) -> u32 {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = *ON.get_or_init(|| std::env::var_os("VICTORY_NO_FOCUS_STEAL").is_some_and(|v| v == "1"));
    if on && parent.is_null() { wm::WS_EX_NOACTIVATE } else { 0 }
}

/// A console title sizes its window to its render target (
/// the title: 3840x2160 on a Series X), which can exceed the host's screen.
/// A top-level window larger than the work area is shrunk to fit, keeping
/// its proportions, and centred; what it shows is scaled to it (the swap
/// chain stretches). `VICTORY_WINDOW_MAX=WIDTHxHEIGHT` applies a smaller
/// limit without changing the title's render targets.
fn fit(
    parent: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    style: u32,
    ex_style: u32,
    has_menu: bool,
) -> (i32, i32, i32, i32) {
    if parent.is_null() {
        if let Some(placed) = placement(style, ex_style, has_menu) {
            tracing::info!(?placed, "title window placed by VICTORY_WINDOW");
            return placed;
        }
    }
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

/// `VICTORY_WINDOW=x,y,width,height`: where a top-level title window opens
/// (outer position in desktop coordinates; a monitor left of the primary one
/// has negative x) and its client size, so 1280,720 is a 1280x720 picture
/// area. The outer size adds the frame of the window's style. Unset or
/// malformed: the window is placed as usual.
fn placement(style: u32, ex_style: u32, has_menu: bool) -> Option<(i32, i32, i32, i32)> {
    let (x, y, w, h) = parse_placement(&std::env::var("VICTORY_WINDOW").ok()?)?;
    let mut frame = RECT { left: 0, top: 0, right: w, bottom: h };
    // SAFETY: `frame` is a local rectangle the call grows by the frame.
    if unsafe { wm::AdjustWindowRectEx(&mut frame, style, i32::from(has_menu), ex_style) } == 0 {
        return Some((x, y, w, h));
    }
    Some((x, y, frame.right - frame.left, frame.bottom - frame.top))
}

fn parse_placement(text: &str) -> Option<(i32, i32, i32, i32)> {
    let v: Vec<i32> = text.split(',').map(|s| s.trim().parse().ok()).collect::<Option<_>>()?;
    match v[..] {
        [x, y, w, h] if w > 0 && h > 0 => Some((x, y, w, h)),
        _ => None,
    }
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
        if wm::SetPropW(hwnd, TITLE_PROC, previous as *mut c_void) == 0 {
            tracing::warn!("could not store the game window procedure");
            return;
        }
        if wm::SetWindowLongPtrW(hwnd, wm::GWLP_WNDPROC, procedure) == 0 {
            wm::RemovePropW(hwnd, TITLE_PROC);
            tracing::warn!("could not attach the game window input handler");
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
    if victory_gpu::window_mode_message(hwnd as usize, message, wparam, lparam) {
        return 0;
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
    std::thread::spawn(move || {
        constrain_then_suspend(&constrain, &suspend);
        finish("title suspended");
    });
    std::thread::spawn(move || {
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

    extern "system" fn noop(_: u8, _: *mut c_void) {}

    #[test]
    fn window_placement_parses() {
        assert_eq!(parse_placement("-1920, 0,1280,720"), Some((-1920, 0, 1280, 720)));
        assert_eq!(parse_placement("0,0,0,720"), None);
        assert_eq!(parse_placement("1,2,3"), None);
        assert_eq!(parse_placement("a,b,c,d"), None);
    }

    #[test]
    fn window_placement_adds_the_frame() {
        let overlapped = wm::WS_OVERLAPPEDWINDOW;
        let mut frame = RECT { left: 0, top: 0, right: 1280, bottom: 720 };
        // SAFETY: a local rectangle.
        unsafe { wm::AdjustWindowRectEx(&mut frame, overlapped, 0, 0) };
        assert!(frame.right - frame.left > 1280 && frame.bottom - frame.top > 720);
    }

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
            wm::SetWindowTextW(hwnd, w!("after"));
            let n = wm::GetWindowTextW(hwnd, text.as_mut_ptr(), 16);
            assert_eq!(String::from_utf16_lossy(&text[..n as usize]), "after");
            assert!(!wm::GetPropW(hwnd, TITLE_PROC).is_null());
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
