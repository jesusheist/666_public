//! The in-game menu (F10; `VICTORY_MENU=0` leaves it out): a panel drawn on
//! the swap chain's final back buffer at every present, in release builds
//! too, over the title's picture and over the two composed planes alike (it
//! is drawn after both, and saved frames, the frame recorder and
//! `screenshot` copy the picture before it is).
//!
//! Options: the texture pool (Auto, 4 to 20 GB; what does not fit within the
//! card's budget less 1.5 GB is not offered), eviction on or off, and lines
//! that only show: the card's memory, the pool in use, copies let go, the
//! share of the budget textures may fill. Changes go through the live
//! settings ([`crate::vram`]) and are logged there. Readouts under the list:
//! the card's memory, the pool, frame rate.
//!
//! A second page, "Live settings" (Tab switches; see `menu_live.rs`), lists
//! every tunable of the registry by group and changes them live.
//!
//! Keys: F10 shows and hides it; Up and Down select, Left and Right change,
//! Enter switches an on/off option, Back and Esc close it. While it is open
//! the title does not see those keys ([`takes_key`], asked by the title's
//! keyboard input in `victory-hle`), and a key held when it closed is not
//! seen until it is let go.
//!
//! How it is drawn: the panel is painted on the CPU into a premultiplied
//! RGBA picture ([`paint`]: rounded shapes with edges anti-aliased by
//! coverage, text from the system's own font drawn through GDI into a bitmap
//! with grayscale anti-aliasing, so it reads at any size) when its content
//! changes (at most ten times a second), sent to a texture on the card, and
//! blended onto the back buffer by one small draw (alpha, fade in and out, a
//! slide of a few pixels). Closed, the cost of a frame is one key query.
//! Scaled to the picture: one scale unit at 1,080 lines.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{PCSTR, s, w};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::host::{Host, transition};
use crate::vram::{self, Snapshot};

#[path = "menu_live.rs"]
mod live;

// ---------------------------------------------------------------------------
// The colours: the gradient of the app's icon (`GTAVI_PORT\.tools\icon\gtavi-256.png`,
// the one `stamp-icon.ps1` puts into the exes). Read down the right-hand
// stroke of its "VI" at y = 40, 80, 120, 160, 200 and 215 of 256: blue-violet
// (59, 94, 207), violet (153, 66, 194), pink (249, 78, 164), coral
// (255, 116, 122), orange (255, 148, 83) and (255, 152, 73). The banner and the
// selection bar run from the orange end to the violet end.

/// The orange at the icon's foot.
pub(crate) const ORANGE: [u8; 3] = [255, 152, 73];
pub(crate) const CORAL: [u8; 3] = [255, 116, 122];
pub(crate) const PINK: [u8; 3] = [249, 78, 164];
pub(crate) const MAGENTA: [u8; 3] = [220, 61, 191];
/// The violet toward the icon's top.
pub(crate) const VIOLET: [u8; 3] = [153, 66, 194];
/// The gradient's stops, evenly spaced.
pub(crate) const STOPS: [[u8; 3]; 5] = [ORANGE, CORAL, PINK, MAGENTA, VIOLET];

/// The panel's body, top and bottom, and its text.
const BODY_TOP: [f32; 3] = [26.0 / 255.0, 16.0 / 255.0, 44.0 / 255.0];
const BODY_BOTTOM: [f32; 3] = [13.0 / 255.0, 9.0 / 255.0, 26.0 / 255.0];
const TEXT: [f32; 3] = [0.96, 0.94, 1.0];
const TEXT_SOFT: [f32; 3] = [0.82, 0.78, 0.90];
const TEXT_DIM: [f32; 3] = [0.56, 0.52, 0.66];
const BODY_ALPHA: f32 = 0.92;

/// The gradient at `t` (0: orange, 1: violet) as 0 to 1 values.
pub(crate) fn gradient(t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0) * (STOPS.len() - 1) as f32;
    let i = (t.floor() as usize).min(STOPS.len() - 2);
    let f = t - i as f32;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    [0, 1, 2].map(|c| (f32::from(a[c]) + (f32::from(b[c]) - f32::from(a[c])) * f) / 255.0)
}

// ---------------------------------------------------------------------------
// What the menu shows and does, apart from drawing.

/// What an option is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// One of several values: Left and Right.
    Choice,
    /// On or off: Left, Right and Enter.
    Toggle,
    /// Only shows.
    Info,
}

/// A change a key makes to the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// The pool setting in megabytes (0: Auto).
    Pool(u64),
    Evict(bool),
}

impl Change {
    fn apply(self) {
        match self {
            Change::Pool(mb) => vram::set_pool_mb(mb),
            Change::Evict(on) => vram::set_evict(on),
        }
    }
}

/// A line of the list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub label: String,
    pub value: String,
    pub kind: Kind,
    pub left: Option<Change>,
    pub right: Option<Change>,
    pub enter: Option<Change>,
}

/// The numbers the menu shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Readouts {
    pub gpu_used: u64,
    pub gpu_budget: u64,
    pub pool_used: u64,
    pub pool_mb: u64,
    pub room_percent: u64,
    pub evict_on: bool,
    pub evict_allowed: bool,
    pub copies_let_go: u64,
    pub mb_let_go: u64,
    pub fps: f32,
    pub frame_ms: f32,
}

const MB: u64 = 1 << 20;
const GB: u64 = 1 << 30;

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GB as f64)
}

fn pool_text(mb: u64) -> String {
    if mb == 0 { "Auto".to_owned() } else { format!("{} GB", mb as f64 / 1024.0).replace(".0 GB", " GB") }
}

/// The pool settings one step from `current_mb` (0: Auto): the next one
/// above and the next one below among what [`vram::pool_choices`] offers,
/// going round. A setting that is not on the list (a variable's) steps to its
/// neighbours.
pub(crate) fn pool_steps(current_mb: u64, budget: u64) -> (u64, u64) {
    let choices: Vec<u64> = vram::pool_choices(budget).into_iter().map(|g| g * 1024).collect();
    let up = choices.iter().copied().find(|&c| c > current_mb && c != 0).unwrap_or(0);
    let down = if current_mb == 0 {
        choices.last().copied().unwrap_or(0)
    } else {
        choices.iter().copied().rev().find(|&c| c < current_mb && c != 0).unwrap_or(0)
    };
    (down, up)
}

/// The list, from what is true now.
pub(crate) fn rows(r: &Readouts) -> Vec<Row> {
    let (down, up) = pool_steps(r.pool_mb, r.gpu_budget);
    let evict = if r.evict_allowed {
        Row {
            label: "Eviction".into(),
            value: if r.evict_on { "On" } else { "Off" }.into(),
            kind: Kind::Toggle,
            left: Some(Change::Evict(!r.evict_on)),
            right: Some(Change::Evict(!r.evict_on)),
            enter: Some(Change::Evict(!r.evict_on)),
        }
    } else {
        Row {
            label: "Eviction".into(),
            value: "Off (VICTORY_EVICT=0)".into(),
            kind: Kind::Info,
            left: None,
            right: None,
            enter: None,
        }
    };
    vec![
        Row {
            label: "Texture pool".into(),
            value: pool_text(r.pool_mb),
            kind: Kind::Choice,
            left: Some(Change::Pool(down)),
            right: Some(Change::Pool(up)),
            enter: None,
        },
        evict,
        info("Card memory in use", format!("{} of {}", gb(r.gpu_used), gb(r.gpu_budget))),
        info("Texture pool in use", gb(r.pool_used)),
        info("Copies let go", format!("{} ({})", r.copies_let_go, gb(r.mb_let_go * MB))),
        info("Texture share of the budget", format!("{} percent", r.room_percent)),
    ]
}

fn info(label: &str, value: String) -> Row {
    Row { label: label.into(), value, kind: Kind::Info, left: None, right: None, enter: None }
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

/// Lines of the list shown at once.
pub(crate) const VISIBLE: usize = 5;

/// Whether the menu is open, which line is selected and where the list is
/// scrolled to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Controller {
    pub open: bool,
    pub selected: usize,
    pub scroll: usize,
}

/// The first line shown so that `selected` is, moving as little as it can.
pub(crate) fn follow(selected: usize, scroll: usize, total: usize) -> usize {
    let most = total.saturating_sub(VISIBLE);
    let scroll = if selected < scroll {
        selected
    } else if selected >= scroll + VISIBLE {
        selected + 1 - VISIBLE
    } else {
        scroll
    };
    scroll.min(most)
}

impl Controller {
    /// Does what `action` asks of the list `rows`; the change to the settings
    /// it makes, if any (the caller applies it).
    pub(crate) fn step(&mut self, action: Action, rows: &[Row]) -> Option<Change> {
        if action == Action::Toggle {
            self.open = !self.open;
            return None;
        }
        if !self.open || rows.is_empty() {
            return None;
        }
        self.selected = self.selected.min(rows.len() - 1);
        let change = match action {
            Action::Up => {
                self.selected = (self.selected + rows.len() - 1) % rows.len();
                None
            }
            Action::Down => {
                self.selected = (self.selected + 1) % rows.len();
                None
            }
            Action::Left => rows[self.selected].left,
            Action::Right => rows[self.selected].right,
            Action::Enter => rows[self.selected].enter,
            Action::Close => {
                self.open = false;
                None
            }
            Action::Toggle => None,
        };
        self.scroll = follow(self.selected, self.scroll, rows.len());
        change
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

/// Sizes at scale 1 (1,080 lines).
const W: f32 = 600.0;
const PAD: f32 = 20.0;
const HEADER: f32 = 88.0;
const ROW: f32 = 46.0;
const GAP: f32 = 10.0;
const FOOTER: f32 = 122.0;
const H: f32 = HEADER + GAP + ROW * VISIBLE as f32 + GAP + FOOTER;

/// The scale for a picture `height` lines high: 1 at 1,080, in eighths.
pub(crate) fn scale_for(height: u32) -> f32 {
    ((height as f32 / 1080.0).clamp(0.6, 4.0) * 8.0).round() / 8.0
}

/// The panel's size in pixels at `scale`.
pub(crate) fn panel_size(scale: f32) -> (u32, u32) {
    ((W * scale).round() as u32, (H * scale).round() as u32)
}

/// A picture of premultiplied colour with alpha, 0 to 1 per channel.
pub(crate) struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 4]>,
}

impl Canvas {
    pub(crate) fn new(w: usize, h: usize) -> Canvas {
        Canvas { w, h, px: vec![[0.0; 4]; w * h] }
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
enum Mode {
    Over,
    Replace,
}

/// A rounded box painted by `colour(x, y)` (the pixel's place in the box, 0
/// to 1 each way) and its alpha.
fn rounded(
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
    fn draw(&mut self, c: &mut Canvas, text: &str, x: f32, y: f32, px: i32, bold: bool, colour: [f32; 3], alpha: f32, right: bool) {
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
    fn draw_shadowed(&mut self, c: &mut Canvas, text: &str, x: f32, y: f32, px: i32, bold: bool, colour: [f32; 3], right: bool) {
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

/// A bar of a gradient, filled to `fraction`.
fn bar(c: &mut Canvas, (left, top, w, h): (f32, f32, f32, f32), fraction: f32) {
    rounded(c, (left, top, w, h), h / 2.0, Mode::Over, |_, _| ([1.0, 1.0, 1.0], 0.10));
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let filled = (w * f).max(h);
        rounded(c, (left, top, filled, h), h / 2.0, Mode::Over, |x, _| (gradient(x * f), 1.0));
    }
}

/// Paints the panel: `rows` (the list, `ctl` says which line is selected and
/// where it is scrolled to) and `readouts`, at `scale`.
pub(crate) fn paint(text: &mut Text, rows: &[Row], ctl: &Controller, r: &Readouts, scale: f32) -> Canvas {
    let (pw, ph) = panel_size(scale);
    let mut c = Canvas::new(pw as usize, ph as usize);
    let u = |v: f32| v * scale;
    let px = |v: f32| (v * scale).round().max(1.0) as i32;
    let (w, h) = (pw as f32, ph as f32);

    // Body, with a rim of the gradient.
    rounded(&mut c, (0.0, 0.0, w, h), u(16.0), Mode::Over, |x, y| (gradient((x * 0.8 + y * 0.2).clamp(0.0, 1.0)), 1.0));
    rounded(&mut c, (u(2.0), u(2.0), w - u(4.0), h - u(4.0)), u(14.0), Mode::Replace, |_, y| {
        let t = y;
        ([0, 1, 2].map(|i| BODY_TOP[i] + (BODY_BOTTOM[i] - BODY_TOP[i]) * t), BODY_ALPHA)
    });

    // Header banner: the gradient, a sun cut by stripes, the title.
    let banner = (u(2.0), u(2.0), w - u(4.0), u(HEADER) - u(2.0));
    rounded(&mut c, banner, u(14.0), Mode::Replace, |x, y| (gradient((x * 0.85 + y * 0.15).clamp(0.0, 1.0)), 1.0));
    let (sx, sy, sr) = (w - u(118.0), u(HEADER) * 0.62, u(46.0));
    let stripe = u(6.0);
    for y in 0..(u(HEADER) as i64 - 2) {
        for x in (sx - sr) as i64..(sx + sr) as i64 {
            let (dx, dy) = (x as f32 + 0.5 - sx, y as f32 + 0.5 - sy);
            let edge = (sr - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            // Stripes cut the lower half more and more.
            let below = ((dy / sr).clamp(0.0, 1.0) * stripe * 0.9).min(stripe * 0.9);
            let cut = if dy > 0.0 && (y as f32).rem_euclid(stripe) < below { 0.0 } else { 1.0 };
            c.over(x, y, [1.0, 0.93, 0.85], 0.30 * edge * cut);
        }
    }
    // A darker band under the title keeps it readable on the orange end.
    rounded(&mut c, (u(2.0), u(HEADER) - u(30.0), w - u(4.0), u(28.0)), u(0.5), Mode::Over, |_, y| ([0.05, 0.02, 0.12], 0.10 + 0.20 * y));
    text.draw_shadowed(&mut c, "VICTORY", u(PAD), u(10.0), px(36.0), true, [1.0, 1.0, 1.0], false);
    text.draw_shadowed(&mut c, "GPU MEMORY MENU", u(PAD + 2.0), u(56.0), px(15.0), true, [1.0, 0.97, 0.94], false);

    // The list.
    let top = u(HEADER + GAP);
    let (row_h, left, width) = (u(ROW), u(PAD), w - u(2.0 * PAD));
    let scroll = ctl.scroll.min(rows.len().saturating_sub(VISIBLE));
    for (k, row) in rows.iter().skip(scroll).take(VISIBLE).enumerate() {
        let y = top + k as f32 * row_h;
        let selected = scroll + k == ctl.selected;
        if selected {
            rounded(&mut c, (left, y + u(3.0), width, row_h - u(6.0)), u(11.0), Mode::Over, |x, _| (gradient(x), 0.96));
            // A light edge along the top.
            rounded(&mut c, (left + u(8.0), y + u(4.0), width - u(16.0), u(1.5)), u(0.7), Mode::Over, |_, _| ([1.0, 1.0, 1.0], 0.35));
        }
        let (label, value) = if selected {
            (TEXT, TEXT)
        } else if row.kind == Kind::Info {
            (TEXT_DIM, TEXT_DIM)
        } else {
            (TEXT_SOFT, TEXT)
        };
        let (ty, size) = (y + (row_h - u(21.0)) / 2.0 - u(2.0), px(20.0));
        let draw = |t: &mut Text, c: &mut Canvas, s: &str, x: f32, col: [f32; 3], right: bool, bold: bool| {
            if selected {
                t.draw_shadowed(c, s, x, ty, size, bold, col, right);
            } else {
                t.draw(c, s, x, ty, size, bold, col, 1.0, right);
            }
        };
        draw(text, &mut c, &row.label, left + u(18.0), label, false, selected);
        let edge = left + width - u(18.0);
        if row.kind == Kind::Info {
            draw(text, &mut c, &row.value, edge, value, true, false);
        } else {
            // "<  value  >" in a box of fixed width: the arrows dim at an end.
            let box_w = u(168.0);
            let arrow = |on: bool| if selected { if on { [1.0, 1.0, 1.0] } else { [1.0, 1.0, 1.0] } } else if on { TEXT_SOFT } else { TEXT_DIM };
            draw(text, &mut c, "<", edge - box_w, arrow(row.left.is_some()), false, true);
            draw(text, &mut c, ">", edge, arrow(row.right.is_some()), true, true);
            let (vw, _) = text.measure(&row.value, size, true);
            let middle = edge - box_w / 2.0;
            draw(text, &mut c, &row.value, middle - vw as f32 / 2.0, value, false, true);
        }
    }
    // Where the list is scrolled to.
    if rows.len() > VISIBLE {
        let track = (w - u(11.0), top + u(6.0), u(3.0), row_h * VISIBLE as f32 - u(12.0));
        rounded(&mut c, track, u(1.5), Mode::Over, |_, _| ([1.0, 1.0, 1.0], 0.12));
        let (thumb_h, travel) = (track.3 * VISIBLE as f32 / rows.len() as f32, track.3 * (1.0 - VISIBLE as f32 / rows.len() as f32));
        let at = track.1 + travel * scroll as f32 / (rows.len() - VISIBLE) as f32;
        rounded(&mut c, (track.0, at, track.2, thumb_h), u(1.5), Mode::Over, |_, y| (gradient(y), 1.0));
    }

    // Footer: readouts.
    let foot = top + row_h * VISIBLE as f32 + u(GAP);
    rounded(&mut c, (left, foot - u(5.0), width, u(1.5)), u(0.7), Mode::Over, |x, _| (gradient(x), 0.55));
    let (label_px, value_px) = (px(14.0), px(16.0));
    let line = |t: &mut Text, c: &mut Canvas, y: f32, label: &str, value: &str, fraction: f32| {
        t.draw(c, label, left + u(2.0), y, label_px, true, TEXT_DIM, 1.0, false);
        t.draw(c, value, left + width - u(2.0), y - u(1.0), value_px, true, TEXT, 1.0, true);
        bar(c, (left + u(2.0), y + u(21.0), width - u(4.0), u(7.0)), fraction);
    };
    let used = r.gpu_used as f32 / r.gpu_budget.max(1) as f32;
    line(text, &mut c, foot + u(2.0), "GPU MEMORY", &format!("{} / {}", gb(r.gpu_used), gb(r.gpu_budget)), used);
    let (pool_text, pool_fraction) = if r.pool_mb == 0 {
        (format!("{} / Auto", gb(r.pool_used)), r.pool_used as f32 / r.gpu_budget.max(1) as f32)
    } else {
        (format!("{} / {}", gb(r.pool_used), pool_text(r.pool_mb)), r.pool_used as f32 / (r.pool_mb * MB) as f32)
    };
    line(text, &mut c, foot + u(40.0), "TEXTURE POOL", &pool_text, pool_fraction);
    let stats = format!("{:.0} FPS     {:.1} ms a frame     {} let go", r.fps, r.frame_ms, gb(r.mb_let_go * MB));
    text.draw(&mut c, &stats, left + u(2.0), foot + u(77.0), px(15.0), false, TEXT_SOFT, 1.0, false);
    text.draw(&mut c, "F10 or Back close      Up Down select      Left Right change      Tab live settings", left + u(2.0), foot + u(99.0), px(13.0), false, TEXT_DIM, 1.0, false);
    c
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
    /// left at (`x`, `y`) and `opacity`: what the blend on the card gives.
    pub(crate) fn composite(&self, backdrop: &mut [u8], bw: usize, x: usize, y: usize, opacity: f32) {
        let bh = backdrop.len() / 3 / bw;
        for j in 0..self.h {
            for i in 0..self.w {
                let (bx, by) = (x + i, y + j);
                if bx >= bw || by >= bh {
                    continue;
                }
                let p = self.px[j * self.w + i];
                let a = p[3] * opacity;
                let at = (by * bw + bx) * 3;
                for k in 0..3 {
                    let dst = f32::from(backdrop[at + k]) / 255.0;
                    backdrop[at + k] = ((p[k] * opacity + dst * (1.0 - a)).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// On the card.

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=12, b0), DescriptorTable(SRV(t0), visibility=SHADER_VISIBILITY_PIXEL), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP, visibility=SHADER_VISIBILITY_PIXEL)"
cbuffer C : register(b0) {
    float4 rect;   // left, top, width, height in pixels
    float4 target; // target width, height, opacity, gain for a linear target
    float4 misc;   // x: 1 for a linear (scRGB) target
};
Texture2D<float4> tex : register(t0);
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
[RootSignature(RS)]
float4 ps(V v) : SV_Target {
    float4 t = tex.SampleLevel(smp, v.uv, 0);
    t *= target.z;
    if (misc.x > 0.5) {
        float3 s = t.rgb / max(t.a, 1e-4);
        s = pow(max(s, 0), 2.2) * target.w;
        t.rgb = s * t.a;
    }
    return t;
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
    views: ID3D12DescriptorHeap,
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
                NumDescriptors: 1,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })?;
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
            device.CreateShaderResourceView(
                &texture,
                Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() } },
                }),
                views.GetCPUDescriptorHandleForHeapStart(),
            );
            let pitch = (size.0 * 4).next_multiple_of(256);
            let bytes = u64::from(pitch) * u64::from(size.1);
            let uploads = (0..UPLOADS).map(|_| host.upload_buffer(bytes)).collect::<Option<Vec<_>>>().ok_or_else(windows::core::Error::empty)?;
            Ok(Overlay { root, pso, format, views, texture, size, pitch, uploads, turn: 0, pending: false, fresh: true, filled: false })
        }
    }

    pub(crate) fn matches(&self, format: DXGI_FORMAT, size: (u32, u32)) -> bool {
        self.format == format && self.size == size
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
            list.ResourceBarrier(&[transition(back, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
            let rtv = host.rtv(Some(back), None);
            list.OMSetRenderTargets(1, Some(&rtv), false, None);
            list.SetDescriptorHeaps(&[Some(self.views.clone())]);
            list.SetGraphicsRootSignature(&self.root);
            list.SetPipelineState(&self.pso);
            // A float target is linear: the picture's sRGB values are put
            // through the curve and given about 200 nits (2.5 of 80).
            let linear = self.format == DXGI_FORMAT_R16G16B16A16_FLOAT;
            let constants: [f32; 12] = [
                left.round(),
                top.round(),
                self.size.0 as f32,
                self.size.1 as f32,
                width as f32,
                height as f32,
                opacity,
                2.5,
                if linear { 1.0 } else { 0.0 },
                0.0,
                0.0,
                0.0,
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
    /// The rows and readouts last painted, and when.
    shown: Option<(Vec<Row>, Controller, Readouts, u32)>,
    painted_at: Option<Instant>,
    readouts: Readouts,
    readouts_at: Option<Instant>,
    frames: u32,
    frames_since: Instant,
    /// The live settings page is shown instead of the memory page (Tab).
    page_live: bool,
    live: live::Page,
    live_extra: live::Extra,
    live_shown: Option<(live::View, u32)>,
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
            shown: None,
            painted_at: None,
            readouts: Readouts::default(),
            readouts_at: None,
            frames: 0,
            frames_since: now,
            page_live: false,
            live: live::Page::default(),
            live_extra: live::Extra::default(),
            live_shown: None,
        }
    }

    fn readouts(&mut self, now: Instant) -> Readouts {
        if self.readouts_at.is_none_or(|at| now - at >= std::time::Duration::from_millis(250)) {
            self.readouts_at = Some(now);
            let s = Snapshot::take(true);
            let ([_, copies, mb, _], _) = vram::evicted();
            let (fps, frame_ms) = (self.readouts.fps, self.readouts.frame_ms);
            self.readouts = Readouts {
                gpu_used: s.local_usage,
                gpu_budget: s.local_budget,
                pool_used: s.pool_used(),
                pool_mb: vram::pool_mb(),
                room_percent: vram::room_percent(),
                evict_on: vram::evict_on(),
                evict_allowed: vram::evict_allowed(),
                copies_let_go: copies,
                mb_let_go: mb,
                fps,
                frame_ms,
            };
        }
        self.readouts
    }

    /// Reads the keys, moves the menu and draws it onto `back`.
    pub(crate) fn draw(&mut self, host: &Host, list: &ID3D12GraphicsCommandList, back: &ID3D12Resource, (width, height): (u32, u32), format: DXGI_FORMAT) {
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
        let actions = self.keys.actions(ms, down, self.ctl.open);
        let (extra_down, shift) = live::read_keys(front);
        let extra = self.live_extra.commands(extra_down, shift, self.ctl.open);
        if !actions.is_empty() || !extra.is_empty() || self.ctl.open {
            let readouts = self.readouts(now);
            let list_rows = rows(&readouts);
            for action in actions {
                if self.page_live && self.ctl.open && action != Action::Toggle {
                    // The live settings page: Back goes to the memory page.
                    if action == Action::Close {
                        self.page_live = false;
                    } else {
                        self.live.step(action, shift);
                    }
                } else if let Some(change) = self.ctl.step(action, &list_rows) {
                    change.apply();
                    // Shown with the new value at once.
                    self.readouts_at = None;
                }
            }
            for cmd in extra {
                match cmd {
                    live::Cmd::Page => {
                        self.page_live = !self.page_live;
                        self.live.ensure();
                        (self.shown, self.live_shown) = (None, None);
                    }
                    live::Cmd::GroupPrev if self.page_live => self.live.move_group(-1),
                    live::Cmd::GroupNext if self.page_live => self.live.move_group(1),
                    live::Cmd::Reset if self.page_live => self.live.reset_selected(),
                    live::Cmd::ResetAll if self.page_live => {
                        self.live.reset_all();
                    }
                    _ => {}
                }
            }
        }
        if !self.ctl.open {
            self.page_live = false;
        }
        OPEN.store(self.ctl.open, Ordering::Relaxed);
        LATCH.store(latch(self.ctl.open, down, LATCH.load(Ordering::Relaxed)), Ordering::Relaxed);
        live::set_latch(self.ctl.open, extra_down);
        let opacity = self.fade.step(self.ctl.open, (now - self.last).as_secs_f32() * 1000.0);
        self.last = now;
        if opacity <= 0.0 || self.failed {
            return;
        }
        // The picture: made again when what it shows changed, a tenth of a second after the last.
        let readouts = self.readouts(now);
        let list_rows = rows(&readouts);
        let scale = scale_for(height);
        let size = if self.page_live { live::panel_size(scale) } else { panel_size(scale) };
        if self.overlay.as_ref().is_none_or(|o| !o.matches(format, size)) {
            match Overlay::new(host, format, size) {
                Ok(o) => {
                    if let Some(old) = self.overlay.replace(o) {
                        crate::host::retire(old);
                    }
                    (self.shown, self.live_shown) = (None, None);
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
        if self.page_live {
            // Read and compared a few times a second, not every frame.
            if due {
                let view = (self.live.view(readouts.fps, readouts.frame_ms), size.0);
                if self.live_shown.as_ref() != Some(&view) {
                    if let (Some(text), Some(overlay)) = (self.text.as_mut(), self.overlay.as_mut()) {
                        overlay.write(&live::paint(text, &view.0, scale).rgba8());
                    }
                    self.live_shown = Some(view);
                }
                self.painted_at = Some(now);
            }
        } else {
            let key = (list_rows.clone(), self.ctl, readouts, size.0);
            let stale = self.shown.as_ref() != Some(&key);
            if stale && due {
                if let (Some(text), Some(overlay)) = (self.text.as_mut(), self.overlay.as_mut()) {
                    let canvas = paint(text, &list_rows, &self.ctl, &readouts, scale);
                    overlay.write(&canvas.rgba8());
                }
                self.shown = Some(key);
                self.painted_at = Some(now);
            }
        }
        let Some(overlay) = self.overlay.as_mut() else { return };
        // Slides in from the left a few pixels as it fades in.
        let left = 48.0 * scale - (1.0 - opacity) * 28.0 * scale;
        let top = ((height as f32 - size.1 as f32) / 2.0).max(8.0);
        overlay.record(host, list, back, (width, height), (left, top), opacity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readouts() -> Readouts {
        Readouts {
            gpu_used: 21_300 * MB,
            gpu_budget: 24_170 * MB,
            pool_used: 7_900 * MB,
            pool_mb: 8192,
            room_percent: 90,
            evict_on: true,
            evict_allowed: true,
            copies_let_go: 1204,
            mb_let_go: 3480,
            fps: 58.2,
            frame_ms: 17.2,
        }
    }

    #[test]
    fn the_gradient_runs_from_the_icons_orange_to_its_violet() {
        assert_eq!(STOPS[0], [255, 152, 73]);
        assert_eq!(STOPS[4], [153, 66, 194]);
        let (a, z) = (gradient(0.0), gradient(1.0));
        assert_eq!(a.map(|v| (v * 255.0).round() as u8), ORANGE);
        assert_eq!(z.map(|v| (v * 255.0).round() as u8), VIOLET);
        // Pink at the middle stop; out-of-range values stop at the ends.
        assert_eq!(gradient(0.5).map(|v| (v * 255.0).round() as u8), PINK);
        assert_eq!(gradient(-3.0), a);
        assert_eq!(gradient(9.0), z);
        // Red falls and blue rises across it.
        assert!(gradient(0.2)[0] > gradient(0.9)[0]);
    }

    #[test]
    fn the_pool_choices_step_round_and_skip_what_does_not_fit() {
        let budget = 24_170 * MB;
        assert_eq!(pool_steps(0, budget), (20 * 1024, 4 * 1024));
        assert_eq!(pool_steps(4 * 1024, budget), (0, 6 * 1024));
        assert_eq!(pool_steps(20 * 1024, budget), (18 * 1024, 0));
        // A setting that is not on the list (a variable's) steps to its neighbours.
        assert_eq!(pool_steps(5000, budget), (4 * 1024, 6 * 1024));
        // 12 GB card: budget 11 GB: up to 8 GB are offered.
        assert_eq!(pool_steps(8 * 1024, 11 * GB), (6 * 1024, 0));
        assert_eq!(pool_steps(0, 11 * GB), (8 * 1024, 4 * 1024));
    }

    #[test]
    fn the_list_says_what_is_true_and_what_a_key_does() {
        let r = rows(&readouts());
        assert_eq!(r.len(), 6);
        assert_eq!(r[0].value, "8 GB");
        assert_eq!(r[0].left, Some(Change::Pool(6144)));
        assert_eq!(r[0].right, Some(Change::Pool(10240)));
        assert_eq!(r[1].value, "On");
        assert_eq!((r[1].left, r[1].right, r[1].enter), (Some(Change::Evict(false)), Some(Change::Evict(false)), Some(Change::Evict(false))));
        assert_eq!(r[2].value, "20.8 GB of 23.6 GB");
        assert!(r[2..].iter().all(|row| row.kind == Kind::Info && row.left.is_none() && row.right.is_none()));
        let auto = rows(&Readouts { pool_mb: 0, ..readouts() });
        assert_eq!(auto[0].value, "Auto");
        // Where eviction is off for good the line only shows.
        let off = rows(&Readouts { evict_allowed: false, evict_on: false, ..readouts() });
        assert_eq!(off[1].kind, Kind::Info);
        assert_eq!(off[1].enter, None);
        assert_eq!(pool_text(10240), "10 GB");
        assert_eq!(pool_text(6144), "6 GB");
        assert_eq!(pool_text(1536), "1.5 GB");
    }

    #[test]
    fn the_controller_opens_moves_changes_and_closes() {
        let list = rows(&readouts());
        let mut c = Controller::default();
        // Closed: only F10 does anything.
        assert_eq!(c.step(Action::Down, &list), None);
        assert_eq!(c.selected, 0);
        assert_eq!(c.step(Action::Toggle, &list), None);
        assert!(c.open);
        assert_eq!(c.step(Action::Right, &list), Some(Change::Pool(10240)));
        assert_eq!(c.step(Action::Left, &list), Some(Change::Pool(6144)));
        assert_eq!(c.step(Action::Enter, &list), None);
        assert_eq!(c.step(Action::Down, &list), None);
        assert_eq!(c.selected, 1);
        assert_eq!(c.step(Action::Enter, &list), Some(Change::Evict(false)));
        // Up from the first goes to the last, Down from the last to the first.
        c.selected = 0;
        c.step(Action::Up, &list);
        assert_eq!(c.selected, 5);
        assert_eq!(c.scroll, 1);
        c.step(Action::Down, &list);
        assert_eq!((c.selected, c.scroll), (0, 0));
        assert_eq!(c.step(Action::Close, &list), None);
        assert!(!c.open);
        c.step(Action::Toggle, &list);
        c.step(Action::Toggle, &list);
        assert!(!c.open);
    }

    #[test]
    fn the_list_scrolls_to_keep_the_selection_in_view() {
        assert_eq!(follow(0, 0, 6), 0);
        assert_eq!(follow(4, 0, 6), 0);
        assert_eq!(follow(5, 0, 6), 1);
        assert_eq!(follow(0, 1, 6), 0);
        assert_eq!(follow(3, 1, 6), 1);
        assert_eq!(follow(9, 0, 12), 5);
        // Never past the end, nothing to scroll in a short list.
        assert_eq!(follow(5, 4, 6), 1);
        assert_eq!(follow(2, 3, 3), 0);
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
    fn the_scale_follows_the_picture() {
        assert_eq!(scale_for(1080), 1.0);
        assert_eq!(scale_for(2160), 2.0);
        assert_eq!(scale_for(720), 0.625);
        assert_eq!(scale_for(200), 0.625);
        assert_eq!(scale_for(10_000), 4.0);
        let (w1, h1) = panel_size(1.0);
        let (w2, h2) = panel_size(2.0);
        assert_eq!((w2, h2), (w1 * 2, h1 * 2));
    }

    #[test]
    fn the_panel_is_painted_with_the_gradient_text_and_a_selection_bar() {
        let mut text = Text::new().expect("a memory context");
        let list = rows(&readouts());
        let ctl = Controller { open: true, selected: 1, scroll: 0 };
        let c = paint(&mut text, &list, &ctl, &readouts(), 1.0);
        assert_eq!((c.w, c.h), (600, 460));
        let at = |x: usize, y: usize| c.px[y * c.w + x];
        // The corners are cut away; the inside is nearly opaque.
        assert_eq!(at(0, 0)[3], 0.0);
        assert!(at(300, 300)[3] > 0.85);
        // The banner is the gradient: orange-ish on the left, violet-ish on the right.
        let (l, r) = (at(10, 40), at(560, 14));
        assert!(l[0] > 0.9 && l[2] < 0.6, "{l:?}");
        assert!(r[2] > 0.55 && r[0] < 0.8, "{r:?}");
        // The selected row (the second, y 98 + 46 to 98 + 92) has its bar and the others do not.
        let bar = at(24, 98 + 46 + 23);
        let plain = at(24, 98 + 23);
        assert!(bar[0] > 0.8, "bar {bar:?}");
        assert!(plain[0] < 0.2, "plain {plain:?}");
        // Text puts bright pixels where the title is.
        let lit = (20..260).flat_map(|x| (8..50).map(move |y| (x, y))).filter(|&(x, y)| {
            let p = at(x, y);
            p[0] > 0.97 && p[1] > 0.97 && p[2] > 0.97
        });
        assert!(lit.count() > 150);
        // The footer's bar carries the used share.
        let foot = 98 + 230 + 10;
        let filled = at(600 / 2 - 100, foot + 2 + 25);
        assert!(filled[3] > 0.9);
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

    /// The preview of the menu the way it looks over a picture: the panel
    /// painted and composited by the same blend the card does, saved for
    /// review. (`victory-menu-preview\menu-preview-cpu.png` in the temp folder.)
    #[test]
    fn the_preview_is_saved() {
        let mut text = Text::new().unwrap();
        let (bw, bh) = (1920usize, 1080usize);
        let mut backdrop = backdrop(bw, bh);
        let list = rows(&readouts());
        let ctl = Controller { open: true, selected: 0, scroll: 0 };
        let scale = scale_for(bh as u32);
        let c = paint(&mut text, &list, &ctl, &readouts(), scale);
        let (left, top) = (48usize, (bh - c.h) / 2);
        c.composite(&mut backdrop, bw, left, top, 1.0);
        let dir = &std::env::temp_dir().join("victory-menu-preview");
        std::fs::create_dir_all(dir).unwrap();
        let png = victory_mcp::shot::encode_png(bw as u32, bh as u32, &backdrop).unwrap();
        std::fs::write(dir.join("menu-preview-cpu.png"), &png).unwrap();
        assert!(png.len() > 10_000);
        // A second view: scrolled, the last line selected, at a 4K picture's scale.
        let ctl = Controller { open: true, selected: 5, scroll: 1 };
        let big = paint(&mut text, &list, &ctl, &readouts(), 2.0);
        let mut backdrop = self::backdrop(3840, 2160);
        big.composite(&mut backdrop, 3840, 96, (2160 - big.h) / 2, 1.0);
        let png = victory_mcp::shot::encode_png(3840, 2160, &backdrop).unwrap();
        std::fs::write(dir.join("menu-preview-4k-scrolled.png"), png).unwrap();
    }

    /// A stand-in for the game's picture: a sky that goes from deep blue to
    /// warm at the horizon, with a dark ground and some stripes.
    fn backdrop(w: usize, h: usize) -> Vec<u8> {
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

    /// The whole path on the card: the overlay draws the panel onto a texture
    /// the way it draws onto the back buffer, and the result is read back.
    #[test]
    fn the_overlay_blends_the_panel_onto_a_back_buffer() {
        let Some(host) = crate::host::host() else {
            return; // no host GPU here
        };
        let format = DXGI_FORMAT_R8G8B8A8_UNORM;
        let (bw, bh) = (1920u32, 1080u32);
        let mut text = Text::new().unwrap();
        let list_rows = rows(&readouts());
        let ctl = Controller { open: true, selected: 1, scroll: 0 };
        let scale = scale_for(bh);
        let canvas = paint(&mut text, &list_rows, &ctl, &readouts(), scale);
        let size = panel_size(scale);
        let mut overlay = Overlay::new(host, format, size).expect("the overlay");
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
        let dir = &std::env::temp_dir().join("victory-menu-preview");
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("menu-preview.png"), victory_mcp::shot::encode_png(bw, bh, &results[0].1).unwrap()).unwrap();
        std::fs::write(dir.join("menu-preview-half-faded.png"), victory_mcp::shot::encode_png(bw, bh, &results[1].1).unwrap()).unwrap();
        // The same blend on the CPU, pixel for pixel (a unit of rounding apart).
        for (opacity, rgb, top) in &results {
            let mut expect = sky.clone();
            canvas.composite(&mut expect, bw as usize, 48, *top, *opacity);
            let mut worst = 0u8;
            let mut changed = 0;
            for (a, b) in rgb.iter().zip(expect.iter()) {
                worst = worst.max(a.abs_diff(*b));
            }
            for (a, b) in rgb.iter().zip(sky.iter()) {
                changed += usize::from(a != b);
            }
            assert!(worst <= 3, "opacity {opacity}: the card's blend is {worst} off the CPU's");
            assert!(changed > 100_000, "opacity {opacity}: the panel is on the picture ({changed} values changed)");
            // Outside the panel the picture is untouched.
            assert_eq!(&rgb[..3 * 1920], &sky[..3 * 1920]);
        }
    }
}
