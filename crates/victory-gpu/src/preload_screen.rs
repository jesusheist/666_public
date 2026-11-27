//! The start screen of the preload ("PRELOADING SHADERS"): a window of its own
//! that shows before the title starts (the title's window does not exist yet),
//! in the look of the F10 menu. It paints what [`victory_core::preload::Progress`]
//! says and returns the player's choice:
//!
//! * ENTER: wait until done (the title starts at 100%).
//! * ESC: start the game now (the launcher stops the workers; the next start
//!   goes on from where they were).
//! * The window's close button: quit the launcher.
//!
//! [`render`] is the picture without a window (the tests save it as a PNG).

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use victory_core::preload::{self as pre, Progress, Stage};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::{Canvas, TEXT_DIM, TEXT_SOFT, TITLE, Text, WHITE, accent_line, band, bar, paint_glass, paint_selected, rule};

/// What the screen ended with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The worker finished: the title starts.
    Done,
    /// ESC: the title starts now, the worker is stopped.
    StartNow,
    /// The window was closed: the launcher ends.
    Quit,
}

/// The panel's size at scale 1.
const PANEL: (f32, f32) = (960.0, 492.0);
const PAD: f32 = 28.0;

/// The window's size in pixels.
const WINDOW: (i32, i32) = (1120, 640);

/// The panel for `progress`, painted at `scale`. `waiting`: ENTER was pressed
/// (or `always_wait` is set), the first choice shows as taken.
pub(crate) fn paint(text: &mut Text, p: &Progress, waiting: bool, scale: f32) -> Canvas {
    let u = |v: f32| v * scale;
    let px = |v: f32| (v * scale).round().max(1.0) as i32;
    let mut c = Canvas::new(u(PANEL.0).round() as usize, u(PANEL.1).round() as usize);
    let w = c.w as f32;
    paint_glass(&mut c, scale, 16.0);
    band(&mut c, u(1.0), u(56.0), |_, _| (WHITE, 0.035));
    text.draw_shadowed(&mut c, TITLE, u(PAD), u(10.0), px(30.0), true, WHITE, false);
    text.draw(&mut c, "PRELOADING SHADERS", w - u(PAD), u(16.0), px(22.0), true, WHITE, 1.0, true);
    accent_line(&mut c, scale, u(56.0) - u(1.5));

    // Stage and the whole bar.
    let finished = p.finished();
    let label = if finished { "READY" } else { p.stage.label() };
    text.draw(&mut c, label, u(PAD), u(78.0), px(26.0), true, WHITE, 1.0, false);
    let percent = (p.fraction() * 100.0).floor() as u32;
    text.draw(&mut c, &format!("{percent}%"), w - u(PAD), u(78.0), px(26.0), true, WHITE, 1.0, true);
    bar(&mut c, (u(PAD), u(122.0), w - u(2.0 * PAD), u(16.0)), p.fraction());

    // The numbers, label at the left and value at the right.
    let mut rows: Vec<(&str, String)> = Vec::new();
    rows.push(("SHADERS IN THE CACHE", pre::thousands(p.shader_count)));
    if p.shader_bytes.1 > 0 {
        rows.push(("TITLE FILES GONE THROUGH", format!("{} OF {}", pre::bytes(p.shader_bytes.0), pre::bytes(p.shader_bytes.1))));
    }
    if p.pipelines.1 > 0 || p.stage >= Stage::Pipelines {
        rows.push(("PIPELINES MADE", format!("{} OF {}", pre::thousands(p.pipelines.0), pre::thousands(p.pipelines.1))));
    }
    let left = match (finished, p.eta_secs()) {
        (true, _) => "NONE".to_owned(),
        (false, Some(s)) => pre::span(s),
        (false, None) => "WORKING IT OUT".to_owned(),
    };
    rows.push(("TIME LEFT IN THIS STAGE", left));
    rows.push(("TIME SO FAR", pre::span(p.elapsed())));
    let mut y = 164.0;
    for (name, value) in rows {
        text.draw(&mut c, name, u(PAD), u(y), px(15.0), true, TEXT_DIM, 1.0, false);
        text.draw(&mut c, &value, w - u(PAD), u(y), px(15.0), true, WHITE, 1.0, true);
        y += 26.0;
    }

    // The two choices.
    let choices = [
        ("[ENTER]  WAIT UNTIL DONE", "THE GAME STARTS WHEN THE BAR REACHES 100%"),
        ("[ESC]  START THE GAME NOW", "STOPS THE WORKERS; THE NEXT START GOES ON FROM HERE"),
    ];
    let top = u(PANEL.1 - 150.0);
    rule(&mut c, scale, u(PAD), top - u(10.0), w - u(2.0 * PAD));
    for (i, (name, note)) in choices.into_iter().enumerate() {
        let y = top + u(i as f32 * 44.0);
        let taken = i == 0 && waiting;
        if taken {
            paint_selected(&mut c, scale, (u(PAD - 8.0), y, w - u(2.0 * PAD - 16.0), u(38.0)));
        }
        text.draw(&mut c, name, u(PAD + 10.0), y + u(8.0), px(18.0), true, if taken { WHITE } else { TEXT_SOFT }, 1.0, false);
        text.draw(&mut c, note, w - u(PAD + 10.0), y + u(11.0), px(13.0), true, TEXT_DIM, 1.0, true);
    }
    text.draw(&mut c, "WORKS AT LOW PRIORITY ON AT MOST TWO THIRDS OF THE CPU CORES", u(PAD), u(PANEL.1 - 34.0), px(12.0), true, TEXT_DIM, 0.8, false);
    c
}

/// The whole window as 8-bit BGRA, top row first: a dark backdrop with the panel in the middle.
pub(crate) fn render(text: &mut Text, p: &Progress, waiting: bool, (width, height): (usize, usize)) -> Vec<u8> {
    let scale = ((width as f32 - 80.0) / PANEL.0).min((height as f32 - 80.0) / PANEL.1).clamp(0.5, 3.0);
    let panel = paint(text, p, waiting, scale);
    let (left, top) = ((width.saturating_sub(panel.w)) / 2, (height.saturating_sub(panel.h)) / 2);
    let mut out = vec![0u8; width * height * 4];
    for y in 0..height {
        let t = y as f32 / height as f32;
        for x in 0..width {
            let s = x as f32 / width as f32;
            // A deep blue-black with a violet haze toward the lower right.
            let bg = [12.0 + 22.0 * t * s, 14.0 + 6.0 * t, 22.0 + 30.0 * t * s + 10.0 * s];
            let mut rgb = bg;
            if x >= left && y >= top && x < left + panel.w && y < top + panel.h {
                let q = panel.px[(y - top) * panel.w + (x - left)];
                for i in 0..3 {
                    rgb[i] = q[i] * 255.0 + bg[i] * (1.0 - q[3]);
                }
            }
            let at = (y * width + x) * 4;
            out[at] = rgb[2].clamp(0.0, 255.0) as u8;
            out[at + 1] = rgb[1].clamp(0.0, 255.0) as u8;
            out[at + 2] = rgb[0].clamp(0.0, 255.0) as u8;
            out[at + 3] = 255;
        }
    }
    out
}

/// 0 nothing, 1 ENTER, 2 ESC, 3 the window is to close.
static KEY: AtomicU32 = AtomicU32::new(0);

extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_KEYDOWN => {
            match wparam.0 as u32 {
                0x0D => KEY.store(1, Ordering::Relaxed),
                0x1B => KEY.store(2, Ordering::Relaxed),
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            KEY.store(3, Ordering::Relaxed);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        // SAFETY: the default handling for our window.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn make_window() -> Option<HWND> {
    const CLASS: PCWSTR = w!("VictoryPreload");
    // SAFETY: registers our class and creates a window of it, centred on the primary screen.
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            ..Default::default()
        };
        RegisterClassExW(&class);
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let (ww, wh) = (WINDOW.0.min(sw), WINDOW.1.min(sh));
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("JESUS666 MENU: PRELOADING SHADERS"),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            (sw - ww) / 2,
            (sh - wh) / 2,
            ww,
            wh,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;
        let _ = SetForegroundWindow(hwnd);
        Some(hwnd)
    }
}

fn blit(hwnd: HWND, bgra: &[u8], (w, h): (usize, usize)) {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: the window's own device context, a top-down 32-bit picture of w by h pixels.
    unsafe {
        let dc = GetDC(Some(hwnd));
        StretchDIBits(dc, 0, 0, w as i32, h as i32, 0, 0, w as i32, h as i32, Some(bgra.as_ptr().cast()), &info, DIB_RGB_COLORS, SRCCOPY);
        ReleaseDC(Some(hwnd), dc);
    }
}

/// Shows the screen until the worker is done (`poll` says: it is asked about
/// five times a second for the numbers as they are) or the player chooses.
/// `wait`: the first choice is taken from the start (`always_wait`).
/// Without a window to make (no desktop), the title simply starts: [`Outcome::StartNow`].
pub fn show(wait: bool, mut poll: impl FnMut() -> Progress) -> Outcome {
    let Some(mut text) = Text::new() else { return Outcome::StartNow };
    let Some(hwnd) = make_window() else { return Outcome::StartNow };
    KEY.store(0, Ordering::Relaxed);
    let mut waiting = wait;
    let mut last: Option<Instant> = None;
    let mut redraw = true;
    let outcome = loop {
        let mut message = MSG::default();
        // SAFETY: the standard message pump of this thread's window.
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        match KEY.swap(0, Ordering::Relaxed) {
            1 => {
                waiting = true;
                redraw = true;
            }
            2 => break Outcome::StartNow,
            3 => break Outcome::Quit,
            _ => {}
        }
        if redraw || last.is_none_or(|t| t.elapsed() >= Duration::from_millis(200)) {
            let p = poll();
            let size = (WINDOW.0 as usize, WINDOW.1 as usize);
            let mut client = windows::Win32::Foundation::RECT::default();
            // SAFETY: our window.
            let size = unsafe { GetClientRect(hwnd, &mut client) }
                .map_or(size, |()| ((client.right - client.left).max(64) as usize, (client.bottom - client.top).max(64) as usize));
            blit(hwnd, &render(&mut text, &p, waiting, size), size);
            last = Some(Instant::now());
            redraw = false;
            if p.finished() {
                std::thread::sleep(Duration::from_millis(500));
                break Outcome::Done;
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    // SAFETY: our window, destroyed once.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_core::preload::{Event, Status};

    fn halfway() -> Progress {
        let mut p = Progress::new(&Status { shaders_done: false, todo: 84_000, scope: 84_000 });
        p.apply(&Event::ShaderStage { total: 5 << 30 }, 0.0);
        p.apply(&Event::Shaders { done: 5 << 30, total: 5 << 30, count: 61_204 }, 240.0);
        p.apply(&Event::PipelineStage { total: 84_000 }, 241.0);
        p.apply(&Event::Pipelines { done: 29_000, total: 84_000 }, 781.0);
        p.apply(&Event::Pipelines { done: 31_500, total: 84_000 }, 841.0);
        p
    }

    #[test]
    fn the_screen_paints_stage_bar_counts_and_choices() {
        let mut text = Text::new().unwrap();
        let p = halfway();
        let c = paint(&mut text, &p, false, 1.0);
        assert_eq!((c.w, c.h), (960, 492));
        // Something is painted all over: the rim, the bar, the text.
        assert!(c.px.iter().filter(|q| q[3] > 0.0).count() > c.px.len() / 2);
        // The bar's filled part is more opaque than the empty track right of it.
        let row = (122.0 + 8.0) as usize * c.w;
        let at = |x: usize| c.px[row + x];
        assert!(at(200)[3] > 0.95, "{:?}", at(200));
        assert!(at(c.w - 60)[3] < 0.6, "{:?}", at(c.w - 60));
        // Pressing ENTER changes the picture (the first choice is taken).
        let waiting = paint(&mut text, &p, true, 1.0);
        assert!(waiting.px != c.px);
        // And a bigger scale gives a bigger panel.
        assert_eq!(paint(&mut text, &p, false, 2.0).w, 1920);
    }

    #[test]
    fn the_window_picture_is_opaque_bgra_with_the_panel_in_the_middle() {
        let mut text = Text::new().unwrap();
        let pic = render(&mut text, &halfway(), true, (1120, 640));
        assert_eq!(pic.len(), 1120 * 640 * 4);
        assert!(pic.chunks(4).all(|q| q[3] == 255));
        // The corner is the backdrop, the middle has the panel's pixels: they differ from the backdrop alone.
        let blank = render(&mut text, &Progress::new(&Status { shaders_done: true, todo: 0, scope: 0 }), false, (1120, 640));
        assert!(pic != blank);
        assert_eq!(pic[..16], blank[..16]);
    }

    /// The preview of the screen (written to VICTORY_PRELOAD_PREVIEW_DIR when set): stored, not compressed.
    #[test]
    fn saves_the_preview_pictures() {
        let Some(dir) = std::env::var_os("VICTORY_PRELOAD_PREVIEW_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let mut text = Text::new().unwrap();
        let size = (1280usize, 800usize);
        let shot = |text: &mut Text, p: &Progress, waiting: bool| {
            let bgra = render(text, p, waiting, size);
            let mut rgb = Vec::with_capacity(size.0 * size.1 * 3);
            for q in bgra.chunks(4) {
                rgb.extend([q[2], q[1], q[0]]);
            }
            super::super::pages::tests::encode_png(size.0 as u32, size.1 as u32, &rgb)
        };
        std::fs::write(dir.join("preload-screen.png"), shot(&mut text, &halfway(), true)).unwrap();
        let mut early = Progress::new(&Status { shaders_done: false, todo: 84_000, scope: 84_000 });
        early.apply(&Event::ShaderStage { total: 5 << 30 }, 0.0);
        early.apply(&Event::Shaders { done: 1 << 30, total: 5 << 30, count: 9_870 }, 50.0);
        std::fs::write(dir.join("preload-screen-start.png"), shot(&mut text, &early, false)).unwrap();
    }
}
