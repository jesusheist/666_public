//! The in-game menu (F10; `VICTORY_MENU=0` leaves it out): a panel drawn on
//! the swap chain's final back buffer at every present, in release builds
//! too, over the title's picture and over the two composed planes alike (it
//! is drawn after both, and saved frames, the frame recorder and
//! `screenshot` copy the picture before it is).
//!
//! One menu, titled "JESUS666 MENU", with seven pages shown as tabs at the top
//! and switched with Tab (Shift+Tab goes back):
//! "Home" (frame rate cap, frame generation, the overlay on or off, mouse and
//! keyboard options, and readouts of the card's memory and the frame rate), "Graphics" (the
//! picture switches), "Performance" (the speed switches), "Overlay" (the look of
//! our performance overlay, `host/panel.rs`: corner, size, lines shown),
//! "Cheats" (the entries of [`crate::cheats`], dim where nothing is plugged in
//! yet), "Controls" (keyboard and mouse rebinding, `menu_controls.rs`) and
//! "Advanced" (every tunable of the registry by group, changed live,
//! `menu_live.rs`). The first five are `menu_pages.rs`; Graphics and Performance
//! end with Save and restart, Save and Reset all lines.
//!
//! Keys: F10 (or F1) shows and hides it; Up and Down select, Left and Right change,
//! Enter switches an on/off option, Tab and Shift+Tab change the page, Delete puts
//! a line back to its default (Shift: the page), Esc (or Back) closes it. F2 shows
//! and hides the overlay. While it is open the title does not see those keys
//! ([`takes_key`], asked by the title's keyboard input in `victory-hle`), and a
//! key held when it closed is not seen until it is let go.
//!
//! How it is drawn: the panel is painted on the CPU into a premultiplied RGBA
//! picture (rounded shapes with edges anti-aliased by coverage, text from the
//! system's own font drawn through GDI into a bitmap with grayscale
//! anti-aliasing, so it reads at any size) when its content changes (at most ten
//! times a second), sent to a texture on the card, and drawn onto the back
//! buffer by one small draw. The painting is only the tint, rim, text and
//! accents: the glass is the card's. Before the draw the panel's rectangle of the
//! back buffer is copied into a scratch texture, and the pixel shader blurs it
//! (a wide soft blur), darkens and tints it, adds a fine grain and puts the
//! painted panel on top, like Windows 11's acrylic. Where the copy cannot be done
//! (a format that cannot be copied, a multisampled buffer) the glass is an
//! opaque dark tint. The fade in and out and a slide of a few pixels are the
//! draw's opacity. Closed, the cost of a frame is one key query.
//! Scaled to the window: one scale unit is 1,080 lines of the window's
//! client area on the screen (see [`scale_for_view`]); the picture the title
//! renders may be of another size, it is stretched to the window.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{PCSTR, s, w};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::host::{Host, transition};

#[path = "menu_live.rs"]
mod live;
#[path = "menu_controls.rs"]
mod controls;
#[path = "menu_pages.rs"]
mod pages;

/// The menu's title.
pub(crate) const TITLE: &str = "JESUS666 MENU";

/// The pages of the menu, in the order Tab goes through them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Home,
    Graphics,
    Performance,
    Overlay,
    Cheats,
    Controls,
    Advanced,
}

impl Tab {
    pub(crate) const ALL: [Tab; 7] = [Tab::Home, Tab::Graphics, Tab::Performance, Tab::Overlay, Tab::Cheats, Tab::Controls, Tab::Advanced];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Tab::Home => "HOME",
            Tab::Graphics => "GRAPHICS",
            Tab::Performance => "PERFORMANCE",
            Tab::Overlay => "OVERLAY",
            Tab::Cheats => "CHEATS",
            Tab::Controls => "CONTROLS",
            Tab::Advanced => "ADVANCED",
        }
    }

    pub(crate) fn at(self) -> usize {
        Tab::ALL.iter().position(|&t| t == self).unwrap_or(0)
    }

    /// The next page, going round.
    pub(crate) fn next(self) -> Tab {
        Tab::ALL[(self.at() + 1) % Tab::ALL.len()]
    }

    /// The page before, going round.
    pub(crate) fn prev(self) -> Tab {
        Tab::ALL[(self.at() + Tab::ALL.len() - 1) % Tab::ALL.len()]
    }
}

// ---------------------------------------------------------------------------
// The colours. The panel is smoky glass (the card makes it from the picture
// behind); the accent is the app icon's hue family, saturated: a crimson end,
// pink, magenta and violet. It is used sparingly: the line under the header,
// the active tab, the selected row, arrows, bars.

pub(crate) const CRIMSON: [f32; 3] = [226.0 / 255.0, 29.0 / 255.0, 72.0 / 255.0];
pub(crate) const PINK: [f32; 3] = [244.0 / 255.0, 42.0 / 255.0, 142.0 / 255.0];
pub(crate) const MAGENTA: [f32; 3] = [206.0 / 255.0, 48.0 / 255.0, 200.0 / 255.0];
pub(crate) const VIOLET: [f32; 3] = [128.0 / 255.0, 76.0 / 255.0, 240.0 / 255.0];
/// The gradient's stops, evenly spaced.
pub(crate) const STOPS: [[f32; 3]; 4] = [CRIMSON, PINK, MAGENTA, VIOLET];
/// The accent where it is a single colour (arrows, marks).
pub(crate) const ACCENT: [f32; 3] = [255.0 / 255.0, 70.0 / 255.0, 150.0 / 255.0];
/// For a value that waits and for what the last action said.
pub(crate) const WARM: [f32; 3] = [255.0 / 255.0, 104.0 / 255.0, 116.0 / 255.0];
pub(crate) const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
/// The glass's tint, deep and smoky (#14171b).
pub(crate) const GLASS_TINT: [f32; 3] = [20.0 / 255.0, 23.0 / 255.0, 27.0 / 255.0];
pub(crate) const TEXT: [f32; 3] = [0.97, 0.97, 0.99];
pub(crate) const TEXT_SOFT: [f32; 3] = [0.80, 0.81, 0.85];
pub(crate) const TEXT_DIM: [f32; 3] = [0.52, 0.54, 0.60];

/// The gradient at `t` (0: crimson, 1: violet) as 0 to 1 values.
pub(crate) fn gradient(t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0) * (STOPS.len() - 1) as f32;
    let i = (t.floor() as usize).min(STOPS.len() - 2);
    let f = t - i as f32;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    [0, 1, 2].map(|c| a[c] + (b[c] - a[c]) * f)
}

// ---------------------------------------------------------------------------
// What the menu shows and does, apart from drawing.

/// The numbers the menu shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Readouts {
    pub gpu_used: u64,
    pub gpu_budget: u64,
    pub fps: f32,
    pub frame_ms: f32,
}

const GB: u64 = 1 << 30;

pub(crate) fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GB as f64)
}

/// What a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Toggle,
    Up,
    Down,
    Left,
    Right,
    Enter,
    Close,
}

/// Whether the menu is open. (Which line is selected is each page's.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Controller {
    pub open: bool,
}

impl Controller {
    /// F10 shows and hides it; Esc and Back close it; the other keys are the page's.
    pub(crate) fn step(&mut self, action: Action) {
        match action {
            Action::Toggle => self.open = !self.open,
            Action::Close => self.open = false,
            _ => {}
        }
    }
}

/// The keys the menu uses, in the order [`Keys`] and [`takes_key`] know them.
pub(crate) const KEYS: [u8; 8] = [0x79, 0x26, 0x28, 0x25, 0x27, 0x0D, 0x08, 0x1B];
const F10: usize = 0;

/// Key presses and their repeats, from the keys' states each frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Keys {
    down: [bool; 8],
    repeat_at: [u64; 8],
}

/// How long an arrow is held before it repeats, and the time between repeats,
/// in milliseconds.
const REPEAT_AFTER: u64 = 380;
const REPEAT_EVERY: u64 = 70;

impl Keys {
    /// The actions the keys' states `down` mean at `now` (milliseconds):
    /// F10 always; the others only while the menu is `open`. An arrow held
    /// repeats.
    pub(crate) fn actions(&mut self, now: u64, down: [bool; 8], open: bool) -> Vec<Action> {
        let mut out = Vec::new();
        for i in 0..KEYS.len() {
            let pressed = down[i] && !self.down[i];
            let repeating = down[i] && self.down[i] && (1..=4).contains(&i) && now >= self.repeat_at[i];
            if pressed {
                self.repeat_at[i] = now + REPEAT_AFTER;
            } else if repeating {
                self.repeat_at[i] = now + REPEAT_EVERY;
            }
            if !(pressed || repeating) || !(open || i == F10) {
                continue;
            }
            out.push(match i {
                0 => Action::Toggle,
                1 => Action::Up,
                2 => Action::Down,
                3 => Action::Left,
                4 => Action::Right,
                5 => Action::Enter,
                _ => Action::Close,
            });
        }
        self.down = down;
        out
    }
}

/// Which keys the title must not see: all of them while the menu is open,
/// and any of them still held that were taken. F10 is always taken while the
/// menu exists. Bit `i` is [`KEYS`]`[i]`.
pub(crate) fn latch(open: bool, down: [bool; 8], before: u32) -> u32 {
    let mut out = 0;
    for (i, &d) in down.iter().enumerate() {
        if d && (open || before & (1 << i) != 0) {
            out |= 1 << i;
        }
    }
    out
}

static OPEN: AtomicBool = AtomicBool::new(false);
static LATCH: AtomicU32 = AtomicU32::new(0);
/// F1, the second key for the menu: the title does not see it while the menu
/// is open, nor while a press that opened or closed it is held.
const F1: u8 = 0x70;
static F1_TAKEN: AtomicBool = AtomicBool::new(false);

/// Whether the menu exists (`VICTORY_MENU=0`: it does not).
pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_MENU").is_none_or(|v| v != "0"))
}

/// Whether the menu is open now (the mouse is the user's then, not the title's).
pub(crate) fn is_open() -> bool {
    enabled() && OPEN.load(Ordering::Relaxed)
}

/// Whether the title must not see the key `virtual_key` now. See the module
/// header.
pub(crate) fn takes_key(virtual_key: u8) -> bool {
    if !enabled() {
        return false;
    }
    // The Controls page waiting for a key takes every key, and the ones held after it.
    if controls::takes_key(virtual_key) {
        return true;
    }
    if virtual_key == F1 {
        return OPEN.load(Ordering::Relaxed) || F1_TAKEN.load(Ordering::Relaxed);
    }
    let Some(i) = KEYS.iter().position(|&k| k == virtual_key) else { return live::takes_key(virtual_key, OPEN.load(Ordering::Relaxed)) };
    i == F10 || OPEN.load(Ordering::Relaxed) || LATCH.load(Ordering::Relaxed) & (1 << i) != 0
}

/// Fade in and out: a value from 0 to 1 that moves toward 1 while the menu
/// is open and toward 0 while it is closed, in [`FADE_MS`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Fade {
    pub value: f32,
}

const FADE_MS: f32 = 180.0;

impl Fade {
    pub(crate) fn step(&mut self, open: bool, elapsed_ms: f32) -> f32 {
        let d = elapsed_ms / FADE_MS;
        self.value = if open { (self.value + d).min(1.0) } else { (self.value - d).max(0.0) };
        // Slow at the ends.
        self.value * self.value * (3.0 - 2.0 * self.value)
    }
}

// ---------------------------------------------------------------------------
// Painting.

/// The least scale on the screen: below it the small print of the panels (12
/// pixels at scale 1) is under nine pixels and not readable.
const MIN_SCALE: f32 = 0.75;

/// The scale for a picture `height` lines high: 1 at 1,080, in eighths (the picture taken as the window;
/// the menu itself uses [`scale_for_view`]).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn scale_for(height: u32) -> f32 {
    ((height as f32 / 1080.0).clamp(MIN_SCALE, 4.0) * 8.0).round() / 8.0
}

/// The size in pixels of the window's client area (what is on the screen:
/// the title's picture is stretched to it), if the window answers.
pub(crate) fn client_size(hwnd: HWND) -> Option<(u32, u32)> {
    let mut area = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: a window handle of ours and a structure to fill.
    let ok = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd.0.cast(), &mut area) } != 0;
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    (ok && w > 0 && h > 0).then_some((w as u32, h as u32))
}

/// The scale of a panel `panel` pixels large at scale 1, painted onto a
/// picture of `picture` pixels that is shown in a client area of `client`
/// (the same when it is not known). The panel is the size of the window's
/// height over 1,080 on the screen, as readable at 1,280 by 720 as at 2,560
/// by 1,440 (never below [`MIN_SCALE`] on the screen), in the picture's pixels
/// by the stretch of the picture to the window. It is kept within the picture
/// (its height, and its width with the 48 pixels it stands away from the left
/// edge), in eighths, down to a quarter.
pub(crate) fn scale_for_view(picture: (u32, u32), client: Option<(u32, u32)>, panel: (f32, f32)) -> f32 {
    let client_h = client.map_or(picture.1, |c| c.1).max(1);
    let stretch = picture.1 as f32 / client_h as f32;
    let wanted = (client_h as f32 / 1080.0).clamp(MIN_SCALE, 4.0) * stretch;
    let fit = (picture.1 as f32 * 0.94 / panel.1).min(picture.0 as f32 / (panel.0 + 48.0));
    ((wanted.min(fit) * 8.0).floor() / 8.0).max(0.25)
}

/// A picture of premultiplied colour with alpha, 0 to 1 per channel.
pub(crate) struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 4]>,
    /// The corner radius of the panel's shape in pixels (set by [`paint_glass`]; 0 until then).
    pub corner: f32,
}

impl Canvas {
    pub(crate) fn new(w: usize, h: usize) -> Canvas {
        Canvas { w, h, px: vec![[0.0; 4]; w * h], corner: 0.0 }
    }

    fn over(&mut self, x: i64, y: i64, c: [f32; 3], a: f32) {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 || a <= 0.0 {
            return;
        }
        let d = &mut self.px[y as usize * self.w + x as usize];
        for i in 0..3 {
            d[i] = c[i] * a + d[i] * (1.0 - a);
        }
        d[3] = a + d[3] * (1.0 - a);
    }

    /// Moves a pixel toward colour `c` with alpha `a`, by `cover`: where the
    /// shape is whole, to it.
    fn replace(&mut self, x: i64, y: i64, c: [f32; 3], a: f32, cover: f32) {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 || cover <= 0.0 {
            return;
        }
        let d = &mut self.px[y as usize * self.w + x as usize];
        let target = [c[0] * a, c[1] * a, c[2] * a, a];
        for i in 0..4 {
            d[i] += (target[i] - d[i]) * cover;
        }
    }
}

/// How much of the pixel at (`x`, `y`) a rounded box covers, 0 to 1.
fn coverage(x: f32, y: f32, left: f32, top: f32, w: f32, h: f32, radius: f32) -> f32 {
    let (cx, cy) = (left + w / 2.0, top + h / 2.0);
    let r = radius.min(w / 2.0).min(h / 2.0);
    let qx = (x + 0.5 - cx).abs() - (w / 2.0 - r);
    let qy = (y + 0.5 - cy).abs() - (h / 2.0 - r);
    let outside = (qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0)).sqrt();
    let d = outside + qx.max(qy).min(0.0) - r;
    (0.5 - d).clamp(0.0, 1.0)
}

/// How a shape is put on the picture.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    Over,
    Replace,
}

/// A rounded box painted by `colour(x, y)` (the pixel's place in the box, 0
/// to 1 each way) and its alpha.
pub(crate) fn rounded(
    c: &mut Canvas,
    (left, top, w, h): (f32, f32, f32, f32),
    radius: f32,
    mode: Mode,
    colour: impl Fn(f32, f32) -> ([f32; 3], f32),
) {
    let (x0, x1) = ((left.floor() as i64).max(0), ((left + w).ceil() as i64).min(c.w as i64));
    let (y0, y1) = ((top.floor() as i64).max(0), ((top + h).ceil() as i64).min(c.h as i64));
    for y in y0..y1 {
        for x in x0..x1 {
            let cover = coverage(x as f32, y as f32, left, top, w, h, radius);
            if cover <= 0.0 {
                continue;
            }
            let (col, a) = colour((x as f32 + 0.5 - left) / w, (y as f32 + 0.5 - top) / h);
            match mode {
                Mode::Over => c.over(x, y, col, a * cover),
                Mode::Replace => c.replace(x, y, col, a, cover),
            }
        }
    }
}

/// Text through GDI: the system's own font, grayscale anti-aliasing, drawn
/// white on black into a bitmap whose brightness is the coverage.
pub(crate) struct Text {
    dc: windows::Win32::Graphics::Gdi::HDC,
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
    bits: *mut u32,
    w: i32,
    h: i32,
    fonts: HashMap<(i32, bool), windows::Win32::Graphics::Gdi::HFONT>,
}

// SAFETY: the bitmap and device context are used by one thread at a time (the
// presenting thread, under the presenter's lock; tests make their own).
unsafe impl Send for Text {}

impl Text {
    pub(crate) fn new() -> Option<Text> {
        use windows::Win32::Graphics::Gdi::*;
        // SAFETY: plain creation of a memory context.
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return None;
        }
        let mut text = Text { dc, bitmap: HBITMAP::default(), bits: std::ptr::null_mut(), w: 0, h: 0, fonts: HashMap::new() };
        text.grow(768, 128)?;
        Some(text)
    }

    fn grow(&mut self, w: i32, h: i32) -> Option<()> {
        use windows::Win32::Graphics::Gdi::*;
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        // SAFETY: a top-down 32-bit section; the pointer it gives is ours until the bitmap is deleted.
        unsafe {
            let bitmap = CreateDIBSection(Some(self.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            SelectObject(self.dc, bitmap.into());
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
            self.bitmap = bitmap;
        }
        (self.bits, self.w, self.h) = (bits.cast(), w, h);
        Some(())
    }

    fn font(&mut self, px: i32, bold: bool) -> windows::Win32::Graphics::Gdi::HFONT {
        use windows::Win32::Graphics::Gdi::*;
        *self.fonts.entry((px, bold)).or_insert_with(|| {
            // SAFETY: plain creation; "Segoe UI" is on every supported Windows, and GDI substitutes if it is not.
            unsafe {
                CreateFontW(
                    -px,
                    0,
                    0,
                    0,
                    if bold { 700 } else { 400 },
                    0,
                    0,
                    0,
                    FONT_CHARSET(1),
                    FONT_OUTPUT_PRECISION(4),
                    FONT_CLIP_PRECISION(0),
                    FONT_QUALITY(4),
                    0,
                    w!("Segoe UI"),
                )
            }
        })
    }

    /// The size of `text` in pixels.
    pub(crate) fn measure(&mut self, text: &str, px: i32, bold: bool) -> (i32, i32) {
        use windows::Win32::Foundation::SIZE;
        use windows::Win32::Graphics::Gdi::*;
        let font = self.font(px, bold);
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut size = SIZE::default();
        // SAFETY: our context, a font we made, a slice of UTF-16.
        unsafe {
            SelectObject(self.dc, font.into());
            let _ = GetTextExtentPoint32W(self.dc, &wide, &mut size);
        }
        (size.cx, size.cy)
    }

    /// The coverage of `text`, 0 to 1, row by row: its width and height.
    fn coverage(&mut self, text: &str, px: i32, bold: bool) -> Option<(Vec<f32>, usize, usize)> {
        use windows::Win32::Graphics::Gdi::*;
        let (w, h) = self.measure(text, px, bold);
        let (w, h) = (w + 2, h + 2);
        if w > self.w || h > self.h {
            self.grow(w.max(self.w), h.max(self.h))?;
        }
        let font = self.font(px, bold);
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: the bitmap is `self.w` by `self.h` pixels of 4 bytes, the text fits in it.
        unsafe {
            std::ptr::write_bytes(self.bits, 0, (self.w * self.h) as usize);
            SelectObject(self.dc, font.into());
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(self.dc, windows::Win32::Foundation::COLORREF(0x00FF_FFFF));
            let _ = TextOutW(self.dc, 1, 1, &wide);
            let _ = GdiFlush();
        }
        let mut out = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                // SAFETY: inside the bitmap.
                let v = unsafe { *self.bits.add((y * self.w + x) as usize) };
                out.push(((v >> 8) & 0xFF) as f32 / 255.0);
            }
        }
        Some((out, w as usize, h as usize))
    }

    /// Draws `text` with its top left at (`x`, `y`), or its top right there
    /// with `right`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(&mut self, c: &mut Canvas, text: &str, x: f32, y: f32, px: i32, bold: bool, colour: [f32; 3], alpha: f32, right: bool) {
        let Some((cover, w, h)) = self.coverage(text, px, bold) else { return };
        let left = if right { x - (w as f32 - 2.0) } else { x };
        let (left, top) = ((left - 1.0).round() as i64, (y - 1.0).round() as i64);
        for j in 0..h {
            for i in 0..w {
                let v = cover[j * w + i];
                if v > 0.0 {
                    c.over(left + i as i64, top + j as i64, colour, v * alpha);
                }
            }
        }
    }

    /// With a soft shadow beneath.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_shadowed(&mut self, c: &mut Canvas, text: &str, x: f32, y: f32, px: i32, bold: bool, colour: [f32; 3], right: bool) {
        let d = (px as f32 / 14.0).max(1.0);
        self.draw(c, text, x + d, y + d, px, bold, [0.0, 0.0, 0.0], 0.45, right);
        self.draw(c, text, x, y, px, bold, colour, 1.0, right);
    }
}

impl Drop for Text {
    fn drop(&mut self) {
        use windows::Win32::Graphics::Gdi::*;
        // SAFETY: objects we made, deleted once; the context is no longer drawn on.
        unsafe {
            for (_, font) in self.fonts.drain() {
                let _ = DeleteObject(font.into());
            }
            let _ = DeleteDC(self.dc);
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The look: shared by every page and by the performance overlay
// (`host/panel.rs`). Pictures are the panel's own size; the card supplies the
// glass (see the module header), so the painter lays down only a thin tint,
// the rim, highlights, text and accents.

/// How far a pixel is inside a rounded box's edge (negative outside).
fn inside(x: f32, y: f32, w: f32, h: f32, radius: f32) -> f32 {
    let (cx, cy) = (w / 2.0, h / 2.0);
    let r = radius.min(cx).min(cy);
    let qx = (x + 0.5 - cx).abs() - (cx - r);
    let qy = (y + 0.5 - cy).abs() - (cy - r);
    let outside = (qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0)).sqrt();
    -(outside + qx.max(qy).min(0.0) - r)
}

/// A band of rows `y0` to `y1` inside the panel's rounded shape, painted by
/// `colour(x, t)` (`x` 0 to 1 across, `t` 0 to 1 down the band) and its alpha.
pub(crate) fn band(c: &mut Canvas, y0: f32, y1: f32, colour: impl Fn(f32, f32) -> ([f32; 3], f32)) {
    let (w, h, corner) = (c.w as f32, c.h as f32, c.corner);
    let (from, to) = ((y0.floor().max(0.0)) as usize, (y1.ceil().max(0.0) as usize).min(c.h));
    for y in from..to {
        let t = ((y as f32 + 0.5 - y0) / (y1 - y0).max(1e-3)).clamp(0.0, 1.0);
        for x in 0..c.w {
            let cover = coverage(x as f32, y as f32, 0.0, 0.0, w, h, corner);
            if cover > 0.0 {
                let (col, a) = colour((x as f32 + 0.5) / w, t);
                c.over(x as i64, y as i64, col, a * cover);
            }
        }
    }
}

/// The glass panel's painted part: a 1 pixel light rim (14 percent white at
/// the top, fading down), a thin smoky tint inside it, a soft edge falloff
/// where a drop shadow would be (the picture is no bigger than the panel) and a
/// soft highlight along the top. `radius` is the corner's, at scale 1.
pub(crate) fn paint_glass(c: &mut Canvas, scale: f32, radius: f32) {
    let u = |v: f32| v * scale;
    let (w, h) = (c.w as f32, c.h as f32);
    let r = u(radius);
    c.corner = r;
    let rim = u(1.0).max(1.0);
    rounded(c, (0.0, 0.0, w, h), r, Mode::Over, |_, y| (WHITE, 0.16 - 0.11 * y));
    rounded(c, (rim, rim, w - 2.0 * rim, h - 2.0 * rim), r - rim, Mode::Replace, |_, y| (GLASS_TINT, 0.12 + 0.08 * y));
    // The falloff inside the rim.
    let depth = u(9.0);
    for y in 0..c.h {
        let edge_y = y.min(c.h - 1 - y) as f32;
        for x in 0..c.w {
            if (x.min(c.w - 1 - x) as f32).min(edge_y) > depth + r {
                continue;
            }
            let d = inside(x as f32, y as f32, w, h, r);
            if d > rim && d < depth {
                let k = 1.0 - (d - rim) / (depth - rim);
                c.over(x as i64, y as i64, [0.0, 0.0, 0.0], 0.26 * k * k);
            }
        }
    }
    band(c, 0.0, u(40.0), |_, t| (WHITE, 0.07 * (1.0 - t) * (1.0 - t)));
}

/// A thin accent line from the left to the right of the panel, `y` down.
pub(crate) fn accent_line(c: &mut Canvas, scale: f32, y: f32) {
    let u = |v: f32| v * scale;
    let w = c.w as f32;
    rounded(c, (u(14.0), y, w - u(28.0), u(1.5).max(1.0)), u(0.7), Mode::Over, |x, _| (gradient(x), 0.90 * (x * 8.0).min((1.0 - x) * 8.0).min(1.0)));
}

/// A faint line, for the footer.
pub(crate) fn rule(c: &mut Canvas, scale: f32, x: f32, y: f32, w: f32) {
    rounded(c, (x, y, w, scale.max(1.0)), 0.0, Mode::Over, |_, _| (WHITE, 0.10));
}

/// The title, the tabs, the accent line under the header, and at the right
/// `group` (large) and `tally` (small) where they are not empty.
pub(crate) fn chrome(text: &mut Text, c: &mut Canvas, scale: f32, header: f32, active: Tab, group: &str, tally: &str) {
    let u = |v: f32| v * scale;
    let px = |v: f32| (v * scale).round().max(1.0) as i32;
    paint_glass(c, scale, 16.0);
    let w = c.w as f32;
    band(c, u(1.0), u(header), |_, _| (WHITE, 0.035));
    text.draw_shadowed(c, TITLE, u(live::PAD), u(8.0), px(30.0), true, WHITE, false);
    paint_tabs(text, c, scale, u(live::PAD + 2.0), u(48.0), active);
    accent_line(c, scale, u(header) - u(1.5));
    if !group.is_empty() {
        text.draw(c, group, w - u(live::PAD), u(10.0), px(22.0), true, WHITE, 1.0, true);
    }
    if !tally.is_empty() {
        text.draw(c, tally, w - u(live::PAD), u(48.0), px(14.0), true, TEXT_SOFT, 1.0, true);
    }
}

/// The selected row: a translucent accent fill and a solid accent pill at its
/// left edge.
pub(crate) fn paint_selected(c: &mut Canvas, scale: f32, (left, y, w, h): (f32, f32, f32, f32)) {
    let u = |v: f32| v * scale;
    rounded(c, (left, y, w, h), u(9.0), Mode::Over, |x, _| (gradient(x * 0.55), 0.34 - 0.16 * x));
    rounded(c, (left + u(8.0), y + u(1.0), w - u(16.0), u(1.0).max(1.0)), 0.0, Mode::Over, |_, _| (WHITE, 0.10));
    rounded(c, (left + u(1.0), y + u(6.0), u(3.0), h - u(12.0)), u(1.5), Mode::Over, |_, yy| (gradient(yy * 0.6), 1.0));
}

/// The scroll bar at the right of a list: `rows` lines, `visible` of them in view from `scroll`.
pub(crate) fn scrollbar(c: &mut Canvas, scale: f32, (x, y, h): (f32, f32, f32), rows: usize, visible: usize, scroll: usize) {
    if rows <= visible {
        return;
    }
    let u = |v: f32| v * scale;
    rounded(c, (x, y, u(3.0), h), u(1.5), Mode::Over, |_, _| (WHITE, 0.10));
    let (thumb, travel) = (h * visible as f32 / rows as f32, h * (1.0 - visible as f32 / rows as f32));
    let at = y + travel * scroll as f32 / (rows - visible) as f32;
    rounded(c, (x, at, u(3.0), thumb), u(1.5), Mode::Over, |_, yy| (gradient(yy), 1.0));
}

/// A bar: a dark track and a saturated accent fill to `fraction`.
pub(crate) fn bar(c: &mut Canvas, (left, top, w, h): (f32, f32, f32, f32), fraction: f32) {
    rounded(c, (left, top, w, h), h / 2.0, Mode::Over, |_, _| ([0.0, 0.0, 0.0], 0.40));
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let filled = (w * f).max(h);
        rounded(c, (left, top, filled, h), h / 2.0, Mode::Over, |x, _| (gradient(x * f), 1.0));
    }
}

/// The tab strip: every page's name as text, the active one white with an
/// accent underline, with its top left at (`x`, `y`) in pixels.
pub(crate) fn paint_tabs(text: &mut Text, c: &mut Canvas, scale: f32, x: f32, y: f32, active: Tab) {
    let px = (15.0 * scale).round().max(1.0) as i32;
    let (pad, gap) = (8.0 * scale, 6.0 * scale);
    let mut at = x;
    for tab in Tab::ALL {
        let label = tab.label();
        let (w, _) = text.measure(label, px, true);
        let w = w as f32;
        if tab == active {
            text.draw(c, label, at, y, px, true, WHITE, 1.0, false);
            rounded(c, (at - 2.0 * scale, y + 23.0 * scale, w + 4.0 * scale, 2.5 * scale), 1.2 * scale, Mode::Over, |x, _| (gradient(x), 1.0));
        } else {
            text.draw(c, label, at, y, px, true, TEXT_SOFT, 0.62, false);
        }
        at += w + 2.0 * pad + gap;
    }
}

/// The glass, as a number of the card's shader: how the picture under the
/// panel becomes the panel's body. The same figures are in [`SOURCE`].
#[cfg(test)]
pub(crate) mod glass {
    /// The blurred picture is darkened to this much, then mixed to the tint.
    pub const DARKEN: f32 = 0.72;
    pub const TINT_MIX: f32 = 0.60;
    pub const SATURATION: f32 = 1.25;
    /// Fine static grain, as a share of full brightness (plus and minus half of it).
    pub const GRAIN: f32 = 0.04;
}

impl Canvas {
    /// The picture as 8-bit premultiplied RGBA, row by row.
    pub(crate) fn rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            out.extend(p.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
        }
        out
    }

    #[cfg(test)]
    /// The picture laid over `backdrop` (8-bit RGB, `bw` wide) with its top
    /// left at (`x`, `y`), `opacity` and the window's `scale`: what the card
    /// makes of it. The shader's glass is emulated here: the picture under the
    /// panel is blurred (two box blurs, close to the shader's wide soft blur),
    /// made a little more saturated, darkened and mixed with the smoky tint, a
    /// fine grain is added, and the painted panel goes on top.
    pub(crate) fn composite(&self, backdrop: &mut [u8], bw: usize, x: usize, y: usize, opacity: f32, scale: f32) {
        let bh = backdrop.len() / 3 / bw;
        let (w, h) = (self.w, self.h);
        // What is under the panel, the edge repeating (as the card's clamped copy does).
        let mut under = vec![[0f32; 3]; w * h];
        for j in 0..h {
            for i in 0..w {
                let at = (((y + j).min(bh - 1)) * bw + (x + i).min(bw - 1)) * 3;
                under[j * w + i] = [0, 1, 2].map(|k| f32::from(backdrop[at + k]) / 255.0);
            }
        }
        let r = ((14.0 * scale).round() as usize).max(1);
        for _ in 0..2 {
            box_blur(&mut under, w, h, r);
        }
        for j in 0..h {
            for i in 0..w {
                let (bx, by) = (x + i, y + j);
                if bx >= bw || by >= bh {
                    continue;
                }
                let p = self.px[j * w + i];
                let a = p[3] * opacity;
                let m = coverage(i as f32, j as f32, 0.0, 0.0, w as f32, h as f32, self.corner) * opacity;
                let b = under[j * w + i];
                let l = 0.2126 * b[0] + 0.7152 * b[1] + 0.0722 * b[2];
                let grain = (hash(i, j) - 0.5) * glass::GRAIN;
                let at = (by * bw + bx) * 3;
                for k in 0..3 {
                    let sat = (l + (b[k] - l) * glass::SATURATION).max(0.0);
                    let g = (sat * glass::DARKEN + (GLASS_TINT[k] - sat * glass::DARKEN) * glass::TINT_MIX + grain).clamp(0.0, 1.0);
                    let dst = f32::from(backdrop[at + k]) / 255.0;
                    let total_a = a + m * (1.0 - a);
                    let v = p[k] * opacity + g * m * (1.0 - a) + dst * (1.0 - total_a);
                    backdrop[at + k] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
        }
    }
}

/// A fixed grain for a pixel, 0 to 1 (tests).
#[cfg(test)]
fn hash(i: usize, j: usize) -> f32 {
    let g = (i as u32).wrapping_mul(73_856_093) ^ (j as u32).wrapping_mul(19_349_663);
    let g = (g ^ (g >> 13)).wrapping_mul(0x5bd1_e995);
    ((g >> 8) & 0xFFFF) as f32 / 65535.0
}

/// A box blur of radius `r` along both axes, the edge repeating (tests).
#[cfg(test)]
fn box_blur(px: &mut [[f32; 3]], w: usize, h: usize, r: usize) {
    let n = (2 * r + 1) as f32;
    let ri = r as i64;
    let mut line = vec![[0f32; 3]; w.max(h)];
    for pass in 0..2 {
        let (len, lines) = if pass == 0 { (w, h) } else { (h, w) };
        let index = |l: usize, k: usize| if pass == 0 { l * w + k } else { k * w + l };
        for l in 0..lines {
            for k in 0..len {
                line[k] = px[index(l, k)];
            }
            let at = |k: i64| line[k.clamp(0, len as i64 - 1) as usize];
            let mut acc = [0f32; 3];
            for k in -ri..=ri {
                let s = at(k);
                (0..3).for_each(|c| acc[c] += s[c]);
            }
            for k in 0..len {
                px[index(l, k)] = acc.map(|v| v / n);
                let (add, sub) = (at(k as i64 + ri + 1), at(k as i64 - ri));
                (0..3).for_each(|c| acc[c] += add[c] - sub[c]);
            }
        }
    }
}


// ---------------------------------------------------------------------------
// On the card.

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=12, b0), DescriptorTable(SRV(t0, numDescriptors=2), visibility=SHADER_VISIBILITY_PIXEL), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP, visibility=SHADER_VISIBILITY_PIXEL)"
cbuffer C : register(b0) {
    float4 rect;   // left, top, width, height in pixels
    float4 target; // target width, height, opacity, gain for a linear target
    float4 misc;   // x: 1 for a linear (scRGB) target, y: corner radius, z: blur radius (pixels), w: 1 when the picture under the panel was copied
};
Texture2D<float4> tex : register(t0);    // the painted panel, premultiplied
Texture2D<float4> behind : register(t1); // the target's pixels under the panel, copied
SamplerState smp : register(s0);
struct V { float4 at : SV_Position; float2 uv : TEXCOORD0; };
[RootSignature(RS)]
V vs(uint id : SV_VertexID) {
    float2 c = float2(id & 1, (id >> 1) & 1);
    float2 p = rect.xy + c * rect.zw;
    V v;
    v.uv = c;
    v.at = float4(p.x / target.x * 2 - 1, 1 - p.y / target.y * 2, 0, 1);
    return v;
}
float sdbox(float2 p, float2 h, float r) {
    float2 q = abs(p) - (h - r);
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}
float ign(float2 p) { return frac(52.9829189 * frac(dot(p, float2(0.06711056, 0.00583715)))); }
float grain(float2 p) {
    float3 q = frac(float3(p.xyx) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return frac((q.x + q.y) * q.z);
}
[RootSignature(RS)]
float4 ps(V v) : SV_Target {
    float2 size = rect.zw;
    float2 p = v.uv * size;
    float cover = saturate(0.5 - sdbox(p - size * 0.5, size * 0.5, misc.y));
    bool lin = misc.x > 0.5;
    // Glass, in display (sRGB-like) values: deep smoky tint, #14171b.
    float3 glass = float3(0.0784, 0.0902, 0.1059);
    if (misc.w > 0.5) {
        // A wide soft blur of the picture under the panel: 24 taps on a spiral,
        // turned a little at every pixel so the taps' pattern is not seen.
        float3 sum = 0;
        float total = 0;
        float turn = ign(v.at.xy) * 6.2831853;
        [unroll] for (int i = 0; i < 24; i++) {
            float rr = sqrt((i + 0.5) / 24.0);
            float a = i * 2.3999632 + turn;
            float w = exp(-1.6 * rr * rr);
            float2 o = float2(cos(a), sin(a)) * (rr * misc.z);
            sum += behind.SampleLevel(smp, (p + o) / size, 0).rgb * w;
            total += w;
        }
        float3 b = sum / total;
        if (lin) b = pow(max(b / target.w, 0.0), 1.0 / 2.2);
        float l = dot(b, float3(0.2126, 0.7152, 0.0722));
        b = max(lerp(l.xxx, b, 1.25), 0.0);
        glass = lerp(b * 0.72, glass, 0.60);
    }
    glass += (grain(v.at.xy) - 0.5) * 0.04;
    glass = saturate(glass);
    if (lin) glass = pow(glass, 2.2) * target.w;
    float4 t = tex.SampleLevel(smp, v.uv, 0);
    t *= target.z;
    if (lin) {
        float3 s = t.rgb / max(t.a, 1e-4);
        s = pow(max(s, 0), 2.2) * target.w;
        t.rgb = s * t.a;
    }
    // The glass over what is there, the painted panel over the glass.
    float g = cover * target.z;
    return float4(t.rgb + glass * g * (1 - t.a), t.a + g * (1 - t.a));
}
"#;

fn compile(entry: PCSTR, target: PCSTR) -> windows::core::Result<ID3DBlob> {
    let mut code: Option<ID3DBlob> = None;
    // SAFETY: the source and names live through the call; the out pointer is ours.
    unsafe {
        D3DCompile(SOURCE.as_ptr().cast(), SOURCE.len(), s!("menu"), None, None, entry, target, 0, 0, &mut code, None)?;
    }
    code.ok_or_else(windows::core::Error::empty)
}

/// Texture uploads in turn: the card reads the others, at least this many
/// updates (a tenth of a second apart) old.
const UPLOADS: usize = 8;

/// The panel's texture, its pipeline, and what draws it onto a back buffer.
pub(crate) struct Overlay {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    format: DXGI_FORMAT,
    /// Two views: the panel's texture, and the copy of the picture under it.
    views: ID3D12DescriptorHeap,
    view_step: u32,
    texture: ID3D12Resource,
    size: (u32, u32),
    pitch: u32,
    uploads: Vec<(ID3D12Resource, *mut u8)>,
    turn: usize,
    /// The newest picture is not on the card yet.
    pending: bool,
    /// The texture was made to be copied into and has not been drawn from.
    fresh: bool,
    /// Something was copied into it: there is a picture to draw.
    filled: bool,
    /// The panel's corner radius and the blur's radius, in pixels.
    radius: f32,
    blur: f32,
    /// The copy of the picture under the panel, made at the first draw; none where it cannot be.
    scratch: Option<ID3D12Resource>,
    /// The copy was made to be copied into and has not been drawn from.
    scratch_fresh: bool,
    scratch_failed: bool,
}

// SAFETY: free-threaded resources, and memory only written under the presenter's lock.
unsafe impl Send for Overlay {}

impl Overlay {
    pub(crate) fn new(host: &Host, format: DXGI_FORMAT, size: (u32, u32)) -> windows::core::Result<Overlay> {
        let (vs, ps) = (compile(s!("vs"), s!("vs_5_1"))?, compile(s!("ps"), s!("ps_5_1"))?);
        let device = &host.device;
        // SAFETY: the blobs' own bytes, alive through the calls; plain object creation.
        unsafe {
            let bytes = |b: &ID3DBlob| std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize());
            let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes(&vs))?;
            let code = |b: &ID3DBlob| D3D12_SHADER_BYTECODE { pShaderBytecode: b.GetBufferPointer(), BytecodeLength: b.GetBufferSize() };
            let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                VS: code(&vs),
                PS: code(&ps),
                SampleMask: u32::MAX,
                RasterizerState: D3D12_RASTERIZER_DESC { FillMode: D3D12_FILL_MODE_SOLID, CullMode: D3D12_CULL_MODE_NONE, ..Default::default() },
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: 1,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                ..Default::default()
            };
            desc.RTVFormats[0] = format;
            // Premultiplied: source + destination * (1 - source alpha).
            desc.BlendState.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D12_BLEND_ONE,
                DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D12_BLEND_OP_ADD,
                SrcBlendAlpha: D3D12_BLEND_ONE,
                DestBlendAlpha: D3D12_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D12_BLEND_OP_ADD,
                LogicOpEnable: false.into(),
                LogicOp: D3D12_LOGIC_OP_NOOP,
                RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let pso = device.CreateGraphicsPipelineState(&desc)?;
            let views: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: 2,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })?;
            let view_step = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            let tex_desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: u64::from(size.0),
                Height: size.1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                ..Default::default()
            };
            let mut texture: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
                D3D12_HEAP_FLAG_NONE,
                &tex_desc,
                D3D12_RESOURCE_STATE_COPY_DEST,
                None,
                &mut texture,
            )?;
            let texture = texture.ok_or_else(windows::core::Error::empty)?;
            let view = |format: DXGI_FORMAT| D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: format,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() } },
            };
            let first = views.GetCPUDescriptorHandleForHeapStart();
            device.CreateShaderResourceView(&texture, Some(&view(DXGI_FORMAT_R8G8B8A8_UNORM)), first);
            // The second view is a null one until the copy of the picture under the panel exists.
            let second = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: first.ptr + view_step as usize };
            device.CreateShaderResourceView(None::<&ID3D12Resource>, Some(&view(format)), second);
            let pitch = (size.0 * 4).next_multiple_of(256);
            let bytes = u64::from(pitch) * u64::from(size.1);
            let uploads = (0..UPLOADS).map(|_| host.upload_buffer(bytes)).collect::<Option<Vec<_>>>().ok_or_else(windows::core::Error::empty)?;
            Ok(Overlay {
                root,
                pso,
                format,
                views,
                view_step,
                texture,
                size,
                pitch,
                uploads,
                turn: 0,
                pending: false,
                fresh: true,
                filled: false,
                radius: 16.0,
                blur: 24.0,
                scratch: None,
                scratch_fresh: true,
                scratch_failed: false,
            })
        }
    }

    pub(crate) fn matches(&self, format: DXGI_FORMAT, size: (u32, u32)) -> bool {
        self.format == format && self.size == size
    }

    /// The panel's corner radius and the blur's radius, in pixels of the picture.
    pub(crate) fn look(&mut self, radius: f32, blur: f32) {
        (self.radius, self.blur) = (radius, blur);
    }

    /// Puts a new picture (8-bit premultiplied RGBA of the texture's size)
    /// into the next upload buffer.
    pub(crate) fn write(&mut self, rgba: &[u8]) {
        let row = self.size.0 as usize * 4;
        if rgba.len() < row * self.size.1 as usize {
            return;
        }
        self.turn = (self.turn + 1) % UPLOADS;
        let to = self.uploads[self.turn].1;
        for y in 0..self.size.1 as usize {
            // SAFETY: a mapped upload buffer of `pitch * height` bytes, which the card read a second or more ago.
            unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr().add(y * row), to.add(y * self.pitch as usize), row) };
        }
        self.pending = true;
    }

    /// Copies the panel's rectangle of `back` (in the copy-destination state)
    /// into the scratch texture the shader blurs, and leaves `back` in the
    /// copy-source state. False, with `back` as it was, where it cannot be done
    /// (the shader then makes an opaque dark glass).
    ///
    /// # Safety
    /// An open `list` and live resources.
    unsafe fn grab(&mut self, host: &Host, list: &ID3D12GraphicsCommandList, back: &ID3D12Resource, (width, height): (u32, u32), (left, top): (u32, u32)) -> bool {
        if self.scratch_failed || left >= width || top >= height {
            return false;
        }
        // SAFETY: the caller's guarantees; plain object creation and commands.
        unsafe {
            if self.scratch.is_none() {
                let have = back.GetDesc();
                if have.SampleDesc.Count != 1 || have.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D {
                    self.scratch_failed = true;
                    return false;
                }
                // The copy takes the buffer's own format (a typed view of it is what the shader reads).
                let want = D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                    Width: u64::from(self.size.0),
                    Height: self.size.1,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    Format: have.Format,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    ..Default::default()
                };
                let mut made: Option<ID3D12Resource> = None;
                let ok = host
                    .device
                    .CreateCommittedResource(
                        &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
                        D3D12_HEAP_FLAG_NONE,
                        &want,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                        None,
                        &mut made,
                    )
                    .is_ok();
                let Some(made) = made.filter(|_| ok) else {
                    self.scratch_failed = true;
                    return false;
                };
                let at = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: self.views.GetCPUDescriptorHandleForHeapStart().ptr + self.view_step as usize };
                host.device.CreateShaderResourceView(
                    &made,
                    Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                        Format: self.format,
                        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() } },
                    }),
                    at,
                );
                self.scratch = Some(made);
                self.scratch_fresh = true;
            }
            let Some(scratch) = self.scratch.as_ref() else { return false };
            let (w, h) = (self.size.0.min(width - left), self.size.1.min(height - top));
            if !std::mem::take(&mut self.scratch_fresh) {
                list.ResourceBarrier(&[transition(scratch, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST)]);
            }
            list.ResourceBarrier(&[transition(back, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            let to = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(scratch),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(back),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            let region = D3D12_BOX { left, top, front: 0, right: left + w, bottom: top + h, back: 1 };
            list.CopyTextureRegion(&to, 0, 0, 0, &from, Some(&region));
            list.ResourceBarrier(&[transition(scratch, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)]);
        }
        true
    }

    /// Records the copy of a new picture, if there is one, and the draw of
    /// the panel with its top left at (`left`, `top`) onto `back` (in the
    /// copy-destination state, and left in it) at `opacity`.
    pub(crate) fn record(
        &mut self,
        host: &Host,
        list: &ID3D12GraphicsCommandList,
        back: &ID3D12Resource,
        (width, height): (u32, u32),
        (left, top): (f32, f32),
        opacity: f32,
    ) {
        if !self.filled && !self.pending {
            return;
        }
        // SAFETY: an open list and live resources; the panel lies where it is drawn.
        unsafe {
            if self.pending {
                self.pending = false;
                if !std::mem::take(&mut self.fresh) {
                    list.ResourceBarrier(&[transition(&self.texture, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST)]);
                }
                let from = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: std::mem::transmute_copy(&self.uploads[self.turn].0),
                    Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                        PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                            Offset: 0,
                            Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                                Width: self.size.0,
                                Height: self.size.1,
                                Depth: 1,
                                RowPitch: self.pitch,
                            },
                        },
                    },
                };
                let to = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: std::mem::transmute_copy(&self.texture),
                    Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
                };
                list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
                list.ResourceBarrier(&[transition(&self.texture, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)]);
                self.filled = true;
            }
            if !self.filled {
                return;
            }
            let (left, top) = (left.round(), top.round());
            // The picture under the panel, for the glass; the buffer then goes from copy source to render target.
            let glass = self.grab(host, list, back, (width, height), (left.max(0.0) as u32, top.max(0.0) as u32));
            let was = if glass { D3D12_RESOURCE_STATE_COPY_SOURCE } else { D3D12_RESOURCE_STATE_COPY_DEST };
            list.ResourceBarrier(&[transition(back, was, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
            let rtv = host.rtv(Some(back), None);
            list.OMSetRenderTargets(1, Some(&rtv), false, None);
            list.SetDescriptorHeaps(&[Some(self.views.clone())]);
            list.SetGraphicsRootSignature(&self.root);
            list.SetPipelineState(&self.pso);
            // A float target is linear: the picture's sRGB values are put
            // through the curve and given about 200 nits (2.5 of 80).
            let linear = self.format == DXGI_FORMAT_R16G16B16A16_FLOAT;
            let constants: [f32; 12] = [
                left,
                top,
                self.size.0 as f32,
                self.size.1 as f32,
                width as f32,
                height as f32,
                opacity,
                2.5,
                if linear { 1.0 } else { 0.0 },
                self.radius,
                self.blur,
                if glass { 1.0 } else { 0.0 },
            ];
            list.SetGraphicsRoot32BitConstants(0, 12, constants.as_ptr().cast(), 0);
            list.SetGraphicsRootDescriptorTable(1, self.views.GetGPUDescriptorHandleForHeapStart());
            list.IASetPrimitiveTopology(windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            list.RSSetViewports(&[D3D12_VIEWPORT { Width: width as f32, Height: height as f32, MaxDepth: 1.0, ..Default::default() }]);
            list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 }]);
            list.DrawInstanced(4, 1, 0, 0);
            list.ResourceBarrier(&[transition(back, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_DEST)]);
        }
    }
}

// ---------------------------------------------------------------------------
// In the presenter.

/// Our window is in front.
fn in_front() -> bool {
    let mut process = 0;
    // SAFETY: plain queries; room for the answer.
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut process) };
    process == std::process::id()
}

/// The menu of a presenter.
pub(crate) struct Menu {
    ctl: Controller,
    keys: Keys,
    fade: Fade,
    last: Instant,
    began: Instant,
    text: Option<Text>,
    overlay: Option<Overlay>,
    failed: bool,
    painted_at: Option<Instant>,
    readouts: Readouts,
    readouts_at: Option<Instant>,
    frames: u32,
    frames_since: Instant,
    /// The page shown (Tab).
    tab: Tab,
    /// The Advanced page (every tunable).
    live: live::Page,
    live_extra: live::Extra,
    live_shown: Option<(live::View, u32)>,
    /// The other pages, each made the first time it is shown (indexed by [`Tab::at`]; Advanced has none here).
    pages: [Option<pages::Curated>; 7],
    /// The Controls page (keyboard and mouse rebinding).
    controls: controls::Page,
    pages_shown: Option<(pages::PView, u32)>,
    /// F1 (a second key for the menu) was down at the last frame.
    f1_down: bool,
}

impl Menu {
    pub(crate) fn new() -> Menu {
        let now = Instant::now();
        Menu {
            ctl: Controller::default(),
            keys: Keys::default(),
            fade: Fade::default(),
            last: now,
            began: now,
            text: None,
            overlay: None,
            failed: false,
            painted_at: None,
            readouts: Readouts::default(),
            readouts_at: None,
            frames: 0,
            frames_since: now,
            tab: Tab::Home,
            live: live::Page::default(),
            live_extra: live::Extra::default(),
            live_shown: None,
            pages: Default::default(),
            controls: controls::Page::default(),
            pages_shown: None,
            f1_down: false,
        }
    }

    /// The page `tab` (every one but Advanced), made the first time.
    fn curated(&mut self, tab: Tab) -> Option<&mut pages::Curated> {
        (tab != Tab::Advanced && tab != Tab::Controls).then(|| self.pages[tab.at()].get_or_insert_with(|| pages::Curated::new(tab)))
    }

    /// Shows page `tab`.
    fn go(&mut self, tab: Tab) {
        self.tab = tab;
        if tab == Tab::Advanced {
            self.live.ensure();
        } else if tab == Tab::Controls {
            self.controls.cancel_capture();
        } else {
            self.curated(tab);
        }
        (self.live_shown, self.pages_shown) = (None, None);
    }

    fn readouts(&mut self, host: &Host, now: Instant) -> Readouts {
        if self.readouts_at.is_none_or(|at| now - at >= std::time::Duration::from_millis(250)) {
            self.readouts_at = Some(now);
            let [budget, used, _, _] = host.video_memory().unwrap_or_default();
            let (fps, frame_ms) = (self.readouts.fps, self.readouts.frame_ms);
            self.readouts = Readouts { gpu_used: used, gpu_budget: budget, fps, frame_ms };
        }
        self.readouts
    }

    /// Reads the keys, moves the menu and draws it onto `back`.
    pub(crate) fn draw(&mut self, host: &Host, list: &ID3D12GraphicsCommandList, back: &ID3D12Resource, (width, height): (u32, u32), format: DXGI_FORMAT, hwnd: HWND) {
        if !enabled() {
            return;
        }
        let now = Instant::now();
        // Frame rate over half seconds.
        self.frames += 1;
        if now - self.frames_since >= std::time::Duration::from_millis(500) {
            let s = (now - self.frames_since).as_secs_f32();
            self.readouts.fps = self.frames as f32 / s;
            self.readouts.frame_ms = s * 1000.0 / self.frames as f32;
            (self.frames, self.frames_since) = (0, now);
        }
        // Keys.
        let front = in_front();
        let mut down = [false; 8];
        if front {
            for (d, &vk) in down.iter_mut().zip(KEYS.iter()) {
                // SAFETY: a plain query.
                *d = unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
            }
        }
        let ms = (now - self.began).as_millis() as u64;
        let open_before = self.ctl.open;
        let mut actions = self.keys.actions(ms, down, self.ctl.open);
        // The Controls page waiting for a key: it reads every key, and the menu's own keys are not commands then
        // (F10 and F1 still close the menu, which ends the wait).
        let waiting = self.ctl.open && self.tab == Tab::Controls && self.controls.capturing();
        let mut held = Vec::new();
        if waiting || controls::latched() {
            held = controls::read_down(front);
        }
        if waiting {
            self.controls.poll_capture(&held);
            actions.retain(|a| *a == Action::Toggle);
        }
        // F1 is a second key for F10 (the toggle); it is read apart from the keys the menu holds.
        // SAFETY: a plain query.
        let f1 = front && unsafe { GetAsyncKeyState(i32::from(F1)) } < 0;
        if f1 && !self.f1_down {
            actions.insert(0, Action::Toggle);
        }
        self.f1_down = f1;
        let (extra_down, shift) = live::read_keys(front);
        let mut extra = self.live_extra.commands(extra_down, shift, self.ctl.open);
        if waiting {
            extra.clear();
        }
        if !actions.is_empty() || !extra.is_empty() || self.ctl.open {
            for action in actions {
                // Esc, Back and F10 close the menu on every page; the other keys go to the page shown.
                if !self.ctl.open || matches!(action, Action::Toggle | Action::Close) {
                    self.ctl.step(action);
                    continue;
                }
                let mut close = false;
                match self.tab {
                    Tab::Advanced => {
                        if let Some(what) = self.live.step(action, shift) {
                            self.live.run(what);
                        }
                    }
                    Tab::Controls => close = self.controls.step(action),
                    tab => {
                        if let Some(page) = self.curated(tab)
                            && let Some(act) = page.step(action)
                        {
                            close = page.run(act);
                        }
                        // The readouts show a change at once.
                        self.readouts_at = None;
                    }
                }
                if close {
                    self.ctl.step(Action::Close);
                }
            }
            for cmd in extra {
                let tab = self.tab;
                match cmd {
                    live::Cmd::PageNext => self.go(tab.next()),
                    live::Cmd::PagePrev => self.go(tab.prev()),
                    live::Cmd::GroupPrev if tab == Tab::Advanced => self.live.move_group(-1),
                    live::Cmd::GroupNext if tab == Tab::Advanced => self.live.move_group(1),
                    live::Cmd::GroupPrev if tab == Tab::Controls => self.controls.section(-1),
                    live::Cmd::GroupNext if tab == Tab::Controls => self.controls.section(1),
                    live::Cmd::Reset if tab == Tab::Controls => self.controls.reset_selected(),
                    live::Cmd::ResetAll if tab == Tab::Controls => self.controls.reset_section(),
                    live::Cmd::Reset if tab == Tab::Advanced => self.live.reset_selected(),
                    live::Cmd::ResetAll if tab == Tab::Advanced => {
                        self.live.reset_all();
                    }
                    live::Cmd::Reset => {
                        if let Some(page) = self.curated(tab) {
                            page.reset_selected();
                        }
                    }
                    live::Cmd::ResetAll => {
                        if let Some(page) = self.curated(tab) {
                            page.reset_all();
                        }
                    }
                    _ => {}
                }
            }
        }
        // A wait ends with the menu closed.
        if !self.ctl.open {
            self.controls.cancel_capture();
        }
        let capturing = self.ctl.open && self.controls.capturing();
        controls::set_state(waiting || capturing, capturing, &held);
        OPEN.store(self.ctl.open, Ordering::Relaxed);
        crate::SETTINGS_MENU_OPEN.store(self.ctl.open && enabled(), Ordering::Relaxed);
        F1_TAKEN.store(f1 && (open_before || self.ctl.open || F1_TAKEN.load(Ordering::Relaxed)), Ordering::Relaxed);
        LATCH.store(latch(self.ctl.open, down, LATCH.load(Ordering::Relaxed)), Ordering::Relaxed);
        live::set_latch(self.ctl.open, extra_down);
        let opacity = self.fade.step(self.ctl.open, (now - self.last).as_secs_f32() * 1000.0);
        self.last = now;
        if opacity <= 0.0 || self.failed {
            return;
        }
        // The picture: made again when what it shows changed, a tenth of a second after the last.
        let readouts = self.readouts(host, now);
        let client = client_size(hwnd);
        let unit = live::panel_size(1.0);
        let scale = scale_for_view((width, height), client, (unit.0 as f32, unit.1 as f32));
        let size = live::panel_size(scale);
        if self.overlay.as_ref().is_none_or(|o| !o.matches(format, size)) {
            match Overlay::new(host, format, size) {
                Ok(o) => {
                    if let Some(old) = self.overlay.replace(o) {
                        crate::host::retire(old);
                    }
                    (self.live_shown, self.pages_shown) = (None, None);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "the in-game menu is not drawn: its pipeline was not made");
                    self.failed = true;
                    return;
                }
            }
        }
        if self.text.is_none() {
            self.text = Text::new();
        }
        let due = self.painted_at.is_none_or(|at| now - at >= std::time::Duration::from_millis(100));
        // Read and compared a few times a second, not every frame.
        if due {
            if self.tab == Tab::Advanced {
                let view = (self.live.view(readouts.fps, readouts.frame_ms), size.0);
                if self.live_shown.as_ref() != Some(&view) {
                    if let (Some(text), Some(overlay)) = (self.text.as_mut(), self.overlay.as_mut()) {
                        overlay.write(&live::paint(text, &view.0, scale).rgba8());
                    }
                    self.live_shown = Some(view);
                }
            } else if self.tab == Tab::Controls {
                let view = (self.controls.view(readouts.fps, readouts.frame_ms), size.0);
                if self.pages_shown.as_ref() != Some(&view) {
                    if let (Some(text), Some(overlay)) = (self.text.as_mut(), self.overlay.as_mut()) {
                        overlay.write(&pages::paint(text, &view.0, scale).rgba8());
                    }
                    self.pages_shown = Some(view);
                }
            } else {
                let tab = self.tab;
                let view = self.curated(tab).map(|page| {
                    let mut v = page.view(readouts.fps, readouts.frame_ms);
                    if tab == Tab::Home {
                        v.mem = Some(readouts);
                    }
                    (v, size.0)
                });
                if let Some(view) = view
                    && self.pages_shown.as_ref() != Some(&view)
                {
                    if let (Some(text), Some(overlay)) = (self.text.as_mut(), self.overlay.as_mut()) {
                        overlay.write(&pages::paint(text, &view.0, scale).rgba8());
                    }
                    self.pages_shown = Some(view);
                }
            }
            self.painted_at = Some(now);
        }
        let Some(overlay) = self.overlay.as_mut() else { return };
        overlay.look(16.0 * scale, 24.0 * scale);
        // Slides in from the left a few pixels as it fades in.
        let left = 48.0 * scale - (1.0 - opacity) * 28.0 * scale;
        let top = ((height as f32 - size.1 as f32) / 2.0).max(8.0);
        overlay.record(host, list, back, (width, height), (left, top), opacity);
    }
}


#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn readouts() -> Readouts {
        Readouts { gpu_used: 21_300 << 20, gpu_budget: 24_170 << 20, fps: 58.2, frame_ms: 17.2 }
    }

    #[test]
    fn the_gradient_runs_from_crimson_to_violet_and_is_saturated() {
        let rgb8 = |c: [f32; 3]| c.map(|v| (v * 255.0).round() as u8);
        assert_eq!(rgb8(STOPS[0]), [226, 29, 72]);
        assert_eq!(rgb8(STOPS[3]), [128, 76, 240]);
        let (a, z) = (gradient(0.0), gradient(1.0));
        assert_eq!(a, CRIMSON);
        assert_eq!(z, VIOLET);
        // The second stop is the pink; out-of-range values stop at the ends.
        assert_eq!(rgb8(gradient(1.0 / 3.0)), rgb8(PINK));
        assert_eq!(gradient(-3.0), a);
        assert_eq!(gradient(9.0), z);
        // Red falls and blue rises across it.
        assert!(gradient(0.1)[0] > gradient(0.9)[0] && gradient(0.1)[2] < gradient(0.9)[2]);
        // Saturated, not pastel: every stop has a channel far below its brightest.
        for s in STOPS {
            let (hi, lo) = (s.iter().copied().fold(0.0f32, f32::max), s.iter().copied().fold(1.0f32, f32::min));
            assert!(hi - lo > 0.6, "{s:?}");
        }
    }

    #[test]
    fn keys_press_once_and_arrows_repeat() {
        let mut k = Keys::default();
        let none = [false; 8];
        let only = |i: usize| {
            let mut d = none;
            d[i] = true;
            d
        };
        // F10 acts closed or open.
        assert_eq!(k.actions(0, only(0), false), vec![Action::Toggle]);
        assert_eq!(k.actions(10, only(0), false), vec![]);
        assert_eq!(k.actions(20, none, false), vec![]);
        // Others do nothing closed.
        assert_eq!(k.actions(30, only(1), false), vec![]);
        assert_eq!(k.actions(40, none, false), vec![]);
        // Open: an arrow acts once, waits, then repeats.
        assert_eq!(k.actions(1000, only(2), true), vec![Action::Down]);
        assert_eq!(k.actions(1100, only(2), true), vec![]);
        assert_eq!(k.actions(1379, only(2), true), vec![]);
        assert_eq!(k.actions(1380, only(2), true), vec![Action::Down]);
        assert_eq!(k.actions(1400, only(2), true), vec![]);
        assert_eq!(k.actions(1450, only(2), true), vec![Action::Down]);
        // Enter and Back do not repeat.
        assert_eq!(k.actions(2000, only(5), true), vec![Action::Enter]);
        assert_eq!(k.actions(3000, only(5), true), vec![]);
        assert_eq!(k.actions(3100, only(6), true), vec![Action::Close]);
        assert_eq!(k.actions(3200, only(7), true), vec![Action::Close]);
    }

    #[test]
    fn the_title_does_not_see_the_keys_while_open_or_until_they_are_let_go() {
        let mut down = [false; 8];
        // Open, Down pressed: taken.
        down[2] = true;
        let l = latch(true, down, 0);
        assert_eq!(l, 1 << 2);
        // Closed with Esc held: Esc stays taken until it is let go.
        let mut down = [false; 8];
        down[7] = true;
        let l = latch(true, down, l);
        assert_eq!(l, 1 << 7);
        let l = latch(false, down, l);
        assert_eq!(l, 1 << 7);
        assert_eq!(latch(false, [false; 8], l), 0);
        // A key pressed while closed is the title's.
        let mut down = [false; 8];
        down[1] = true;
        assert_eq!(latch(false, down, 0), 0);
    }

    #[test]
    fn takes_key_follows_the_state_and_the_switch() {
        // (The tests do not set VICTORY_MENU: the menu exists.)
        assert!(enabled());
        OPEN.store(false, Ordering::Relaxed);
        LATCH.store(0, Ordering::Relaxed);
        assert!(takes_key(0x79), "F10 is always taken");
        assert!(!takes_key(0x26));
        assert!(!takes_key(0x41), "other keys are never taken");
        OPEN.store(true, Ordering::Relaxed);
        for vk in KEYS {
            assert!(takes_key(vk), "{vk:#x}");
        }
        assert!(!takes_key(0x41) && !takes_key(0x20));
        OPEN.store(false, Ordering::Relaxed);
        LATCH.store(1 << 7, Ordering::Relaxed);
        assert!(takes_key(0x1B));
        assert!(!takes_key(0x08));
        LATCH.store(0, Ordering::Relaxed);
    }

    #[test]
    fn the_controller_opens_and_closes() {
        let mut c = Controller::default();
        // Closed: Esc does nothing, F10 opens it.
        c.step(Action::Close);
        assert!(!c.open);
        c.step(Action::Toggle);
        assert!(c.open);
        // The page's keys are not the controller's.
        c.step(Action::Down);
        c.step(Action::Enter);
        assert!(c.open);
        c.step(Action::Close);
        assert!(!c.open);
        c.step(Action::Toggle);
        c.step(Action::Toggle);
        assert!(!c.open);
    }

    #[test]
    fn tab_goes_round_the_seven_pages_both_ways() {
        let order = [Tab::Home, Tab::Graphics, Tab::Performance, Tab::Overlay, Tab::Cheats, Tab::Controls, Tab::Advanced];
        assert_eq!(Tab::ALL, order);
        for (i, t) in order.iter().enumerate() {
            assert_eq!(t.next(), order[(i + 1) % 7]);
            assert_eq!(t.prev(), order[(i + 6) % 7]);
            assert_eq!(t.next().prev(), *t);
            assert_eq!(t.at(), i);
        }
        assert_eq!(Tab::Advanced.next(), Tab::Home);
        assert_eq!(Tab::Home.prev(), Tab::Advanced);
        assert_eq!(order.map(Tab::label), ["HOME", "GRAPHICS", "PERFORMANCE", "OVERLAY", "CHEATS", "CONTROLS", "ADVANCED"]);
        assert_eq!(Tab::Cheats.next(), Tab::Controls);
        assert_eq!(Tab::Controls.next(), Tab::Advanced);
        assert_eq!(TITLE, "JESUS666 MENU");
    }

    #[test]
    fn the_tab_strip_marks_the_page_shown_with_an_accent_underline() {
        let mut text = Text::new().expect("gdi");
        let mut strip = |active: Tab| {
            let mut c = Canvas::new(700, 40);
            paint_tabs(&mut text, &mut c, 1.0, 20.0, 4.0, active);
            c
        };
        let (a, b) = (strip(Tab::Home), strip(Tab::Overlay));
        assert_ne!(a.px, b.px);
        // The underline is under the page shown only: a saturated pink row below its name.
        let underline = |c: &Canvas| c.px[(4 + 24) * c.w + 30][3];
        assert!(underline(&a) > 0.9 && underline(&b) < 0.05, "{} {}", underline(&a), underline(&b));
        let p = a.px[(4 + 24) * a.w + 22];
        assert!(p[0] > 0.7 && p[1] < 0.3, "{p:?}");
        assert!(a.px.iter().filter(|p| p[3] > 0.5).count() > 200);
        // All seven names fit before the right side's text starts (it takes the last 190 pixels or so at scale 1).
        let last = Tab::ALL.iter().fold(20.0f32, |at, t| at + text.measure(t.label(), 15, true).0 as f32 + 22.0);
        assert!(last < 700.0, "{last}");
    }

    #[test]
    fn the_menu_is_readable_and_fits_at_720_and_1440() {
        let page = live::panel_size(1.0);
        let page = (page.0 as f32, page.1 as f32);
        for (w, h) in [(1280u32, 720u32), (2560, 1440), (1920, 1080), (3840, 2160)] {
            let s = scale_for_view((w, h), Some((w, h)), page);
            // At least the readable least on the screen, and the panel inside the picture.
            assert!(s >= MIN_SCALE.min(0.75) && s <= 4.0, "{w}x{h}: {s}");
            assert!(page.1 * s <= h as f32, "{w}x{h}: {s}");
            assert!((page.0 + 48.0) * s <= w as f32, "{w}x{h}: {s}");
        }
        assert_eq!(scale_for_view((1280, 720), Some((1280, 720)), page), 0.75);
        // The same window with a picture of another size: the same size on the screen.
        let in_window = |picture: (u32, u32)| scale_for_view(picture, Some((1280, 720)), page) * 720.0 / picture.1 as f32;
        assert_eq!(in_window((1280, 720)), 0.75);
        assert_eq!(in_window((2560, 1440)), 0.75);
        assert_eq!(in_window((3840, 2160)), 0.75);
        // Not known: the picture is the window.
        assert_eq!(scale_for_view((1920, 1080), None, page), 1.0);
        // A window that is small for the title's scale: the panel keeps within the picture.
        assert!(scale_for_view((640, 360), Some((640, 360)), page) * page.1 <= 360.0);
    }

    #[test]
    fn the_scale_follows_the_picture() {
        assert_eq!(scale_for(1080), 1.0);
        assert_eq!(scale_for(2160), 2.0);
        assert_eq!(scale_for(720), 0.75);
        assert_eq!(scale_for(200), 0.75);
        assert_eq!(scale_for(10_000), 4.0);
        let (w1, h1) = live::panel_size(1.0);
        let (w2, h2) = live::panel_size(2.0);
        assert_eq!((w2, h2), (w1 * 2, h1 * 2));
    }

    #[test]
    fn the_glass_has_a_rim_a_thin_tint_and_cut_corners() {
        let mut c = Canvas::new(400, 300);
        paint_glass(&mut c, 1.0, 16.0);
        let at = |x: usize, y: usize| c.px[y * c.w + x];
        // The corners are cut away.
        assert_eq!(at(0, 0)[3], 0.0);
        // The rim is a light line, brighter at the top than at the bottom.
        let (top, bottom) = (at(200, 0), at(200, 299));
        assert!(top[0] > 0.12 && top[0] > bottom[0] * 1.5 && bottom[0] > 0.03, "{top:?} {bottom:?}");
        // The body is mostly transparent (the card puts the glass there), dark where it is not.
        let body = at(200, 200);
        assert!(body[3] > 0.05 && body[3] < 0.35 && body[0] < 0.1, "{body:?}");
        // The edge falls off to dark just inside the rim.
        assert!(at(200, 3)[3] > at(200, 150)[3] - 0.2 && at(3, 150)[3] > body[3]);
        // The CPU stand-in of the glass: a bright striped picture under the panel is blurred and darkened.
        let (bw, bh) = (500usize, 400usize);
        let mut pic = vec![0u8; bw * bh * 3];
        for y in 0..bh {
            for x in 0..bw {
                let v = if (x / 6) % 2 == 0 { 230 } else { 30 };
                pic[(y * bw + x) * 3..][..3].copy_from_slice(&[v, v, v]);
            }
        }
        let before = pic.clone();
        c.composite(&mut pic, bw, 50, 50, 1.0, 1.0);
        let px = |p: &[u8], x: usize, y: usize| p[(y * bw + x) * 3];
        // Inside: no stripes left (blurred), darker than the picture's brightest, not its darkest.
        let seen: Vec<u8> = (250..262).map(|x| px(&pic, x, 200)).collect();
        let (lo, hi) = (*seen.iter().min().unwrap(), *seen.iter().max().unwrap());
        assert!(hi - lo < 25 && hi < 120 && lo > 25, "{seen:?}");
        // Outside the panel the picture is untouched.
        assert_eq!(px(&pic, 10, 10), px(&before, 10, 10));
        assert_eq!(px(&pic, 49, 200), px(&before, 49, 200));
        // Opacity 0 leaves it all alone.
        let mut faded = before.clone();
        c.composite(&mut faded, bw, 50, 50, 0.0, 1.0);
        assert_eq!(faded, before);
    }

    #[test]
    fn the_chrome_names_the_menu() {
        let mut text = Text::new().expect("a memory context");
        let mut c = Canvas::new(900, 120);
        chrome(&mut text, &mut c, 1.0, 78.0, Tab::Graphics, "", "3 changed of 11 lines");
        // The title is white text in the first rows; the accent line sits at the header's foot.
        let lit = (20..300).flat_map(|x| (8..48).map(move |y| (x, y))).filter(|&(x, y)| c.px[y * c.w + x][0] > 0.9 && c.px[y * c.w + x][2] > 0.9).count();
        assert!(lit > 150, "{lit}");
        let line = c.px[(78 - 1) * c.w + 450];
        assert!(line[3] > 0.6 && line[0] > line[1], "{line:?}");
    }

    #[test]
    fn the_fade_takes_its_time_both_ways_and_eases() {
        let mut f = Fade::default();
        assert_eq!(f.step(true, 0.0), 0.0);
        let half = f.step(true, FADE_MS / 2.0);
        assert!((half - 0.5).abs() < 1e-6, "{half}");
        // Slow at the start: a tenth of the way is less than a tenth of the value.
        let mut g = Fade::default();
        assert!(g.step(true, FADE_MS / 10.0) < 0.05);
        assert_eq!(f.step(true, FADE_MS * 5.0), 1.0);
        assert_eq!(f.step(false, FADE_MS * 5.0), 0.0);
        assert_eq!(f.value, 0.0);
    }

    #[test]
    fn f1_is_a_second_key_and_is_taken_only_with_the_menu() {
        OPEN.store(false, Ordering::Relaxed);
        F1_TAKEN.store(false, Ordering::Relaxed);
        assert!(!takes_key(F1), "closed: the title keeps F1");
        F1_TAKEN.store(true, Ordering::Relaxed);
        assert!(takes_key(F1), "held after it opened or closed the menu");
        F1_TAKEN.store(false, Ordering::Relaxed);
        OPEN.store(true, Ordering::Relaxed);
        assert!(takes_key(F1));
        OPEN.store(false, Ordering::Relaxed);
    }

    #[test]
    fn the_text_has_soft_edges_and_measures_larger_at_larger_sizes() {
        let mut text = Text::new().unwrap();
        let (w20, h20) = text.measure("Texture pool", 20, false);
        let (w40, h40) = text.measure("Texture pool", 40, false);
        assert!(w20 > 60 && h20 > 15);
        assert!(w40 > w20 * 3 / 2 && h40 > h20 * 3 / 2);
        let (cover, ..) = text.coverage("Eviction", 20, true).unwrap();
        // Anti-aliased: many values between none and full.
        let soft = cover.iter().filter(|&&v| v > 0.1 && v < 0.9).count();
        let full = cover.iter().filter(|&&v| v >= 0.9).count();
        assert!(soft > 40 && full > 40, "soft {soft} full {full}");
        // Bold is heavier than regular.
        let (regular, ..) = text.coverage("Eviction", 20, false).unwrap();
        assert!(cover.iter().sum::<f32>() > regular.iter().sum::<f32>());
        // A long text grows the bitmap instead of being cut.
        let long = "x".repeat(300);
        let (c, w, h) = text.coverage(&long, 20, false).unwrap();
        assert_eq!(c.len(), w * h);
        assert!(w > 768);
    }

    #[test]
    fn shapes_have_anti_aliased_edges() {
        let mut c = Canvas::new(40, 40);
        rounded(&mut c, (4.0, 4.0, 32.0, 32.0), 10.0, Mode::Over, |_, _| ([1.0, 0.0, 0.0], 1.0));
        assert_eq!(c.px[0][3], 0.0);
        assert_eq!(c.px[20 * 40 + 20], [1.0, 0.0, 0.0, 1.0]);
        // Along the rounded corner there are partial pixels.
        let partial = c.px.iter().filter(|p| p[3] > 0.05 && p[3] < 0.95).count();
        assert!(partial >= 8, "{partial}");
        // Premultiplied: a half-covered pixel's colour is half its alpha.
        let p = c.px.iter().find(|p| p[3] > 0.3 && p[3] < 0.7).unwrap();
        assert!((p[0] - p[3]).abs() < 1e-5);
    }

    /// A stand-in for the game's picture: a sky that goes from deep blue to
    /// warm at the horizon, with a dark ground and some stripes.
    pub(crate) fn backdrop(w: usize, h: usize) -> Vec<u8> {
        let mut out = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let t = y as f32 / h as f32;
                let rgb = if t < 0.62 {
                    let k = t / 0.62;
                    [30.0 + 190.0 * k * k, 50.0 + 90.0 * k, 110.0 + 20.0 * k - 60.0 * k * k]
                } else {
                    let k = (t - 0.62) / 0.38;
                    [40.0 - 25.0 * k, 36.0 - 22.0 * k, 44.0 - 26.0 * k]
                };
                let stripe = if (x / 24 + y / 24) % 2 == 0 { 6.0 } else { 0.0 };
                for i in 0..3 {
                    out[(y * w + x) * 3 + i] = (rgb[i] + stripe).clamp(0.0, 255.0) as u8;
                }
            }
        }
        out
    }

    /// The whole path on the card: the overlay draws the panel (with its glass,
    /// from a copy of the picture under it) onto a texture the way it draws
    /// onto the back buffer, and the result is read back.
    #[test]
    fn the_overlay_blends_the_panel_onto_a_back_buffer() {
        let Some(host) = crate::host::host() else {
            return; // no host GPU here
        };
        let format = DXGI_FORMAT_R8G8B8A8_UNORM;
        let (bw, bh) = (1920u32, 1080u32);
        let mut text = Text::new().unwrap();
        let scale = scale_for(bh);
        let page = pages::Curated::new(Tab::Home);
        let mut view = page.view(58.2, 17.2);
        view.mem = Some(readouts());
        let canvas = pages::paint(&mut text, &view, scale);
        let size = live::panel_size(scale);
        let mut overlay = Overlay::new(host, format, size).expect("the overlay");
        overlay.look(16.0 * scale, 24.0 * scale);
        assert!(overlay.matches(format, size) && !overlay.matches(DXGI_FORMAT_B8G8R8A8_UNORM, size));
        overlay.write(&canvas.rgba8());
        let target = {
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: u64::from(bw),
                Height: bh,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                ..Default::default()
            };
            let mut t: Option<ID3D12Resource> = None;
            // SAFETY: plain creation.
            unsafe {
                host.device
                    .CreateCommittedResource(
                        &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
                        D3D12_HEAP_FLAG_NONE,
                        &desc,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                        None,
                        &mut t,
                    )
                    .unwrap();
            }
            t.unwrap()
        };
        // What is cleared: each band of four rows in the colour of the sample's first pixel at its middle.
        let sky = {
            let full = self::backdrop(bw as usize, bh as usize);
            let mut flat = full.clone();
            for y in 0..bh as usize {
                let from = ((y / 4) * 4 + 2) * bw as usize * 3;
                for x in 0..bw as usize {
                    flat[(y * bw as usize + x) * 3..][..3].copy_from_slice(&full[from..from + 3]);
                }
            }
            flat
        };
        let opacities = [1.0f32, 0.5];
        let mut results = Vec::new();
        for opacity in opacities {
            let list = host.new_list().unwrap();
            let cmds = &list.list;
            // The backdrop, band by band, so the picture behind is not flat.
            // SAFETY: an open list, a live texture, plain commands.
            unsafe {
                cmds.ResourceBarrier(&[transition(&target, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
                let rtv = host.rtv(Some(&target), None);
                for band in 0..(bh / 4) {
                    let y = (band * 4 + 2) as usize;
                    let at = (y * bw as usize) * 3;
                    let c = [f32::from(sky[at]) / 255.0, f32::from(sky[at + 1]) / 255.0, f32::from(sky[at + 2]) / 255.0, 1.0];
                    let rect = RECT { left: 0, top: (band * 4) as i32, right: bw as i32, bottom: (band * 4 + 4) as i32 };
                    cmds.ClearRenderTargetView(rtv, &c, Some(&[rect]));
                }
                cmds.ResourceBarrier(&[transition(&target, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_DEST)]);
            }
            let top = ((bh as f32 - size.1 as f32) / 2.0).round();
            overlay.record(host, cmds, &target, (bw, bh), (48.0, top), opacity);
            // SAFETY: as above; a readback buffer made for the picture.
            let (buffer, pitch) = unsafe {
                cmds.ResourceBarrier(&[transition(&target, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
                let pitch = (bw * 4).next_multiple_of(256);
                let mut buffer: Option<ID3D12Resource> = None;
                host.device
                    .CreateCommittedResource(
                        &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() },
                        D3D12_HEAP_FLAG_NONE,
                        &D3D12_RESOURCE_DESC {
                            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                            Width: u64::from(pitch) * u64::from(bh),
                            Height: 1,
                            DepthOrArraySize: 1,
                            MipLevels: 1,
                            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                            ..Default::default()
                        },
                        D3D12_RESOURCE_STATE_COPY_DEST,
                        None,
                        &mut buffer,
                    )
                    .unwrap();
                let buffer = buffer.unwrap();
                let to = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: std::mem::transmute_copy(&buffer),
                    Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                        PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                            Offset: 0,
                            Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: format, Width: bw, Height: bh, Depth: 1, RowPitch: pitch },
                        },
                    },
                };
                let from = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: std::mem::transmute_copy(&target),
                    Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
                };
                cmds.CopyTextureRegion(&to, 0, 0, 0, &from, None);
                cmds.ResourceBarrier(&[transition(&target, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST)]);
                (buffer, pitch)
            };
            list.close();
            host.execute(&[&list]);
            // SAFETY: the list has run (execute waits for it); the mapping spans the rows.
            let rgb = unsafe {
                host.device.GetDeviceRemovedReason().unwrap();
                let mut mapped = std::ptr::null_mut();
                buffer.Map(0, None, Some(&mut mapped)).unwrap();
                let bytes = std::slice::from_raw_parts(mapped.cast::<u8>(), pitch as usize * bh as usize);
                let mut rgb = Vec::with_capacity((bw * bh * 3) as usize);
                for y in 0..bh as usize {
                    for x in 0..bw as usize {
                        let p = &bytes[y * pitch as usize + x * 4..];
                        rgb.extend_from_slice(&p[..3]);
                    }
                }
                buffer.Unmap(0, None);
                rgb
            };
            results.push((opacity, rgb, top as usize));
        }

        let dir = std::path::Path::new(r"D:\.devops\.temp\cool_fox\live");
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("menu4-gpu-glass.png"), victory_mcp::shot::encode_png(bw, bh, &results[0].1).unwrap()).unwrap();
        std::fs::write(dir.join("menu4-gpu-glass-half-faded.png"), victory_mcp::shot::encode_png(bw, bh, &results[1].1).unwrap()).unwrap();
        // The card's blur is a spiral of taps and the CPU's two box blurs, so the two glasses are close, not equal:
        // on average a few levels apart inside the panel, and the picture outside it untouched.
        for (opacity, rgb, top) in &results {
            let mut expect = sky.clone();
            canvas.composite(&mut expect, bw as usize, 48, *top, *opacity, scale);
            let (mut sum, mut n, mut changed) = (0u64, 0u64, 0usize);
            let (px0, px1) = (48usize, 48 + size.0 as usize);
            for y in *top..(*top + size.1 as usize).min(bh as usize) {
                for x in px0..px1.min(bw as usize) {
                    for k in 0..3 {
                        let at = (y * bw as usize + x) * 3 + k;
                        sum += u64::from(rgb[at].abs_diff(expect[at]));
                        n += 1;
                        changed += usize::from(rgb[at] != sky[at]);
                    }
                }
            }
            let mean = sum as f64 / n as f64;
            assert!(mean < 14.0, "opacity {opacity}: the card's glass is {mean} levels off the CPU's on average");
            assert!(changed > 100_000, "opacity {opacity}: the panel is on the picture ({changed} values changed)");
            // The glass is darker than the picture behind it (full opacity): the mean over the panel falls.
            if *opacity == 1.0 {
                let mean_of = |p: &[u8]| {
                    let mut s = 0u64;
                    for y in *top..(*top + size.1 as usize) {
                        for x in px0..px1 {
                            s += u64::from(p[(y * bw as usize + x) * 3]);
                        }
                    }
                    s
                };
                assert!(mean_of(rgb) * 10 < mean_of(&sky) * 8, "the glass darkens what is behind it");
            }
            // Outside the panel the picture is untouched: the rows above it, and the columns left of it.
            assert_eq!(&rgb[..3 * 1920], &sky[..3 * 1920]);
            let row = (*top + 100) * bw as usize * 3;
            assert_eq!(&rgb[row..row + 3 * 40], &sky[row..row + 3 * 40]);
        }
    }
}
