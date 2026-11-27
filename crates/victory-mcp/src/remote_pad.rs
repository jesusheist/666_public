//! A pad pressed from outside (the `press`, `stick`, `release_all` and
//! `input_script` tools): no window focus and no physical pad needed.
//!
//! The state is a few held buttons and sticks, each with the time it lets
//! go. Nothing runs on a timer: [`read`] drops what has expired, so a hold
//! ends exactly when the title next looks. The runtime ORs [`read`]'s answer
//! into the pad the title reads (`victory-hle`, `modules/remote_pad.rs`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Button bits, the same values as the gamepad buttons the title reads
/// (`victory-hle` `modules/gameinput.rs`; a test there keeps them equal).
pub mod bit {
    pub const MENU: u32 = 0x1;
    pub const VIEW: u32 = 0x2;
    pub const A: u32 = 0x4;
    pub const B: u32 = 0x8;
    pub const X: u32 = 0x10;
    pub const Y: u32 = 0x20;
    pub const UP: u32 = 0x40;
    pub const DOWN: u32 = 0x80;
    pub const LEFT: u32 = 0x100;
    pub const RIGHT: u32 = 0x200;
    pub const LEFT_SHOULDER: u32 = 0x400;
    pub const RIGHT_SHOULDER: u32 = 0x800;
    pub const LEFT_STICK: u32 = 0x1000;
    pub const RIGHT_STICK: u32 = 0x2000;
}

/// How long a press or stick push lasts when no time is given.
pub const DEFAULT_HOLD_MS: u64 = 120;
/// The longest single hold or wait.
pub const MAX_HOLD_MS: u64 = 30_000;
/// The most steps in a script.
pub const MAX_STEPS: usize = 64;
/// The longest a whole script may run.
pub const MAX_SCRIPT_MS: u64 = 120_000;

/// What the pad shows now: triggers 0 to 1, sticks -1 to 1 (up positive).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub buttons: u32,
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_x: f32,
    pub left_y: f32,
    pub right_x: f32,
    pub right_y: f32,
}

/// One thing a press holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Control {
    Buttons(u32),
    LeftTrigger,
    RightTrigger,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    Left,
    Right,
}

/// One script step.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Hold these controls (and these stick pushes) for the time.
    Hold { controls: Vec<Control>, sticks: Vec<(Side, f32, f32)>, ms: u64 },
    Wait(u64),
}

/// The pad's held state. Times are passed in, so tests need no clock.
#[derive(Default)]
pub struct State {
    used: bool,
    buttons: Vec<(u32, Instant)>,
    triggers: [Option<Instant>; 2],
    sticks: [Option<(f32, f32, Instant)>; 2],
}

impl State {
    pub fn hold(&mut self, control: Control, until: Instant) {
        self.used = true;
        match control {
            Control::Buttons(mask) => self.buttons.push((mask, until)),
            Control::LeftTrigger => self.triggers[0] = Some(until),
            Control::RightTrigger => self.triggers[1] = Some(until),
        }
    }

    pub fn push(&mut self, side: Side, x: f32, y: f32, until: Instant) {
        self.used = true;
        self.sticks[side as usize] = Some((x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0), until));
    }

    pub fn clear(&mut self) {
        self.buttons.clear();
        self.triggers = [None; 2];
        self.sticks = [None; 2];
    }

    /// The pad at `now`, `None` until the first press or push (after that
    /// the pad stays connected, showing nothing held). Expired holds go.
    pub fn read(&mut self, now: Instant) -> Option<Snapshot> {
        if !self.used {
            return None;
        }
        self.buttons.retain(|&(_, until)| until > now);
        for t in &mut self.triggers {
            if t.is_some_and(|until| until <= now) {
                *t = None;
            }
        }
        for s in &mut self.sticks {
            if s.is_some_and(|(_, _, until)| until <= now) {
                *s = None;
            }
        }
        let stick = |s: Option<(f32, f32, Instant)>| s.map_or((0.0, 0.0), |(x, y, _)| (x, y));
        let (left_x, left_y) = stick(self.sticks[0]);
        let (right_x, right_y) = stick(self.sticks[1]);
        Some(Snapshot {
            buttons: self.buttons.iter().fold(0, |all, &(mask, _)| all | mask),
            left_trigger: f32::from(u8::from(self.triggers[0].is_some())),
            right_trigger: f32::from(u8::from(self.triggers[1].is_some())),
            left_x,
            left_y,
            right_x,
            right_y,
        })
    }
}

static STATE: Mutex<State> = Mutex::new(State {
    used: false,
    buttons: Vec::new(),
    triggers: [None; 2],
    sticks: [None; 2],
});
/// Bumped by `release_all` and by every new script: a running script stops.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The pad now, for the runtime's input poll. `None` until a tool was used.
pub fn read() -> Option<Snapshot> {
    state().read(Instant::now())
}

fn until(ms: u64) -> Instant {
    Instant::now() + Duration::from_millis(ms.min(MAX_HOLD_MS))
}

/// Presses the named buttons (`A`, `A+B`, `DpadUp`, `LT`...) for `ms`.
pub fn press(names: &str, ms: u64) -> Result<(), String> {
    let controls = parse_controls(names)?;
    let end = until(ms);
    let mut s = state();
    for c in controls {
        s.hold(c, end);
    }
    Ok(())
}

/// Pushes a stick for `ms`; a push of (0, 0) puts it back to centre.
pub fn stick(side: Side, x: f32, y: f32, ms: u64) {
    state().push(side, x, y, until(ms));
}

/// Lets go of everything and stops any running script.
pub fn release_all() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    state().clear();
}

/// Button names as the tools take them (any case), with the control each is.
pub fn control(name: &str) -> Option<Control> {
    let key: String = name.chars().filter(|c| !matches!(c, '_' | '-' | ' ')).collect::<String>().to_ascii_lowercase();
    Some(match key.as_str() {
        "a" => Control::Buttons(bit::A),
        "b" => Control::Buttons(bit::B),
        "x" => Control::Buttons(bit::X),
        "y" => Control::Buttons(bit::Y),
        "lb" => Control::Buttons(bit::LEFT_SHOULDER),
        "rb" => Control::Buttons(bit::RIGHT_SHOULDER),
        "lt" => Control::LeftTrigger,
        "rt" => Control::RightTrigger,
        "view" => Control::Buttons(bit::VIEW),
        "menu" => Control::Buttons(bit::MENU),
        "ls" => Control::Buttons(bit::LEFT_STICK),
        "rs" => Control::Buttons(bit::RIGHT_STICK),
        "dpadup" => Control::Buttons(bit::UP),
        "dpaddown" => Control::Buttons(bit::DOWN),
        "dpadleft" => Control::Buttons(bit::LEFT),
        "dpadright" => Control::Buttons(bit::RIGHT),
        _ => return None,
    })
}

fn parse_controls(names: &str) -> Result<Vec<Control>, String> {
    names
        .split('+')
        .map(|n| {
            control(n.trim()).ok_or_else(|| {
                format!(
                    "unknown button {:?}: use A B X Y LB RB LT RT View Menu LS RS DpadUp DpadDown DpadLeft DpadRight",
                    n.trim()
                )
            })
        })
        .collect()
}

/// A stick direction word: `LeftUp`, `RightDownLeft`... (a side, then one or
/// two of Up, Down, Left, Right).
fn stick_word(word: &str) -> Option<(Side, f32, f32)> {
    let w = word.to_ascii_lowercase();
    let (side, mut rest) = if let Some(r) = w.strip_prefix("left") {
        (Side::Left, r)
    } else if let Some(r) = w.strip_prefix("right") {
        (Side::Right, r)
    } else {
        return None;
    };
    if rest.is_empty() {
        return None;
    }
    let (mut x, mut y) = (0.0, 0.0);
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("up") {
            (y, rest) = (1.0, r);
        } else if let Some(r) = rest.strip_prefix("down") {
            (y, rest) = (-1.0, r);
        } else if let Some(r) = rest.strip_prefix("left") {
            (x, rest) = (-1.0, r);
        } else if let Some(r) = rest.strip_prefix("right") {
            (x, rest) = (1.0, r);
        } else {
            return None;
        }
    }
    Some((side, x, y))
}

/// Parses a script: steps split by commas, semicolons or new lines. A step
/// is `<what> [ms]` (hold time, default 120) or `wait <ms>`. `<what>` is a
/// button, buttons joined by `+` (`A+RT`), or a stick direction (`LeftUp`,
/// `RightDownLeft`; full push). Each step lasts its time, then the next
/// starts: `"A 120, wait 500, LeftUp 2000"` presses A for 0.12 s, waits half
/// a second, holds the left stick up for 2 s.
pub fn parse_script(text: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    let mut total = 0u64;
    for raw in text.split([',', ';', '\n']) {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let mut words = raw.split_whitespace();
        let what = words.next().unwrap_or_default();
        let time = words.next();
        if words.next().is_some() {
            return Err(format!("step {raw:?}: expected \"<what> [ms]\""));
        }
        let ms_of = |default: Option<u64>| -> Result<u64, String> {
            let ms = match (time, default) {
                (Some(t), _) => t
                    .trim_end_matches("ms")
                    .parse::<u64>()
                    .map_err(|_| format!("step {raw:?}: {t:?} is not a number of milliseconds"))?,
                (None, Some(d)) => d,
                (None, None) => return Err(format!("step {raw:?}: wait needs a time in ms")),
            };
            if ms > MAX_HOLD_MS {
                return Err(format!("step {raw:?}: at most {MAX_HOLD_MS} ms"));
            }
            Ok(ms)
        };
        let step = if what.eq_ignore_ascii_case("wait") {
            Step::Wait(ms_of(None)?)
        } else if let Some(stick) = stick_word(what) {
            Step::Hold { controls: Vec::new(), sticks: vec![stick], ms: ms_of(Some(DEFAULT_HOLD_MS))? }
        } else {
            Step::Hold {
                controls: parse_controls(what).map_err(|e| format!("step {raw:?}: {e}"))?,
                sticks: Vec::new(),
                ms: ms_of(Some(DEFAULT_HOLD_MS))?,
            }
        };
        total += match &step {
            Step::Wait(ms) | Step::Hold { ms, .. } => *ms,
        };
        steps.push(step);
        if steps.len() > MAX_STEPS {
            return Err(format!("at most {MAX_STEPS} steps"));
        }
    }
    if steps.is_empty() {
        return Err("the script has no steps".into());
    }
    if total > MAX_SCRIPT_MS {
        return Err(format!("the script runs {total} ms; at most {MAX_SCRIPT_MS}"));
    }
    Ok(steps)
}

/// Total running time of the steps, in milliseconds.
pub fn script_ms(steps: &[Step]) -> u64 {
    steps
        .iter()
        .map(|s| match s {
            Step::Wait(ms) | Step::Hold { ms, .. } => *ms,
        })
        .sum()
}

/// Runs the steps on a thread of their own (the call returns at once). A new
/// script or `release_all` stops the one running.
pub fn run_script(steps: Vec<Step>) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let spawned = std::thread::Builder::new().name("remote-pad-script".into()).spawn(move || {
        let alive = || GENERATION.load(Ordering::SeqCst) == generation;
        let nap = |ms: u64| {
            let end = Instant::now() + Duration::from_millis(ms);
            while alive() {
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                std::thread::sleep(left.min(Duration::from_millis(15)));
            }
        };
        for step in steps {
            if !alive() {
                return;
            }
            match step {
                Step::Wait(ms) => nap(ms),
                Step::Hold { controls, sticks, ms } => {
                    let end = until(ms);
                    {
                        let mut s = state();
                        for c in controls {
                            s.hold(c, end);
                        }
                        for (side, x, y) in sticks {
                            s.push(side, x, y, end);
                        }
                    }
                    nap(ms);
                }
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not start the input script");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_parses_buttons_sticks_and_waits() {
        let steps = parse_script("A 120, wait 500, LeftUp 2000").unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0], Step::Hold { controls: vec![Control::Buttons(bit::A)], sticks: vec![], ms: 120 });
        assert_eq!(steps[1], Step::Wait(500));
        assert_eq!(steps[2], Step::Hold { controls: vec![], sticks: vec![(Side::Left, 0.0, 1.0)], ms: 2000 });
        assert_eq!(script_ms(&steps), 2620);
    }

    #[test]
    fn script_defaults_combos_and_separators() {
        let steps = parse_script("a+rt;RightDownLeft 50ms\nDpadUp").unwrap();
        assert_eq!(
            steps[0],
            Step::Hold { controls: vec![Control::Buttons(bit::A), Control::RightTrigger], sticks: vec![], ms: DEFAULT_HOLD_MS }
        );
        assert_eq!(steps[1], Step::Hold { controls: vec![], sticks: vec![(Side::Right, -1.0, -1.0)], ms: 50 });
        assert_eq!(steps[2], Step::Hold { controls: vec![Control::Buttons(bit::UP)], sticks: vec![], ms: DEFAULT_HOLD_MS });
    }

    #[test]
    fn script_rejects_bad_input() {
        let many = "A,".repeat(MAX_STEPS + 1);
        let long = "wait 30000,".repeat(5);
        for bad in ["", "Q 100", "A abc", "wait", "A 100 200", "LeftSideways", "A 999999", many.as_str(), long.as_str()] {
            assert!(parse_script(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn holds_end_at_read_time_and_the_pad_stays_connected() {
        let mut s = State::default();
        let t0 = Instant::now();
        assert_eq!(s.read(t0), None);
        s.hold(Control::Buttons(bit::A | bit::B), t0 + Duration::from_millis(100));
        s.hold(Control::Buttons(bit::X), t0 + Duration::from_millis(300));
        s.hold(Control::RightTrigger, t0 + Duration::from_millis(100));
        s.push(Side::Left, 2.0, -0.5, t0 + Duration::from_millis(200));
        let a = s.read(t0 + Duration::from_millis(50)).unwrap();
        assert_eq!((a.buttons, a.right_trigger, a.left_x, a.left_y), (bit::A | bit::B | bit::X, 1.0, 1.0, -0.5));
        let b = s.read(t0 + Duration::from_millis(150)).unwrap();
        assert_eq!((b.buttons, b.right_trigger, b.left_x), (bit::X, 0.0, 1.0));
        let c = s.read(t0 + Duration::from_millis(250)).unwrap();
        assert_eq!((c.buttons, c.left_x, c.left_y), (bit::X, 0.0, 0.0));
        assert_eq!(s.read(t0 + Duration::from_secs(1)), Some(Snapshot::default()));
        s.hold(Control::Buttons(bit::Y), t0 + Duration::from_secs(9));
        s.clear();
        assert_eq!(s.read(t0 + Duration::from_secs(2)), Some(Snapshot::default()));
    }

    #[test]
    fn every_button_name_is_known() {
        for n in [
            "A", "B", "X", "Y", "LB", "RB", "LT", "RT", "View", "Menu", "LS", "RS", "DpadUp", "DpadDown", "DpadLeft", "DpadRight",
            "dpad_up",
        ] {
            assert!(control(n).is_some(), "{n}");
        }
        assert!(control("Start").is_none());
    }
}
