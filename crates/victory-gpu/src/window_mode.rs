//! Borderless fullscreen for the window that owns the swap chain.
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging as wm;

const START: u32 = wm::WM_APP + 0x410;
const TOKEN: usize = 0x5649;
const F11: usize = 0x7A;

static WINDOW: AtomicUsize = AtomicUsize::new(0);
static WINDOWED: Mutex<Option<Windowed>> = Mutex::new(None);

struct Windowed {
    placement: wm::WINDOWPLACEMENT,
    style: isize,
    ex_style: isize,
}

pub(crate) fn presenting(hwnd: HWND) {
    WINDOW.store(hwnd.0 as usize, Ordering::Release);
    if std::env::var_os("VICTORY_FULLSCREEN").is_some_and(|value| value == "1") {
        // The owner applies window styles on its message thread.
        if let Err(error) = unsafe { wm::PostMessageW(Some(hwnd), START, WPARAM(TOKEN), LPARAM(0)) }
        {
            tracing::warn!(%error, "could not request fullscreen");
        }
    }
}

/// Handles fullscreen messages for either the title or fallback window.
pub fn window_mode_message(window: usize, message: u32, key: usize, flags: isize) -> bool {
    if WINDOW.load(Ordering::Acquire) != window {
        return false;
    }
    if message == wm::WM_NCDESTROY {
        WINDOW.store(0, Ordering::Release);
        WINDOWED.lock().unwrap_or_else(|e| e.into_inner()).take();
        return false;
    }
    if message == START && key == TOKEN {
        let in_fullscreen = WINDOWED.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        if !in_fullscreen {
            toggle(HWND(window as *mut _));
        }
        return true;
    }
    if key == F11 && matches!(message, wm::WM_KEYDOWN | wm::WM_KEYUP) {
        if message == wm::WM_KEYDOWN && flags & (1 << 30) == 0 {
            toggle(HWND(window as *mut _));
        }
        return true;
    }
    false
}

fn toggle(hwnd: HWND) {
    let previous = WINDOWED.lock().unwrap_or_else(|e| e.into_inner()).take();
    let result = if let Some(previous) = previous {
        match restore(hwnd, &previous) {
            Ok(()) => {
                tracing::info!("restored the windowed game");
                Ok(())
            }
            Err(error) => {
                *WINDOWED.lock().unwrap_or_else(|e| e.into_inner()) = Some(previous);
                Err(error)
            }
        }
    } else {
        enter(hwnd).map(|previous| {
            *WINDOWED.lock().unwrap_or_else(|e| e.into_inner()) = Some(previous);
        })
    };
    if let Err(error) = result {
        tracing::warn!(%error, "could not change the game window mode");
    }
}

fn enter(hwnd: HWND) -> windows::core::Result<Windowed> {
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let mut placement = wm::WINDOWPLACEMENT {
        length: size_of::<wm::WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: the swap chain's window is on this thread, and both structures are initialized.
    unsafe {
        GetMonitorInfoW(
            MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        )
        .ok()?;
        wm::GetWindowPlacement(hwnd, &mut placement)?;
        let style = wm::GetWindowLongPtrW(hwnd, wm::GWL_STYLE);
        let ex_style = wm::GetWindowLongPtrW(hwnd, wm::GWL_EXSTYLE);
        let previous = Windowed {
            placement,
            style,
            ex_style,
        };
        let border = wm::WS_EX_WINDOWEDGE.0
            | wm::WS_EX_CLIENTEDGE.0
            | wm::WS_EX_DLGMODALFRAME.0
            | wm::WS_EX_STATICEDGE.0;
        wm::SetWindowLongPtrW(
            hwnd,
            wm::GWL_STYLE,
            (style & !(wm::WS_OVERLAPPEDWINDOW.0 as isize)) | wm::WS_POPUP.0 as isize,
        );
        wm::SetWindowLongPtrW(hwnd, wm::GWL_EXSTYLE, ex_style & !(border as isize));
        let rect = monitor.rcMonitor;
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        if let Err(error) =
            wm::SetWindowPos(hwnd, None, rect.left, rect.top, width, height, FRAME_FLAGS)
        {
            let _ = restore(hwnd, &previous);
            return Err(error);
        }
        tracing::info!(width, height, "game window entered borderless fullscreen");
        Ok(previous)
    }
}

const FRAME_FLAGS: wm::SET_WINDOW_POS_FLAGS =
    wm::SET_WINDOW_POS_FLAGS(wm::SWP_NOZORDER.0 | wm::SWP_NOACTIVATE.0 | wm::SWP_FRAMECHANGED.0);

fn restore(hwnd: HWND, previous: &Windowed) -> windows::core::Result<()> {
    // SAFETY: we saved the styles and placement from this same window.
    unsafe {
        wm::SetWindowLongPtrW(hwnd, wm::GWL_STYLE, previous.style);
        wm::SetWindowLongPtrW(hwnd, wm::GWL_EXSTYLE, previous.ex_style);
        wm::SetWindowPlacement(hwnd, &previous.placement)?;
        wm::SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            FRAME_FLAGS | wm::SWP_NOMOVE | wm::SWP_NOSIZE,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::RECT;
    use windows::core::w;

    #[test]
    fn fullscreen_fits_the_monitor_and_restores_window_style() {
        // SAFETY: a hidden window created and destroyed on this test's thread.
        unsafe {
            let hwnd = wm::CreateWindowExW(
                wm::WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("VIctory window test"),
                wm::WS_OVERLAPPEDWINDOW,
                80,
                80,
                320,
                240,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let previous = enter(hwnd).unwrap();
            let mut area = RECT::default();
            wm::GetClientRect(hwnd, &mut area).unwrap();
            let mut monitor = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetMonitorInfoW(
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
            .ok()
            .unwrap();
            assert_eq!(area.right, monitor.rcMonitor.right - monitor.rcMonitor.left);
            assert_eq!(
                area.bottom,
                monitor.rcMonitor.bottom - monitor.rcMonitor.top
            );
            restore(hwnd, &previous).unwrap();
            assert_eq!(
                wm::GetWindowLongPtrW(hwnd, wm::GWL_STYLE) & wm::WS_OVERLAPPEDWINDOW.0 as isize,
                previous.style & wm::WS_OVERLAPPEDWINDOW.0 as isize
            );
            wm::DestroyWindow(hwnd).unwrap();
        }
    }
}
