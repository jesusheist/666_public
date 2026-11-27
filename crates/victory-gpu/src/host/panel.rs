//! The run's numbers on the picture, in every build (`VICTORY_STATS_PANEL=1`
//! or the older `VICTORY_PANEL=1`; off by default; F2 shows and hides
//! it while our window has the focus; the time spent recording is counted in
//! diagnostics builds only). It looks like the in-game menu (F10): a rounded
//! panel of frosted glass (the card blurs the picture under it, see `menu.rs`), a
//! banner with the title "666" at the left and the frame rate (big) and frame
//! time (small) at the right, and a thin accent bar under the banner that fills
//! as the frame gets slower
//! (full at 33.3 ms). With all the lines, grouped rows follow: FRAME, CPU / GPU
//! and MEMORY (with a bar for the video memory in use), labels dim at the left of
//! each column and values bold at its right, and a small footer with what the
//! panel costs itself on the submitting thread. With the frame rate only, the
//! box is the banner and its bar, nothing else.
//!
//! The numbers are read twice a second from the counters of the frame-rate
//! line, memory and textures. The picture is painted on the CPU only when they
//! (or the corner, size or lines) change, into a texture that the menu's own
//! blitter draws onto the swap chain's buffer, behind the title's picture:
//! saved frames and the frame recorder see the title's own.
//!
//! Its look is the "Overlay" page of the in-game menu (F10): the corner
//! (`VICTORY_STATS_PANEL_CORNER`), the size (`VICTORY_STATS_PANEL_SIZE`: small,
//! normal or large, 0.8, 1 and 1.3 times the window's scale) and the lines shown
//! (`VICTORY_STATS_PANEL_LINES`: all the numbers, or the frame rate and frame
//! time only).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F2};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use super::{Host, HostList};
use crate::menu::{self, Canvas, Mode, TEXT, TEXT_DIM, Text, WHITE, band, bar, rounded};

/// How often the numbers change.
const EVERY: Duration = Duration::from_millis(500);


/// Whether it is asked for at the start: the tunable `VICTORY_STATS_PANEL`
/// (environment, live settings file or the Live settings page), or the older
/// switch `VICTORY_PANEL`.
fn asked() -> bool {
    crate::tunables::known::PANEL.get() || crate::switch::on("VICTORY_PANEL")
}

/// Whether it is shown now. F2 flips it for this run; a change of the
/// tunable (the menu's Overlay page, the Live page) sets it.
pub(super) fn shown() -> &'static AtomicBool {
    static SHOWN: OnceLock<AtomicBool> = OnceLock::new();
    SHOWN.get_or_init(|| {
        crate::tunables::known::PANEL.on_change(|on| shown().store(on, Ordering::Relaxed));
        AtomicBool::new(asked())
    })
}

/// F2 was pressed since the last call, with one of our windows in front.
fn pressed() -> bool {
    static DOWN: AtomicBool = AtomicBool::new(false);
    // SAFETY: a plain query.
    let down = unsafe { GetAsyncKeyState(i32::from(VK_F2)) } < 0;
    let edge = down && !DOWN.swap(down, Ordering::Relaxed);
    if !down {
        DOWN.store(false, Ordering::Relaxed);
    }
    if !edge {
        return false;
    }
    let mut process = 0;
    // SAFETY: plain queries; room for the answer.
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut process) };
    process == std::process::id()
}



// ---------------------------------------------------------------------------
// What is shown.

/// The numbers of the last half second, as the painter lays them out.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(super) struct Values {
    pub fps: f64,
    /// A frame's time, ms.
    pub ms: f64,
    /// The longest frame's, ms.
    pub longest: f64,
    pub draws: f64,
    pub dropped: f64,
    /// Per frame, ms.
    pub submit_busy: f64,
    pub waits_gpu: f64,
    pub gpu_busy: f64,
    pub idle_behind: f64,
    pub copy_back: f64,
    /// Time spent recording, ms a frame (diagnostics builds).
    pub recording: f64,
    /// Per frame, MB.
    pub mb_up: f64,
    pub textures: f64,
    pub back: f64,
    /// Bytes.
    pub ram: u64,
    pub in_memory: u64,
    pub vram_used: u64,
    pub vram_budget: u64,
    pub shared: u64,
    /// What the panel takes of a frame, in microseconds.
    pub cost_us: f64,
}

/// Which lines are shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lines {
    Full,
    Fps,
}

impl Lines {
    fn of(text: &str) -> Lines {
        if text == "fps" { Lines::Fps } else { Lines::Full }
    }
}

// Sizes at scale 1, in pixels.
const FULL_W: f32 = 320.0;
const FPS_W: f32 = 236.0;
const PAD: f32 = 14.0;
const RIM: f32 = 2.0;
const BANNER: f32 = 34.0;
const BAR_GAP: f32 = 5.0;
const BAR_H: f32 = 4.0;
const AFTER_BAR: f32 = 7.0;
const HEAD: f32 = 18.0;
const ROW: f32 = 18.0;
const GROUP_GAP: f32 = 6.0;
const VRAM_ROW: f32 = 30.0;
const FOOT: f32 = 16.0;
const BOTTOM: f32 = 8.0;
const COL_GAP: f32 = 14.0;
/// The margin to the picture's edge.
const MARGIN: f32 = 14.0;
/// A frame time that fills the thin bar, ms.
const SLOW_MS: f64 = 1000.0 / 30.0;

/// Items in the groups (the recording time only in diagnostics builds).
const FRAME_N: usize = 3;
const CPU_N: usize = if cfg!(feature = "diagnostics") { 6 } else { 5 };
const MEMORY_N: usize = 6;

/// Where the banner ends and the groups begin, at scale 1.
const TOP: f32 = RIM + BANNER + BAR_GAP + BAR_H + AFTER_BAR;

/// Rows of two columns for `n` items.
const fn rows_of(n: usize) -> f32 {
    n.div_ceil(2) as f32
}

/// A group's height at scale 1.
const fn group_h(n: usize, extra: f32) -> f32 {
    HEAD + extra + rows_of(n) * ROW + GROUP_GAP
}

/// The box's size at scale 1.
fn unit(lines: Lines) -> (f32, f32) {
    match lines {
        Lines::Fps => (FPS_W, TOP),
        Lines::Full => (
            FULL_W,
            TOP + group_h(FRAME_N, 0.0) + group_h(CPU_N, 0.0) + group_h(MEMORY_N, VRAM_ROW) + FOOT + BOTTOM,
        ),
    }
}

/// The box's size in pixels at `scale`: exactly what [`paint`] draws.
fn size(lines: Lines, scale: f32) -> (u32, u32) {
    let (w, h) = unit(lines);
    ((w * scale).round().max(1.0) as u32, (h * scale).round().max(1.0) as u32)
}

/// The corner as (right, bottom) from the tunable's text.
fn corner_of(text: &str) -> (bool, bool) {
    match text {
        "top-right" => (true, false),
        "bottom-left" => (false, true),
        "bottom-right" => (true, true),
        _ => (false, false),
    }
}

/// Where the box's top left goes so that it sits in `corner` of the picture,
/// `margin` pixels from its edges (never outside it).
fn place(corner: (bool, bool), (width, height): (u32, u32), (w, h): (u32, u32), margin: u32) -> (u32, u32) {
    let x = if corner.0 { width.saturating_sub(w + margin) } else { margin.min(width.saturating_sub(w)) };
    let y = if corner.1 { height.saturating_sub(h + margin) } else { margin.min(height.saturating_sub(h)) };
    (x, y)
}

/// The scale of the box: the window's scale (as the menu's: the full box is the
/// unit, so the box follows the window) times the size setting's factor, kept
/// within the picture.
fn scale(picture: (u32, u32), client: Option<(u32, u32)>, text_size: &str, lines: Lines) -> f32 {
    let (uw, uh) = unit(Lines::Full);
    let window = menu::scale_for_view(picture, client, (uw, uh));
    let factor = match text_size {
        "small" => 0.8,
        "large" => 1.3,
        _ => 1.0,
    };
    let (w, h) = unit(lines);
    let fit = (picture.0 as f32 / w).min(picture.1 as f32 / h);
    (window * factor).min(fit).max(0.25)
}

/// "74 FPS" and "13.5 ms".
fn fps_text(v: &Values) -> String {
    format!("{:.0} FPS", v.fps)
}

fn ms_text(v: &Values) -> String {
    format!("{:.1} ms", v.ms)
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
}

fn ms(v: f64) -> String {
    format!("{v:.1} ms")
}

fn mb(v: f64) -> String {
    format!("{v:.0} MB")
}

/// The groups: a name and its labels with values.
fn groups(v: &Values) -> [(&'static str, Vec<(&'static str, String)>); 3] {
    let mut cpu = vec![
        ("SUBMIT BUSY", ms(v.submit_busy)),
        ("WAITS FOR GPU", ms(v.waits_gpu)),
        ("GPU BUSY", ms(v.gpu_busy)),
        ("IDLE BEHIND US", ms(v.idle_behind)),
        ("COPY BACK", ms(v.copy_back)),
    ];
    if cfg!(feature = "diagnostics") {
        cpu.push(("RECORDING", ms(v.recording)));
    }
    [
        (
            "FRAME",
            vec![("LONGEST", ms(v.longest)), ("DRAWS", format!("{:.0}", v.draws)), ("DROPPED", format!("{:.0}", v.dropped))],
        ),
        ("CPU / GPU", cpu),
        (
            "MEMORY",
            vec![
                ("RAM", gb(v.ram)),
                ("IN MEMORY", gb(v.in_memory)),
                ("SHARED", gb(v.shared)),
                ("MB UP", mb(v.mb_up)),
                ("TEXTURES", mb(v.textures)),
                ("BACK", mb(v.back)),
            ],
        ),
    ]
}

/// Where the banner's texts go, in pixels of the box: the title's right edge, and the
/// frame rate's and frame time's left edges.
fn banner_layout(text: &mut Text, v: &Values, scale: f32, width: f32) -> (f32, f32, f32) {
    let px = |s: f32| (s * scale).round().max(1.0) as i32;
    let title_right = (PAD + 2.0) * scale + text.measure("666", px(22.0), true).0 as f32;
    let ms_w = text.measure(&ms_text(v), px(12.0), true).0 as f32;
    let fps_w = text.measure(&fps_text(v), px(22.0), true).0 as f32;
    let ms_left = width - PAD * scale - ms_w;
    (title_right, ms_left - 8.0 * scale - fps_w, ms_left)
}

/// Paints the box of `lines` at `scale`.
fn paint(text: &mut Text, v: &Values, lines: Lines, scale: f32) -> Canvas {
    let (pw, ph) = size(lines, scale);
    let mut c = Canvas::new(pw as usize, ph as usize);
    let u = |x: f32| x * scale;
    let px = |x: f32| (x * scale).round().max(1.0) as i32;
    let w = pw as f32;

    // Glass: the card blurs the picture under the box; this is the rim, the thin tint and the highlights.
    menu::paint_glass(&mut c, scale, 12.0);

    // Banner: a lighter band of the glass, the title, the frame rate and the frame time.
    band(&mut c, u(RIM), u(RIM + BANNER), |_, t| (WHITE, 0.03 + 0.03 * t));
    text.draw_shadowed(&mut c, "666", u(PAD + 2.0), u(RIM + 3.0), px(22.0), true, WHITE, false);
    let (_, fps_left, ms_left) = banner_layout(text, v, scale, w);
    text.draw(&mut c, &fps_text(v), fps_left, u(RIM + 3.0), px(22.0), true, WHITE, 1.0, false);
    text.draw(&mut c, &ms_text(v), ms_left, u(RIM + 11.0), px(12.0), true, menu::TEXT_SOFT, 1.0, false);

    // Frame time against 33.3 ms: full is slow.
    let slow = (v.ms / SLOW_MS).clamp(0.0, 1.0) as f32;
    bar(&mut c, (u(PAD), u(RIM + BANNER + BAR_GAP), w - u(2.0 * PAD), u(BAR_H)), slow);
    if lines == Lines::Fps {
        return c;
    }

    // Groups.
    let col_w = (w - u(2.0 * PAD + COL_GAP)) / 2.0;
    let mut y = u(TOP);
    for (name, items) in groups(v) {
        let name_w = text.measure(name, px(10.0), true).0 as f32;
        text.draw(&mut c, name, u(PAD), y + u(2.0), px(10.0), true, TEXT_DIM, 1.0, false);
        rounded(&mut c, (u(PAD) + name_w + u(8.0), y + u(8.0), w - u(2.0 * PAD) - name_w - u(8.0), u(1.0).max(1.0)), 0.0, Mode::Over, |_, _| (WHITE, 0.10));
        y += u(HEAD);
        if name == "MEMORY" {
            let used = v.vram_used as f32 / v.vram_budget.max(1) as f32;
            text.draw(&mut c, "VRAM", u(PAD), y, px(11.0), false, TEXT_DIM, 1.0, false);
            let value = format!("{} / {}", gb(v.vram_used), gb(v.vram_budget));
            text.draw(&mut c, &value, w - u(PAD), y - u(1.0), px(12.0), true, TEXT, 1.0, true);
            bar(&mut c, (u(PAD), y + u(17.0), w - u(2.0 * PAD), u(6.0)), used);
            y += u(VRAM_ROW);
        }
        for (i, (label, value)) in items.iter().enumerate() {
            let x0 = u(PAD) + (i % 2) as f32 * (col_w + u(COL_GAP));
            let ry = y + (i / 2) as f32 * u(ROW);
            text.draw(&mut c, label, x0, ry, px(11.0), false, TEXT_DIM, 1.0, false);
            text.draw(&mut c, value, x0 + col_w, ry - u(1.0), px(12.0), true, TEXT, 1.0, true);
        }
        y += rows_of(items.len()) * u(ROW) + u(GROUP_GAP);
    }
    let foot = format!("overlay {:.0} us a frame  |  F2 hides", v.cost_us);
    text.draw(&mut c, &foot, u(PAD), y, px(10.0), false, TEXT_DIM, 0.8, false);
    c
}

/// The counters read for it. They grow, and go back to 0 with the frame-rate
/// line (right behind a look of ours).
#[derive(Clone, Copy, Default)]
struct Counters([u64; COUNTERS]);

const COUNTERS: usize = 11;

impl Counters {
    fn now() -> Counters {
        use super::{CARD_CLOCK_US, COPY_BACK, PREPARING};
        #[cfg(feature = "diagnostics")]
        let recording = crate::copystat::peek(crate::copystat::C::RecordNs);
        #[cfg(not(feature = "diagnostics"))]
        let recording = 0;
        let get = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
        Counters([
            super::worked(),
            get(&CARD_CLOCK_US[0]),
            get(&CARD_CLOCK_US[1]),
            get(&CARD_CLOCK_US[2]),
            get(&PREPARING[3]),
            get(&COPY_BACK[0]),
            get(&COPY_BACK[1]),
            get(&crate::shadow::CARD_STATS[0]),
            crate::draw::dropped_now(),
            recording,
            crate::draw::drawn(),
        ])
    }

    /// What came since `before`, per frame.
    fn since(self, before: Counters, frames: f64) -> [f64; COUNTERS] {
        let mut out = [0.0; COUNTERS];
        for (i, out) in out.iter_mut().enumerate() {
            let (was, is) = (before.0[i], self.0[i]);
            *out = if is >= was { is - was } else { is } as f64 / frames;
        }
        out
    }
}

/// The process's memory in bytes: in physical memory, and committed
/// (private bytes).
fn process_memory() -> (u64, u64) {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: our own process and a sized structure.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (c.WorkingSetSize as u64, c.PagefileUsage as u64)
}


/// The numbers. `c`: [`Counters`] per frame, `ms` a frame's time, `worst` the
/// longest frame's, `cost` what this takes of a frame, in microseconds.
fn values(c: &[f64; COUNTERS], ms: f64, worst: f64, cost: f64, host: &Host) -> Values {
    let (working, private) = process_memory();
    let [budget, used, _, shared] = host.video_memory().unwrap_or_default();
    let idle = (c[1] / 1e3).min(ms);
    Values {
        fps: 1e3 / ms.max(0.001),
        ms,
        longest: worst,
        draws: c[10],
        dropped: c[8],
        submit_busy: c[0] / 1e3,
        waits_gpu: c[3] / 1e3,
        gpu_busy: ms - idle,
        idle_behind: c[2] / 1e3,
        copy_back: c[5] / 1e3,
        recording: c[9] / 1e6,
        mb_up: c[7] / 1e6,
        textures: c[4] / 1e6,
        back: c[6] / 1e6,
        ram: private,
        in_memory: working,
        vram_used: used,
        vram_budget: budget,
        shared,
        cost_us: cost,
    }
}

/// What the picture on the card was painted from.
#[derive(PartialEq)]
struct Painted {
    values: Values,
    lines: Lines,
    scale: u32,
    size: (u32, u32),
}

/// The panel of a presenter.
pub(super) struct Panel {
    since: Instant,
    frames: u32,
    last: Instant,
    worst: f64,
    /// What this took since the last numbers.
    cost: Duration,
    before: Counters,
    values: Option<Values>,
    painted: Option<Painted>,
    overlay: Option<menu::Overlay>,
    text: Option<Text>,
    failed: bool,
}

impl Panel {
    pub(super) fn new() -> Panel {
        let now = Instant::now();
        Panel {
            since: now,
            frames: 0,
            last: now,
            worst: 0.0,
            cost: Duration::ZERO,
            before: Counters::default(),
            values: None,
            painted: None,
            overlay: None,
            text: None,
            failed: false,
        }
    }

    /// A frame was presented; `line`: the counters go back to 0 right behind
    /// this call. (Not shown it asks for the key's state and no more.)
    pub(super) fn frame(&mut self, line: bool) {
        if pressed() {
            shown().fetch_xor(true, Ordering::Relaxed);
        }
        if !shown().load(Ordering::Relaxed) {
            self.frames = 0;
            return;
        }
        let now = Instant::now();
        if self.frames == 0 && now - self.last > 2 * EVERY {
            // (Shown from here on.)
            (self.since, self.last, self.worst, self.before) = (now, now, 0.0, Counters::now());
            self.frames = 0;
        }
        self.frames += 1;
        self.worst = self.worst.max((now - self.last).as_secs_f64() * 1e3);
        self.last = now;
        let elapsed = now - self.since;
        if elapsed < EVERY && !(line && elapsed * 2 >= EVERY) {
            return;
        }
        let Some(host) = super::host() else { return };
        let frames = f64::from(self.frames);
        let counters = Counters::now();
        let c = counters.since(self.before, frames);
        self.before = counters;
        let cost = self.cost.as_secs_f64() * 1e6 / frames;
        self.values = Some(values(&c, elapsed.as_secs_f64() * 1e3 / frames, self.worst, cost, host));
        (self.since, self.frames, self.worst) = (now, 0, 0.0);
        self.cost = now.elapsed();
    }

    /// Records the draw of the box onto `back`, a buffer of the swap chain of
    /// that size and format in the state to be copied into (and left in it).
    pub(super) fn draw(
        &mut self,
        host: &Host,
        list: &HostList,
        back: &ID3D12Resource,
        (width, height): (u32, u32),
        format: DXGI_FORMAT,
        hwnd: HWND,
    ) {
        let Some(values) = self.values else { return };
        if !shown().load(Ordering::Relaxed) || self.failed {
            return;
        }
        let began = Instant::now();
        // Not the title's: the frame recorder leaves it out.
        #[cfg(feature = "diagnostics")]
        let _ours = crate::replay::record::unrecorded();
        use crate::tunables::known::{STATS_PANEL_CORNER, STATS_PANEL_LINES, STATS_PANEL_SIZE};
        let lines = Lines::of(STATS_PANEL_LINES.get_str());
        let scale = scale((width, height), menu::client_size(hwnd), STATS_PANEL_SIZE.get_str(), lines);
        let size = size(lines, scale);
        if self.overlay.as_ref().is_none_or(|o| !o.matches(format, size)) {
            match menu::Overlay::new(host, format, size) {
                Ok(o) => {
                    if let Some(old) = self.overlay.replace(o) {
                        super::retire(old);
                    }
                    self.painted = None;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "the stats panel is not drawn: its pipeline was not made");
                    self.failed = true;
                    return;
                }
            }
        }
        if self.text.is_none() {
            self.text = Text::new();
        }
        let want = Painted { values, lines, scale: scale.to_bits(), size };
        if self.painted.as_ref() != Some(&want) {
            if let (Some(text), Some(overlay)) = (self.text.as_mut(), self.overlay.as_mut()) {
                overlay.write(&paint(text, &values, lines, scale).rgba8());
                self.painted = Some(want);
            }
        }
        let margin = (MARGIN * scale).round() as u32;
        let (x, y) = place(corner_of(STATS_PANEL_CORNER.get_str()), (width, height), size, margin);
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.look(12.0 * scale, 24.0 * scale);
            overlay.record(host, &list.list, back, (width, height), (x as f32, y as f32), 1.0);
        }
        self.cost += began.elapsed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::tests::backdrop;

    fn sample() -> Values {
        Values {
            fps: 74.0,
            ms: 13.5,
            longest: 65.7,
            draws: 8123.0,
            dropped: 2.0,
            submit_busy: 9.8,
            waits_gpu: 0.4,
            gpu_busy: 12.1,
            idle_behind: 1.4,
            copy_back: 0.2,
            recording: 0.0,
            mb_up: 31.0,
            textures: 18.0,
            back: 4.0,
            ram: 21_500_000_000,
            in_memory: 14_800_000_000,
            vram_used: 17_300_000_000,
            vram_budget: 24_000_000_000,
            shared: 2_100_000_000,
            cost_us: 63.0,
        }
    }

    #[test]
    fn the_group_counts_match_the_size() {
        let g = groups(&sample());
        assert_eq!((g[0].1.len(), g[1].1.len(), g[2].1.len()), (FRAME_N, CPU_N, MEMORY_N));
    }

    /// With the frame rate only, the box is just the banner and its bar.
    #[test]
    fn the_fps_box_is_much_smaller_and_has_no_second_row() {
        let (fw, fh) = size(Lines::Full, 1.0);
        let (sw, sh) = size(Lines::Fps, 1.0);
        assert!(sh * 4 < fh && sw < fw, "{sw}x{sh} against {fw}x{fh}");
        assert_eq!(sh as f32, TOP);
        assert_eq!(Lines::of("fps"), Lines::Fps);
        assert_eq!(Lines::of("full"), Lines::Full);
        assert_eq!(Lines::of("anything else"), Lines::Full);
        let mut text = Text::new().unwrap();
        let c = paint(&mut text, &sample(), Lines::Fps, 1.0);
        assert_eq!((c.w as u32, c.h as u32), (sw, sh));
        // Below the thin bar there is no box at all: the last row of pixels is the rim, not a cut line.
        let last = &c.px[(c.h - 1) * c.w..];
        assert!(last[c.w / 2][3] > 0.03, "the box ends with its own rim");
        // The banner's texts do not run into each other.
        for scale in [0.8, 1.0, 1.3, 2.0] {
            let (_, h) = size(Lines::Fps, scale);
            let c = paint(&mut text, &sample(), Lines::Fps, scale);
            assert_eq!(c.h as u32, h);
            let worst = Values { fps: 999.0, ms: 99.9, ..sample() };
            let (title, fps_left, ms_left) = banner_layout(&mut text, &worst, scale, c.w as f32);
            assert!(title + 4.0 * scale <= fps_left && fps_left < ms_left, "{title} {fps_left} {ms_left} at {scale}");
        }
    }

    #[test]
    fn the_canvas_matches_the_size_exactly() {
        let mut text = Text::new().unwrap();
        for lines in [Lines::Full, Lines::Fps] {
            for scale in [0.8, 1.0, 1.3, 2.0] {
                let c = paint(&mut text, &sample(), lines, scale);
                let (w, h) = size(lines, scale);
                assert_eq!((c.w as u32, c.h as u32), (w, h), "{lines:?} at {scale}");
            }
        }
        // Full is where the layout ends: the footer lies inside the box.
        let (_, h) = unit(Lines::Full);
        assert!(h > 300.0 && h < 360.0, "{h}");
    }

    #[test]
    fn the_box_sits_in_each_corner_inside_the_picture() {
        for pic in [(1280, 720), (1920, 1080), (3840, 2160)] {
            for text_size in ["small", "normal", "large"] {
                for lines in [Lines::Full, Lines::Fps] {
                    let s = scale(pic, Some(pic), text_size, lines);
                    let (w, h) = size(lines, s);
                    let m = (MARGIN * s).round() as u32;
                    for corner in ["top-left", "top-right", "bottom-left", "bottom-right"] {
                        let (x, y) = place(corner_of(corner), pic, (w, h), m);
                        assert!(x + w <= pic.0 && y + h <= pic.1, "{corner} {pic:?} {text_size} {lines:?}");
                        let (right, bottom) = corner_of(corner);
                        assert_eq!(x, if right { pic.0 - w - m } else { m });
                        assert_eq!(y, if bottom { pic.1 - h - m } else { m });
                    }
                }
            }
        }
        // Too big for the picture: it stays at the edge, never negative.
        assert_eq!(place((true, true), (100, 50), (300, 200), 14), (0, 0));
    }

    #[test]
    fn the_scale_follows_the_size_setting_and_the_window() {
        let s = |t: &str, p, c| scale(p, c, t, Lines::Full);
        let p = (1920, 1080);
        let (small, normal, large) = (s("small", p, Some(p)), s("normal", p, Some(p)), s("large", p, Some(p)));
        assert!((small / normal - 0.8).abs() < 0.01, "{small} {normal}");
        assert!((large / normal - 1.3).abs() < 0.01, "{large} {normal}");
        assert!((normal - 1.0).abs() < 0.01);
        // Another word is normal.
        assert_eq!(s("whatever", p, Some(p)), normal);
        // A 4K picture stretched to a 1080p window: the same share of the window.
        let big = s("normal", (3840, 2160), Some((1920, 1080)));
        assert!((big / 2.0 - normal).abs() < 0.01, "{big}");
        // A small window gives a smaller box, but never an unreadable one.
        let tiny = s("small", (640, 360), Some((640, 360)));
        assert!(tiny >= 0.25 && tiny < small);
    }

    #[test]
    fn counters_that_went_back_to_nothing_count_from_there() {
        let (mut a, mut b) = (Counters::default(), Counters::default());
        (a.0[0], b.0[0]) = (10, 30);
        (a.0[1], b.0[1]) = (50, 20);
        let c = b.since(a, 10.0);
        assert_eq!((c[0], c[1]), (2.0, 2.0));
    }

    /// The boxes laid over a stand-in picture the way the card blends them,
    /// saved for review.
    #[test]
    fn the_preview_is_saved() {
        let mut text = Text::new().unwrap();
        let dir = &std::env::var_os("VICTORY_PREVIEW_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("victory-previews"));
        std::fs::create_dir_all(dir).unwrap();
        let v = sample();
        let save = |text: &mut Text, name: &str, (bw, bh): (usize, usize), lines: Lines, corner: &str, size_text: &str| {
            let mut pic = backdrop(bw, bh);
            let s = scale((bw as u32, bh as u32), Some((bw as u32, bh as u32)), size_text, lines);
            let c = paint(text, &v, lines, s);
            let m = (MARGIN * s).round() as u32;
            let (x, y) = place(corner_of(corner), (bw as u32, bh as u32), (c.w as u32, c.h as u32), m);
            c.composite(&mut pic, bw, x as usize, y as usize, 1.0, s);
            let png = victory_mcp::shot::encode_png(bw as u32, bh as u32, &pic).unwrap();
            assert!(png.len() > 10_000);
            std::fs::write(dir.join(name), png).unwrap();
        };
        save(&mut text, "menu4-hud-full.png", (1920, 1080), Lines::Full, "top-left", "normal");
        save(&mut text, "menu4-hud-fps.png", (1920, 1080), Lines::Fps, "top-right", "normal");
        save(&mut text, "menu4-hud-4k.png", (3840, 2160), Lines::Full, "bottom-right", "normal");
    }
}
