//! The "Controls" page of the F10 menu: keyboard and mouse rebinding.
//!
//! The page lists the actions of one key layout (ON FOOT, VEHICLE or AIRCRAFT,
//! switched by the first line or Page Up and Page Down), each with the keys it
//! has now and its default. The bindings are the registry of
//! [`victory_core::keybinds`], which `victory-hle` fills and builds its layouts
//! from, so a change applies at once; "Save" writes them into the controls file
//! the run uses (a `.bak` copy first).
//!
//! Enter on an action line asks for a key: the next key or mouse button pressed
//! becomes its key (the keys it had are let go); Right asks for one more key
//! instead. Esc cancels, Del puts the line's default back. The key that is
//! pressed is taken from the title ([`takes_key`]: every key while the page waits
//! for one, and each one held after it, until it is let go). A key that another
//! action had goes to the new one, and the note says which action lost it.
//!
//! The page is painted by `menu_pages::paint` (the same look as the other pages).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use victory_core::keybinds::{self as binds, Layout, Line};

use super::Action;
use super::Tab;
use super::pages::{Curated, PLine, PView};
use super::live;

/// The lines after the actions.
const ACTION_LINES: [(&str, &str); 3] = [
    ("SAVE TO THE CONTROLS FILE", "Writes the bindings into the controls file this run uses (a .bak copy is made first). The changes already apply in the game."),
    ("RESET THIS SECTION TO DEFAULT", "Puts every line of this section back to its default keys."),
    ("CLOSE", "Closes the menu (Esc or F10 does too)."),
];

const SECTION: &str = "SECTION";
/// What the section line shows on the options section (after the three layouts).
const OPTIONS: &str = "OPTIONS";

/// What the page waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Capture {
    id: String,
    label: String,
    /// One more key for the action, not a new one in place of the old.
    add: bool,
    /// Every key was up once since it began (the press that began it is over).
    armed: bool,
}

/// The page's own state.
pub(crate) struct Page {
    layout: Layout,
    selected: usize,
    scroll: usize,
    note: String,
    capture: Option<Capture>,
    /// The options section (after AIRCRAFT): the keyboard and mouse lines that are not key bindings.
    /// Its lines are those of a [`Curated`] page; this page adds the section line over them.
    options: Option<Curated>,
}

impl Default for Page {
    fn default() -> Page {
        Page { layout: Layout::OnFoot, selected: 0, scroll: 0, note: String::new(), capture: None, options: None }
    }
}

/// What the selected row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Section,
    Line(usize),
    Save,
    Reset,
    Close,
}

/// A key as the page shows it.
pub(crate) fn pretty(key: &str) -> String {
    let named = match key {
        "LShift" => "LEFT SHIFT",
        "RShift" => "RIGHT SHIFT",
        "LCtrl" => "LEFT CTRL",
        "RCtrl" => "RIGHT CTRL",
        "LAlt" => "LEFT ALT",
        "RAlt" => "RIGHT ALT",
        "MouseLeft" => "MOUSE LEFT",
        "MouseRight" => "MOUSE RIGHT",
        "MouseMiddle" => "MOUSE MIDDLE",
        "Mouse4" => "MOUSE 4",
        "Mouse5" => "MOUSE 5",
        "WheelUp" => "WHEEL UP",
        "WheelDown" => "WHEEL DOWN",
        "PageUp" => "PAGE UP",
        "PageDown" => "PAGE DOWN",
        "CapsLock" => "CAPS LOCK",
        "NumMultiply" => "NUM *",
        "NumAdd" => "NUM +",
        "NumSubtract" => "NUM -",
        "NumDecimal" => "NUM .",
        "NumDivide" => "NUM /",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_owned();
    }
    match key.strip_prefix("Num") {
        Some(rest) if rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit() => format!("NUM {rest}"),
        _ => key.to_ascii_uppercase(),
    }
}

fn keys_text(keys: &[String]) -> String {
    if keys.is_empty() { "NONE".to_owned() } else { keys.iter().map(|k| pretty(k)).collect::<Vec<_>>().join(", ") }
}

impl Page {
    fn lines(&self) -> Vec<Line> {
        binds::lines(self.layout)
    }

    fn rows(&self, n: usize) -> usize {
        1 + n + ACTION_LINES.len()
    }

    fn row(&self, n: usize) -> Row {
        match self.selected.min(self.rows(n) - 1) {
            0 => Row::Section,
            i if i <= n => Row::Line(i - 1),
            i => [Row::Save, Row::Reset, Row::Close][i - n - 1],
        }
    }

    /// Whether the page waits for a key now.
    pub(crate) fn capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Stops waiting for a key (the menu closed).
    pub(crate) fn cancel_capture(&mut self) {
        self.capture = None;
    }

    fn go_section(&mut self, dir: i32) {
        if self.options.take().is_some() {
            self.layout = if dir > 0 { Layout::OnFoot } else { Layout::Aircraft };
        } else if (dir > 0 && self.layout == Layout::Aircraft) || (dir < 0 && self.layout == Layout::OnFoot) {
            self.layout = Layout::Aircraft;
            self.options = Some(Curated::new(Tab::Controls));
        } else {
            self.layout = if dir > 0 { self.layout.next() } else { self.layout.prev() };
        }
        self.selected = 0;
        self.scroll = 0;
        self.note.clear();
    }

    fn begin(&mut self, line: &Line, add: bool) {
        if !binds::ready() {
            "The keyboard and mouse controls are not set up (no [controls] in the config file)".clone_into(&mut self.note);
            return;
        }
        self.capture = Some(Capture { id: line.id.clone(), label: line.label.clone(), add, armed: false });
        self.note.clear();
    }

    /// Does what `action` asks (Up, Down, Left, Right, Enter; the others are the menu's); whether the menu is to close.
    pub(crate) fn step(&mut self, action: Action) -> bool {
        if self.capture.is_some() {
            return false;
        }
        if self.options.is_some() {
            return self.step_options(action);
        }
        let lines = self.lines();
        let n = lines.len();
        let total = self.rows(n);
        self.selected = self.selected.min(total - 1);
        let row = self.row(n);
        // What an action line said stays until another key is pressed.
        if !(action == Action::Enter && matches!(row, Row::Save | Row::Reset)) {
            self.note.clear();
        }
        let mut close = false;
        match (action, row) {
            (Action::Up, _) => self.selected = (self.selected + total - 1) % total,
            (Action::Down, _) => self.selected = (self.selected + 1) % total,
            (Action::Left, Row::Section) => self.go_section(-1),
            (Action::Right | Action::Enter, Row::Section) => self.go_section(1),
            (Action::Enter, Row::Line(i)) => self.begin(&lines[i], false),
            (Action::Right, Row::Line(i)) => self.begin(&lines[i], true),
            (Action::Enter, Row::Save) => self.note = binds::save().unwrap_or_else(|e| e),
            (Action::Enter, Row::Reset) => self.reset_section(),
            (Action::Enter, Row::Close) => close = true,
            _ => {}
        }
        self.scroll = live::follow(self.selected, self.scroll, self.rows(n));
        close
    }

    /// A key in the options section: row 0 is the section line, the others are the lines of the options page.
    fn step_options(&mut self, action: Action) -> bool {
        let total = 1 + self.options.as_ref().map_or(0, Curated::rows);
        self.selected = self.selected.min(total - 1);
        let mut close = false;
        match (action, self.selected) {
            (Action::Up, _) => self.selected = (self.selected + total - 1) % total,
            (Action::Down, _) => self.selected = (self.selected + 1) % total,
            (Action::Left, 0) => {
                self.go_section(-1);
                return false;
            }
            (Action::Right | Action::Enter, 0) => {
                self.go_section(1);
                return false;
            }
            (action, at) if at >= 1 => {
                if let Some(page) = self.options.as_mut() {
                    page.select(at - 1);
                    if let Some(act) = page.step(action) {
                        close = page.run(act);
                    }
                }
            }
            _ => {}
        }
        let at = self.selected;
        if let Some(page) = self.options.as_mut() {
            if at >= 1 {
                page.select(at - 1);
            }
            self.scroll = live::follow(at, self.scroll, total);
        }
        close
    }

    /// Page Up and Page Down.
    pub(crate) fn section(&mut self, dir: i32) {
        if self.capture.is_none() {
            self.go_section(dir);
        }
    }

    /// The selected line back to its default keys.
    pub(crate) fn reset_selected(&mut self) {
        if let Some(page) = self.options.as_mut() {
            if self.selected >= 1 {
                page.select(self.selected - 1);
                page.reset_selected();
            }
            return;
        }
        let lines = self.lines();
        if let Row::Line(i) = self.row(lines.len()) {
            binds::reset(self.layout, &lines[i].id);
            self.note = format!("{} is back to {}", lines[i].label, keys_text(&lines[i].default));
        }
    }

    /// Every line of the section back to its default keys.
    pub(crate) fn reset_section(&mut self) {
        if let Some(page) = self.options.as_ref() {
            let n = page.reset_all();
            self.note = if n == 0 { "Nothing was changed here".to_owned() } else { format!("{n} lines back to their defaults") };
            return;
        }
        let n = binds::reset_layout(self.layout);
        self.note = if n == 0 { "Nothing was changed here".to_owned() } else { format!("{n} lines back to their default keys") };
    }

    /// Waiting for a key: `down` are the keys and mouse buttons held now (virtual keys). The first one pressed
    /// after every key was up becomes the binding; Esc cancels and Del puts the default back.
    pub(crate) fn poll_capture(&mut self, down: &[u8]) {
        let Some(cap) = &mut self.capture else { return };
        if !cap.armed {
            cap.armed = down.is_empty();
            return;
        }
        // F10 and F1 are the menu's; they close it (which ends the wait).
        let Some(&vk) = down.iter().find(|&&vk| !matches!(vk, 0x79 | 0x70)) else { return };
        let (id, label, add) = (cap.id.clone(), cap.label.clone(), cap.add);
        match vk {
            0x1B => {
                self.capture = None;
                "Cancelled".clone_into(&mut self.note);
            }
            0x2E => {
                self.capture = None;
                binds::reset(self.layout, &id);
                let default = binds::lines(self.layout).into_iter().find(|l| l.id == id).map(|l| keys_text(&l.default)).unwrap_or_default();
                self.note = format!("{label} is back to {default}");
            }
            _ => match binds::name_of(vk) {
                None => "That key has no name in the controls file, press another".clone_into(&mut self.note),
                Some(name) => {
                    self.capture = None;
                    self.note = binds::assign(self.layout, &id, &name, add).unwrap_or_else(|e| e);
                }
            },
        }
    }

    /// What to paint now; `fps` and `ms` are the frame rate and frame time.
    pub(crate) fn view(&self, fps: f32, ms: f32) -> PView {
        if let Some(page) = &self.options {
            return self.options_view(page, fps, ms);
        }
        let lines = self.lines();
        let n = lines.len();
        let rows = self.rows(n);
        let selected = self.selected.min(rows - 1);
        let scroll = self.scroll.min(rows.saturating_sub(live::ROWS));
        let mut out = Vec::with_capacity(live::ROWS.min(rows));
        for at in scroll..rows.min(scroll + live::ROWS) {
            let is_selected = at == selected;
            if at == 0 {
                out.push(PLine { label: SECTION.to_owned(), value: self.layout.label().to_owned(), selected: is_selected, ..PLine::default() });
            } else if at > n {
                out.push(PLine { label: ACTION_LINES[at - n - 1].0.to_owned(), selected: is_selected, action: true, ..PLine::default() });
            } else {
                let line = &lines[at - 1];
                let waiting = is_selected && self.capture.as_ref().is_some_and(|c| c.id == line.id);
                let value = if waiting {
                    if self.capture.as_ref().is_some_and(|c| c.add) { "PRESS A KEY TO ADD..." } else { "PRESS A KEY..." }.to_owned()
                } else {
                    keys_text(&line.keys)
                };
                out.push(PLine {
                    label: line.label.clone(),
                    value,
                    default: keys_text(&line.default),
                    changed: line.changed,
                    pending: waiting,
                    selected: is_selected,
                    plain: true,
                    ..PLine::default()
                });
            }
        }
        let desc = self.desc(&lines);
        let changed_all = lines.iter().filter(|l| l.changed).count();
        let unsaved = binds::unsaved();
        let tally = if !binds::ready() || !binds::active() {
            "controls are off".to_owned()
        } else if changed_all == 0 && !unsaved {
            "no change".to_owned()
        } else {
            // Short: it shares the tab strip's row.
            format!("{changed_all} changed{}", if unsaved { ", not saved" } else { "" })
        };
        PView {
            tab: Tab::Controls,
            lines: out,
            rows,
            scroll,
            desc,
            env: if binds::ready() { binds::source_name().unwrap_or_else(|| "no controls file".to_owned()) } else { String::new() },
            note: self.note.clone(),
            tally,
            changed_all,
            total: n,
            fps_x10: (fps * 10.0).round() as u32,
            ms_x100: (ms * 100.0).round() as u32,
            mem: None,
        }
    }

    /// The options section: the section line, then the options page's rows.
    fn options_view(&self, page: &Curated, fps: f32, ms: f32) -> PView {
        let mut inner = page.view(fps, ms);
        let on_section = self.selected == 0;
        let section = PLine { label: SECTION.to_owned(), value: OPTIONS.to_owned(), selected: on_section, ..PLine::default() };
        let mut lines = vec![section];
        for mut line in inner.lines.drain(..) {
            line.selected &= !on_section;
            lines.push(line);
        }
        let desc = if on_section {
            "Left and Right (or Page Up and Page Down) switch between ON FOOT, VEHICLE, AIRCRAFT and OPTIONS. Options are saved with the lines at the bottom, not with the keys.".to_owned()
        } else {
            inner.desc
        };
        PView { tab: Tab::Controls, rows: 1 + inner.rows, scroll: 0, lines, desc, note: if self.note.is_empty() { inner.note } else { self.note.clone() }, env: String::new(), ..inner }
    }

    fn desc(&self, lines: &[Line]) -> String {
        let n = lines.len();
        if let Some(cap) = &self.capture {
            return format!("Press a key or mouse button for {}. Esc cancels, Del puts the default back. The game does not see the key.", cap.label);
        }
        if !binds::ready() {
            return "The keyboard and mouse controls are not set up: the config file has no [controls] table.".to_owned();
        }
        match self.row(n) {
            Row::Section => {
                let aircraft = if self.layout == Layout::Aircraft { " These keys are the vehicle keys when the controls file has vehicle_layout = \"aircraft\"." } else { "" };
                format!("Left and Right (or Page Up and Page Down) switch between ON FOOT, VEHICLE, AIRCRAFT and OPTIONS.{aircraft}")
            }
            Row::Line(i) => {
                let line = &lines[i];
                let mut text = format!("{}. Pad: {}. Enter: press a new key. Right: add a key. Del: default.", line.label, line.pad);
                if !line.shared.is_empty() {
                    text.push_str(&format!(" Same key as: {}.", line.shared.join(", ")));
                }
                if !binds::active() {
                    text.push_str(" Controls are off in the config file: kept for later.");
                }
                text
            }
            Row::Save | Row::Reset | Row::Close => {
                let at = match self.row(n) {
                    Row::Save => 0,
                    Row::Reset => 1,
                    _ => 2,
                };
                ACTION_LINES[at].1.to_owned()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Keeping the pressed key from the title.

static CAPTURING: AtomicBool = AtomicBool::new(false);
/// The keys (bit per virtual key) held since the page waited for one: the title does not see them either.
static LATCH: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn bits(down: &[u8]) -> [u64; 4] {
    let mut out = [0u64; 4];
    for &vk in down {
        out[usize::from(vk >> 6)] |= 1 << (vk & 63);
    }
    out
}

/// The latch after a frame: every key held while the page waited (or just stopped waiting), then
/// only those of them that are still held.
pub(crate) fn next_latch(before: [u64; 4], waiting: bool, down: &[u8]) -> [u64; 4] {
    let now = bits(down);
    if waiting {
        return now;
    }
    std::array::from_fn(|i| before[i] & now[i])
}

/// Whether any key is latched now.
pub(crate) fn latched() -> bool {
    LATCH.iter().any(|w| w.load(Ordering::Relaxed) != 0)
}

/// Sets what the title is kept from: `waiting` is that the page waited for a key this frame or began to,
/// `capturing` that it waits now; `down` the keys held now.
pub(crate) fn set_state(waiting: bool, capturing: bool, down: &[u8]) {
    let before = std::array::from_fn(|i| LATCH[i].load(Ordering::Relaxed));
    for (slot, bits) in LATCH.iter().zip(next_latch(before, waiting, down)) {
        slot.store(bits, Ordering::Relaxed);
    }
    CAPTURING.store(capturing, Ordering::Relaxed);
}

/// Whether the title must not see the key `vk` now.
pub(crate) fn takes_key(vk: u8) -> bool {
    CAPTURING.load(Ordering::Relaxed) || LATCH[usize::from(vk >> 6)].load(Ordering::Relaxed) & (1 << (vk & 63)) != 0
}

/// The keys and mouse buttons held now (virtual keys; Shift, Ctrl and Alt as their left and right keys).
pub(crate) fn read_down(front: bool) -> Vec<u8> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    if !front {
        return Vec::new();
    }
    [0x01u8, 0x02, 0x04, 0x05, 0x06]
        .into_iter()
        .chain(0x08..=0xFEu8)
        .filter(|vk| !(0x10..=0x12).contains(vk))
        // SAFETY: a plain query.
        .filter(|&vk| unsafe { GetAsyncKeyState(i32::from(vk)) } < 0)
        .collect()
}

// ---------------------------------------------------------------------------
// Tests.

/// Tests that use the registry take turns (it is process-wide).
#[cfg(test)]
pub(crate) fn test_turn() -> std::sync::MutexGuard<'static, ()> {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    TURN.lock().unwrap_or_else(|e| e.into_inner())
}

/// A registry like the real one, in part: the actions of the three layouts with their usual keys.
#[cfg(test)]
pub(crate) fn test_setup() -> binds::Setup {
    use binds::{Def, Table};
    let table = |rows: &[(&str, &[&str])]| -> Table { rows.iter().map(|(k, v)| ((*k).to_owned(), v.iter().map(|s| (*s).to_owned()).collect())).collect() };
    let foot = table(&[
        ("W", &["MoveForward"]),
        ("S", &["MoveBack"]),
        ("A", &["MoveLeft"]),
        ("D", &["MoveRight"]),
        ("LAlt", &["Walk"]),
        ("LShift", &["Sprint"]),
        ("Space", &["Jump"]),
        ("LCtrl", &["Crouch"]),
        ("C", &["Crouch"]),
        ("F", &["Interact"]),
        ("G", &["ToggleVehicle"]),
        ("R", &["Melee"]),
        ("Q", &["Cover"]),
        ("V", &["Map"]),
        ("M", &["Map"]),
        ("Tab", &["tap:Holster", "hold400:Loadout"]),
        ("MouseLeft", &["Shoot"]),
        ("MouseRight", &["Aim"]),
        ("MouseMiddle", &["LookBehind"]),
        ("Up", &["Phone"]),
        ("Left", &["Radio"]),
        ("Right", &["Objective"]),
        ("P", &["Pause"]),
    ]);
    let vehicle = table(&[("W", &["Accelerate"]), ("S", &["Brake"]), ("Space", &["Handbrake"]), ("LShift", &[]), ("F", &["Interact"]), ("E", &["Horn"])]);
    let aircraft = table(&[("W", &["ThrottleUp"]), ("S", &["ThrottleDown"]), ("A", &["YawLeft"]), ("D", &["YawRight"]), ("G", &["Undercarriage"])]);
    let defs = |rows: &[(&str, &str, &str)]| rows.iter().map(|(id, label, pad)| Def::new(id, label, pad)).collect::<Vec<_>>();
    binds::Setup {
        defs: [
            defs(&[
                ("MoveForward", "Move forward", "left stick up"),
                ("MoveBack", "Move back", "left stick down"),
                ("MoveLeft", "Move left", "left stick left"),
                ("MoveRight", "Move right", "left stick right"),
                ("Walk", "Walk slowly (hold)", "left stick at walk speed"),
                ("Sprint", "Sprint", "LS click"),
                ("Jump", "Jump", "A"),
                ("Crouch", "Crouch / rest", "B"),
                ("Interact", "Interact / get in (doors, prompts)", "Y"),
                ("ToggleVehicle", "Switch to vehicle keys", "no pad button, switches the key layout"),
                ("Melee", "Melee", "X"),
                ("Cover", "Cover / quick throw", "RB"),
                ("Map", "Map", "View"),
                ("Holster", "Holster / loadout", "LB"),
                ("Shoot", "Shoot", "RT"),
                ("Aim", "Aim", "LT"),
                ("LookBehind", "Look behind", "RS click"),
                ("Phone", "Phone", "D-pad up"),
                ("Radio", "Radio", "D-pad left"),
                ("Objective", "Objective", "D-pad right"),
                ("Pause", "Pause", "Menu"),
            ]),
            defs(&[
                ("Accelerate", "Accelerate", "RT"),
                ("Brake", "Brake / reverse", "LT"),
                ("Handbrake", "Handbrake", "A"),
                ("Horn", "Horn", "LS click"),
                ("Interact", "Interact / get out (doors, prompts)", "Y"),
            ]),
            defs(&[
                ("ThrottleUp", "Throttle up", "RT"),
                ("ThrottleDown", "Throttle down", "LT"),
                ("YawLeft", "Yaw left", "X"),
                ("YawRight", "Yaw right", "B"),
                ("Undercarriage", "Landing gear", "LS click"),
            ]),
        ],
        defaults: [foot.clone(), vehicle.clone(), aircraft.clone()],
        current: [foot, vehicle, aircraft],
        source: None,
        aircraft_active: false,
        active: true,
    }
}

/// Names for the keys the tests press.
#[cfg(test)]
pub(crate) fn test_namer(vk: u8) -> Option<String> {
    match vk {
        b'A'..=b'Z' | b'0'..=b'9' => Some(char::from(vk).to_string()),
        0x01 => Some("MouseLeft".to_owned()),
        0x08 => Some("Backspace".to_owned()),
        0x20 => Some("Space".to_owned()),
        0xA0 => Some("LShift".to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> (std::sync::MutexGuard<'static, ()>, Page) {
        let turn = test_turn();
        binds::install(test_setup());
        binds::set_namer(test_namer);
        set_state(false, false, &[]);
        (turn, Page::default())
    }

    /// Down to the line `label` of the section.
    fn select(page: &mut Page, label: &str) {
        let at = page.lines().iter().position(|l| l.label == label).unwrap_or_else(|| panic!("no line {label}"));
        page.selected = at + 1;
        page.scroll = live::follow(page.selected, page.scroll, page.rows(page.lines().len()));
    }

    /// Enter on the selected line, then a key: every key up first (the Enter), then the key.
    fn press(page: &mut Page, vk: u8) {
        page.poll_capture(&[0x0D]);
        page.poll_capture(&[]);
        page.poll_capture(&[vk]);
    }

    #[test]
    fn the_sections_switch_and_the_page_lists_the_actions_with_their_keys() {
        let (_turn, mut page) = fresh();
        let v = page.view(60.0, 16.7);
        assert_eq!(v.tab, Tab::Controls);
        assert_eq!((v.lines[0].label.as_str(), v.lines[0].value.as_str()), ("SECTION", "ON FOOT"));
        assert_eq!(v.total, 21);
        assert_eq!(v.rows, 1 + 21 + 3);
        assert_eq!(v.lines.len(), live::ROWS);
        let sprint = v.lines.iter().find(|l| l.label == "Sprint").unwrap();
        assert_eq!((sprint.value.as_str(), sprint.default.as_str()), ("LEFT SHIFT", "LEFT SHIFT"));
        let crouch = v.lines.iter().find(|l| l.label == "Crouch / rest").unwrap();
        assert_eq!(crouch.value, "C, LEFT CTRL");
        assert!(v.lines[0].selected && v.desc.contains("Left and Right"), "{}", v.desc);
        // Left and Right on the section line, and Page Up and Page Down anywhere.
        page.step(Action::Right);
        assert_eq!(page.view(0.0, 0.0).lines[0].value, "VEHICLE");
        page.step(Action::Right);
        let v = page.view(0.0, 0.0);
        assert_eq!(v.lines[0].value, "AIRCRAFT");
        assert!(v.desc.contains("vehicle_layout"), "{}", v.desc);
        assert_eq!(v.total, 5);
        page.step(Action::Left);
        page.step(Action::Left);
        assert_eq!(page.layout, Layout::OnFoot);
        page.step(Action::Down);
        page.section(1);
        assert_eq!((page.layout, page.selected), (Layout::Vehicle, 0));
        page.section(-1);
        assert_eq!(page.layout, Layout::OnFoot);
        // A line says its pad button; a key held by an action with a style shows too.
        select(&mut page, "Jump");
        assert!(page.view(0.0, 0.0).desc.starts_with("Jump. Pad: A. Enter: press a new key."), "{}", page.view(0.0, 0.0).desc);
        select(&mut page, "Holster / loadout");
        assert_eq!(page.view(0.0, 0.0).lines.iter().find(|l| l.selected).unwrap().value, "TAB");
        binds::clear();
    }

    #[test]
    fn the_options_section_follows_aircraft_and_lists_the_mouse_and_vehicle_lines() {
        let (_turn, mut page) = fresh();
        page.section(-1);
        assert!(page.options.is_some(), "Page Up from ON FOOT goes round to OPTIONS");
        page.section(1);
        assert!(page.options.is_none());
        assert_eq!(page.layout, Layout::OnFoot);
        page.section(1);
        page.section(1);
        assert_eq!(page.layout, Layout::Aircraft);
        page.section(1);
        let v = page.view(0.0, 0.0);
        assert_eq!((v.lines[0].label.as_str(), v.lines[0].value.as_str()), ("SECTION", "OPTIONS"));
        assert!(v.lines[0].selected);
        let labels: Vec<&str> = v.lines.iter().map(|l| l.label.as_str()).collect();
        for want in ["KEYBOARD AND MOUSE AS PAD", "AUTO VEHICLE KEYS", "KEY PROMPTS", "MOUSE SENSITIVITY", "INVERT MOUSE Y", "DRAG WINDOW BY ITS TITLE BAR", "SAVE FOR NEXT START"] {
            assert!(labels.contains(&want), "{want}");
        }
        assert_eq!(v.rows, v.lines.len());
        // Down puts the selection on the first option line; Right and Left on the section line go round.
        page.step(Action::Down);
        let v = page.view(0.0, 0.0);
        assert!(!v.lines[0].selected && v.lines[1].selected);
        page.step(Action::Up);
        page.step(Action::Right);
        assert_eq!((page.layout, page.options.is_none()), (Layout::OnFoot, true));
        binds::clear();
    }

    #[test]
    fn enter_waits_for_a_key_and_the_next_press_after_the_enter_is_let_go_is_the_binding() {
        let (_turn, mut page) = fresh();
        select(&mut page, "Sprint");
        page.step(Action::Enter);
        assert!(page.capturing());
        let v = page.view(0.0, 0.0);
        let line = v.lines.iter().find(|l| l.selected).unwrap();
        assert_eq!(line.value, "PRESS A KEY...");
        assert!(line.pending && v.desc.starts_with("Press a key or mouse button for Sprint."), "{}", v.desc);
        // Enter is still down: it is not the answer.
        page.poll_capture(&[0x0D]);
        assert!(page.capturing());
        page.poll_capture(&[]);
        // F10 is the menu's.
        page.poll_capture(&[0x79]);
        assert!(page.capturing());
        // A key with no name in the controls file is refused and the wait goes on.
        page.poll_capture(&[0x5B]);
        assert!(page.capturing() && page.note.contains("no name"), "{}", page.note);
        page.poll_capture(&[b'Z']);
        assert!(!page.capturing());
        assert_eq!(page.note, "Sprint is now Z");
        assert_eq!(binds::lines(Layout::OnFoot).iter().find(|l| l.id == "Sprint").unwrap().keys, ["Z"]);
        let v = page.view(0.0, 0.0);
        let sprint = v.lines.iter().find(|l| l.label == "Sprint").unwrap();
        assert!(sprint.changed && sprint.value == "Z" && sprint.default == "LEFT SHIFT");
        assert_eq!(v.changed_all, 1);
        assert_eq!(v.tally, "1 changed, not saved");
        binds::clear();
    }

    #[test]
    fn mouse_buttons_bind_and_right_adds_a_key() {
        let (_turn, mut page) = fresh();
        select(&mut page, "Melee");
        page.step(Action::Enter);
        press(&mut page, 0x01);
        // The left button was the shot: it goes to Melee, and the note says so.
        assert_eq!(page.note, "MouseLeft was Shoot, which now has no key there");
        assert_eq!(binds::lines(Layout::OnFoot).iter().find(|l| l.id == "Shoot").unwrap().keys, Vec::<String>::new());
        select(&mut page, "Jump");
        page.step(Action::Right);
        assert_eq!(page.view(0.0, 0.0).lines.iter().find(|l| l.selected).unwrap().value, "PRESS A KEY TO ADD...");
        press(&mut page, b'J');
        assert_eq!(binds::lines(Layout::OnFoot).iter().find(|l| l.id == "Jump").unwrap().keys, ["J", "Space"]);
        assert_eq!(page.view(0.0, 0.0).lines.iter().find(|l| l.label == "Jump").unwrap().value, "J, SPACE");
        // A line with no key shows NONE.
        select(&mut page, "Shoot");
        assert_eq!(page.view(0.0, 0.0).lines.iter().find(|l| l.label == "Shoot").unwrap().value, "NONE");
        binds::clear();
    }

    #[test]
    fn esc_cancels_and_delete_puts_the_default_back() {
        let (_turn, mut page) = fresh();
        select(&mut page, "Sprint");
        page.step(Action::Enter);
        press(&mut page, 0x1B);
        assert!(!page.capturing() && page.note == "Cancelled");
        assert!(!binds::unsaved());
        // Del while waiting: this line's default.
        page.step(Action::Enter);
        press(&mut page, b'Z');
        assert!(binds::unsaved());
        page.step(Action::Enter);
        press(&mut page, 0x2E);
        assert_eq!(page.note, "Sprint is back to LEFT SHIFT");
        assert!(!binds::unsaved());
        // Del on the line (not waiting) does the same; on the whole section with Shift.
        binds::assign(Layout::OnFoot, "Sprint", "Z", false).unwrap();
        binds::assign(Layout::OnFoot, "Jump", "J", false).unwrap();
        page.reset_selected();
        assert_eq!(binds::lines(Layout::OnFoot).iter().find(|l| l.id == "Sprint").unwrap().keys, ["LShift"]);
        assert_eq!(binds::lines(Layout::OnFoot).iter().find(|l| l.id == "Jump").unwrap().keys, ["J"]);
        page.reset_section();
        assert_eq!(page.note, "1 lines back to their default keys");
        assert!(!binds::unsaved());
        // A cancelled wait (the menu closed) leaves nothing waiting.
        page.step(Action::Enter);
        page.cancel_capture();
        assert!(!page.capturing());
        binds::clear();
    }

    #[test]
    fn while_the_page_waits_the_title_sees_no_key_and_none_that_was_pressed_until_it_is_let_go() {
        let _turn = test_turn();
        set_state(false, false, &[]);
        assert!(!takes_key(b'W') && !latched());
        // Waiting: every key is taken.
        set_state(true, true, &[0x0D]);
        assert!(takes_key(b'W') && takes_key(0x01) && takes_key(0xFE));
        // The key pressed ends the wait: it stays taken while it is held, and only it.
        set_state(true, false, &[b'Z']);
        assert!(takes_key(b'Z') && !takes_key(b'W') && latched());
        set_state(false, false, &[b'Z', b'W']);
        assert!(takes_key(b'Z'), "still held");
        assert!(!takes_key(b'W'), "pressed after the wait: the title's");
        set_state(false, false, &[]);
        assert!(!takes_key(b'Z') && !latched());
        // The latch is a bit per key over all the codes.
        assert_eq!(bits(&[0x00, 0x40, 0xFE]), [1, 1, 0, 1 << 62]);
        assert_eq!(next_latch([u64::MAX; 4], false, &[0x20]), bits(&[0x20]));
    }

    #[test]
    fn save_without_a_controls_file_says_so_and_with_one_writes_it() {
        let (_turn, mut page) = fresh();
        page.selected = 1 + page.lines().len();
        page.step(Action::Enter);
        assert!(page.note.contains("No controls file"), "{}", page.note);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("victory-pc.toml");
        std::fs::write(&path, "[controls]\nenabled = true\n").unwrap();
        binds::install(binds::Setup { source: Some(path.clone()), ..test_setup() });
        select(&mut page, "Jump");
        page.step(Action::Enter);
        press(&mut page, b'J');
        page.selected = 1 + page.lines().len();
        assert!(!page.step(Action::Enter));
        assert_eq!(page.note, "Saved to victory-pc.toml (copy in victory-pc.toml.bak)");
        assert!(std::fs::read_to_string(&path).unwrap().contains("J = \"Jump\""));
        assert!(dir.path().join("victory-pc.toml.bak").exists());
        assert!(!binds::unsaved() && page.view(0.0, 0.0).tally == "1 changed");
        // The last line closes the menu.
        page.step(Action::Down);
        page.step(Action::Down);
        assert!(page.step(Action::Enter));
        binds::clear();
    }

    #[test]
    fn the_page_is_painted_and_says_when_nothing_is_set_up() {
        let (_turn, mut page) = fresh();
        let mut text = super::super::Text::new().expect("gdi");
        for scale in [0.75f32, 1.0, 2.0] {
            let canvas = super::super::pages::paint(&mut text, &page.view(59.6, 16.8), scale);
            assert_eq!((canvas.w as u32, canvas.h as u32), live::panel_size(scale));
            assert!(canvas.px.iter().filter(|p| p[3] > 0.05).count() > canvas.px.len() / 2);
        }
        page.step(Action::Enter);
        binds::clear();
        let v = page.view(0.0, 0.0);
        assert!(v.desc.contains("not set up") && v.tally == "controls are off", "{} {}", v.desc, v.tally);
        assert!(v.lines.iter().any(|l| l.label == "SECTION") && v.total == 0);
        // Nothing to wait for.
        page.step(Action::Down);
        page.selected = 0;
        let before = page.capturing();
        page.step(Action::Enter);
        assert_eq!(before, page.capturing());
        let canvas = super::super::pages::paint(&mut text, &v, 1.0);
        assert_eq!((canvas.w as u32, canvas.h as u32), live::panel_size(1.0));
    }

    #[test]
    fn key_names_read_well() {
        assert_eq!(pretty("LShift"), "LEFT SHIFT");
        assert_eq!(pretty("Num5"), "NUM 5");
        assert_eq!(pretty("Mouse4"), "MOUSE 4");
        assert_eq!(pretty("W"), "W");
        assert_eq!(pretty("Space"), "SPACE");
        assert_eq!(keys_text(&[]), "NONE");
    }
}
