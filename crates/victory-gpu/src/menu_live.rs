//! The "Advanced" page of the F10 menu: every tunable of the registry
//! (`victory_core::tunables`), grouped by the first word of its name, one
//! group at a time.
//!
//! Keys (the menu's own keys keep their meaning): Tab and Shift+Tab switch
//! the page; Page Up and Page Down switch the group; Up and
//! Down select; Left and Right change the value (Shift: ten times the step);
//! Enter does what Right does; Delete puts the selected one back to its
//! default, Shift and Delete all of them; Esc, Back, F10 or F1 closes the menu.
//!
//! After the tunables of a group come three lines that Enter does: "Save and
//! restart now", "Save for next start" and "Reset all" (see [`Do`]). Every
//! change is already kept in the live settings file by the registry; saving
//! writes the file now, and "Save and restart now" then starts the program
//! again with the same command line and environment and ends this run, so
//! that the values of the Restart class (waiting as pending) are the running
//! ones. A value that is also set in the environment keeps winning there.
//!
//! A change goes through the registry's `set_text`, so its range check, its
//! live class (applied at once, at the next frame, at the next start) and the
//! live settings file are the same as for the debug tools. A tunable that is
//! not at its default has a `*` in front, one with a value waiting for a
//! frame or a start has a `!` and says so in its class column.
//!
//! The list of tunables is made once, the first time the page is shown.
//! Closed, the page costs nothing; open, the lines in view are read about ten
//! times a second and painted only when they changed.

use victory_core::tunables::{self as reg, Live, Slot};

use super::{ACCENT, Action, Canvas, TEXT, TEXT_DIM, TEXT_SOFT, Tab, Text, WARM, WHITE, chrome, latch, paint_selected, rule, scrollbar};

// ---------------------------------------------------------------------------
// Values: what a key does to a value, apart from the registry.

/// `10^k` for a whole `k`, exact for the small powers used here.
fn pow10(k: i32) -> f64 {
    if k >= 0 { 10f64.powi(k) } else { 1.0 / 10f64.powi(-k) }
}

/// The largest power of ten that is at most `raw` (`raw` above 0).
fn floor_pow10(raw: f64) -> f64 {
    let mut k = 0i32;
    let limit = raw * (1.0 + 1e-9);
    while pow10(k + 1) <= limit && k < 300 {
        k += 1;
    }
    while pow10(k) > limit && k > -300 {
        k -= 1;
    }
    pow10(k)
}

/// A step of a whole number: a power of ten, a hundredth of the range at
/// most (1 for a short range).
pub(crate) fn int_step(min: i64, max: i64) -> i64 {
    let range = (i128::from(max) - i128::from(min)).max(1);
    let raw = (range / 100).max(1);
    let mut step = 1i128;
    while step * 10 <= raw {
        step *= 10;
    }
    step as i64
}

/// A step of a number with a fraction: a power of ten, about a hundredth of
/// the range (1 where the range is not usable).
pub(crate) fn float_step(min: f64, max: f64) -> f64 {
    let range = max - min;
    if !range.is_finite() || range <= 0.0 {
        return 1.0;
    }
    floor_pow10(range / 100.0)
}

/// The text `current` becomes one step `dir` (1 up, -1 down) on, or `None`
/// where there is nowhere to go (a number at its end, an empty list). A
/// switch flips, a choice goes round, a number steps by [`int_step`] or
/// [`float_step`] (ten times that with `big`) and stops at its range's ends;
/// a fraction first lands on the step's grid.
pub(crate) fn step_text(kind: reg::Kind, current: &str, dir: i32, big: bool) -> Option<String> {
    let mult = if big { 10.0 } else { 1.0 };
    match kind {
        reg::Kind::Bool => Some(if current.trim().eq_ignore_ascii_case("true") { "false" } else { "true" }.to_owned()),
        reg::Kind::Choice(list) => {
            if list.is_empty() {
                return None;
            }
            let at = list.iter().position(|c| *c == current).map_or(if dir > 0 { list.len() - 1 } else { 0 }, |i| i);
            let next = (at as i64 + i64::from(dir)).rem_euclid(list.len() as i64) as usize;
            Some(list[next].to_owned())
        }
        reg::Kind::Int { min, max } => {
            let cur: i64 = current.trim().parse().ok()?;
            let step = i128::from(int_step(min, max)) * if big { 10 } else { 1 };
            let next = (i128::from(cur) + step * i128::from(dir)).clamp(i128::from(min), i128::from(max)) as i64;
            (next != cur).then(|| next.to_string())
        }
        reg::Kind::Float { min, max } => {
            let cur: f64 = current.trim().parse().ok()?;
            if !cur.is_finite() {
                return None;
            }
            let step = float_step(min, max) * mult;
            let q = cur / step;
            let n = if (q - q.round()).abs() < 1e-6 {
                q.round() + f64::from(dir)
            } else if dir > 0 {
                q.ceil()
            } else {
                q.floor()
            };
            let next = (n * step).clamp(min, max);
            let next = (next * 1e10).round() / 1e10;
            ((next - cur).abs() > step * 1e-9).then(|| next.to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Groups.

/// Groups with fewer members than this are put together under [`MISC`].
const SMALL: usize = 3;
const MISC: &str = "MISC";

/// The first word of a name, upper case.
pub(crate) fn prefix(key: &str) -> String {
    key.split('_').next().unwrap_or(key).to_ascii_uppercase()
}

/// The groups of `keys` (sorted names): each as its title and the indexes of
/// its members, in order. Titles go alphabetically, [`MISC`] (the first words
/// that fewer than [`SMALL`] names share) last.
pub(crate) fn build_groups(keys: &[String]) -> Vec<(String, Vec<usize>)> {
    let prefixes: Vec<String> = keys.iter().map(|k| prefix(k)).collect();
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for p in &prefixes {
        *counts.entry(p.as_str()).or_default() += 1;
    }
    let mut groups: std::collections::BTreeMap<String, Vec<usize>> = std::collections::BTreeMap::new();
    let mut misc = Vec::new();
    for (i, p) in prefixes.iter().enumerate() {
        if counts[p.as_str()] < SMALL { misc.push(i) } else { groups.entry(p.clone()).or_default().push(i) }
    }
    let mut out: Vec<(String, Vec<usize>)> = groups.into_iter().collect();
    if !misc.is_empty() {
        out.push((MISC.to_owned(), misc));
    }
    out
}

// ---------------------------------------------------------------------------
// The page.

/// Lines of the list shown at once.
pub(crate) const ROWS: usize = 12;

/// The first line shown so that `selected` is, moving as little as it can.
pub(crate) fn follow(selected: usize, scroll: usize, total: usize) -> usize {
    follow_in(selected, scroll, total, ROWS)
}

/// [`follow`] for a list that shows `visible` lines.
pub(crate) fn follow_in(selected: usize, scroll: usize, total: usize, visible: usize) -> usize {
    let most = total.saturating_sub(visible);
    let scroll = if selected < scroll {
        selected
    } else if selected >= scroll + visible {
        selected + 1 - visible
    } else {
        scroll
    };
    scroll.min(most)
}

struct Entry {
    slot: &'static Slot,
}

/// The lines after the tunables of a group, and what each does.
pub(crate) const ACTIONS: [(&str, &str); 3] = [
    ("Save and restart now", "Writes the settings to the live settings file, then starts the program again with the same arguments and environment and ends this run. Restart values apply."),
    ("Save for next start", "Writes the settings to the live settings file now. Restart values apply at the next start."),
    ("Reset all", "Puts every tunable back to its default and forgets it in the live settings file."),
];

/// What an action line does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Do {
    SaveRestart,
    Save,
    ResetAll,
}

impl Do {
    fn from_index(i: usize) -> Do {
        match i {
            0 => Do::SaveRestart,
            1 => Do::Save,
            _ => Do::ResetAll,
        }
    }
}

/// `text` as the content of a PowerShell single-quoted string.
fn ps_quote(text: &str) -> String {
    text.replace('\'', "''")
}

/// The PowerShell command that starts `exe` again with `args` a few seconds
/// on (the old run has let go of the window and its files by then).
pub(crate) fn restart_command(exe: &str, args: &[String]) -> String {
    let mut command = format!("Start-Sleep 3; & '{}'", ps_quote(exe));
    for a in args {
        command.push_str(&format!(" '{}'", ps_quote(a)));
    }
    command
}

/// Writes the live settings file now; whether it could, and what to tell. The
/// note says how many values wait for a start and how many of those an
/// environment variable overrides. `slots` is every tunable.
pub(crate) fn save(slots: &[&'static Slot]) -> (bool, String) {
    let Some(path) = reg::file_path() else {
        return (false, "No settings file: set VICTORY_DATA_DIR or VICTORY_LIVE_SETTINGS".to_owned());
    };
    reg::flush();
    if !path.exists() && slots.iter().all(|s| !is_changed(s)) {
        return (true, "Nothing to save: all at the defaults".to_owned());
    }
    let waiting = slots.iter().filter(|s| s.needs_restart()).count();
    let overridden = slots.iter().filter(|s| s.needs_restart() && std::env::var_os(s.meta().env).is_some()).count();
    let note = match (waiting, overridden) {
        (0, _) => "Saved".to_owned(),
        (w, 0) => format!("Saved, {w} apply at the next start"),
        (w, o) => format!("Saved, {w} apply at the next start ({o} set in the environment keep winning)"),
    };
    (true, note)
}

/// Starts this program again, as it was started, in a new console a few seconds on, and ends
/// this run. The switches the runtime put into the environment from the saved file at its start
/// are left out of the new run's environment (the file holds their value, and a variable would
/// win over it). Returns only when it could not, with what to tell.
pub(crate) fn restart() -> Option<String> {
    use std::os::windows::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else {
        return Some("Saved, but the program's own path is not known: start it again yourself".to_owned());
    };
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let command = restart_command(&exe.to_string_lossy(), &args);
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    let mut process = std::process::Command::new("powershell.exe");
    process.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &command]).creation_flags(CREATE_NEW_CONSOLE);
    for name in reg::exported_env() {
        process.env_remove(name);
    }
    match process.spawn() {
        Ok(_) => {
            tracing::info!(command, "live settings: restarting");
            // Ended at once: std::process::exit runs the DLLs' unloading, which may not come back.
            // SAFETY: this process, with nothing of its own left to save (the file was written).
            unsafe {
                use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
                TerminateProcess(GetCurrentProcess(), 0);
            }
            std::process::exit(0);
        }
        Err(e) => {
            tracing::warn!(error = %e, "live settings: restart failed");
            Some("Saved, but the restart failed: start it again yourself".to_owned())
        }
    }
}

/// What a line shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Line {
    pub name: String,
    pub value: String,
    pub default: String,
    pub class: &'static str,
    pub changed: bool,
    /// A value waits for the next frame or the next start.
    pub pending: bool,
    pub selected: bool,
    pub can_down: bool,
    pub can_up: bool,
    /// An action line (it has no value).
    pub action: bool,
}

/// What the page shows, to be compared with what was painted last.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct View {
    pub group: String,
    pub group_at: (usize, usize),
    pub lines: Vec<Line>,
    pub total: usize,
    pub scroll: usize,
    pub desc: String,
    pub env: String,
    /// The lines of the list with the action lines after them (`total` counts the group's tunables).
    pub rows: usize,
    pub note: String,
    pub changed_all: usize,
    pub fps_x10: u32,
    pub ms_x100: u32,
}

fn class_text(live: Live) -> &'static str {
    match live {
        Live::Safe => "Safe",
        Live::NextFrame => "Next frame",
        Live::Restart => "Restart",
    }
}

pub(crate) fn is_changed(slot: &Slot) -> bool {
    slot.value_text() != slot.default_text() || slot.pending_text().is_some()
}

/// The page's state: the groups, which one is shown and which line is selected.
#[derive(Default)]
pub(crate) struct Page {
    entries: Vec<Entry>,
    groups: Vec<(String, Vec<usize>)>,
    group: usize,
    selected: usize,
    scroll: usize,
    /// What the last action said.
    note: String,
}

impl Page {
    /// Reads the registry the first time.
    pub(crate) fn ensure(&mut self) {
        if self.entries.is_empty() {
            self.fill(reg::list());
        }
    }

    fn fill(&mut self, slots: Vec<&'static Slot>) {
        let keys: Vec<String> = slots.iter().map(|s| s.key()).collect();
        self.groups = build_groups(&keys);
        self.entries = slots.into_iter().map(|slot| Entry { slot }).collect();
        (self.group, self.selected, self.scroll) = (0, 0, 0);
    }

    fn members(&self) -> &[usize] {
        self.groups.get(self.group).map_or(&[], |g| g.1.as_slice())
    }

    fn current(&self) -> Option<&Entry> {
        self.members().get(self.selected).map(|&i| &self.entries[i])
    }

    /// Another group, `by` places on, going round.
    pub(crate) fn move_group(&mut self, by: i64) {
        if self.groups.is_empty() {
            return;
        }
        self.group = (self.group as i64 + by).rem_euclid(self.groups.len() as i64) as usize;
        (self.selected, self.scroll) = (0, 0);
    }

    /// Does what `action` asks (Up, Down, Left, Right, Enter; the others are
    /// the menu's); `big` is the Shift key. Enter on an action line answers
    /// what it asks for (the caller runs it with [`Page::run`]).
    pub(crate) fn step(&mut self, action: Action, big: bool) -> Option<Do> {
        let n = self.members().len();
        let total = n + ACTIONS.len();
        self.selected = self.selected.min(total - 1);
        let mut asked = None;
        match action {
            Action::Up => self.selected = (self.selected + total - 1) % total,
            Action::Down => self.selected = (self.selected + 1) % total,
            Action::Left if self.selected < n => self.change(-1, big),
            Action::Right if self.selected < n => self.change(1, big),
            Action::Enter if self.selected < n => self.change(1, big),
            Action::Enter => asked = Some(Do::from_index(self.selected - n)),
            Action::Left | Action::Right | Action::Toggle | Action::Close => {}
        }
        if asked.is_none() {
            self.note.clear();
        }
        self.scroll = follow(self.selected, self.scroll, total);
        asked
    }

    /// Does an action line. Saving writes the live settings file now;
    /// restarting then relaunches the program and ends this run.
    pub(crate) fn run(&mut self, what: Do) {
        match what {
            Do::Save => {
                self.save();
            }
            Do::SaveRestart => {
                if self.save() {
                    self.restart();
                }
            }
            Do::ResetAll => {
                let n = self.reset_all();
                reg::flush();
                self.note = if n == 0 { "Nothing was changed".to_owned() } else { format!("{n} back to default and saved") };
            }
        }
    }

    /// Writes the live settings file now; whether it could.
    fn save(&mut self) -> bool {
        let slots: Vec<&'static Slot> = self.entries.iter().map(|e| e.slot).collect();
        let (ok, note) = save(&slots);
        self.note = note;
        ok
    }

    /// Starts this program again (see [`restart`]); the note says if it could not.
    fn restart(&mut self) {
        if let Some(note) = restart() {
            self.note = note;
        }
    }

    fn change(&self, dir: i32, big: bool) {
        let Some(e) = self.current() else { return };
        let base = e.slot.pending_text().unwrap_or_else(|| e.slot.value_text());
        if let Some(text) = step_text(e.slot.meta().kind, &base, dir, big)
            && let Err(err) = e.slot.set_text(&text)
        {
            tracing::debug!(target: "victory::tunables", "menu: {err}");
        }
    }

    /// The selected one back to its default.
    pub(crate) fn reset_selected(&self) {
        if let Some(e) = self.current() {
            e.slot.reset();
        }
    }

    /// All of them back to their defaults; how many were changed.
    pub(crate) fn reset_all(&self) -> usize {
        let mut n = 0;
        for e in &self.entries {
            if is_changed(e.slot) {
                e.slot.reset();
                n += 1;
            }
        }
        n
    }

    /// What to paint now; `fps` and `ms` are the frame rate and frame time.
    pub(crate) fn view(&self, fps: f32, ms: f32) -> View {
        let members = self.members();
        let n = members.len();
        let rows = n + ACTIONS.len();
        let selected = self.selected.min(rows - 1);
        let scroll = self.scroll.min(rows.saturating_sub(ROWS));
        let mut lines = Vec::with_capacity(ROWS.min(rows));
        for k in scroll..rows.min(scroll + ROWS) {
            if k >= n {
                lines.push(Line { name: ACTIONS[k - n].0.to_owned(), selected: k == selected, action: true, ..Line::default() });
                continue;
            }
            let slot = self.entries[members[k]].slot;
            let meta = slot.meta();
            let pending = slot.pending_text();
            let value = pending.clone().unwrap_or_else(|| slot.value_text());
            let can = |dir: i32| step_text(meta.kind, &value, dir, false).is_some();
            lines.push(Line {
                name: slot.key(),
                changed: pending.is_some() || slot.value_text() != slot.default_text(),
                pending: pending.is_some(),
                default: slot.default_text(),
                class: class_text(meta.live),
                selected: k == selected,
                can_down: can(-1),
                can_up: can(1),
                value,
                action: false,
            });
        }
        let (desc, env) = if selected >= n {
            (ACTIONS[selected - n].1.to_owned(), String::new())
        } else {
            self.current().map_or_else(Default::default, |e| (e.slot.meta().desc.to_owned(), e.slot.meta().env.to_owned()))
        };
        View {
            group: self.groups.get(self.group).map(|g| g.0.clone()).unwrap_or_default(),
            group_at: (self.group + 1, self.groups.len()),
            lines,
            total: n,
            rows,
            note: self.note.clone(),
            scroll,
            desc,
            env,
            changed_all: self.entries.iter().filter(|e| is_changed(e.slot)).count(),
            fps_x10: (fps * 10.0).round() as u32,
            ms_x100: (ms * 100.0).round() as u32,
        }
    }
}

// ---------------------------------------------------------------------------
// The keys this page adds to the menu's.

/// Tab, Page Up, Page Down, Delete.
pub(crate) const EXTRA: [u8; 4] = [0x09, 0x21, 0x22, 0x2E];
const SHIFT: i32 = 0x10;

/// What the extra keys ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cmd {
    PageNext,
    PagePrev,
    GroupPrev,
    GroupNext,
    Reset,
    ResetAll,
}

/// Presses of the extra keys, from their states each frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Extra {
    down: [bool; 4],
}

impl Extra {
    /// The commands the states `down` mean now: a key counts once when it
    /// goes down; only while the menu is `open` (`shift` makes Delete reset all and Tab go back a page).
    pub(crate) fn commands(&mut self, down: [bool; 4], shift: bool, open: bool) -> Vec<Cmd> {
        let mut out = Vec::new();
        for i in 0..EXTRA.len() {
            if down[i] && !self.down[i] && open {
                out.push(match i {
                    0 if shift => Cmd::PagePrev,
                    0 => Cmd::PageNext,
                    1 => Cmd::GroupPrev,
                    2 => Cmd::GroupNext,
                    _ if shift => Cmd::ResetAll,
                    _ => Cmd::Reset,
                });
            }
        }
        self.down = down;
        out
    }
}

/// Reads the extra keys and Shift (all up where our window is not in front).
pub(crate) fn read_keys(front: bool) -> ([bool; 4], bool) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    let mut down = [false; 4];
    if !front {
        return (down, false);
    }
    for (d, &vk) in down.iter_mut().zip(EXTRA.iter()) {
        // SAFETY: a plain query.
        *d = unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
    }
    // SAFETY: a plain query.
    (down, unsafe { GetAsyncKeyState(SHIFT) } < 0)
}

static LATCH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Updates which extra keys the title must not see (see [`latch`]).
pub(crate) fn set_latch(open: bool, down: [bool; 4]) {
    use std::sync::atomic::Ordering::Relaxed;
    let mut wide = [false; 8];
    wide[..4].copy_from_slice(&down);
    LATCH.store(latch(open, wide, LATCH.load(Relaxed)), Relaxed);
}

/// Whether the title must not see the extra key `virtual_key` now.
pub(crate) fn takes_key(virtual_key: u8, open: bool) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let Some(i) = EXTRA.iter().position(|&k| k == virtual_key) else { return false };
    open || LATCH.load(Relaxed) & (1 << i) != 0
}

// ---------------------------------------------------------------------------
// Painting.

/// Sizes at scale 1 (1,080 lines).
pub(crate) const W: f32 = 900.0;
pub(crate) const PAD: f32 = 20.0;
pub(crate) const HEADER: f32 = 78.0;
pub(crate) const ROW: f32 = 32.0;
pub(crate) const GAP: f32 = 10.0;
pub(crate) const FOOTER: f32 = 118.0;
pub(crate) const H: f32 = HEADER + GAP + ROW * ROWS as f32 + GAP + FOOTER;

/// The panel's size in pixels at `scale`.
pub(crate) fn panel_size(scale: f32) -> (u32, u32) {
    ((W * scale).round() as u32, (H * scale).round() as u32)
}

/// `text` cut with ".." at the end to fit `max` pixels.
pub(crate) fn fit(t: &mut Text, text: &str, px: i32, bold: bool, max: f32) -> String {
    if (t.measure(text, px, bold).0 as f32) <= max {
        return text.to_owned();
    }
    let mut s: String = text.to_owned();
    while s.pop().is_some() {
        let cut = format!("{s}..");
        if (t.measure(&cut, px, bold).0 as f32) <= max {
            return cut;
        }
    }
    String::new()
}

/// `text` over at most two lines of `max` pixels, the second cut to fit.
pub(crate) fn wrap2(t: &mut Text, text: &str, px: i32, max: f32) -> [String; 2] {
    let mut first = String::new();
    let mut rest = String::new();
    for word in text.split_whitespace() {
        if !rest.is_empty() {
            rest.push(' ');
            rest.push_str(word);
            continue;
        }
        let try_line = if first.is_empty() { word.to_owned() } else { format!("{first} {word}") };
        if first.is_empty() || (t.measure(&try_line, px, false).0 as f32) <= max {
            first = try_line;
        } else {
            rest = word.to_owned();
        }
    }
    let second = fit(t, &rest, px, false, max);
    [first, second]
}

/// Paints the page for `v` at `scale`.
pub(crate) fn paint(text: &mut Text, v: &View, scale: f32) -> Canvas {
    let (pw, ph) = panel_size(scale);
    let mut c = Canvas::new(pw as usize, ph as usize);
    let u = |x: f32| x * scale;
    let px = |x: f32| (x * scale).round().max(1.0) as i32;
    let w = pw as f32;

    // Glass, the header with the title and the tabs, the group and the tally at its right.
    let group = format!("{}   {} of {}", v.group, v.group_at.0, v.group_at.1);
    let tally = if v.changed_all == 0 { format!("none changed, {} in this group", v.total) } else { format!("{} changed of the {} in this group", v.changed_all, v.total) };
    chrome(text, &mut c, scale, HEADER, Tab::Advanced, &group, &tally);

    // The list.
    let top = u(HEADER + GAP);
    let (row_h, left, width) = (u(ROW), u(PAD), w - u(2.0 * PAD));
    let (size, small) = (px(16.0), px(13.0));
    let name_max = u(300.0);
    let (box_left, box_right) = (left + u(334.0), left + u(520.0));
    let default_right = left + u(690.0);
    let class_right = left + width - u(14.0);
    for (k, line) in v.lines.iter().enumerate() {
        let y = top + k as f32 * row_h;
        if line.selected {
            paint_selected(&mut c, scale, (left, y + u(2.0), width, row_h - u(4.0)));
        }
        let (name_col, value_col, dim) = if line.selected { (WHITE, WHITE, TEXT_SOFT) } else { (TEXT_SOFT, TEXT, TEXT_DIM) };
        let ty = y + (row_h - u(17.0)) / 2.0 - u(2.0);
        if line.action {
            // An action line: its name, bold, and what Enter does to it.
            let col = if line.selected { WHITE } else { WARM };
            text.draw(&mut c, &line.name, left + u(24.0), ty, size, true, col, 1.0, false);
            if line.selected {
                text.draw(&mut c, "Enter", left + width - u(14.0), ty, small, true, ACCENT, 1.0, true);
            }
            continue;
        }
        let put = |t: &mut Text, c: &mut Canvas, s: &str, x: f32, px_: i32, col: [f32; 3], right: bool, bold: bool| {
            t.draw(c, s, x, ty, px_, bold, col, 1.0, right);
        };
        if line.pending {
            put(text, &mut c, "!", left + u(9.0), size, WARM, false, true);
        } else if line.changed {
            put(text, &mut c, "*", left + u(9.0), size, ACCENT, false, true);
        }
        let name = fit(text, &line.name, size, line.selected, name_max);
        put(text, &mut c, &name, left + u(24.0), size, name_col, false, line.selected);
        // "<  value  >": the arrows are accent where the value can go on and dim where it cannot.
        let arrow = |on: bool| if on { ACCENT } else { TEXT_DIM };
        put(text, &mut c, "<", box_left, size, arrow(line.can_down), false, true);
        put(text, &mut c, ">", box_right, size, arrow(line.can_up), true, true);
        let value = fit(text, &line.value, size, true, box_right - box_left - u(40.0));
        let (vw, _) = text.measure(&value, size, true);
        put(text, &mut c, &value, (box_left + box_right) / 2.0 - vw as f32 / 2.0, size, value_col, false, true);
        let default = fit(text, &format!("default {}", line.default), small, false, u(120.0));
        put(text, &mut c, &default, default_right, small, dim, true, false);
        let class = match (line.pending, line.class) {
            (true, "Restart") => "Restart, pending".to_owned(),
            (true, c) => format!("{c}, pending"),
            (false, c) => c.to_owned(),
        };
        let class_col = if line.pending { WARM } else { dim };
        put(text, &mut c, &class, class_right, small, class_col, true, false);
    }
    scrollbar(&mut c, scale, (w - u(11.0), top + u(6.0), row_h * ROWS as f32 - u(12.0)), v.rows, ROWS, v.scroll);

    // Footer: the description, the frame rate, the keys.
    let foot = top + row_h * ROWS as f32 + u(GAP);
    rule(&mut c, scale, left, foot - u(5.0), width);
    let lines = wrap2(text, &v.desc, px(15.0), width - u(4.0));
    text.draw(&mut c, &lines[0], left + u(2.0), foot + u(1.0), px(15.0), false, TEXT, 1.0, false);
    text.draw(&mut c, &lines[1], left + u(2.0), foot + u(22.0), px(15.0), false, TEXT, 1.0, false);
    text.draw(&mut c, &v.env, left + width - u(2.0), foot + u(44.0), px(12.0), false, TEXT_DIM, 1.0, true);
    let stats = format!("{:.0} FPS     {:.2} ms a frame", v.fps_x10 as f32 / 10.0, v.ms_x100 as f32 / 100.0);
    text.draw(&mut c, &stats, left + u(2.0), foot + u(46.0), px(15.0), true, TEXT_SOFT, 1.0, false);
    // What the last action said (Save, Reset all).
    let note = fit(text, &v.note, px(14.0), true, width - u(4.0));
    text.draw(&mut c, &note, left + u(2.0), foot + u(66.0), px(14.0), true, WARM, 1.0, false);
    let keys = "Tab next page      Shift+Tab previous      PgUp PgDn group      Up Down select      Left Right change (Shift: x10)      Del default (Shift: all)      Esc or F10 close";
    text.draw(&mut c, keys, left + u(2.0), foot + u(88.0), px(12.0), false, TEXT_DIM, 1.0, false);
    let last = "The last three lines: Enter saves and restarts, saves for the next start, or resets all";
    text.draw(&mut c, last, left + u(2.0), foot + u(103.0), px(12.0), false, TEXT_DIM, 1.0, false);
    c
}

/// Tests that point the registry at a settings file (a process-wide choice) take turns (the
/// pages of the menu share this one).
#[cfg(test)]
static LIVE_FILE: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn live_file() -> std::sync::MutexGuard<'static, ()> {
    LIVE_FILE.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_core::tunable;

    #[test]
    fn steps_follow_the_range() {
        assert_eq!(int_step(0, 1), 1);
        assert_eq!(int_step(0, 64), 1);
        assert_eq!(int_step(0, 1000), 10);
        assert_eq!(int_step(0, 100_000), 1000);
        assert_eq!(int_step(i64::MIN, i64::MAX), 10i64.pow(17));
        assert_eq!(float_step(0.0, 1.0), 0.01);
        assert_eq!(float_step(0.25, 4.0), 0.01);
        assert_eq!(float_step(0.0, 100.0), 1.0);
        assert_eq!(float_step(0.0, 5000.0), 10.0);
        assert_eq!(float_step(1.0, 1.0), 1.0);
        assert_eq!(float_step(0.0, f64::INFINITY), 1.0);
    }

    #[test]
    fn a_switch_flips_and_a_choice_goes_round() {
        assert_eq!(step_text(reg::Kind::Bool, "true", 1, false).as_deref(), Some("false"));
        assert_eq!(step_text(reg::Kind::Bool, "false", -1, true).as_deref(), Some("true"));
        let list: &'static [&'static str] = &["a", "b", "c"];
        let k = reg::Kind::Choice(list);
        assert_eq!(step_text(k, "a", 1, false).as_deref(), Some("b"));
        assert_eq!(step_text(k, "c", 1, false).as_deref(), Some("a"));
        assert_eq!(step_text(k, "a", -1, false).as_deref(), Some("c"));
        assert_eq!(step_text(k, "", 1, false).as_deref(), Some("a"));
        assert_eq!(step_text(reg::Kind::Choice(&[]), "", 1, false), None);
    }

    #[test]
    fn whole_numbers_step_and_stop_at_the_ends() {
        let k = reg::Kind::Int { min: 0, max: 1000 };
        assert_eq!(step_text(k, "50", 1, false).as_deref(), Some("60"));
        assert_eq!(step_text(k, "50", 1, true).as_deref(), Some("150"));
        assert_eq!(step_text(k, "50", -1, true).as_deref(), Some("0"));
        assert_eq!(step_text(k, "0", -1, false), None);
        assert_eq!(step_text(k, "995", 1, false).as_deref(), Some("1000"));
        assert_eq!(step_text(k, "1000", 1, true), None);
        assert_eq!(step_text(k, "x", 1, false), None);
        let short = reg::Kind::Int { min: -5, max: 5 };
        assert_eq!(step_text(short, "-5", 1, false).as_deref(), Some("-4"));
        let wide = reg::Kind::Int { min: i64::MIN, max: i64::MAX };
        assert_eq!(step_text(wide, &i64::MAX.to_string(), 1, true), None);
    }

    #[test]
    fn fractions_step_on_a_grid_without_noise() {
        let k = reg::Kind::Float { min: 0.0, max: 1.0 };
        assert_eq!(step_text(k, "0.3", 1, false).as_deref(), Some("0.31"));
        assert_eq!(step_text(k, "0.29", 1, false).as_deref(), Some("0.3"));
        assert_eq!(step_text(k, "0.3", 1, true).as_deref(), Some("0.4"));
        assert_eq!(step_text(k, "0.3", -1, false).as_deref(), Some("0.29"));
        // Off the grid: goes to the next grid point in its direction.
        assert_eq!(step_text(k, "0.333", 1, false).as_deref(), Some("0.34"));
        assert_eq!(step_text(k, "0.333", -1, false).as_deref(), Some("0.33"));
        assert_eq!(step_text(k, "1.0", 1, false), None);
        assert_eq!(step_text(k, "0.0", -1, false), None);
        assert_eq!(step_text(k, "0.95", 1, true).as_deref(), Some("1"));
        // Repeated steps do not drift.
        let mut v = "0.0".to_owned();
        for _ in 0..30 {
            v = step_text(k, &v, 1, false).unwrap();
        }
        assert_eq!(v, "0.3");
    }

    #[test]
    fn names_group_by_their_first_word_and_small_groups_go_to_misc() {
        let keys: Vec<String> = ["card_a", "card_b", "card_c", "land_x", "texture_a", "texture_b", "texture_c", "texture_d", "wide"].iter().map(|s| (*s).to_owned()).collect();
        let g = build_groups(&keys);
        let names: Vec<&str> = g.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["CARD", "TEXTURE", "MISC"]);
        assert_eq!(g[0].1, [0, 1, 2]);
        assert_eq!(g[1].1, [4, 5, 6, 7]);
        assert_eq!(g[2].1, [3, 8]);
        assert!(build_groups(&[]).is_empty());
        assert_eq!(prefix("gpu"), "GPU");
    }

    #[test]
    fn the_list_scrolls_to_keep_the_selection_in_view() {
        assert_eq!(follow(0, 0, 30), 0);
        assert_eq!(follow(ROWS - 1, 0, 30), 0);
        assert_eq!(follow(ROWS, 0, 30), 1);
        assert_eq!(follow(2, 5, 30), 2);
        assert_eq!(follow(29, 0, 30), 30 - ROWS);
        assert_eq!(follow(3, 4, 5), 0);
    }

    #[test]
    fn extra_keys_count_once_and_only_while_open() {
        let mut e = Extra::default();
        assert!(e.commands([true, false, false, false], false, false).is_empty());
        // Held from before the menu opened: no press.
        assert!(e.commands([true, false, false, false], false, true).is_empty());
        assert_eq!(e.commands([false, false, true, false], false, true), [Cmd::GroupNext]);
        assert!(e.commands([false, false, true, false], false, true).is_empty());
        assert_eq!(e.commands([false, true, false, true], false, true), [Cmd::GroupPrev, Cmd::Reset]);
        assert_eq!(e.commands([true, false, false, false], false, true), [Cmd::PageNext]);
        assert_eq!(e.commands([false; 4], true, true), Vec::<Cmd>::new());
        assert_eq!(e.commands([false, false, false, true], true, true), [Cmd::ResetAll]);
        // Tab goes to the next page, Shift and Tab to the one before.
        assert_eq!(e.commands([false; 4], false, true), Vec::<Cmd>::new());
        assert_eq!(e.commands([true, false, false, false], true, true), [Cmd::PagePrev]);
    }

    // Test tunables, one of each kind (the registry is process-wide).
    tunable!(T_FLAG: bool = false, "VICTORY_TEST_PANEL_FLAG", "A switch", live = Safe);
    tunable!(T_COUNT: i64 = 5, 0..=1000, "VICTORY_TEST_PANEL_COUNT", "A count", live = Safe);
    tunable!(T_SCALE: f64 = 1.0, 0.0..=2.0, "VICTORY_TEST_PANEL_SCALE", "A scale", live = NextFrame);
    tunable!(T_MODE: enum = "b", ["a", "b", "c"], "VICTORY_TEST_PANEL_MODE", "A mode", live = Restart);

    #[test]
    fn keys_change_the_registry_through_its_set() {
        let _turn = live_file();
        victory_core::tunables::set_live_path(None);
        let mut page = Page::default();
        page.fill(vec![T_FLAG.slot(), T_COUNT.slot(), T_SCALE.slot(), T_MODE.slot()]);
        assert_eq!(page.groups.len(), 1);
        assert_eq!(page.groups[0].0, "T");
        // The switch is first: Right flips it, Left flips back.
        page.step(Action::Right, false);
        assert!(T_FLAG.get());
        page.step(Action::Left, false);
        assert!(!T_FLAG.get());
        // The count: step 10, x10 with Shift, Delete puts it back.
        page.step(Action::Down, false);
        page.step(Action::Right, false);
        assert_eq!(T_COUNT.get(), 15);
        page.step(Action::Right, true);
        assert_eq!(T_COUNT.get(), 115);
        let v = page.view(60.0, 16.7);
        assert!(v.lines[1].changed && v.lines[1].selected && !v.lines[0].changed);
        assert_eq!((v.lines[1].value.as_str(), v.lines[1].default.as_str(), v.lines[1].class), ("115", "5", "Safe"));
        assert_eq!(v.desc, "A count");
        assert_eq!((v.fps_x10, v.ms_x100), (600, 1670));
        page.reset_selected();
        assert_eq!(T_COUNT.get(), 5);
        // A next-frame value waits as pending, and steps on from the pending value.
        page.step(Action::Down, false);
        page.step(Action::Right, false);
        page.step(Action::Right, false);
        assert_eq!(T_SCALE.get(), 1.0);
        let v = page.view(0.0, 0.0);
        assert!(v.lines[2].pending && v.lines[2].changed);
        assert_eq!(v.lines[2].value, "1.02");
        assert_eq!(v.lines[2].class, "Next frame");
        // A restart value is pending, the running one stays.
        page.step(Action::Down, false);
        page.step(Action::Right, false);
        assert_eq!(T_MODE.get_str(), "b");
        let v = page.view(0.0, 0.0);
        assert!(v.lines[3].pending);
        assert_eq!(v.lines[3].value, "c");
        assert_eq!(v.changed_all, 2);
        // Reset all.
        assert_eq!(page.reset_all(), 2);
        let v = page.view(0.0, 0.0);
        assert!(v.lines.iter().all(|l| !l.changed && !l.pending));
        assert_eq!(v.changed_all, 0);
        // Selection goes round; Page keys wrap over one group.
        page.step(Action::Up, false);
        assert_eq!(page.selected, 2);
        page.move_group(1);
        assert_eq!((page.group, page.selected), (0, 0));
    }

    #[test]
    fn the_page_is_painted() {
        let _turn = live_file();
        victory_core::tunables::set_live_path(None);
        let mut page = Page::default();
        page.fill(vec![T_FLAG.slot(), T_COUNT.slot(), T_SCALE.slot(), T_MODE.slot()]);
        let view = page.view(59.6, 16.8);
        let mut text = Text::new().expect("gdi");
        let canvas = paint(&mut text, &view, 1.0);
        assert_eq!((canvas.w as u32, canvas.h as u32), panel_size(1.0));
        let drawn = canvas.px.iter().filter(|p| p[3] > 0.05).count();
        assert!(drawn > canvas.px.len() / 2);
        let long = wrap2(&mut text, "a long description that goes on and on and on and on and on and on and on and on and on and on", 15, 200.0);
        assert!(!long[0].is_empty() && !long[1].is_empty());
        // With an action line selected, the lines include the actions and the page paints.
        let mut page = Page::default();
        page.fill(vec![T_FLAG.slot(), T_COUNT.slot(), T_SCALE.slot(), T_MODE.slot()]);
        for _ in 0..5 {
            page.step(Action::Down, false);
        }
        let view = page.view(60.0, 16.7);
        assert_eq!(view.rows, 7);
        assert!(view.lines.iter().filter(|l| l.action).count() == 3 && view.lines[5].selected);
        assert_eq!(view.desc, ACTIONS[1].1);
        let canvas = paint(&mut text, &view, 1.0);
        assert_eq!((canvas.w as u32, canvas.h as u32), panel_size(1.0));
    }

    #[test]
    fn the_action_lines_follow_the_tunables_and_enter_asks_for_them() {
        let _turn = live_file();
        let dir = tempfile::tempdir_in(std::env::current_exe().unwrap().parent().unwrap()).unwrap();
        let path = dir.path().join("victory-live.toml");
        victory_core::tunables::set_live_path(Some(path.clone()));
        let mut page = Page::default();
        page.fill(vec![T_FLAG.slot(), T_COUNT.slot(), T_SCALE.slot(), T_MODE.slot()]);
        // Enter on a tunable changes it and asks for nothing.
        assert_eq!(page.step(Action::Enter, false), None);
        assert!(T_FLAG.get());
        // Down to the first action line (after the four tunables), then the others, then round.
        for _ in 0..3 {
            assert_eq!(page.step(Action::Down, false), None);
        }
        assert_eq!(page.step(Action::Down, false), None);
        assert_eq!(page.selected, 4);
        // Left and Right do nothing there.
        assert_eq!(page.step(Action::Right, false), None);
        assert_eq!(page.step(Action::Left, false), None);
        assert_eq!(page.step(Action::Enter, false), Some(Do::SaveRestart));
        page.step(Action::Down, false);
        assert_eq!(page.step(Action::Enter, false), Some(Do::Save));
        page.step(Action::Down, false);
        assert_eq!(page.step(Action::Enter, false), Some(Do::ResetAll));
        page.step(Action::Down, false);
        assert_eq!(page.selected, 0);
        // The list scrolls over the actions as it does over the tunables.
        page.step(Action::Up, false);
        assert_eq!(page.selected, 6);
        assert_eq!(follow(6, 0, 7), 0);
        // A Restart value is pending until a start; Save writes it to the file now.
        for _ in 0..3 {
            page.step(Action::Up, false);
        }
        assert_eq!(page.selected, 3);
        page.step(Action::Right, false);
        assert_eq!(T_MODE.get_str(), "b");
        page.run(Do::Save);
        assert!(page.note.starts_with("Saved, 1 apply at the next start"), "{}", page.note);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("t_mode") && text.contains("\"c\""), "{text}");
        assert!(text.contains("t_flag"), "{text}");
        // Reset all puts them back and forgets them in the file.
        page.run(Do::ResetAll);
        assert!(!T_FLAG.get());
        assert!(page.note.contains("back to default"), "{}", page.note);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("t_mode") && !text.contains("t_flag"), "{text}");
        // A key that is not Enter clears the note.
        page.step(Action::Down, false);
        assert!(page.note.is_empty());
        victory_core::tunables::set_live_path(None);
    }

    #[test]
    fn no_settings_file_means_no_save_and_no_restart() {
        let _turn = live_file();
        victory_core::tunables::set_live_path(None);
        let mut page = Page::default();
        page.fill(vec![T_FLAG.slot(), T_COUNT.slot(), T_SCALE.slot(), T_MODE.slot()]);
        // (With nothing to write to, a restart would lose the values: it does not happen.)
        page.run(Do::SaveRestart);
        assert!(page.note.starts_with("No settings file"), "{}", page.note);
    }

    #[test]
    fn the_restart_command_has_the_exe_and_its_arguments_quoted() {
        assert_eq!(restart_command("D:/a b/victory.exe", &[]), "Start-Sleep 3; & 'D:/a b/victory.exe'");
        let args = ["--title".to_owned(), "it's here".to_owned(), "x".to_owned()];
        assert_eq!(restart_command("C:/it's/v.exe", &args), "Start-Sleep 3; & 'C:/it''s/v.exe' '--title' 'it''s here' 'x'");
    }
}
