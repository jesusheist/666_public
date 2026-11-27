//! Keyboard and mouse played the way PC games are: keys to pad buttons and
//! sticks, the mouse to the right stick, mouse buttons to the triggers.
//!
//! The title reads gamepads and keyboards only (it registers no
//! mouse), and turns its right stick into a turn rate. Mouse counts are a
//! distance, so they become a rate when the title reads the pad: the counts
//! since its last read over the time since then. The camera then turns the
//! same angle for the same hand movement at any frame rate.
//!
//! All of this is plain data: the host side of `gameinput` feeds it and the
//! title's reads drain it. `[controls]` in victory.toml sets and rebinds it.
//!
//! # Binding syntax (`[controls.on_foot]`, `[controls.vehicle]`, `[controls.aircraft]`)
//!
//! A key maps to one string or a list of strings. Each string is one binding:
//! `[style:]name`. `name` is a raw pad output (`A`, `LB`, `LT`, `DpadUp`,
//! `LeftUp`, `Walk`, ...) or an action (`Jump`, `Sprint`, `Crouch`, `Shoot`,
//! `Accelerate`, ... see `ACTIONS`; some actions differ on foot and in a
//! vehicle, `Shoot` is RT on foot and RB in a vehicle). No style: the output
//! is held while the key is held. An empty string unbinds the key.
//!
//! Styles: `tap[ms]` presses briefly when the key is released within ms
//! (default 250); `hold[ms]` presses once the key has been held for ms
//! (default 300), until release; `double[ms]` presses briefly on a second
//! press within ms (default 250); `toggle` flips the output on and off with
//! each press; `macro:` runs a short sequence, `macro:Y 60, wait 100, A 60`
//! (press Y for 60 ms, wait 100 ms, press A for 60 ms; at most 16 steps, each
//! at most 2000 ms, 5000 ms in all, pad buttons, triggers and sticks only).
//!
//! ```toml
//! [controls.on_foot]
//! Tab = ["tap:Holster", "hold400:Loadout"]   # quick tap, long press
//! CapsLock = "toggle:Sprint"                  # run on, run off
//! X = "macro:Y 60, wait 100, A 60"            # interact, then jump
//! ```
//!
//! `F` is `Interact` (Y) and never switches the key layout. `G` switches it by
//! hand. With `VICTORY_VEHICLE_FLAG` set (a byte the title keeps for "the player is
//! in a vehicle", `img+74dfca5` for game 1.0.2.1) the layout follows the title.
//! `EnterVehicle` and `ExitVehicle` stay as names for older controls files: they
//! press Y and switch the layout only while no flag is in use.

use std::collections::{BTreeMap, VecDeque};

use victory_core::keybinds::{Def, Setup, Table};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::time::Instant;

use super::gameinput::{
    A, B, DOWN, LEFT, LEFT_SHOULDER, LEFT_STICK, MENU, PadState, RIGHT, RIGHT_SHOULDER,
    RIGHT_STICK, UP, VIEW, X, Y,
};

/// Key codes are Windows virtual keys; mouse buttons are theirs too (1, 2,
/// 4, 5, 6) and the wheel takes two codes Windows leaves unassigned.
pub(super) const WHEEL_UP: u8 = 0x0E;
pub(super) const WHEEL_DOWN: u8 = 0x0F;
/// Mouse buttons by virtual key: left, right, middle, back, forward.
pub(super) const MOUSE_BUTTONS: [u8; 5] = [0x01, 0x02, 0x04, 0x05, 0x06];

/// Mouse counts per second that make a full stick at sensitivity 1.
const FULL_RATE: f64 = 2000.0;
/// Title reads closer together than this see the same mouse stick.
const MIN_TAKE_US: u64 = 2_000;
/// The first read's time span, and the longest one (a title that paused;
/// the title's world frames take 1-2 s here for now, measured).
const FIRST_TAKE_US: u64 = 16_667;
const MAX_TAKE_US: u64 = 1_000_000;
/// Counts past a full stick wait at most this long (seconds of full stick).
const CARRY_S: f64 = 0.1;
/// A pad stick past this takes its stick back from the keyboard and mouse.
/// Detection only: the pad's own value is passed on as it is.
const PAD_TAKES_OVER: f32 = 0.2;
/// Wheel steps waiting to be pressed.
const WHEEL_QUEUE: usize = 8;

/// What a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Output {
    Button(u32),
    LeftTrigger,
    RightTrigger,
    /// One stick direction.
    Stick {
        right: bool,
        x: bool,
        positive: bool,
    },
    /// While held, the left stick moves at the walk speed.
    Walk,
    /// Switches between the on-foot and the vehicle layout.
    ToggleVehicle,
    /// Switches to the vehicle layout (true) or the on-foot layout (false).
    SetVehicle(bool),
    /// Frees the mouse cursor, or captures it again.
    ReleaseCursor,
}

const fn stick(right: bool, x: bool, positive: bool) -> Output {
    Output::Stick { right, x, positive }
}

const OUTPUTS: [(&str, Output); 29] = [
    ("A", Output::Button(A)),
    ("B", Output::Button(B)),
    ("X", Output::Button(X)),
    ("Y", Output::Button(Y)),
    ("LB", Output::Button(LEFT_SHOULDER)),
    ("RB", Output::Button(RIGHT_SHOULDER)),
    ("LT", Output::LeftTrigger),
    ("RT", Output::RightTrigger),
    ("LS", Output::Button(LEFT_STICK)),
    ("RS", Output::Button(RIGHT_STICK)),
    ("View", Output::Button(VIEW)),
    ("Menu", Output::Button(MENU)),
    ("DpadUp", Output::Button(UP)),
    ("DpadDown", Output::Button(DOWN)),
    ("DpadLeft", Output::Button(LEFT)),
    ("DpadRight", Output::Button(RIGHT)),
    ("LeftUp", stick(false, false, true)),
    ("LeftDown", stick(false, false, false)),
    ("LeftLeft", stick(false, true, false)),
    ("LeftRight", stick(false, true, true)),
    ("RightUp", stick(true, false, true)),
    ("RightDown", stick(true, false, false)),
    ("RightLeft", stick(true, true, false)),
    ("RightRight", stick(true, true, true)),
    ("Walk", Output::Walk),
    ("ToggleVehicle", Output::ToggleVehicle),
    ("LayoutVehicle", Output::SetVehicle(true)),
    ("LayoutFoot", Output::SetVehicle(false)),
    ("ReleaseCursor", Output::ReleaseCursor),
];

/// Where a binding is used: on foot, or in a vehicle (aircraft included).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Context {
    Foot,
    Vehicle,
}

/// Named actions: the pad outputs this title's own Default control scheme
/// gives each one, on foot and in a vehicle (raw output names, see `OUTPUTS`).
const ACTIONS: &[(&str, &[&str], &[&str])] = &[
    // Moving.
    ("MoveForward", &["LeftUp"], &["LeftUp"]),
    ("MoveBack", &["LeftDown"], &["LeftDown"]),
    ("MoveLeft", &["LeftLeft"], &["LeftLeft"]),
    ("MoveRight", &["LeftRight"], &["LeftRight"]),
    ("SteerLeft", &["LeftLeft"], &["LeftLeft"]),
    ("SteerRight", &["LeftRight"], &["LeftRight"]),
    ("LeanForward", &["LeftUp"], &["LeftUp"]),
    ("LeanBack", &["LeftDown"], &["LeftDown"]),
    ("LookUp", &["RightUp"], &["RightUp"]),
    ("LookDown", &["RightDown"], &["RightDown"]),
    ("LookLeft", &["RightLeft"], &["RightLeft"]),
    ("LookRight", &["RightRight"], &["RightRight"]),
    // On foot.
    ("Jump", &["A"], &["A"]),
    ("Climb", &["A"], &["A"]),
    ("Sprint", &["LS"], &["A"]),
    ("Crouch", &["B"], &["B"]),
    ("Rest", &["B"], &["B"]),
    ("Interact", &["Y"], &["Y"]),
    ("Cover", &["RB"], &["RB"]),
    ("QuickThrow", &["RB"], &["RB"]),
    ("Melee", &["X"], &["X"]),
    ("Aim", &["LT"], &["LT"]),
    ("Shoot", &["RT"], &["RB"]),
    ("Holster", &["LB"], &["LB"]),
    ("Loadout", &["LB"], &["LB"]),
    ("QuickSwap", &["Y"], &["Y"]),
    ("Block", &["A"], &["A"]),
    ("Dodge", &["A"], &["A"]),
    ("Grapple", &["RT"], &["RT"]),
    ("LockOn", &["LT"], &["LT"]),
    ("Dive", &["RB"], &["RB"]),
    ("Surface", &["A"], &["A"]),
    ("OpenParachute", &["A"], &["A"]),
    ("BrakeLeft", &["LT"], &["LT"]),
    ("BrakeRight", &["RT"], &["RT"]),
    ("ScopeIn", &["DpadUp"], &["DpadUp"]),
    ("ScopeOut", &["DpadDown"], &["DpadDown"]),
    ("SwitchShoulder", &["DpadLeft"], &["DpadLeft"]),
    // Everywhere.
    ("Map", &["View"], &["View"]),
    ("Pause", &["Menu"], &["Menu"]),
    ("LookBehind", &["RS"], &["RS"]),
    ("Zoom", &["RS"], &["RS"]),
    ("Phone", &["DpadUp"], &["DpadUp"]),
    ("Radio", &["DpadLeft"], &["DpadLeft"]),
    ("Objective", &["DpadRight"], &["DpadRight"]),
    ("Detonate", &["DpadDown"], &["DpadDown"]),
    ("PartnerMenu", &["DpadDown"], &["DpadDown"]),
    // Vehicles.
    ("EnterVehicle", &["Y", "LayoutVehicle"], &["Y", "LayoutVehicle"]),
    ("ExitVehicle", &["Y", "LayoutFoot"], &["Y", "LayoutFoot"]),
    ("Accelerate", &["RT"], &["RT"]),
    ("Brake", &["LT"], &["LT"]),
    ("Reverse", &["LT"], &["LT"]),
    ("Handbrake", &["A"], &["A"]),
    ("Horn", &["LS"], &["LS"]),
    ("Nitrous", &["LS"], &["LS"]),
    ("Headlights", &["B"], &["B"]),
    ("Duck", &["B"], &["B"]),
    ("IndicateLeft", &["DpadLeft"], &["DpadLeft"]),
    ("IndicateRight", &["DpadRight"], &["DpadRight"]),
    ("Bunnyhop", &["B"], &["B"]),
    ("Bell", &["LS"], &["LS"]),
    // Aircraft.
    ("ThrottleUp", &["RT"], &["RT"]),
    ("ThrottleDown", &["LT"], &["LT"]),
    ("YawLeft", &["X"], &["X"]),
    ("YawRight", &["B"], &["B"]),
    ("Undercarriage", &["LS"], &["LS"]),
];

/// Longest macro, each step, and all steps together (milliseconds).
const MACRO_STEPS: usize = 16;
const MACRO_STEP_MS: u64 = 2000;
const MACRO_TOTAL_MS: u64 = 5000;
/// How long a tap, double-tap or macro press stays down.
const PULSE_MS: u64 = 80;
/// Default tap, double-tap and hold times.
const TAP_MS: u64 = 250;
const HOLD_MS: u64 = 300;
/// Most pulses waiting or showing at once.
const MAX_TIMED: usize = 64;

/// How a binding reacts to its key.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Style {
    /// Held while the key is held.
    Held,
    /// A short press when the key is released within the time.
    Tap(u64),
    /// Held from the time the key has been down until it is released.
    Hold(u64),
    /// A short press on a second press within the time.
    Double(u64),
    /// Each press flips it on or off.
    Toggle,
    /// A sequence run on press.
    Macro(Vec<Step>),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Step {
    Press(Vec<Output>, u64),
    Wait(u64),
}

/// One binding of a key.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Bind {
    style: Style,
    outs: Vec<Output>,
}

/// Key names besides single letters and digits, `F1`-`F24` and `Num0`-`Num9`.
const KEYS: [(&str, u8); 48] = [
    ("MouseLeft", 0x01),
    ("MouseRight", 0x02),
    ("MouseMiddle", 0x04),
    ("Mouse4", 0x05),
    ("Mouse5", 0x06),
    ("WheelUp", WHEEL_UP),
    ("WheelDown", WHEEL_DOWN),
    ("Backspace", 0x08),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Pause", 0x13),
    ("CapsLock", 0x14),
    ("Esc", 0x1B),
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("NumMultiply", 0x6A),
    ("NumAdd", 0x6B),
    ("NumSubtract", 0x6D),
    ("NumDecimal", 0x6E),
    ("NumDivide", 0x6F),
    ("LShift", 0xA0),
    ("RShift", 0xA1),
    ("LCtrl", 0xA2),
    ("RCtrl", 0xA3),
    ("LAlt", 0xA4),
    ("RAlt", 0xA5),
    ("Semicolon", 0xBA),
    ("Equals", 0xBB),
    ("Comma", 0xBC),
    ("Minus", 0xBD),
    ("Period", 0xBE),
    ("Slash", 0xBF),
    ("Grave", 0xC0),
    ("LBracket", 0xDB),
    ("Backslash", 0xDC),
    ("RBracket", 0xDD),
    ("Quote", 0xDE),
    ("Shift", 0xA0),
    ("Ctrl", 0xA2),
];

/// The virtual key a key name stands for (case does not matter).
pub(super) fn key_code(name: &str) -> Option<u8> {
    let name = name.trim();
    if let [c] = name.as_bytes()
        && c.is_ascii_alphanumeric()
    {
        return Some(c.to_ascii_uppercase());
    }
    if let Some((_, vk)) = KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        return Some(*vk);
    }
    if name.eq_ignore_ascii_case("Alt") {
        return Some(0xA4);
    }
    let numbered = |prefix: &str, first: u8, count: u8| {
        let rest = name.get(prefix.len()..)?;
        if !name.get(..prefix.len())?.eq_ignore_ascii_case(prefix) {
            return None;
        }
        let n: u8 = rest.parse().ok()?;
        (n < count).then(|| first + n)
    };
    numbered("Num", 0x60, 10).or_else(|| {
        numbered("F", 0x6F, 25).filter(|&vk| vk > 0x6F) // F1 = 0x70
    })
}

/// The key a US keyboard has at the place of scan code `scan` (letters,
/// digits and punctuation; `None` for keys that sit at the same place on
/// every layout, which keep their virtual key). Bindings name keys by that
/// place, so the hand position is the same on every layout: the keys of
/// W A S D are Z Q S D on an AZERTY keyboard.
pub(super) fn us_key(scan: u32) -> Option<u8> {
    // Rows of a US keyboard from their first scan code: digits to `=`,
    // Q to `]`, A to the grave accent, `\` to `/`.
    const ROWS: [(u32, &[u8]); 4] = [
        (
            0x02,
            &[
                b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', 0xBD, 0xBB,
            ],
        ),
        (
            0x10,
            &[
                b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', 0xDB, 0xDD,
            ],
        ),
        (
            0x1E,
            &[
                b'A', b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L', 0xBA, 0xDE, 0xC0,
            ],
        ),
        (
            0x2B,
            &[
                0xDC, b'Z', b'X', b'C', b'V', b'B', b'N', b'M', 0xBC, 0xBE, 0xBF,
            ],
        ),
    ];
    ROWS.iter().find_map(|&(first, keys)| {
        let at = scan.checked_sub(first)?;
        keys.get(at as usize).copied()
    })
}

/// A key's name, for messages.
pub(super) fn key_name(vk: u8) -> String {
    if vk.is_ascii_uppercase() || vk.is_ascii_digit() {
        return char::from(vk).to_string();
    }
    match vk {
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => KEYS
            .iter()
            .find(|(_, k)| *k == vk)
            .map_or_else(|| format!("{vk:#04x}"), |(n, _)| (*n).to_string()),
    }
}

fn output(name: &str) -> Option<Output> {
    OUTPUTS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
        .map(|(_, o)| *o)
}

/// A raw output name or an action name as pad outputs.
fn resolve(name: &str, context: Context) -> Option<Vec<Output>> {
    let name = name.trim();
    if let Some(out) = output(name) {
        return Some(vec![out]);
    }
    let (_, foot, vehicle) = ACTIONS.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(name))?;
    let names = match context {
        Context::Foot => foot,
        Context::Vehicle => vehicle,
    };
    names.iter().map(|n| output(n)).collect()
}

/// `tap`, `hold300`, ...: the style word and its optional milliseconds.
fn style_word(word: &str) -> Option<(&str, Option<u64>)> {
    let split = word.find(|c: char| c.is_ascii_digit()).unwrap_or(word.len());
    let (name, digits) = word.split_at(split);
    let ms = if digits.is_empty() {
        None
    } else {
        Some(digits.parse().ok()?)
    };
    ["tap", "hold", "double", "toggle"]
        .iter()
        .find(|s| s.eq_ignore_ascii_case(name))
        .map(|s| (*s, ms))
}

fn parse_macro(text: &str, context: Context) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    let mut total = 0;
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if steps.len() == MACRO_STEPS {
            return Err(format!("a macro has at most {MACRO_STEPS} steps"));
        }
        let mut words = part.split_whitespace();
        let Some(name) = words.next() else {
            continue;
        };
        let rest: Vec<&str> = words.collect();
        let ms_of = |default: u64| -> Result<u64, String> {
            match rest.as_slice() {
                [] => Ok(default),
                [ms] => ms
                    .trim_end_matches("ms")
                    .parse::<u64>()
                    .ok()
                    .filter(|&ms| ms <= MACRO_STEP_MS)
                    .ok_or_else(|| {
                        format!("macro step `{part}`: time must be 0 to {MACRO_STEP_MS} ms")
                    }),
                _ => Err(format!("macro step `{part}`: expected `name [ms]`")),
            }
        };
        let step = if name.eq_ignore_ascii_case("wait") {
            let ms = ms_of(0)?;
            total += ms;
            Step::Wait(ms)
        } else {
            let outs = resolve(name, context)
                .ok_or_else(|| format!("unknown pad output `{name}` in a macro"))?;
            if !outs.iter().all(|o| {
                matches!(
                    o,
                    Output::Button(_)
                        | Output::LeftTrigger
                        | Output::RightTrigger
                        | Output::Stick { .. }
                )
            }) {
                return Err(format!(
                    "macro step `{part}`: only pad buttons, triggers and sticks can be pressed"
                ));
            }
            let ms = ms_of(PULSE_MS)?;
            total += ms;
            Step::Press(outs, ms)
        };
        steps.push(step);
        if total > MACRO_TOTAL_MS {
            return Err(format!("a macro runs at most {MACRO_TOTAL_MS} ms in all"));
        }
    }
    if steps.is_empty() {
        return Err("a macro needs at least one step".into());
    }
    Ok(steps)
}

/// One binding string: `[style:]name` or `macro:steps`.
fn parse_bind(spec: &str, context: Context) -> Result<Bind, String> {
    let spec = spec.trim();
    let Some((word, rest)) = spec.split_once(':') else {
        let outs = resolve(spec, context).ok_or_else(|| format!("unknown pad output `{spec}`"))?;
        return Ok(Bind {
            style: Style::Held,
            outs,
        });
    };
    let word = word.trim();
    if word.eq_ignore_ascii_case("macro") {
        return Ok(Bind {
            style: Style::Macro(parse_macro(rest, context)?),
            outs: Vec::new(),
        });
    }
    let (style, ms) = style_word(word).ok_or_else(|| {
        format!("unknown press style `{word}` (tap, hold, double, toggle, macro)")
    })?;
    let time = |default: u64| match ms {
        Some(ms) if !(20..=2000).contains(&ms) => {
            Err(format!("press time {ms} ms is outside 20 to 2000"))
        }
        Some(ms) => Ok(ms),
        None => Ok(default),
    };
    let style = match style {
        "tap" => Style::Tap(time(TAP_MS)?),
        "hold" => Style::Hold(time(HOLD_MS)?),
        "double" => Style::Double(time(TAP_MS)?),
        _ if ms.is_some() => return Err("`toggle` takes no time".into()),
        _ => Style::Toggle,
    };
    let outs =
        resolve(rest, context).ok_or_else(|| format!("unknown pad output `{}`", rest.trim()))?;
    Ok(Bind { style, outs })
}

/// Keys and what each does, for one situation (on foot, in a vehicle).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Layout(BTreeMap<u8, Vec<Bind>>);

impl Layout {
    fn binds(&self, vk: u8) -> &[Bind] {
        self.0.get(&vk).map_or(&[], Vec::as_slice)
    }

    /// Every output the key's bindings name (for tests).
    #[cfg(test)]
    fn outputs(&self, vk: u8) -> Vec<Output> {
        self.binds(vk)
            .iter()
            .flat_map(|b| b.outs.iter().copied())
            .collect()
    }

    /// Binds `key` to `outputs` (none: unbinds it).
    fn bind(&mut self, key: &str, outputs: &[&str], context: Context) -> Result<(), String> {
        let vk = key_code(key).ok_or_else(|| format!("unknown key `{key}`"))?;
        let binds = outputs
            .iter()
            .filter(|o| !o.trim().is_empty())
            .map(|o| parse_bind(o, context).map_err(|e| format!("{e} for key `{key}`")))
            .collect::<Result<Vec<_>, _>>()?;
        if binds.is_empty() {
            self.0.remove(&vk);
        } else {
            self.0.insert(vk, binds);
        }
        Ok(())
    }

    /// The layout a table of key names and binding strings makes (a binding that does not parse is left out).
    fn from_table(table: &Table, context: Context) -> Layout {
        let mut layout = Layout::default();
        for (key, binds) in table {
            let outs: Vec<&str> = binds.iter().map(String::as_str).collect();
            if let Err(why) = layout.bind(key, &outs, context) {
                tracing::warn!(%why, "controls: a binding was left out");
            }
        }
        layout
    }

    fn of(table: &[(&str, &[&str])], context: Context) -> Layout {
        let mut layout = Layout::default();
        for (key, outs) in table {
            layout
                .bind(key, outs, context)
                .expect("default bindings use known names");
        }
        layout
    }
}

/// On foot: the usual PC keys on the actions this title's own Default
/// control scheme has (circled icon = hold, plain = tap on the pad; the same
/// button does both, by how long the key is held): A jump, LS click run, B tap
/// crouch / hold rest, X melee, Y interact, RB cover, LB tap holster / hold
/// loadout, LT aim, RT shoot, RS click look behind, d-pad up phone / left
/// radio / right objective / down detonate.
const ON_FOOT: &[(&str, &[&str])] = &[
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
    ("Tab", &["Holster"]),
    ("WheelUp", &["ScopeIn"]),
    ("WheelDown", &["ScopeOut"]),
    ("MouseLeft", &["Shoot"]),
    ("MouseRight", &["Aim"]),
    ("MouseMiddle", &["LookBehind"]),
    ("Up", &["Phone"]),
    ("Down", &["Detonate"]),
    ("Left", &["Radio"]),
    ("Right", &["Objective"]),
    ("Enter", &["A"]),
    ("Backspace", &["B"]),
    ("Esc", &["Menu", "ReleaseCursor"]),
    ("P", &["Pause"]),
];

/// Land and water vehicles, including bicycles: RT accelerate, LT brake and
/// reverse, A handbrake, LS click horn, B tap headlights / hold duck, Y exit,
/// RB shoot, X melee, RS click look behind, d-pad left / right indicators.
const VEHICLE: &[(&str, &[&str])] = &[
    ("W", &["Accelerate"]),
    ("S", &["Brake"]),
    ("A", &["SteerLeft"]),
    ("D", &["SteerRight"]),
    ("Num8", &["LeanForward"]),
    ("Num5", &["LeanBack"]),
    ("Space", &["Handbrake"]),
    // Shift would reach the handbrake through Sprint (A in a vehicle): the handbrake is Space alone.
    ("LShift", &[]),
    ("E", &["Horn"]),
    ("H", &["Headlights"]),
    ("LCtrl", &["Duck"]),
    ("F", &["Interact"]),
    ("G", &["ToggleVehicle"]),
    ("R", &["Melee"]),
    ("C", &["LookBehind"]),
    ("V", &["Map"]),
    ("M", &["Map"]),
    ("Tab", &["Holster"]),
    ("WheelUp", &["ScopeIn"]),
    ("WheelDown", &["ScopeOut"]),
    ("MouseLeft", &["Shoot"]),
    // Aim is LT, which is the brake and reverse in a vehicle, and the title's vehicle scheme has no aim
    // (RB shoots, LB is holster and loadout): the right button does nothing in a vehicle.
    ("MouseRight", &[]),
    ("MouseMiddle", &["LookBehind"]),
    ("Up", &["Phone"]),
    ("Down", &["PartnerMenu"]),
    ("Left", &["IndicateLeft"]),
    ("Right", &["IndicateRight"]),
    ("Enter", &["A"]),
    ("Backspace", &["B"]),
    ("Esc", &["Menu", "ReleaseCursor"]),
    ("P", &["Pause"]),
];

/// Aircraft, laid over the vehicle table: RT throttle up, LT throttle down,
/// X yaw left, B yaw right, LS click undercarriage, RB shoot, Y exit.
const AIRCRAFT: &[(&str, &[&str])] = &[
    ("W", &["ThrottleUp"]),
    ("S", &["ThrottleDown"]),
    ("A", &["YawLeft"]),
    ("D", &["YawRight"]),
    ("Num8", &["LeftUp"]),
    ("Num5", &["LeftDown"]),
    ("Num4", &["LeftLeft"]),
    ("Num6", &["LeftRight"]),
    ("Space", &[]),
    ("E", &[]),
    ("G", &["Undercarriage"]),
    ("Tab", &["Holster"]),
    ("MouseLeft", &["Shoot"]),
    ("MouseRight", &[]),
];

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Settings {
    /// Off: the old stand-in pad (README's table) and nothing else.
    pub enabled: bool,
    /// Keep the keyboard separate from the stand-in pad for a native input probe.
    pub keys_as_pad: bool,
    pub sensitivity: f32,
    /// Vertical sensitivity relative to horizontal.
    pub y_scale: f32,
    pub invert_y: bool,
    /// Capture (hide and hold) the cursor while the title window has focus.
    pub capture: bool,
    /// Counts past a full stick are turned later instead of dropped.
    pub carry: bool,
    /// The title's own stick dead zone and response exponent, undone so the
    /// smallest movement turns and the response is linear.
    pub deadzone: f32,
    pub curve: f32,
    pub walk_speed: f32,
    /// Letters, digits and punctuation are the keys at those places on a
    /// US keyboard ([`us_key`]), whatever the layout prints on them.
    pub by_position: bool,
    pub on_foot: Layout,
    pub vehicle: Layout,
    pub(super) start_in_vehicle: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            enabled: true,
            keys_as_pad: true,
            sensitivity: 1.0,
            y_scale: 1.0,
            invert_y: false,
            capture: true,
            carry: true,
            deadzone: 0.3,
            curve: 1.0,
            walk_speed: 0.5,
            by_position: true,
            on_foot: Layout::of(ON_FOOT, Context::Foot),
            vehicle: Layout::of(VEHICLE, Context::Vehicle),
            start_in_vehicle: false,
        }
    }
}

/// Typed options from the launcher, independent of the config file format.
#[derive(Default)]
pub struct Options {
    pub enabled: Option<bool>,
    pub mouse_sensitivity: Option<f32>,
    pub mouse_y_scale: Option<f32>,
    pub mouse_invert_y: Option<bool>,
    pub mouse_capture: Option<bool>,
    pub mouse_carry: Option<bool>,
    pub stick_deadzone: Option<f32>,
    pub stick_curve: Option<f32>,
    pub walk_speed: Option<f32>,
    pub keys_by_position: Option<bool>,
    pub start_in_vehicle: Option<bool>,
    pub aircraft_vehicle: bool,
    /// The controls file the run was started from (the CONTROLS page saves into it).
    pub config_path: Option<std::path::PathBuf>,
    pub on_foot: BTreeMap<String, Vec<String>>,
    pub vehicle: BTreeMap<String, Vec<String>>,
    pub aircraft: BTreeMap<String, Vec<String>>,
}

impl Settings {
    fn with_options(file: &Options) -> Result<Settings, String> {
        let mut s = Settings::default();
        let range = |name: &str, v: Option<f32>, lo: f32, hi: f32, into: &mut f32| match v {
            Some(v) if !(lo..=hi).contains(&v) => {
                Err(format!("[controls] {name} = {v} is outside {lo} to {hi}"))
            }
            Some(v) => {
                *into = v;
                Ok(())
            }
            None => Ok(()),
        };
        range(
            "mouse_sensitivity",
            file.mouse_sensitivity,
            0.01,
            100.0,
            &mut s.sensitivity,
        )?;
        range(
            "mouse_y_scale",
            file.mouse_y_scale,
            0.01,
            100.0,
            &mut s.y_scale,
        )?;
        range(
            "stick_deadzone",
            file.stick_deadzone,
            0.0,
            0.9,
            &mut s.deadzone,
        )?;
        range("stick_curve", file.stick_curve, 0.2, 5.0, &mut s.curve)?;
        range("walk_speed", file.walk_speed, 0.05, 1.0, &mut s.walk_speed)?;
        s.enabled = file.enabled.unwrap_or(s.enabled);
        s.invert_y = file.mouse_invert_y.unwrap_or(s.invert_y);
        s.capture = file.mouse_capture.unwrap_or(s.capture);
        s.carry = file.mouse_carry.unwrap_or(s.carry);
        s.by_position = file.keys_by_position.unwrap_or(s.by_position);
        s.start_in_vehicle = file.start_in_vehicle.unwrap_or_default();
        let vehicle = if file.aircraft_vehicle {
            for &(key, outputs) in AIRCRAFT {
                s.vehicle
                    .bind(key, outputs, Context::Vehicle)
                    .expect("valid aircraft defaults");
            }
            (&file.aircraft, "aircraft")
        } else {
            (&file.vehicle, "vehicle")
        };
        // Validate either override table even when the other profile is active.
        let mut unused_vehicle = Layout::default();
        let unused = if file.aircraft_vehicle {
            (&file.vehicle, "vehicle")
        } else {
            (&file.aircraft, "aircraft")
        };
        for (layout, context, (binds, name)) in [
            (&mut s.on_foot, Context::Foot, (&file.on_foot, "on_foot")),
            (&mut s.vehicle, Context::Vehicle, vehicle),
            (&mut unused_vehicle, Context::Vehicle, unused),
        ] {
            for (key, binding) in binds {
                let outs: Vec<&str> = binding.iter().map(String::as_str).collect();
                layout
                    .bind(key, &outs, context)
                    .map_err(|e| format!("[controls.{name}]: {e}"))?;
            }
        }
        Ok(s)
    }
}

static SETTINGS: OnceLock<Settings> = OnceLock::new();

fn fallback_settings() -> Settings {
    Settings {
        enabled: false,
        ..Settings::default()
    }
}

/// Takes `[controls]` from the config file (absent: CGBJ input). Call it
/// before the title starts; later calls change nothing.
pub fn configure(options: Option<&Options>) -> Result<(), String> {
    let mut s = match options {
        Some(options) => Settings::with_options(options)?,
        None => fallback_settings(),
    };
    s.keys_as_pad = std::env::var_os("VICTORY_KEYS_AS_PAD").is_none_or(|value| value != "0");
    tracing::info!(
        enabled = s.enabled,
        keys_as_pad = s.keys_as_pad,
        sensitivity = s.sensitivity,
        invert_y = s.invert_y,
        capture = s.capture,
        deadzone = s.deadzone,
        curve = s.curve,
        "keyboard and mouse controls"
    );
    if let Some(options) = options {
        victory_core::keybinds::install(bindings_setup(&s, options));
        victory_core::keybinds::set_namer(name_for_vk);
    }
    let _ = SETTINGS.set(s);
    Ok(())
}

pub(super) fn settings() -> Settings {
    SETTINGS.get_or_init(fallback_settings).clone()
}

pub(super) fn keys_as_pad() -> bool {
    SETTINGS.get().is_none_or(|settings| settings.keys_as_pad)
}

/// Whether the vehicle layout is in force now (written where the layout
/// changes, read by [`prompt_key`]).
static LAYOUT_IS_VEHICLE: AtomicBool = AtomicBool::new(false);

/// Whether `[controls]` is on (the keyboard drives the stand-in pad). After
/// [`configure`].
pub fn enabled() -> bool {
    SETTINGS.get().is_some_and(|settings| settings.enabled)
}

/// Whether the keyboard and mouse drive the stand-in pad: `[controls]` is on and `VICTORY_KEYS_AS_PAD` is not 0.
pub fn mapped_input_enabled() -> bool {
    SETTINGS
        .get()
        .is_some_and(|settings| settings.enabled && settings.keys_as_pad)
}

/// The key the layout in force binds to the pad output `output` (`A`, `LB`,
/// `DpadUp`, `LeftUp`, ...), for the title's key cap prompts. A key that
/// holds the output (a plain hold first, then tap, hold, double or toggle
/// styles) counts; a macro does not. `None` when `[controls]` is off or no
/// key binds it. Mouse wheel steps are not offered: they have no key cap.
pub fn prompt_key(output: &str) -> Option<u8> {
    let settings = SETTINGS.get().filter(|settings| settings.enabled)?;
    let wanted = self::output(output)?;
    key_for(settings, LAYOUT_IS_VEHICLE.load(Ordering::Relaxed), wanted)
}

/// The key of `settings`' on-foot or vehicle layout whose bindings hold
/// `wanted`: a plain hold first, then any other style but a macro.
fn key_for(settings: &Settings, vehicle: bool, wanted: Output) -> Option<u8> {
    let layout = if vehicle {
        &settings.vehicle
    } else {
        &settings.on_foot
    };
    let pick = |plain_only: bool| {
        layout.0.iter().find_map(|(&vk, binds)| {
            let holds = binds.iter().any(|bind| {
                let style_ok = match bind.style {
                    Style::Held => true,
                    Style::Macro(_) => false,
                    _ => !plain_only,
                };
                style_ok && bind.outs.contains(&wanted)
            });
            (holds && vk != WHEEL_UP && vk != WHEEL_DOWN).then_some(vk)
        })
    };
    pick(true).or_else(|| pick(false))
}

// ---------------------------------------------------------------------------
// The CONTROLS page of the in-game menu (`victory_core::keybinds`).

/// The actions the page lists for the on-foot, vehicle and aircraft layouts:
/// the name the controls file uses, and what the page calls it.
const PAGE_ON_FOOT: &[(&str, &str)] = &[
    ("MoveForward", "Move forward"),
    ("MoveBack", "Move back"),
    ("MoveLeft", "Move left"),
    ("MoveRight", "Move right"),
    ("Walk", "Walk slowly (hold)"),
    ("Sprint", "Sprint"),
    ("Jump", "Jump"),
    ("Crouch", "Crouch / rest"),
    ("Interact", "Interact / get in (doors, prompts)"),
    ("ToggleVehicle", "Switch to vehicle keys"),
    ("Melee", "Melee"),
    ("Cover", "Cover / quick throw"),
    ("Aim", "Aim"),
    ("Shoot", "Shoot"),
    ("Holster", "Holster / loadout"),
    ("ScopeIn", "Scope in (d-pad up)"),
    ("ScopeOut", "Scope out (d-pad down)"),
    ("LookBehind", "Look behind"),
    ("Map", "Map"),
    ("Phone", "Phone"),
    ("Radio", "Radio"),
    ("Objective", "Objective"),
    ("Detonate", "Detonate"),
    ("Pause", "Pause"),
    ("A", "Menu: confirm"),
    ("B", "Menu: back"),
];

const PAGE_VEHICLE: &[(&str, &str)] = &[
    ("Accelerate", "Accelerate"),
    ("Brake", "Brake / reverse"),
    ("SteerLeft", "Steer left"),
    ("SteerRight", "Steer right"),
    ("LeanForward", "Lean forward (bikes)"),
    ("LeanBack", "Lean back (bikes)"),
    ("Handbrake", "Handbrake"),
    ("Horn", "Horn"),
    ("Headlights", "Headlights"),
    ("Duck", "Duck"),
    ("Interact", "Interact / get out (doors, prompts)"),
    ("ToggleVehicle", "Switch to on-foot keys"),
    ("Melee", "Melee"),
    ("LookBehind", "Look behind"),
    ("Map", "Map"),
    ("Holster", "Holster / loadout"),
    ("ScopeIn", "Scope in (d-pad up)"),
    ("ScopeOut", "Scope out (d-pad down)"),
    ("Shoot", "Shoot"),
    ("Phone", "Phone"),
    ("PartnerMenu", "Partner menu"),
    ("IndicateLeft", "Indicate left"),
    ("IndicateRight", "Indicate right"),
    ("Pause", "Pause"),
    ("A", "Menu: confirm"),
    ("B", "Menu: back"),
];

const PAGE_AIRCRAFT: &[(&str, &str)] = &[
    ("ThrottleUp", "Throttle up"),
    ("ThrottleDown", "Throttle down"),
    ("YawLeft", "Yaw left"),
    ("YawRight", "Yaw right"),
    ("LeftUp", "Left stick up"),
    ("LeftDown", "Left stick down"),
    ("LeftLeft", "Left stick left"),
    ("LeftRight", "Left stick right"),
    ("Undercarriage", "Landing gear"),
    ("Interact", "Interact / get out"),
    ("Headlights", "Headlights"),
    ("Shoot", "Shoot"),
    ("Holster", "Holster / loadout"),
    ("LookBehind", "Look behind"),
    ("Map", "Map"),
    ("Phone", "Phone"),
    ("Pause", "Pause"),
    ("A", "Menu: confirm"),
    ("B", "Menu: back"),
];

/// What a pad output is called on the page.
fn pad_name(output: &str) -> String {
    match output {
        "LS" => "LS click".into(),
        "RS" => "RS click".into(),
        "DpadUp" => "D-pad up".into(),
        "DpadDown" => "D-pad down".into(),
        "DpadLeft" => "D-pad left".into(),
        "DpadRight" => "D-pad right".into(),
        "LeftUp" => "left stick up".into(),
        "LeftDown" => "left stick down".into(),
        "LeftLeft" => "left stick left".into(),
        "LeftRight" => "left stick right".into(),
        "RightUp" => "right stick up".into(),
        "RightDown" => "right stick down".into(),
        "RightLeft" => "right stick left".into(),
        "RightRight" => "right stick right".into(),
        "Walk" => "left stick at walk speed".into(),
        "ToggleVehicle" => "no pad button, switches the key layout".into(),
        "LayoutVehicle" => "switches to the vehicle keys".into(),
        "LayoutFoot" => "switches to the on-foot keys".into(),
        "ReleaseCursor" => "frees the mouse".into(),
        other => other.to_owned(),
    }
}

/// What the pad does for action `id` in a vehicle layout (`vehicle`) or on foot.
fn pad_text(id: &str, vehicle: bool) -> String {
    let outs: Vec<&str> = match ACTIONS.iter().find(|(name, _, _)| name.eq_ignore_ascii_case(id)) {
        Some((_, foot, car)) => (if vehicle { car } else { foot }).to_vec(),
        None => vec![id],
    };
    outs.into_iter().map(pad_name).collect::<Vec<_>>().join(" and ")
}

/// A key name as the controls file spells it (`Shift` is `LShift`; names that are not keys stay as they are).
fn canonical_key(name: &str) -> String {
    key_code(name).map_or_else(|| name.trim().to_owned(), key_name)
}

fn default_table(rows: &[&[(&str, &[&str])]]) -> Table {
    let mut table = Table::new();
    for &(key, outs) in rows.iter().flat_map(|r| r.iter()) {
        table.insert(canonical_key(key), outs.iter().filter(|o| !o.trim().is_empty()).map(|o| (*o).to_owned()).collect());
    }
    table
}

fn laid_over(mut table: Table, file: &BTreeMap<String, Vec<String>>) -> Table {
    for (key, outs) in file {
        table.insert(canonical_key(key), outs.iter().filter(|o| !o.trim().is_empty()).cloned().collect());
    }
    table
}

/// What the CONTROLS page is given: its actions, the defaults and the bindings in force.
fn bindings_setup(s: &Settings, options: &Options) -> Setup {
    let defs = |list: &[(&str, &str)], vehicle: bool| -> Vec<Def> {
        list.iter().map(|&(id, label)| Def::new(id, label, &pad_text(id, vehicle))).collect()
    };
    let foot = default_table(&[ON_FOOT]);
    let vehicle = default_table(&[VEHICLE]);
    let aircraft = default_table(&[VEHICLE, AIRCRAFT]);
    Setup {
        defs: [defs(PAGE_ON_FOOT, false), defs(PAGE_VEHICLE, true), defs(PAGE_AIRCRAFT, true)],
        current: [
            laid_over(foot.clone(), &options.on_foot),
            laid_over(vehicle.clone(), &options.vehicle),
            laid_over(aircraft.clone(), &options.aircraft),
        ],
        defaults: [foot, vehicle, aircraft],
        source: options.config_path.clone(),
        aircraft_active: options.aircraft_vehicle,
        active: s.enabled,
    }
}

/// The controls file's name of the virtual key `vk` as the title will see it: with
/// `keys_by_position` the key at that place of a US keyboard. `None` for a key the file
/// has no name for.
fn name_for_vk(vk: u8) -> Option<String> {
    #[cfg(windows)]
    let vk = if vk >= 0x08 && SETTINGS.get().is_none_or(|s| s.by_position) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC_EX, MapVirtualKeyW};
        // SAFETY: a plain query.
        let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC_EX) };
        us_key(scan).unwrap_or(vk)
    } else {
        vk
    };
    let name = key_name(vk);
    (key_code(&name) == Some(vk)).then_some(name)
}

/// Mouse motion and wheel steps since they were last drained, added from
/// the host's raw input thread (only while the cursor is captured).
#[derive(Default)]
pub(super) struct Counts {
    x: AtomicI64,
    y: AtomicI64,
    wheel: AtomicI32,
}

impl Counts {
    pub(super) fn add(&self, dx: i64, dy: i64) {
        self.x.fetch_add(dx, Ordering::Relaxed);
        self.y.fetch_add(dy, Ordering::Relaxed);
    }
    /// Steps away from the user are positive, as Windows reports them.
    pub(super) fn add_wheel(&self, steps: i32) {
        self.wheel.fetch_add(steps, Ordering::Relaxed);
    }
    fn drain(&self) -> (i64, i64) {
        (
            self.x.swap(0, Ordering::Relaxed),
            self.y.swap(0, Ordering::Relaxed),
        )
    }
    fn drain_wheel(&self) -> i32 {
        self.wheel.swap(0, Ordering::Relaxed)
    }
}

/// Which device moved a stick last, or was used last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Owner {
    Pad,
    Kbm,
}

/// What a set of held keys does.
#[derive(Clone, Copy, Default, PartialEq)]
struct Derived {
    buttons: u32,
    left_trigger: bool,
    right_trigger: bool,
    left: (f32, f32),
    right: (f32, f32),
    left_moved: bool,
    right_moved: bool,
}

/// A wheel step is a press the title has read once, then a release it has
/// read once, so steps survive any frame rate.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pulse {
    Idle,
    Down(u8),
    Gap,
}

/// The mouse as a right stick.
#[derive(Default)]
struct Look {
    /// Counts not yet turned (y as Windows reports it: down positive).
    pending: (f64, f64),
    last_take: Option<u64>,
    value: (f32, f32),
    moved: bool,
}

impl Look {
    fn take(&mut self, now: u64, (dx, dy): (i64, i64), s: &Settings) -> (f32, f32) {
        self.pending.0 += dx as f64;
        self.pending.1 += dy as f64;
        if let Some(last) = self.last_take
            && now.saturating_sub(last) < MIN_TAKE_US
        {
            return self.value;
        }
        let span = self.last_take.map_or(FIRST_TAKE_US, |last| {
            now.saturating_sub(last).min(MAX_TAKE_US)
        });
        self.last_take = Some(now);
        let dt = span as f64 / 1e6;
        let full = FULL_RATE / f64::from(s.sensitivity);
        let mut ux = self.pending.0 / dt / full;
        let mut uy = -self.pending.1 * f64::from(s.y_scale) / dt / full;
        let mut m = ux.hypot(uy);
        self.moved = m > 0.0;
        if m <= 1.0 {
            self.pending = (0.0, 0.0);
        } else {
            // Full stick in the direction moved; the rest waits (capped).
            let keep = if s.carry { 1.0 - 1.0 / m } else { 0.0 };
            self.pending.0 *= keep;
            self.pending.1 *= keep;
            let cap = CARRY_S * full;
            let left = self.pending.0.hypot(self.pending.1);
            if left > cap {
                self.pending.0 *= cap / left;
                self.pending.1 *= cap / left;
            }
            ux /= m;
            uy /= m;
            m = 1.0;
        }
        if m > 0.0 {
            let d = f64::from(s.deadzone);
            let shaped = d + (1.0 - d) * m.powf(1.0 / f64::from(s.curve));
            ux *= shaped / m;
            uy *= shaped / m;
        }
        if s.invert_y {
            uy = -uy;
        }
        self.value = (ux as f32, uy as f32);
        self.value
    }
}

/// What a press style remembers about one binding.
#[derive(Clone, Copy, Default)]
struct BindState {
    /// A hold has reached its time and is down.
    fired: bool,
    toggled: bool,
    last_press: Option<u64>,
}

/// An output pressed between two times.
struct Timed {
    start: u64,
    end: u64,
    outs: Vec<Output>,
    seen: bool,
}

/// Keyboard and mouse state between host polls and title reads.
pub(super) struct Controls {
    pub(super) s: Settings,
    vehicle: bool,
    held: Vec<u8>,
    derived: Derived,
    /// Buttons and triggers pressed since the title's last read: they stay
    /// down until it has read them, so a tap shorter than a frame counts.
    latched: u32,
    latched_triggers: (bool, bool),
    wheel: VecDeque<u8>,
    pulse: Pulse,
    look: Look,
    /// Clock for press styles (host polls carry no time of their own).
    start: Instant,
    /// When each held key went down (milliseconds on that clock).
    down: BTreeMap<u8, u64>,
    /// Per-binding state of the press styles, by key and place in its list.
    states: BTreeMap<(u8, usize), BindState>,
    /// Pulses and macro presses, started or waiting.
    timed: Vec<Timed>,
    left_owner: Owner,
    right_owner: Owner,
    last_used: Option<Owner>,
    /// The release key was pressed: the host frees or captures the cursor.
    pub(super) toggle_capture: bool,
    /// `VICTORY_VEHICLE_FLAG`: the title's own "in a vehicle" word, if set.
    flag: Option<VehicleFlag>,
    /// What the flag said at the last poll that could read it.
    flag_state: Option<bool>,
    /// The CONTROLS page's change count the layouts were built from last.
    bindings_seen: u64,
    /// The controls file's mouse sensitivity and inversion: what the menu's INPUT lines fall back to.
    file_mouse: (f32, bool),
}

/// Where the title's flag lies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FlagAt {
    Absolute(u64),
    /// An offset from the title image's base.
    Image(u64),
}

/// A word of title memory that says whether the player is in a vehicle
/// (`VICTORY_VEHICLE_FLAG`, found with the `mem_scan_*` debug tools).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VehicleFlag {
    at: FlagAt,
    /// Bytes: 1, 2 or 4.
    width: usize,
    /// The value that means in a vehicle; none: any non-zero value.
    value: Option<u64>,
}

impl VehicleFlag {
    fn in_vehicle(&self, raw: u64) -> bool {
        self.value.map_or(raw != 0, |v| raw == v)
    }
}

fn parse_number(text: &str) -> Option<u64> {
    let t = text.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => t.parse().ok(),
    }
}

/// `<hex address>[:<width 1|2|4>][:<value in a vehicle>]`; the address is
/// absolute or `img+<hex offset>`; the value is decimal or `0x` hex. Empty
/// text: off (`None`).
fn parse_vehicle_flag(text: &str) -> Result<Option<VehicleFlag>, String> {
    let text = text.trim();
    if text.is_empty() || matches!(text.to_ascii_lowercase().as_str(), "off" | "none") {
        return Ok(None);
    }
    let mut parts = text.split(':');
    let place = parts.next().unwrap_or("").trim();
    let hex = |s: &str| u64::from_str_radix(s.trim().trim_start_matches("0x").trim_start_matches("0X"), 16).ok();
    let at = match place.get(..4).filter(|p| p.eq_ignore_ascii_case("img+")) {
        Some(_) => FlagAt::Image(hex(&place[4..]).ok_or_else(|| format!("{place:?}: the offset after img+ must be hex"))?),
        None => FlagAt::Absolute(hex(place).ok_or_else(|| format!("{place:?}: the address must be hex or img+<hex offset>"))?),
    };
    let width = match parts.next().map(str::trim) {
        None | Some("") => 1,
        Some("1") => 1,
        Some("2") => 2,
        Some("4") => 4,
        Some(w) => return Err(format!("{w:?}: the width must be 1, 2 or 4")),
    };
    let value = match parts.next().map(str::trim) {
        None | Some("") => None,
        Some(v) => {
            let n = parse_number(v).ok_or_else(|| format!("{v:?}: the value must be a number (0x for hex)"))?;
            if n >> (8 * width) != 0 {
                return Err(format!("{v:?} does not fit in {width} byte(s)"));
            }
            Some(n)
        }
    };
    if parts.next().is_some() {
        return Err("too many parts: <address>[:<width>][:<value>]".into());
    }
    Ok(Some(VehicleFlag { at, width, value }))
}

/// The address `VICTORY_AUTO_VEHICLE=1` registers from the known title build: a
/// byte that is 0 or 1. Only used when `VICTORY_VEHICLE_FLAG` is not set.
static AUTO_FLAG: OnceLock<usize> = OnceLock::new();

/// Registers the absolute address of the known build's vehicle byte (the
/// fallback of `VICTORY_AUTO_VEHICLE`). Call it before the title starts.
pub fn set_vehicle_flag(address: usize) {
    let _ = AUTO_FLAG.set(address);
}

/// `VICTORY_VEHICLE_FLAG`; when that is not set, the `VICTORY_AUTO_VEHICLE`
/// address; else none (off).
fn vehicle_flag() -> Option<VehicleFlag> {
    let unset = std::env::var("VICTORY_VEHICLE_FLAG").map_or(true, |text| text.trim().is_empty());
    if unset {
        return AUTO_FLAG.get().map(|&address| VehicleFlag { at: FlagAt::Absolute(address as u64), width: 1, value: Some(1) });
    }
    vehicle_flag_from_env()
}

/// `VICTORY_VEHICLE_FLAG`, or none (off) when it is unset, empty, `off` or does
/// not parse (said once).
fn vehicle_flag_from_env() -> Option<VehicleFlag> {
    let text = std::env::var("VICTORY_VEHICLE_FLAG").ok()?;
    match parse_vehicle_flag(&text) {
        Ok(flag) => {
            if flag.is_some() {
                tracing::info!(setting = %text.trim(), "VICTORY_VEHICLE_FLAG: the layout follows the title's vehicle flag");
            }
            flag
        }
        Err(why) => {
            tracing::warn!("VICTORY_VEHICLE_FLAG ignored: {why}");
            None
        }
    }
}

/// The flag's word now, read from title memory without faulting; `None` if it
/// cannot be read (the title image is not mapped yet, or the page is gone).
#[cfg(windows)]
fn read_flag(flag: &VehicleFlag) -> Option<u64> {
    let address = match flag.at {
        FlagAt::Absolute(a) => a,
        FlagAt::Image(offset) => crate::runtime::main_image()? as u64 + offset,
    };
    let mut bytes = [0u8; 8];
    victory_gpu::read_title_memory(address as usize, &mut bytes[..flag.width]).then(|| u64::from_le_bytes(bytes))
}

#[cfg(not(windows))]
fn read_flag(_: &VehicleFlag) -> Option<u64> {
    None
}

impl Controls {
    pub(super) fn new(s: Settings) -> Controls {
        LAYOUT_IS_VEHICLE.store(s.start_in_vehicle, Ordering::Relaxed);
        let file_mouse = (s.sensitivity, s.invert_y);
        Controls {
            vehicle: s.start_in_vehicle,
            s,
            held: Vec::new(),
            derived: Derived::default(),
            latched: 0,
            latched_triggers: (false, false),
            wheel: VecDeque::new(),
            pulse: Pulse::Idle,
            look: Look::default(),
            start: Instant::now(),
            down: BTreeMap::new(),
            states: BTreeMap::new(),
            timed: Vec::new(),
            left_owner: Owner::Kbm,
            right_owner: Owner::Kbm,
            last_used: None,
            toggle_capture: false,
            flag: vehicle_flag(),
            flag_state: None,
            bindings_seen: 0,
            file_mouse,
        }
    }

    /// The mouse the INPUT page of the menu sets over the controls file's: a sensitivity
    /// above 0 replaces the file's, inversion `1` (off) or `2` (on) replaces the file's
    /// (`0` keeps it). Changes apply at the next read.
    fn follow_mouse(&mut self, sensitivity: f64, invert: usize) {
        self.s.sensitivity = if sensitivity > 0.0 { sensitivity.clamp(0.01, 100.0) as f32 } else { self.file_mouse.0 };
        self.s.invert_y = match invert {
            1 => false,
            2 => true,
            _ => self.file_mouse.1,
        };
    }

    /// Builds the layouts again from the tables of the CONTROLS page when it changed them
    /// (`generation` is its change count). Press styles in flight are let go: their places in a key's list moved.
    fn follow_bindings(&mut self, generation: u64, tables: impl FnOnce() -> Option<(Table, Table)>) {
        if generation == self.bindings_seen {
            return;
        }
        self.bindings_seen = generation;
        let Some((foot, vehicle)) = tables() else { return };
        self.s.on_foot = Layout::from_table(&foot, Context::Foot);
        self.s.vehicle = Layout::from_table(&vehicle, Context::Vehicle);
        self.states.clear();
        self.timed.clear();
        tracing::info!("controls: bindings changed in the menu");
    }

    /// Follows the title's own vehicle flag: when what it says changes, the
    /// layout changes with it (as `SetVehicle` does). The manual keys still
    /// work; what they set stays until the flag next changes. A word that
    /// cannot be read leaves the layout alone.
    fn follow_flag(&mut self, read: impl FnOnce(&VehicleFlag) -> Option<u64>) {
        let Some(flag) = self.flag else { return };
        let Some(raw) = read(&flag) else { return };
        let now = flag.in_vehicle(raw);
        if self.flag_state != Some(now) {
            self.flag_state = Some(now);
            self.vehicle = now;
            LAYOUT_IS_VEHICLE.store(now, Ordering::Relaxed);
            tracing::info!(vehicle = now, raw, "controls layout (title vehicle flag)");
        }
    }

    fn layout(&self) -> &Layout {
        if self.vehicle {
            &self.s.vehicle
        } else {
            &self.s.on_foot
        }
    }

    #[cfg(test)]
    fn in_vehicle(&self) -> bool {
        self.vehicle
    }

    /// What the held keys, toggles and running pulses press now.
    fn level_outputs(&self, now: u64) -> Vec<Output> {
        let mut outs = Vec::new();
        for &vk in &self.held {
            for (i, bind) in self.layout().binds(vk).iter().enumerate() {
                let on = match bind.style {
                    Style::Held => true,
                    Style::Hold(_) => self.states.get(&(vk, i)).is_some_and(|st| st.fired),
                    _ => false,
                };
                if on {
                    outs.extend_from_slice(&bind.outs);
                }
            }
        }
        for (&(vk, i), st) in &self.states {
            if st.toggled
                && let Some(bind) = self.layout().binds(vk).get(i)
            {
                outs.extend_from_slice(&bind.outs);
            }
        }
        for t in &self.timed {
            if t.start <= now && now < t.end {
                outs.extend_from_slice(&t.outs);
            }
        }
        outs
    }

    /// What one wheel step presses (its tap and plain bindings).
    fn wheel_outputs(&self, key: u8) -> Vec<Output> {
        self.layout()
            .binds(key)
            .iter()
            .filter(|b| matches!(b.style, Style::Held | Style::Tap(_)))
            .flat_map(|b| b.outs.iter().copied())
            .collect()
    }

    fn derive(&self, outputs: impl Iterator<Item = Output>) -> Derived {
        let mut d = Derived::default();
        let (mut l, mut r) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
        let mut walk = false;
        for out in outputs {
            match out {
                Output::Button(b) => d.buttons |= b,
                Output::LeftTrigger => d.left_trigger = true,
                Output::RightTrigger => d.right_trigger = true,
                Output::Stick { right, x, positive } => {
                    let stick = if right { &mut r } else { &mut l };
                    let axis = if x { &mut stick.0 } else { &mut stick.1 };
                    *axis += if positive { 1.0 } else { -1.0 };
                    if right {
                        d.right_moved = true;
                    } else {
                        d.left_moved = true;
                    }
                }
                Output::Walk => walk = true,
                Output::ToggleVehicle | Output::SetVehicle(_) | Output::ReleaseCursor => {}
            }
        }
        let round = |(x, y): (f32, f32)| {
            let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
            let m = x.hypot(y);
            if m > 1.0 { (x / m, y / m) } else { (x, y) }
        };
        let speed = if walk { self.s.walk_speed } else { 1.0 };
        let (lx, ly) = round(l);
        Derived {
            left: (lx * speed, ly * speed),
            right: round(r),
            ..d
        }
    }

    /// A host poll: the keys and mouse buttons held now, wheel steps since
    /// the last poll.
    pub(super) fn feed(&mut self, held: &[u8], counts: &Counts) {
        let now = self.start.elapsed().as_millis() as u64;
        self.follow_flag(read_flag);
        self.follow_mouse(
            victory_core::tunables::known::MOUSE_SENSITIVITY.get(),
            victory_core::tunables::known::MOUSE_INVERT_Y.get(),
        );
        self.follow_bindings(victory_core::keybinds::generation(), victory_core::keybinds::live_tables);
        self.feed_at(held, counts, now);
    }

    /// Fires what a binding does once: layout switches and the cursor.
    fn fire_once(&mut self, outs: &[Output], vk: u8) {
        for out in outs {
            match *out {
                Output::ToggleVehicle => self.vehicle = !self.vehicle,
                // With the title's vehicle flag in use, EnterVehicle and ExitVehicle are the title's
                // own interact button (Y) and nothing more: doors and other prompts no longer flip
                // the layout; the flag does. G (ToggleVehicle) still switches by hand.
                Output::SetVehicle(_) if self.flag.is_some() => continue,
                Output::SetVehicle(v) => self.vehicle = v,
                Output::ReleaseCursor => {
                    self.toggle_capture = true;
                    continue;
                }
                _ => continue,
            }
            LAYOUT_IS_VEHICLE.store(self.vehicle, Ordering::Relaxed);
            tracing::info!(vehicle = self.vehicle, key = %key_name(vk), "controls layout");
        }
    }

    fn pulse_out(&mut self, outs: &[Output], vk: u8, now: u64) {
        self.fire_once(outs, vk);
        if self.timed.len() < MAX_TIMED {
            self.timed.push(Timed {
                start: now,
                end: now + PULSE_MS,
                outs: outs.to_vec(),
                seen: false,
            });
        }
    }

    fn on_press(&mut self, vk: u8, now: u64) {
        let binds = self.layout().binds(vk).to_vec();
        for (i, bind) in binds.iter().enumerate() {
            let mut st = *self.states.entry((vk, i)).or_default();
            match &bind.style {
                Style::Held => self.fire_once(&bind.outs, vk),
                Style::Toggle => {
                    st.toggled = !st.toggled;
                    self.fire_once(&bind.outs, vk);
                }
                Style::Double(ms) => match st.last_press {
                    Some(p) if now.saturating_sub(p) <= *ms => {
                        st.last_press = None;
                        self.pulse_out(&bind.outs, vk, now);
                    }
                    _ => st.last_press = Some(now),
                },
                Style::Macro(steps) => {
                    let mut at = now;
                    for step in steps {
                        match step {
                            Step::Wait(ms) => at += ms,
                            Step::Press(outs, ms) => {
                                if self.timed.len() < MAX_TIMED {
                                    self.timed.push(Timed {
                                        start: at,
                                        end: at + ms,
                                        outs: outs.clone(),
                                        seen: false,
                                    });
                                }
                                at += ms;
                            }
                        }
                    }
                }
                Style::Tap(_) | Style::Hold(_) => {}
            }
            self.states.insert((vk, i), st);
        }
    }

    fn on_release(&mut self, vk: u8, now: u64, down_at: u64) {
        let binds = self.layout().binds(vk).to_vec();
        for (i, bind) in binds.iter().enumerate() {
            if let Some(st) = self.states.get_mut(&(vk, i)) {
                st.fired = false;
            }
            if let Style::Tap(ms) = bind.style
                && now.saturating_sub(down_at) < ms
            {
                self.pulse_out(&bind.outs, vk, now);
            }
        }
    }

    fn feed_at(&mut self, held: &[u8], counts: &Counts, now: u64) {
        let steps = counts.drain_wheel();
        let key = if steps > 0 { WHEEL_UP } else { WHEEL_DOWN };
        for _ in 0..steps.unsigned_abs() {
            if self.wheel.len() < WHEEL_QUEUE {
                self.wheel.push_back(key);
            }
        }
        let pressed: Vec<u8> = held
            .iter()
            .copied()
            .filter(|vk| !self.held.contains(vk))
            .collect();
        let released: Vec<u8> = self
            .held
            .iter()
            .copied()
            .filter(|vk| !held.contains(vk))
            .collect();
        for &vk in &released {
            let down_at = self.down.remove(&vk).unwrap_or(now);
            self.on_release(vk, now, down_at);
        }
        for &vk in &pressed {
            self.down.insert(vk, now);
            self.on_press(vk, now);
        }
        self.held = held.to_vec();
        // Holds that have been down long enough.
        for vk in self.held.clone() {
            let down_at = self.down.get(&vk).copied().unwrap_or(now);
            let binds = self.layout().binds(vk).to_vec();
            for (i, bind) in binds.iter().enumerate() {
                if let Style::Hold(ms) = bind.style
                    && now.saturating_sub(down_at) >= ms
                    && !self.states.entry((vk, i)).or_default().fired
                {
                    self.states.entry((vk, i)).or_default().fired = true;
                    self.fire_once(&bind.outs, vk);
                }
            }
        }
        // A pulse that began and ended between two polls is still latched.
        let mut latched = 0;
        let mut triggers = (false, false);
        for t in self.timed.iter_mut().filter(|t| t.start <= now && !t.seen) {
            t.seen = true;
            for out in &t.outs {
                match *out {
                    Output::Button(b) => latched |= b,
                    Output::LeftTrigger => triggers.0 = true,
                    Output::RightTrigger => triggers.1 = true,
                    _ => {}
                }
            }
        }
        let outs = self.level_outputs(now);
        self.timed.retain(|t| t.end > now);
        let before = self.derived;
        self.derived = self.derive(outs.into_iter());
        self.latched |= (self.derived.buttons & !before.buttons) | latched;
        self.latched_triggers.0 |=
            (self.derived.left_trigger && !before.left_trigger) || triggers.0;
        self.latched_triggers.1 |=
            (self.derived.right_trigger && !before.right_trigger) || triggers.1;
        if self.pulse == Pulse::Idle
            && let Some(key) = self.wheel.pop_front()
        {
            self.pulse = Pulse::Down(key);
        }
    }

    /// The keyboard and mouse pad as it is now (a host poll's reading).
    fn kbm(&self) -> PadState {
        let mut d = self.derived;
        if let Pulse::Down(key) = self.pulse {
            let p = self.derive(self.wheel_outputs(key).into_iter());
            d.buttons |= p.buttons;
            d.left_trigger |= p.left_trigger;
            d.right_trigger |= p.right_trigger;
        }
        let on = |b: bool| if b { 1.0 } else { 0.0 };
        let (rx, ry) = (d.right.0 + self.look.value.0, d.right.1 + self.look.value.1);
        let m = rx.hypot(ry);
        let (rx, ry) = if m > 1.0 { (rx / m, ry / m) } else { (rx, ry) };
        PadState {
            buttons: d.buttons | self.latched,
            left_trigger: on(d.left_trigger || self.latched_triggers.0),
            right_trigger: on(d.right_trigger || self.latched_triggers.1),
            left_x: d.left.0,
            left_y: d.left.1,
            right_x: rx,
            right_y: ry,
        }
    }

    /// The pad a host poll publishes: `pad` (a host pad, if any) with the
    /// keyboard and mouse on it.
    pub(super) fn peek(&mut self, pad: Option<PadState>) -> PadState {
        self.merge(pad)
    }

    /// The pad the title reads now: the mouse counts since its last read
    /// become the right stick, and what it has now seen is let go.
    pub(super) fn take(&mut self, now: u64, pad: Option<PadState>, counts: &Counts) -> PadState {
        let before = self.look.last_take;
        self.look.take(now, counts.drain(), &self.s);
        let state = self.merge(pad);
        if self.look.last_take != before {
            self.latched = 0;
            self.latched_triggers = (false, false);
            self.pulse = match self.pulse {
                Pulse::Down(_) => Pulse::Gap,
                Pulse::Gap | Pulse::Idle => Pulse::Idle,
            };
        }
        state
    }

    fn merge(&mut self, pad: Option<PadState>) -> PadState {
        let kbm = self.kbm();
        let kbm_used = !self.held.is_empty() || self.look.moved || self.pulse != Pulse::Idle;
        let Some(pad) = pad else {
            self.used(kbm_used.then_some(Owner::Kbm));
            return kbm;
        };
        let pushed = |x: f32, y: f32| x.hypot(y) > PAD_TAKES_OVER;
        let pad_left = pushed(pad.left_x, pad.left_y);
        let pad_right = pushed(pad.right_x, pad.right_y);
        if self.derived.left_moved {
            self.left_owner = Owner::Kbm;
        } else if pad_left {
            self.left_owner = Owner::Pad;
        }
        if self.derived.right_moved || self.look.moved {
            self.right_owner = Owner::Kbm;
        } else if pad_right {
            self.right_owner = Owner::Pad;
        }
        let pad_used = pad.buttons != 0
            || pad_left
            || pad_right
            || pad.left_trigger > 0.1
            || pad.right_trigger > 0.1;
        self.used(if kbm_used {
            Some(Owner::Kbm)
        } else {
            pad_used.then_some(Owner::Pad)
        });
        let (lx, ly) = match self.left_owner {
            Owner::Kbm => (kbm.left_x, kbm.left_y),
            Owner::Pad => (pad.left_x, pad.left_y),
        };
        let (rx, ry) = match self.right_owner {
            Owner::Kbm => (kbm.right_x, kbm.right_y),
            Owner::Pad => (pad.right_x, pad.right_y),
        };
        PadState {
            buttons: pad.buttons | kbm.buttons,
            left_trigger: pad.left_trigger.max(kbm.left_trigger),
            right_trigger: pad.right_trigger.max(kbm.right_trigger),
            left_x: lx,
            left_y: ly,
            right_x: rx,
            right_y: ry,
        }
    }

    fn used(&mut self, now: Option<Owner>) {
        if let Some(owner) = now
            && self.last_used != Some(owner)
        {
            self.last_used = Some(owner);
            tracing::info!(
                device = match owner {
                    Owner::Pad => "gamepad",
                    Owner::Kbm => "keyboard and mouse",
                },
                "input: now using"
            );
        }
    }

    /// The device used last (for prompts, once they can change).
    #[allow(dead_code)]
    pub(super) fn last_used(&self) -> Option<Owner> {
        self.last_used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vk(name: &str) -> u8 {
        key_code(name).unwrap()
    }

    fn linear() -> Settings {
        Settings {
            deadzone: 0.0,
            curve: 1.0,
            ..Settings::default()
        }
    }

    fn controls() -> Controls {
        Controls::new(linear())
    }

    fn held(c: &mut Controls, keys: &[&str]) -> PadState {
        let keys: Vec<u8> = keys.iter().map(|k| vk(k)).collect();
        c.feed(&keys, &Counts::default());
        c.peek(None)
    }

    #[test]
    fn key_names_round_trip() {
        for (name, code) in [
            ("w", b'W'),
            ("7", b'7'),
            ("Space", 0x20),
            ("lshift", 0xA0),
            ("F1", 0x70),
            ("F24", 0x87),
            ("Num5", 0x65),
            ("MouseLeft", 0x01),
            ("WheelDown", WHEEL_DOWN),
            ("Alt", 0xA4),
        ] {
            assert_eq!(key_code(name), Some(code), "{name}");
        }
        assert_eq!(key_code("F0"), None);
        assert_eq!(key_code("F25"), None);
        assert_eq!(key_code("Num10"), None);
        assert_eq!(key_code("Banana"), None);
        assert_eq!(key_name(0x70), "F1");
        assert_eq!(key_name(0xA0), "LShift");
        assert_eq!(key_name(b'Q'), "Q");
    }

    #[test]
    fn letters_are_named_by_their_place_on_a_us_keyboard() {
        for (scan, key) in [
            (0x11, b'W'),
            (0x1E, b'A'),
            (0x1F, b'S'),
            (0x20, b'D'),
            (0x10, b'Q'),
            (0x2C, b'Z'),
            (0x32, b'M'),
            (0x02, b'1'),
            (0x0B, b'0'),
            (0x29, 0xC0),
            (0x2B, 0xDC),
            (0x35, 0xBF),
        ] {
            assert_eq!(us_key(scan), Some(key), "{scan:#x}");
        }
        // Esc, Tab, Enter, Left Ctrl, Space, Left Shift, the extended keys.
        for scan in [0x01, 0x0F, 0x1C, 0x1D, 0x39, 0x2A, 0xE035, 0xE048] {
            assert_eq!(us_key(scan), None, "{scan:#x}");
        }
        let off = Options {
            keys_by_position: Some(false),
            ..Options::default()
        };
        assert!(Settings::default().by_position);
        assert!(!Settings::with_options(&off).unwrap().by_position);
    }

    #[test]
    fn wasd_moves_the_left_stick_round_and_walk_slows_it() {
        let mut c = controls();
        let p = held(&mut c, &["W"]);
        assert_eq!((p.left_x, p.left_y), (0.0, 1.0));
        let p = held(&mut c, &["W", "D"]);
        let r = std::f32::consts::FRAC_1_SQRT_2;
        assert!((p.left_x - r).abs() < 1e-6 && (p.left_y - r).abs() < 1e-6);
        let p = held(&mut c, &["A", "D"]);
        assert_eq!((p.left_x, p.left_y), (0.0, 0.0));
        let p = held(&mut c, &["S", "LAlt"]);
        assert_eq!((p.left_x, p.left_y), (0.0, -0.5));
    }

    #[test]
    fn keys_and_mouse_buttons_press_pad_buttons_and_triggers() {
        let mut c = controls();
        let p = held(
            &mut c,
            &["Space", "LShift", "MouseLeft", "MouseRight", "Tab"],
        );
        assert_eq!(p.buttons, A | LEFT_STICK | LEFT_SHOULDER);
        assert_eq!((p.left_trigger, p.right_trigger), (1.0, 1.0));
        let p = held(&mut c, &["Esc"]);
        assert!(p.buttons & MENU != 0);
        assert!(c.toggle_capture, "Esc frees the cursor");
    }

    #[test]
    fn a_vehicle_checkpoint_starts_with_throttle_and_brake_keys() {
        let options = Options {
            start_in_vehicle: Some(true),
            ..Options::default()
        };
        let mut controls = Controls::new(Settings::with_options(&options).unwrap());
        let counts = Counts::default();
        controls.feed(&[b'W'], &counts);
        let pad = controls.peek(None);
        assert_eq!(
            (pad.right_trigger, pad.left_trigger, pad.left_y),
            (1.0, 0.0, 0.0)
        );

        let mut controls = Controls::new(Settings::with_options(&options).unwrap());
        controls.feed(&[b'S'], &counts);
        let pad = controls.peek(None);
        assert_eq!(
            (pad.right_trigger, pad.left_trigger, pad.left_y),
            (0.0, 1.0, 0.0)
        );
        let pad = held(&mut controls, &["Num8"]);
        assert_eq!(pad.left_y, 1.0);
    }

    #[test]
    fn aircraft_profile_uses_flight_bindings_without_layout_hotkeys() {
        let options = Options {
            aircraft_vehicle: true,
            start_in_vehicle: Some(true),
            ..Options::default()
        };
        let mut c = Controls::new(Settings::with_options(&options).unwrap());
        let p = held(&mut c, &["F"]);
        assert_eq!(p.buttons, Y);
        assert!(c.in_vehicle());
        c.take(1, None, &Counts::default());
        let p = held(
            &mut c,
            &["W", "A", "G", "MouseLeft", "Num6", "Tab", "F9", "F10"],
        );
        assert_eq!(p.buttons, LEFT_SHOULDER | LEFT_STICK | RIGHT_SHOULDER | X);
        assert_eq!((p.right_trigger, p.left_x), (1.0, 1.0));
        assert!(c.in_vehicle());
        held(&mut c, &["F"]);
        assert!(c.in_vehicle(), "F is interact only");
    }

    #[test]
    fn g_switches_to_the_vehicle_layout_and_f_does_not() {
        let mut c = controls();
        held(&mut c, &["F"]);
        assert!(!c.in_vehicle());
        held(&mut c, &[]);
        held(&mut c, &["G"]);
        assert!(c.in_vehicle());
        held(&mut c, &[]);
        let p = held(&mut c, &["W", "A"]);
        assert_eq!(p.right_trigger, 1.0, "W is throttle");
        assert_eq!(
            (p.left_x, p.left_y),
            (-1.0, 0.0),
            "A steers, W does not move the stick"
        );
        // Holding G does not switch again; F changes nothing.
        held(&mut c, &["G"]);
        held(&mut c, &["G"]);
        assert!(!c.in_vehicle());
        held(&mut c, &[]);
        held(&mut c, &["F"]);
        assert!(!c.in_vehicle());
    }

    #[test]
    fn a_tap_between_two_reads_is_read_once() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        c.feed(&[vk("Space")], &counts);
        c.feed(&[], &counts);
        assert_eq!(c.peek(None).buttons, A, "held for the title until it reads");
        assert_eq!(c.take(10_000, None, &counts).buttons, A);
        assert_eq!(c.take(20_000, None, &counts).buttons, 0);
        // A quick click shoots once.
        c.feed(&[vk("MouseLeft")], &counts);
        c.feed(&[], &counts);
        assert_eq!(c.take(30_000, None, &counts).right_trigger, 1.0);
        assert_eq!(c.take(40_000, None, &counts).right_trigger, 0.0);
    }

    #[test]
    fn wheel_steps_are_each_a_press_the_title_reads() {
        let mut c = controls();
        let counts = Counts::default();
        counts.add_wheel(2);
        let mut seen = Vec::new();
        for i in 0..6 {
            c.feed(&[], &counts);
            seen.push(c.take(i * 10_000, None, &counts).buttons);
        }
        assert_eq!(seen, [UP, 0, UP, 0, 0, 0]);
        counts.add_wheel(-1);
        c.feed(&[], &counts);
        assert_eq!(c.take(100_000, None, &counts).buttons, DOWN);
    }

    #[test]
    fn mouse_counts_become_a_rate_at_each_read() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        // 2,000 counts/s = full stick at sensitivity 1: 10 counts in 10 ms is half.
        counts.add(10, 0);
        let p = c.take(10_000, None, &counts);
        assert!((p.right_x - 0.5).abs() < 1e-6, "{}", p.right_x);
        // The same distance over a 4x longer frame is a 4x smaller rate: the
        // title turns rate x frame time, so the same angle.
        counts.add(10, 0);
        let p = c.take(50_000, None, &counts);
        assert!((p.right_x - 0.125).abs() < 1e-6, "{}", p.right_x);
        // Mouse up looks up; no motion lets go.
        counts.add(0, -5);
        let p = c.take(60_000, None, &counts);
        assert!((p.right_y - 0.25).abs() < 1e-6 && p.right_x == 0.0);
        assert_eq!(c.take(70_000, None, &counts).right_y, 0.0);
    }

    #[test]
    fn slow_frames_get_the_rate_over_their_whole_span() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        // 1,000 counts over an 800 ms frame: half a stick, not saturated.
        counts.add(1000, 0);
        let p = c.take(800_000, None, &counts);
        assert!((p.right_x - 0.625).abs() < 1e-6, "{}", p.right_x);
        // Past the longest span the title counts as paused.
        counts.add(1000, 0);
        let p = c.take(3_800_000, None, &counts);
        assert!((p.right_x - 0.5).abs() < 1e-6, "{}", p.right_x);
    }

    #[test]
    fn reads_close_together_see_the_same_mouse_stick() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        counts.add(10, 0);
        let a = c.take(10_000, None, &counts);
        let b = c.take(10_500, None, &counts);
        assert_eq!(a, b);
        counts.add(4, 0);
        let p = c.take(11_000, None, &counts);
        assert_eq!(p, a, "counts wait for the next read");
        let p = c.take(20_000, None, &counts);
        assert!((p.right_x - 0.2).abs() < 1e-6, "{}", p.right_x);
    }

    #[test]
    fn dead_zone_curve_and_inversion_shape_the_mouse_stick() {
        let s = Settings {
            deadzone: 0.2,
            curve: 2.0,
            invert_y: true,
            ..Settings::default()
        };
        let mut c = Controls::new(s);
        let counts = Counts::default();
        c.take(0, None, &counts);
        counts.add(0, -5); // a quarter stick up
        let p = c.take(10_000, None, &counts);
        // 0.2 + 0.8 * sqrt(0.25) = 0.6, then inverted.
        assert!((p.right_y + 0.6).abs() < 1e-6, "{}", p.right_y);
        // One count still clears the dead zone.
        counts.add(1, 0);
        let p = c.take(20_000, None, &counts);
        assert!(p.right_x > 0.2);
    }

    #[test]
    fn a_fast_flick_is_turned_in_full_later() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        counts.add(50, 0); // 2.5 full sticks in 10 ms
        let xs: Vec<f32> = (1..=4)
            .map(|i| c.take(i * 10_000, None, &counts).right_x)
            .collect();
        assert_eq!(&xs[..2], &[1.0, 1.0]);
        assert!((xs[2] - 0.5).abs() < 1e-6, "{xs:?}");
        assert_eq!(xs[3], 0.0);
        let mut c = Controls::new(Settings {
            carry: false,
            ..linear()
        });
        c.take(0, None, &counts);
        counts.add(50, 0);
        assert_eq!(c.take(10_000, None, &counts).right_x, 1.0);
        assert_eq!(c.take(20_000, None, &counts).right_x, 0.0);
    }

    #[test]
    fn a_resting_pad_never_overrides_the_mouse_and_a_pushed_one_does() {
        let mut c = controls();
        let counts = Counts::default();
        let drift = PadState {
            right_x: 0.1,
            left_y: -0.05,
            buttons: B,
            right_trigger: 0.4,
            ..Default::default()
        };
        c.take(0, Some(drift), &counts);
        counts.add(10, 0);
        c.feed(&[vk("W"), vk("MouseLeft")], &counts);
        let p = c.take(10_000, Some(drift), &counts);
        assert!((p.right_x - 0.5).abs() < 1e-6);
        assert_eq!((p.left_y, p.buttons, p.right_trigger), (1.0, B, 1.0));
        c.feed(&[], &counts);
        assert_eq!(
            c.take(20_000, Some(drift), &counts).right_x,
            0.0,
            "drift stays out"
        );
        let pushed = PadState {
            right_x: 0.7,
            left_x: -0.9,
            ..Default::default()
        };
        let p = c.take(30_000, Some(pushed), &counts);
        assert_eq!((p.right_x, p.left_x), (0.7, -0.9));
        assert_eq!(c.last_used(), Some(Owner::Pad));
    }

    #[test]
    fn config_rebinds_unbinds_and_refuses_typos() {
        let mut options = Options {
            mouse_sensitivity: Some(2.5),
            mouse_invert_y: Some(true),
            stick_deadzone: Some(0.24),
            on_foot: BTreeMap::from([
                ("Space".into(), vec!["X".into()]),
                ("LShift".into(), vec!["".into()]),
                ("Mouse4".into(), vec!["LB".into(), "RB".into()]),
            ]),
            vehicle: BTreeMap::from([("W".into(), vec!["RightTrigger_typo".into()])]),
            ..Options::default()
        };
        let error = Settings::with_options(&options).unwrap_err();
        assert!(error.contains("RightTrigger_typo"), "{error}");
        options.vehicle.clear();
        let s = Settings::with_options(&options).unwrap();
        assert_eq!((s.sensitivity, s.invert_y, s.deadzone), (2.5, true, 0.24));
        assert_eq!(s.on_foot.outputs(0x20), &[Output::Button(X)]);
        assert!(s.on_foot.outputs(0xA0).is_empty());
        assert_eq!(
            s.on_foot.outputs(0x05),
            &[
                Output::Button(LEFT_SHOULDER),
                Output::Button(RIGHT_SHOULDER)
            ]
        );
        assert_eq!(
            s.on_foot.outputs(b'W'),
            Settings::default().on_foot.outputs(b'W')
        );
        for bad in [
            Options {
                stick_deadzone: Some(1.5),
                ..Options::default()
            },
            Options {
                on_foot: BTreeMap::from([("Banana".into(), vec!["A".into()])]),
                ..Options::default()
            },
        ] {
            assert!(Settings::with_options(&bad).is_err());
        }
        let off = Options {
            enabled: Some(false),
            ..Options::default()
        };
        assert!(!Settings::with_options(&off).unwrap().enabled);
    }

    fn with(on_foot: &[(&str, &[&str])]) -> Controls {
        let options = Options {
            on_foot: on_foot
                .iter()
                .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
                .collect(),
            ..Options::default()
        };
        let mut s = Settings::with_options(&options).unwrap();
        s.deadzone = 0.0;
        Controls::new(s)
    }

    fn at(c: &mut Controls, keys: &[&str], now: u64) -> PadState {
        let keys: Vec<u8> = keys.iter().map(|k| vk(k)).collect();
        c.feed_at(&keys, &Counts::default(), now);
        c.peek(None)
    }

    /// The title reads twice, so latched presses are let go.
    fn read(c: &mut Controls) {
        let counts = Counts::default();
        let t = c.look.last_take.unwrap_or(0);
        c.take(t + 20_000, None, &counts);
        c.take(t + 40_000, None, &counts);
    }

    #[test]
    fn actions_and_styles_parse_and_bad_ones_are_refused() {
        let foot = |s: &str| parse_bind(s, Context::Foot);
        let car = |s: &str| parse_bind(s, Context::Vehicle);
        assert_eq!(foot("Jump").unwrap().outs, [Output::Button(A)]);
        assert_eq!(foot("Shoot").unwrap().outs, [Output::RightTrigger]);
        assert_eq!(car("Shoot").unwrap().outs, [Output::Button(RIGHT_SHOULDER)]);
        assert_eq!(foot("Sprint").unwrap().outs, [Output::Button(LEFT_STICK)]);
        assert_eq!(foot("tap:Crouch").unwrap().style, Style::Tap(TAP_MS));
        assert_eq!(foot("Hold450:Loadout").unwrap().style, Style::Hold(450));
        assert_eq!(foot("double:Jump").unwrap().style, Style::Double(TAP_MS));
        assert_eq!(foot("toggle:Sprint").unwrap().style, Style::Toggle);
        assert_eq!(
            foot("macro:Y 60, wait 100, A").unwrap().style,
            Style::Macro(vec![
                Step::Press(vec![Output::Button(Y)], 60),
                Step::Wait(100),
                Step::Press(vec![Output::Button(A)], PULSE_MS),
            ])
        );
        for bad in [
            "flip:Jump",
            "tap:Banana",
            "tap5:Jump",
            "toggle9:Jump",
            "macro:",
            "macro:Walk 60",
            "macro:EnterVehicle 60",
            "macro:Y 5000",
            "macro:Y 2000, wait 2000, wait 2000",
            "macro:Y,Y,Y,Y,Y,Y,Y,Y,Y,Y,Y,Y,Y,Y,Y,Y,Y",
            "macro:Y 1 2",
        ] {
            assert!(foot(bad).is_err(), "{bad}");
        }
        // Raw names still work.
        assert_eq!(foot("LB").unwrap().outs, [Output::Button(LEFT_SHOULDER)]);
    }

    #[test]
    fn tap_presses_briefly_on_release_and_hold_waits_for_its_time() {
        let mut c = with(&[("X", &["tap:Crouch"])]);
        assert_eq!(at(&mut c, &["X"], 0).buttons, 0, "a tap acts on release");
        assert_eq!(at(&mut c, &[], 100).buttons, B, "released in time: pressed");
        read(&mut c);
        assert_eq!(at(&mut c, &[], 100 + PULSE_MS + 1).buttons, 0);
        // Held past the tap time: no tap.
        let mut c = with(&[("X", &["tap:Crouch"])]);
        at(&mut c, &["X"], 0);
        at(&mut c, &["X"], 400);
        assert_eq!(at(&mut c, &[], 500).buttons, 0);

        let mut c = with(&[("X", &["hold300:Crouch"])]);
        assert_eq!(at(&mut c, &["X"], 0).buttons, 0);
        assert_eq!(at(&mut c, &["X"], 299).buttons, 0);
        assert_eq!(at(&mut c, &["X"], 300).buttons, B);
        assert_eq!(at(&mut c, &["X"], 900).buttons, B, "stays down while held");
        at(&mut c, &[], 950);
        read(&mut c);
        assert_eq!(at(&mut c, &[], 1000).buttons, 0);
    }

    #[test]
    fn tap_and_hold_on_one_key_press_different_buttons() {
        let mut c = with(&[("X", &["tap:Crouch", "hold300:Melee"])]);
        assert_eq!(at(&mut c, &["X"], 0).buttons, 0);
        assert_eq!(at(&mut c, &["X"], 350).buttons, X);
        let mut c = with(&[("X", &["tap:Crouch", "hold300:Melee"])]);
        at(&mut c, &["X"], 0);
        assert_eq!(at(&mut c, &[], 100).buttons, B);
    }

    #[test]
    fn double_tap_needs_two_presses_in_time() {
        let mut c = with(&[("X", &["double200:Jump"])]);
        assert_eq!(at(&mut c, &["X"], 0).buttons, 0);
        at(&mut c, &[], 50);
        assert_eq!(at(&mut c, &["X"], 150).buttons, A);
        at(&mut c, &[], 160);
        read(&mut c);
        assert_eq!(at(&mut c, &[], 150 + PULSE_MS + 5).buttons, 0);
        // Too slow: two single presses.
        let mut c = with(&[("X", &["double200:Jump"])]);
        at(&mut c, &["X"], 0);
        at(&mut c, &[], 50);
        assert_eq!(at(&mut c, &["X"], 400).buttons, 0);
    }

    #[test]
    fn toggle_flips_each_press() {
        let mut c = with(&[("CapsLock", &["toggle:Sprint"])]);
        assert_eq!(at(&mut c, &["CapsLock"], 0).buttons, LEFT_STICK);
        assert_eq!(at(&mut c, &[], 10).buttons, LEFT_STICK, "stays on");
        at(&mut c, &["CapsLock"], 20);
        read(&mut c);
        assert_eq!(at(&mut c, &[], 30).buttons, 0, "the second press turned it off");
    }

    #[test]
    fn a_macro_runs_its_steps_in_order() {
        let mut c = with(&[("X", &["macro:Y 60, wait 100, A 60"])]);
        assert_eq!(at(&mut c, &["X"], 0).buttons, Y);
        read(&mut c);
        assert_eq!(at(&mut c, &[], 80).buttons, 0, "between the presses");
        assert_eq!(at(&mut c, &[], 159).buttons, 0, "A starts at 160 ms");
        assert_eq!(at(&mut c, &[], 165).buttons, A);
        read(&mut c);
        assert_eq!(at(&mut c, &[], 230).buttons, 0);
        // A poll that comes late still lets the title see a short step.
        let mut c = with(&[("X", &["macro:Y 60"])]);
        at(&mut c, &["X"], 0);
        at(&mut c, &[], 5000);
        assert_eq!(c.peek(None).buttons, Y, "latched until read");
        read(&mut c);
        assert_eq!(at(&mut c, &[], 5001).buttons, 0);
    }

    #[test]
    fn f_is_interact_only_and_g_is_the_manual_switch() {
        let mut c = controls();
        assert!(!c.in_vehicle());
        let p = held(&mut c, &["F"]);
        assert!(!c.in_vehicle(), "F on foot does not switch the layout");
        assert_eq!(p.buttons, Y);
        held(&mut c, &[]);
        held(&mut c, &["G"]);
        assert!(c.in_vehicle());
        held(&mut c, &[]);
        let p = held(&mut c, &["F"]);
        assert!(c.in_vehicle(), "F in a vehicle does not switch it either");
        assert_eq!(p.buttons, Y);
        held(&mut c, &[]);
        held(&mut c, &["G"]);
        assert!(!c.in_vehicle());
        // The old names still switch while no flag is in use.
        let mut d = Controls::new(Settings::with_options(&Options {
            on_foot: [("H".to_owned(), vec!["EnterVehicle".to_owned()])].into(),
            ..Options::default()
        }).unwrap());
        held(&mut d, &["H"]);
        assert!(d.in_vehicle());
    }

    #[test]
    fn default_tables_follow_the_game_layout() {
        let mut c = controls();
        let p = held(&mut c, &["Space", "LShift", "C", "R", "Q", "Tab", "M", "Up"]);
        assert_eq!(
            p.buttons,
            A | LEFT_STICK | B | X | RIGHT_SHOULDER | LEFT_SHOULDER | VIEW | UP
        );
        read(&mut c);
        let p = held(&mut c, &["MouseLeft", "MouseRight", "MouseMiddle"]);
        assert_eq!(
            (p.right_trigger, p.left_trigger, p.buttons),
            (1.0, 1.0, RIGHT_STICK)
        );
        held(&mut c, &["G"]);
        held(&mut c, &[]);
        read(&mut c);
        let p = held(&mut c, &["W", "Space", "E", "H", "MouseLeft", "Left"]);
        assert_eq!(p.right_trigger, 1.0);
        assert_eq!(p.buttons, A | LEFT_STICK | B | RIGHT_SHOULDER | LEFT);
    }

    fn flag(text: &str) -> VehicleFlag {
        parse_vehicle_flag(text).unwrap().unwrap()
    }

    #[test]
    fn the_vehicle_flag_text_has_an_address_a_width_and_a_value() {
        assert_eq!(parse_vehicle_flag(""), Ok(None));
        assert_eq!(parse_vehicle_flag("  "), Ok(None));
        assert_eq!(parse_vehicle_flag("off"), Ok(None), "the menu's off");
        assert_eq!(flag("img+74dfca5"), VehicleFlag { at: FlagAt::Image(0x74d_fca5), width: 1, value: None });
        assert_eq!(flag("7ff612340000"), VehicleFlag { at: FlagAt::Absolute(0x7ff6_1234_0000), width: 1, value: None });
        assert_eq!(flag("0xDEAD:4"), VehicleFlag { at: FlagAt::Absolute(0xdead), width: 4, value: None });
        assert_eq!(flag("img+98d26d8:2:3"), VehicleFlag { at: FlagAt::Image(0x98d_26d8), width: 2, value: Some(3) });
        assert_eq!(flag("IMG+0x10::0x20"), VehicleFlag { at: FlagAt::Image(0x10), width: 1, value: Some(0x20) });
        assert_eq!(flag("10:1:0").value, Some(0));
        for bad in ["zz", "img+", "img+xyz", "10:3", "10:8", "10:1:256", "10:1:-1", "10:1:x", "10:1:1:1", ":1"] {
            assert!(parse_vehicle_flag(bad).is_err(), "{bad}");
        }
        assert!(flag("10").in_vehicle(1) && flag("10").in_vehicle(0xff) && !flag("10").in_vehicle(0));
        assert!(flag("10:1:2").in_vehicle(2) && !flag("10:1:2").in_vehicle(1) && !flag("10:1:2").in_vehicle(0));
        assert!(flag("10:1:0").in_vehicle(0) && !flag("10:1:0").in_vehicle(1));
    }

    #[test]
    fn the_layout_follows_the_flag_when_it_changes_and_the_keys_still_work() {
        let mut c = controls();
        // Off by default: reading does nothing.
        c.follow_flag(|_| panic!("no flag is set"));
        assert!(!c.in_vehicle());
        c.flag = Some(flag("10"));
        // The first reading sets the layout.
        c.follow_flag(|_| Some(1));
        assert!(c.in_vehicle());
        // The same reading again changes nothing, so a manual switch stands
        // (the keys are fed with the flag off, so no real memory is read).
        let on = c.flag.take();
        held(&mut c, &["G"]);
        held(&mut c, &[]);
        c.flag = on;
        assert!(!c.in_vehicle());
        c.follow_flag(|_| Some(1));
        assert!(!c.in_vehicle(), "the flag has not changed: the manual choice stays");
        // Until the flag changes.
        c.follow_flag(|_| Some(0));
        assert!(!c.in_vehicle());
        c.follow_flag(|_| Some(5));
        assert!(c.in_vehicle());
        // A word that cannot be read leaves things as they are.
        c.follow_flag(|_| None);
        assert!(c.in_vehicle());
        // A value that means in a vehicle.
        let mut d = controls();
        d.flag = Some(flag("10:1:2"));
        d.follow_flag(|_| Some(1));
        assert!(!d.in_vehicle());
        d.follow_flag(|_| Some(2));
        assert!(d.in_vehicle());
    }

    #[test]
    fn with_the_flag_f_is_only_interact_and_does_not_switch_the_layout() {
        // feed_at, not held: held reads the flag from real memory.
        let at = |c: &mut Controls, keys: &[&str], now: u64| {
            let keys: Vec<u8> = keys.iter().map(|k| vk(k)).collect();
            c.feed_at(&keys, &Counts::default(), now);
            c.peek(None)
        };
        let mut c = controls();
        c.flag = Some(flag("10"));
        let pad = at(&mut c, &["F"], 0);
        assert!(pad.buttons & Y != 0, "F still presses Y (interact, doors, get in)");
        at(&mut c, &[], 50);
        assert!(!c.in_vehicle(), "F does not flip the layout while the flag decides");
        // G still switches by hand.
        at(&mut c, &["G"], 100);
        at(&mut c, &[], 150);
        assert!(c.in_vehicle());
        // Without the flag F does not switch either.
        let mut d = controls();
        at(&mut d, &["F"], 0);
        at(&mut d, &[], 50);
        assert!(!d.in_vehicle());
    }

    #[test]
    fn the_menus_mouse_lines_go_over_the_controls_file() {
        let mut c = Controls::new(Settings::with_options(&Options {
            mouse_sensitivity: Some(2.5),
            mouse_invert_y: Some(true),
            ..Options::default()
        }).unwrap());
        c.follow_mouse(0.0, 0);
        assert_eq!((c.s.sensitivity, c.s.invert_y), (2.5, true), "the file's values stand");
        c.follow_mouse(4.0, 1);
        assert_eq!((c.s.sensitivity, c.s.invert_y), (4.0, false));
        c.follow_mouse(0.0, 2);
        assert_eq!((c.s.sensitivity, c.s.invert_y), (2.5, true), "back to the file's sensitivity");
    }

    #[test]
    fn prompts_get_the_key_the_layout_in_force_binds() {
        let s = Settings::default();
        let key = |vehicle, name: &str| key_for(&s, vehicle, output(name).unwrap());
        // On foot: the tap or hold styles count, a plain hold wins.
        assert_eq!(key(false, "A"), Some(0x0D), "Enter is A in menus");
        assert_eq!(key(false, "LeftUp"), Some(b'W'));
        assert_eq!(key(false, "RT"), Some(0x01), "left mouse button");
        assert_eq!(key(false, "LT"), Some(0x02));
        assert_eq!(key(false, "LS"), Some(0xA0), "Sprint");
        assert_eq!(key(false, "Y"), Some(b'F'), "Interact presses Y");
        assert_eq!(key(false, "DpadUp"), Some(0x26));
        assert_eq!(key(false, "LB"), Some(0x09), "tap Holster");
        // In a vehicle the layout in force answers.
        assert_eq!(key(true, "RT"), Some(b'W'), "Accelerate");
        assert_eq!(key(true, "LS"), Some(b'E'), "Horn");
        assert_eq!(key(true, "RB"), Some(0x01), "Shoot is RB in a vehicle");
        // Nothing binds it: no key, the picture stays.
        assert_eq!(key(false, "RightLeft"), None);
        assert_eq!(key(false, "DpadRight"), Some(0x27));
    }

    #[test]
    fn prompts_skip_the_wheel_and_macros() {
        let mut s = Settings::default();
        s.on_foot.bind("WheelUp", &["RightLeft"], Context::Foot).unwrap();
        s.on_foot.bind("N", &["macro:RightLeft 60"], Context::Foot).unwrap();
        assert_eq!(key_for(&s, false, output("RightLeft").unwrap()), None);
        s.on_foot.bind("Num4", &["RightLeft"], Context::Foot).unwrap();
        assert_eq!(key_for(&s, false, output("RightLeft").unwrap()), Some(0x64));
    }

    #[test]
    fn the_controls_page_lists_only_actions_the_layouts_can_resolve() {
        for (list, context) in [(PAGE_ON_FOOT, Context::Foot), (PAGE_VEHICLE, Context::Vehicle), (PAGE_AIRCRAFT, Context::Vehicle)] {
            let mut seen = Vec::new();
            for &(id, label) in list {
                assert!(resolve(id, context).is_some(), "{id} is no action");
                assert!(!label.is_empty() && !seen.contains(&id), "{id}");
                seen.push(id);
            }
        }
        // Each default key table reads back as the layout the tables of the page make.
        assert_eq!(Layout::from_table(&default_table(&[ON_FOOT]), Context::Foot), Layout::of(ON_FOOT, Context::Foot));
        assert_eq!(Layout::from_table(&default_table(&[VEHICLE]), Context::Vehicle), Layout::of(VEHICLE, Context::Vehicle));
        let mut aircraft = Settings::with_options(&Options { aircraft_vehicle: true, ..Options::default() }).unwrap();
        assert_eq!(Layout::from_table(&default_table(&[VEHICLE, AIRCRAFT]), Context::Vehicle), std::mem::take(&mut aircraft.vehicle));
    }

    #[test]
    fn the_controls_page_gets_pad_texts_defaults_and_the_file_laid_over() {
        assert_eq!(pad_text("Jump", false), "A");
        assert_eq!(pad_text("Sprint", false), "LS click");
        assert_eq!(pad_text("Sprint", true), "A");
        assert_eq!(pad_text("Shoot", false), "RT");
        assert_eq!(pad_text("Shoot", true), "RB");
        assert_eq!(pad_text("EnterVehicle", false), "Y and switches to the vehicle keys");
        assert_eq!(pad_text("A", false), "A");
        let options = Options {
            config_path: Some("victory-pc.toml".into()),
            on_foot: BTreeMap::from([("shift".to_owned(), vec![String::new()]), ("Z".to_owned(), vec!["Sprint".to_owned()])]),
            ..Options::default()
        };
        let setup = bindings_setup(&Settings::default(), &options);
        assert!(setup.active && !setup.aircraft_active);
        assert_eq!(setup.source.as_deref(), Some(std::path::Path::new("victory-pc.toml")));
        // "shift" is the file's alias of LShift; it unbinds the default Sprint key, and Z takes it.
        assert_eq!(setup.defaults[0]["LShift"], ["Sprint"]);
        assert!(setup.current[0]["LShift"].is_empty());
        assert_eq!(setup.current[0]["Z"], ["Sprint"]);
        assert_eq!(setup.current[1], setup.defaults[1]);
        // The aircraft table is the vehicle one with the flight keys over it.
        assert_eq!(setup.defaults[2]["W"], ["ThrottleUp"]);
        assert!(setup.defaults[2]["Space"].is_empty());
        assert_eq!(setup.defaults[2]["F"], ["Interact"]);
        let pad = |layout: usize, id: &str| setup.defs[layout].iter().find(|d| d.id == id).unwrap().pad.clone();
        assert_eq!((pad(0, "Jump").as_str(), pad(1, "Handbrake").as_str(), pad(2, "ThrottleUp").as_str()), ("A", "A", "RT"));
    }

    #[test]
    fn the_layouts_follow_the_menus_changes_once_and_at_once() {
        // Fed without the page's registry (which other tests share): only the tables given are followed.
        let pressed = |c: &mut Controls, key: &str| {
            c.feed_at(&[vk(key)], &Counts::default(), 0);
            let buttons = c.peek(None).buttons;
            c.feed_at(&[], &Counts::default(), 1);
            buttons
        };
        let mut foot = default_table(&[ON_FOOT]);
        let vehicle = default_table(&[VEHICLE]);
        // Jump moves from Space to J.
        foot.insert("Space".into(), Vec::new());
        foot.insert("J".into(), vec!["Jump".into()]);
        let mut c = controls();
        assert_eq!(pressed(&mut c, "Space"), A, "Space jumps before the change");
        let mut c = controls();
        c.follow_bindings(5, || Some((foot.clone(), vehicle.clone())));
        assert_eq!(pressed(&mut c, "Space"), 0);
        assert_eq!(pressed(&mut c, "J"), A);
        // The same change count builds nothing again (a table that is gone is not asked for).
        c.follow_bindings(5, || panic!("asked twice"));
        // A count with no tables leaves the layouts as they are.
        c.follow_bindings(6, || None);
        assert_eq!(pressed(&mut c, "J"), A);
        // The vehicle table builds the vehicle layout.
        let mut car = vehicle;
        car.insert("Q".into(), vec!["Handbrake".into()]);
        c.follow_bindings(7, || Some((foot, car)));
        c.vehicle = true;
        assert_eq!(pressed(&mut c, "Q"), A);
    }

    #[test]
    fn key_names_of_the_page_need_a_name_the_file_reads_back() {
        for vk in [b'W', 0x20, 0x70, 0x01, 0x05, 0xA0, 0xBA] {
            let name = name_for_vk(vk).unwrap_or_else(|| panic!("{vk:#x}"));
            assert!(key_code(&name).is_some(), "{name}");
        }
        assert_eq!(name_for_vk(0x01).as_deref(), Some("MouseLeft"));
        assert_eq!(name_for_vk(0x05).as_deref(), Some("Mouse4"));
        // Keys with no name in the file are refused (a hex name would stop the next start).
        assert_eq!(name_for_vk(0x5B), None);
        assert_eq!(canonical_key("shift"), "LShift");
        assert_eq!(canonical_key("w"), "W");
    }
}
