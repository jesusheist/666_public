//! The pages of the F10 menu other than "Advanced" and "Controls": "Home",
//! "Graphics", "Performance", "Overlay" and "Cheats".
//!
//! Each page is a short list of lines, each with a plain label and its values
//! shown as `< value >` (the same kind of page as the fork's old F1 menu, whose
//! settings are all here):
//!
//! - Home: the frame rate cap, frame generation, the overlay on or off, the
//!   borderless window, mouse capture and the English keyboard, with a readout
//!   of the card's memory under the list.
//! - Graphics: DLSS mode and model, the console, the picture size and the
//!   picture switches (light and reflection estimates, fog, ray tracing, water
//!   light, the heat haze and decal programs).
//! - Performance: the speed switches (early depth, copy-backs, soft views,
//!   texture waits, relay, card steps, indirect batches, constants heap).
//! - Overlay: the look of our performance overlay (`host/panel.rs`): on or off,
//!   corner, size, lines shown.
//! - Cheats: the entries of [`crate::cheats`]; the ones nothing is plugged into
//!   yet are dim with their note.
//!
//! Home, Graphics and Performance end with the lines "Save and restart now",
//! "Save for next start", "Reset all to play defaults" and "Close".
//!
//! The lines of the launcher's own settings (`menu_env`) are saved to the
//! launcher's settings file and a restart runs the launcher again (`victory
//! play`); a line that needs a restart has an "R" at its right end and its new
//! value waits (marked "!") until then. Frame generation changes at once when
//! the run started with it ready. The overlay line is what F2 flips, and a cheat
//! is a [`crate::cheats`] entry. Lines that are tunables of the registry
//! (`victory_core::tunables`) go through its range check and its live class.
//!
//! Keys are the menu's: Up and Down select, Left and Right (and Enter) change
//! the value, Delete puts the line back to its default (Shift: the whole page).

use victory_core::tunables::{self as reg, Kind as RegKind, Live, Slot};

use super::live;
use super::{ACCENT, Action, Canvas, Readouts, TEXT, TEXT_DIM, TEXT_SOFT, Tab, Text, WARM, WHITE, bar, chrome, gb, paint_selected, rule, scrollbar};
use crate::cheats::{self, Id};
use crate::tunables::known as k;
use crate::menu_env;

// ---------------------------------------------------------------------------
// The lines.

/// Where a line's value lives.
#[derive(Clone, Copy)]
pub(crate) enum Src {
    /// A tunable of the registry.
    Slot(&'static Slot),
    /// A setting of the launcher (`menu_env`): its variable.
    Env(&'static str),
    /// Whether the overlay is shown now (what F2 flips); changing it sets `VICTORY_PANEL`.
    PanelShown,
    /// An entry of the cheats registry.
    Cheat(Id),
}

/// A line: its label, where its value lives and the values it goes through
/// as (the value's text, what is shown). A value that is none of those is
/// shown as the text has it. The first value of an `Env` line is what the
/// launcher starts with.
pub(crate) struct Item {
    pub label: &'static str,
    pub src: Src,
    pub values: &'static [(&'static str, &'static str)],
    /// What it does, for the line under the list (the registry's own text where it has one).
    pub desc: &'static str,
}

const OFF_ON: &[(&str, &str)] = &[("false", "OFF"), ("true", "ON")];
const ON_UNSET: &[(&str, &str)] = &[("1", "ON"), ("", "OFF")];
const UNSET_ON: &[(&str, &str)] = &[("", "OFF"), ("1", "ON")];
const STAGES: &[(&str, &str)] = &[("2", "2"), ("1", "1"), ("0", "0 OFF")];
const CORNERS: &[(&str, &str)] = &[("top-left", "TOP LEFT"), ("top-right", "TOP RIGHT"), ("bottom-left", "BOTTOM LEFT"), ("bottom-right", "BOTTOM RIGHT")];
const SIZES: &[(&str, &str)] = &[("small", "SMALL"), ("normal", "NORMAL"), ("large", "LARGE")];
const LINES: &[(&str, &str)] = &[("full", "FULL"), ("fps", "FPS AND FRAME ONLY")];

fn slot_item(label: &'static str, slot: &'static Slot, values: &'static [(&'static str, &'static str)]) -> Item {
    Item { label, src: Src::Slot(slot), values, desc: slot.meta().desc }
}

fn env_item(label: &'static str, var: &'static str, values: &'static [(&'static str, &'static str)], desc: &'static str) -> Item {
    Item { label, src: Src::Env(var), values, desc }
}

const OVERLAY_DESC: &str = "The performance overlay: frame rate, frame time, GPU and memory numbers on the picture. F2 shows and hides it too.";

/// The Home list: the frame rate, the window and the input options.
pub(crate) fn home_items() -> Vec<Item> {
    vec![
        env_item("FRAME RATE CAP", "VICTORY_MENU_FPS", &[("1000", "NONE"), ("120", "120"), ("60", "60"), ("30", "30 (THE GAME)")], "Frames begin at most this many a second whatever the title asks (30 is the game's own rate)."),
        env_item("FRAME GENERATION", "VICTORY_MENU_FRAMEGEN", &[("0", "OFF"), ("2", "2X"), ("3", "3X"), ("4", "4X"), ("5", "5X"), ("6", "6X (MOST)")], "DLSS frame generation: frames shown for each one rendered. Changes at once when the run started with it ready."),
        Item { label: "OVERLAY", src: Src::PanelShown, values: OFF_ON, desc: OVERLAY_DESC },
        env_item("FULLSCREEN (NO BORDER)", "VICTORY_FULLSCREEN", UNSET_ON, "A borderless window over the whole screen."),
        env_item("MOUSE CAPTURE", "VICTORY_MOUSE_CAPTURE", UNSET_ON, "Hides and holds the mouse cursor while the window has the focus."),
        env_item("ENGLISH KEYBOARD", "VICTORY_IME_ENGLISH", UNSET_ON, "Forces the English keyboard layout for the game window."),
    ]
}

/// The Graphics list: DLSS, the console and picture size, and the picture switches.
pub(crate) fn graphics_items() -> Vec<Item> {
    vec![
        env_item("DLSS", "VICTORY_MENU_DLSS", &[("quality", "QUALITY"), ("balanced", "BALANCED"), ("performance", "PERFORMANCE"), ("dlaa", "DLAA (NATIVE)"), ("off", "OFF")], "DLSS upscaling mode."),
        env_item("DLSS MODEL", "VICTORY_DLSS_PRESET", &[("", "DEFAULT"), ("k", "K"), ("j", "J"), ("m", "M"), ("l", "L")], "Which DLSS model preset is used."),
        env_item("CONSOLE", "VICTORY_MENU_CONSOLE", &[("series-x", "SERIES X"), ("series-s", "SERIES S (FASTER)")], "Which console the title thinks it runs on."),
        env_item("SERIES X PICTURE", "VICTORY_MENU_RES", &[("1440", "2560X1440"), ("1080", "1920X1080"), ("1080to1440", "1080P TO 1440P"), ("4k", "3840X2160")], "The picture size the title renders at on a Series X."),
        env_item("LIGHT ESTIMATE", "VICTORY_AMBIENT", &[("0.8", "0.8"), ("1", "1"), ("0.5", "0.5"), ("", "OFF")], "A share of the light the visible surfaces send out, used as ambient light."),
        env_item("REFLECTION ESTIMATE", "VICTORY_AMBIENT_SPECULAR", ON_UNSET, "The same estimate used for reflections."),
        env_item("FOG CAP", "VICTORY_AMBIENT_AIR", &[("1.5", "1.5"), ("1", "1"), ("2", "2"), ("", "NONE")], "Most the air scatters, as a multiple of the light around."),
        env_item("RAY TRACING (SLOW, BROKEN)", "VICTORY_BVH_BUILDER", UNSET_ON, "Runs the title's own ray tracing structure builder. Slow and broken."),
        env_item("WATER LIGHT", "VICTORY_WATER_LIGHT", &[("", "ON"), ("0", "OFF")], "Off leaves out the water's two tiled light programs on 32-lane hosts."),
        env_item("HEAT HAZE AND DECAL DEFRAG", "VICTORY_LEAVE_OUT", &[("02384a60fda6f2d0,731a39171b568298", "LEFT OUT"), ("", "RUN (SLOWER, SMEARS)")], "Leaves out the heat haze and decal defrag programs: faster, no smear."),
    ]
}

/// The Performance list: switches that change speed.
pub(crate) fn performance_items() -> Vec<Item> {
    vec![
        env_item("EARLY DEPTH TEST", "VICTORY_EARLY_DEPTH", ON_UNSET, "Early depth-stencil test for pixel programs whose console code tests depth first."),
        env_item("SKIP BIG COPY-BACKS", "VICTORY_WC_NO_BACK", ON_UNSET, "Skips big copy-backs of write-combined memory."),
        env_item("SOFT VIEWS (NO CUT FREEZES)", "VICTORY_SOFT_VIEWS", &[("copies", "ON"), ("", "OFF")], "Soft views of memory sections: no freezes at cuts."),
        env_item("TEXTURE NO WAIT", "VICTORY_TEXTURE_NO_WAIT", ON_UNSET, "Texture uploads do not wait for the card."),
        env_item("RELAY (SPEED, TRIANGLES)", "VICTORY_RELAY", STAGES, "Relay stage: 2 and 1 on, 0 off."),
        env_item("CARD STEPS", "VICTORY_CARD_STEPS", STAGES, "Card steps stage."),
        env_item("INDIRECT BATCH", "VICTORY_INDIRECT_BATCH", STAGES, "Indirect draw batching stage."),
        env_item("CARD COMPARES FROM (KB)", "VICTORY_CARD_BUFFERS_MIN_KB", &[("64", "64"), ("1024", "1024")], "Least size in KiB of a buffer copied onto the card."),
        env_item("CONSTANTS HEAP IN PLACE", "VICTORY_PORT_CONSTANTS", UNSET_ON, "Constants heap in place."),
    ]
}

/// The Overlay list.
pub(crate) fn overlay_items() -> Vec<Item> {
    vec![
        Item { label: "OVERLAY", src: Src::PanelShown, values: OFF_ON, desc: OVERLAY_DESC },
        slot_item("POSITION", k::STATS_PANEL_CORNER.slot(), CORNERS),
        slot_item("SIZE", k::STATS_PANEL_SIZE.slot(), SIZES),
        slot_item("LINES SHOWN", k::STATS_PANEL_LINES.slot(), LINES),
    ]
}

/// What a cheat does, for the line under the list.
fn cheat_desc(id: Id) -> &'static str {
    match id {
        Id::TimeOfDay => "Sets the game's hour: Left and Right move it by one hour, round the clock.",
        Id::FreezeTime => "Holds the game's clock still while on.",
        Id::InfiniteHealth => "Keeps the player's health full while on.",
        Id::NoWanted => "Keeps the wanted level at none while on.",
        Id::MaxMoney => "Gives the most money the game can hold (Enter).",
        Id::InfiniteAmmo => "Keeps every weapon's ammo full while on.",
        Id::Title(4) => "The title's own cheat 4, an on or off switch (Enter). The menu cannot read its real state.",
        Id::Title(2) => "The title's own cheat 2: resets several player fields, probably a health refill (Enter).",
        Id::Title(7 | 10) => "The title's own vehicle cheat: a car appears 10 m ahead (Enter). Does nothing while in a vehicle.",
        Id::Title(n) if n >= 7 => "The title's own vehicle cheat: a car appears 10 m ahead (Enter).",
        Id::Title(_) => "One of the title's own cheats, effect not named yet (Enter).",
    }
}

/// The Cheats list: every entry of the registry.
pub(crate) fn cheats_items() -> Vec<Item> {
    cheats::CHEATS.iter().map(|c| Item { label: c.name, src: Src::Cheat(c.id), values: &[], desc: cheat_desc(c.id) }).collect()
}

/// Whether two texts are the same value of a kind (numbers by their value, so "1" is "1.0").
fn same(kind: RegKind, a: &str, b: &str) -> bool {
    match kind {
        RegKind::Int { .. } | RegKind::Float { .. } => match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
            (Ok(a), Ok(b)) => (a - b).abs() < 1e-9,
            _ => false,
        },
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    }
}

/// The place of the value `current` among `values`.
fn index_of(kind: RegKind, values: &[(&str, &str)], current: &str) -> Option<usize> {
    values.iter().position(|(text, _)| same(kind, text, current))
}

/// What is shown for the value `current`: its label from the list, else the text itself.
pub(crate) fn shown(kind: RegKind, values: &[(&str, &str)], current: &str) -> String {
    index_of(kind, values, current).map_or_else(|| current.trim().to_ascii_uppercase(), |i| values[i].1.to_owned())
}

/// The registry text one step `dir` (1 up, -1 down) from `current`, going
/// round; a value that is not in the list goes to its first (up) or last (down).
pub(crate) fn step_value(kind: RegKind, values: &[(&'static str, &'static str)], current: &str, dir: i32) -> Option<&'static str> {
    if values.is_empty() {
        return None;
    }
    let next = match index_of(kind, values, current) {
        Some(i) => (i as i64 + i64::from(dir)).rem_euclid(values.len() as i64) as usize,
        None if dir > 0 => 0,
        None => values.len() - 1,
    };
    Some(values[next].0)
}

fn when(live: Live) -> &'static str {
    match live {
        Live::Safe => "applies at once",
        Live::NextFrame => "applies next frame",
        Live::Restart => "needs a restart",
    }
}

/// Saves what changed: the launcher's settings when a line of those changed, else the live
/// settings file. Whether it could, and what to tell.
fn save_all() -> (bool, String) {
    if menu_env::any_changed() {
        let (ok, note) = menu_env::save();
        reg::flush();
        (ok, note)
    } else {
        live::save(&reg::list())
    }
}

// ---------------------------------------------------------------------------
// The page.

/// What an action line asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    SaveRestart,
    Save,
    ResetAll,
    Close,
}

/// The action lines of Graphics and Performance, in order, and what each does.
pub(crate) const ACTIONS: [(&str, &str); 4] = [
    ("SAVE AND RESTART NOW", "Writes the settings to the live settings file, then starts the program again with the same arguments and environment and ends this run. Lines marked R apply."),
    ("SAVE FOR NEXT START", "Writes the settings to the live settings file now. Lines marked R apply at the next start."),
    ("RESET ALL TO PLAY DEFAULTS", "Puts every line of this page back to its play default and forgets it in the live settings file."),
    ("CLOSE", "Closes the menu (Esc or F10 does too)."),
];

/// A line as painted.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PLine {
    pub label: String,
    pub value: String,
    pub default: String,
    /// It needs a restart (the "R" mark).
    pub restart: bool,
    pub changed: bool,
    /// A value waits for the next frame or the next start.
    pub pending: bool,
    pub selected: bool,
    pub action: bool,
    /// It cannot be used now (a cheat nothing is plugged into): drawn dim, with `note` in place of the value.
    pub dim: bool,
    pub note: String,
    /// The value is plain text (no `<` and `>` round it): a line that is not stepped with Left and Right.
    pub plain: bool,
}

/// What a page shows, to be compared with what was painted last.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PView {
    pub tab: Tab,
    pub lines: Vec<PLine>,
    pub rows: usize,
    pub scroll: usize,
    pub desc: String,
    pub env: String,
    pub note: String,
    /// The count at the header's right ("2 changed of 11 lines").
    pub tally: String,
    pub changed_all: usize,
    pub total: usize,
    pub fps_x10: u32,
    pub ms_x100: u32,
    /// The memory and frame numbers of the Home page's readouts (the menu sets it).
    pub mem: Option<Readouts>,
}

/// One of the pages.
pub(crate) struct Curated {
    tab: Tab,
    items: Vec<Item>,
    /// Action lines after the items (Graphics and Performance have them).
    actions: bool,
    selected: usize,
    scroll: usize,
    note: String,
}

impl Curated {
    /// The page `tab` (Advanced is not one of these: it has no lines here).
    pub(crate) fn new(tab: Tab) -> Curated {
        let (items, actions) = match tab {
            Tab::Home => (home_items(), true),
            Tab::Graphics => (graphics_items(), true),
            Tab::Performance => (performance_items(), true),
            Tab::Overlay => (overlay_items(), false),
            Tab::Cheats => (cheats_items(), false),
            Tab::Controls | Tab::Advanced => (Vec::new(), false),
        };
        Curated { tab, items, actions, selected: 0, scroll: 0, note: String::new() }
    }

    fn rows(&self) -> usize {
        self.items.len() + if self.actions { ACTIONS.len() } else { 0 }
    }

    /// What the line `item` holds now, as the value's text (a waiting value counts).
    fn value_text(item: &Item) -> String {
        match item.src {
            Src::Slot(s) => s.pending_text().unwrap_or_else(|| s.value_text()),
            Src::Env(var) => {
                // A variable the launcher did not set is its first value (the play value).
                let now = menu_env::current(var);
                match item.values.first() {
                    Some((first, _)) if now.is_empty() && !item.values.iter().any(|(v, _)| v.is_empty()) => (*first).to_owned(),
                    _ => now,
                }
            }
            Src::PanelShown => crate::host::stats_panel_shown().to_string(),
            Src::Cheat(id) => cheats::value_text(id),
        }
    }

    fn kind_of(item: &Item) -> RegKind {
        match item.src {
            Src::Slot(s) => s.meta().kind,
            Src::Env(_) | Src::PanelShown | Src::Cheat(_) => RegKind::Bool,
        }
    }

    fn is_changed(item: &Item) -> bool {
        match item.src {
            Src::Slot(s) => live::is_changed(s),
            Src::Env(_) => item.values.first().is_some_and(|(first, _)| !same(RegKind::Bool, first, &Self::value_text(item))),
            Src::PanelShown => crate::host::stats_panel_shown(),
            Src::Cheat(id) => cheats::is_on(id),
        }
    }

    fn restart_class(item: &Item) -> bool {
        match item.src {
            Src::Slot(s) => s.meta().live == Live::Restart,
            Src::Env(var) => !menu_env::live(var),
            _ => false,
        }
    }

    /// A value waits for the next start (a launcher line changed and not saved yet).
    fn waiting(item: &Item) -> bool {
        match item.src {
            Src::Slot(s) => s.pending_text().is_some(),
            Src::Env(var) => !menu_env::live(var) && menu_env::changed(var),
            _ => false,
        }
    }

    /// Does what `action` asks (Up, Down, Left, Right, Enter; the others are
    /// the menu's). Enter on an action line answers what it asks for.
    pub(crate) fn step(&mut self, action: Action) -> Option<Act> {
        let total = self.rows();
        if total == 0 {
            return None;
        }
        self.selected = self.selected.min(total - 1);
        let n = self.items.len();
        let mut asked = None;
        // What the last action said stays until another key is pressed.
        if !(action == Action::Enter && self.selected >= n) {
            self.note.clear();
        }
        match action {
            Action::Up => self.selected = (self.selected + total - 1) % total,
            Action::Down => self.selected = (self.selected + 1) % total,
            Action::Left if self.selected < n => self.change(self.selected, -1),
            Action::Right | Action::Enter if self.selected < n => self.change(self.selected, 1),
            Action::Enter => asked = Some(Self::act_of(self.selected - n)),
            Action::Left | Action::Right | Action::Toggle | Action::Close => {}
        }
        self.scroll = live::follow(self.selected, self.scroll, total);
        asked
    }

    fn act_of(i: usize) -> Act {
        match i {
            0 => Act::SaveRestart,
            1 => Act::Save,
            2 => Act::ResetAll,
            _ => Act::Close,
        }
    }

    fn change(&mut self, at: usize, dir: i32) {
        let (src, values) = (self.items[at].src, self.items[at].values);
        match src {
            Src::Env(var) => {
                let current = Self::value_text(&self.items[at]);
                if let Some(text) = step_value(RegKind::Bool, values, &current, dir) {
                    menu_env::set(var, text);
                }
            }
            Src::PanelShown => {
                let _ = k::PANEL.set(!crate::host::stats_panel_shown());
            }
            Src::Cheat(id) => {
                // An entry nothing is plugged into says why; one that did not take says so.
                if let Err(why) = cheats::step(id, dir) {
                    why.clone_into(&mut self.note);
                }
            }
            Src::Slot(slot) => {
                let current = Self::value_text(&self.items[at]);
                if let Some(text) = step_value(slot.meta().kind, values, &current, dir)
                    && let Err(err) = slot.set_text(text)
                {
                    tracing::debug!(target: "victory::tunables", "menu: {err}");
                }
            }
        }
    }

    /// The selected line back to its default.
    pub(crate) fn reset_selected(&self) {
        if let Some(item) = self.items.get(self.selected) {
            Self::reset(item);
        }
    }

    fn reset(item: &Item) {
        match item.src {
            Src::Slot(s) => {
                s.reset();
            }
            Src::Env(var) => {
                if let Some((first, _)) = item.values.first() {
                    menu_env::set(var, first);
                }
            }
            Src::PanelShown => {
                let _ = k::PANEL.set(false);
            }
            Src::Cheat(id) => cheats::reset(id),
        }
    }

    /// Every line of the page back to its default; how many were changed.
    pub(crate) fn reset_all(&self) -> usize {
        let mut n = 0;
        for item in &self.items {
            if Self::is_changed(item) {
                Self::reset(item);
                n += 1;
            }
        }
        n
    }

    /// Does an action line; whether the menu is to close. Saving writes the
    /// launcher's settings file (and the live settings file) now; restarting
    /// then runs the launcher again (or this program, when it was not started by
    /// the launcher) and ends this run.
    pub(crate) fn run(&mut self, act: Act) -> bool {
        match act {
            Act::Close => return true,
            Act::Save => {
                self.note = save_all().1;
            }
            Act::SaveRestart => {
                let (ok, note) = save_all();
                self.note = note;
                if ok {
                    let failed = if std::env::var_os("VICTORY_MENU_PLAY").is_some() { menu_env::restart() } else { live::restart() };
                    if let Some(failed) = failed {
                        self.note = failed;
                    }
                }
            }
            Act::ResetAll => {
                let n = self.reset_all();
                let defaults: Vec<(&'static str, &'static str)> = self
                    .items
                    .iter()
                    .filter_map(|i| match (i.src, i.values.first()) {
                        (Src::Env(var), Some((first, _))) => Some((var, *first)),
                        _ => None,
                    })
                    .collect();
                menu_env::reset(&defaults);
                reg::flush();
                self.note = if n == 0 { "Nothing was changed".to_owned() } else { format!("{n} back to play default and saved") };
            }
        }
        false
    }

    /// What to paint now; `fps` and `ms` are the frame rate and frame time.
    pub(crate) fn view(&self, fps: f32, ms: f32) -> PView {
        let n = self.items.len();
        let rows = self.rows();
        let selected = self.selected.min(rows.saturating_sub(1));
        let scroll = self.scroll.min(rows.saturating_sub(live::ROWS));
        let mut lines = Vec::with_capacity(live::ROWS.min(rows));
        for at in scroll..rows.min(scroll + live::ROWS) {
            if at >= n {
                lines.push(PLine { label: ACTIONS[at - n].0.to_owned(), selected: at == selected, action: true, ..PLine::default() });
                continue;
            }
            let item = &self.items[at];
            let kind = Self::kind_of(item);
            let mut line = PLine { label: item.label.to_owned(), selected: at == selected, ..PLine::default() };
            match item.src {
                Src::Env(_) => {
                    line.value = shown(kind, item.values, &Self::value_text(item));
                    line.default = item.values.first().map(|(_, label)| (*label).to_owned()).unwrap_or_default();
                    line.pending = Self::waiting(item);
                }
                Src::PanelShown => {
                    line.value = shown(kind, item.values, &Self::value_text(item));
                    line.default = "OFF".to_owned();
                }
                Src::Slot(s) => {
                    line.value = shown(kind, item.values, &Self::value_text(item));
                    line.default = shown(kind, item.values, &s.default_text());
                    line.pending = Self::waiting(item);
                }
                Src::Cheat(id) => {
                    line.value = cheats::value_text(id);
                    if !cheats::available(id) {
                        line.dim = true;
                        line.note = cheats::cheat(id).note.to_owned();
                    }
                }
            }
            line.restart = Self::restart_class(item);
            line.changed = Self::is_changed(item);
            lines.push(line);
        }
        let (desc, env) = if selected >= n {
            (ACTIONS[selected - n].1.to_owned(), String::new())
        } else {
            let item = &self.items[selected];
            match item.src {
                Src::Slot(s) => (format!("{} ({})", item.desc, when(s.meta().live)), s.meta().env.to_owned()),
                Src::Env(var) => (format!("{} ({})", item.desc, if menu_env::live(var) { "applies at once" } else { "needs a restart" }), var.to_owned()),
                Src::Cheat(id) if !cheats::available(id) => (format!("{} Not available yet: {}.", item.desc, cheats::cheat(id).note), String::new()),
                Src::Cheat(_) => (format!("{} (applies at once)", item.desc), String::new()),
                _ => (format!("{} (applies at once)", item.desc), String::new()),
            }
        };
        let changed_all = self.items.iter().filter(|i| Self::is_changed(i)).count();
        let tally = match self.tab {
            Tab::Home => String::new(),
            Tab::Cheats => format!("{} of {} available", cheats::available_count(), n),
            _ if changed_all == 0 => format!("none changed, {n} lines"),
            _ => format!("{changed_all} changed of {n} lines"),
        };
        PView {
            tab: self.tab,
            lines,
            rows,
            scroll,
            desc,
            env,
            note: self.note.clone(),
            tally,
            changed_all,
            total: n,
            fps_x10: (fps * 10.0).round() as u32,
            ms_x100: (ms * 100.0).round() as u32,
            mem: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Painting (the sizes are the Advanced page's).

/// The Home page's readout under its list: the card's memory as a bar.
fn paint_readouts(text: &mut Text, c: &mut Canvas, scale: f32, (left, top, width): (f32, f32, f32), r: &Readouts) {
    let u = |x: f32| x * scale;
    let px = |x: f32| (x * scale).round().max(1.0) as i32;
    rule(c, scale, left, top, width);
    let (label_px, value_px) = (px(13.0), px(16.0));
    let y = top + u(12.0);
    text.draw(c, "GPU MEMORY", left + u(2.0), y, label_px, true, TEXT_DIM, 1.0, false);
    text.draw(c, &format!("{} / {}", gb(r.gpu_used), gb(r.gpu_budget)), left + width - u(2.0), y - u(1.0), value_px, true, TEXT, 1.0, true);
    bar(c, (left + u(2.0), y + u(22.0), width - u(4.0), u(7.0)), r.gpu_used as f32 / r.gpu_budget.max(1) as f32);
}

/// Paints the page for `v` at `scale`.
pub(crate) fn paint(text: &mut Text, v: &PView, scale: f32) -> Canvas {
    let (pw, ph) = live::panel_size(scale);
    let mut c = Canvas::new(pw as usize, ph as usize);
    let u = |x: f32| x * scale;
    let px = |x: f32| (x * scale).round().max(1.0) as i32;
    let w = pw as f32;
    let (pad, header, gap, row) = (live::PAD, live::HEADER, live::GAP, live::ROW);

    chrome(text, &mut c, scale, header, v.tab, "", &v.tally);

    // The list.
    let top = u(header + gap);
    let (row_h, left, width) = (u(row), u(pad), w - u(2.0 * pad));
    let (size, small) = (px(16.0), px(13.0));
    let name_max = u(300.0);
    let (box_left, box_right) = (left + u(334.0), left + u(580.0));
    let default_right = left + width - u(60.0);
    let mark_right = left + width - u(14.0);
    for (at, line) in v.lines.iter().enumerate() {
        let y = top + at as f32 * row_h;
        if line.selected {
            paint_selected(&mut c, scale, (left, y + u(2.0), width, row_h - u(4.0)));
        }
        let (name_col, value_col, dim) = if line.dim {
            (TEXT_DIM, TEXT_DIM, TEXT_DIM)
        } else if line.selected {
            (WHITE, WHITE, TEXT_SOFT)
        } else {
            (TEXT_SOFT, TEXT, TEXT_DIM)
        };
        let ty = y + (row_h - u(17.0)) / 2.0 - u(2.0);
        if line.action {
            let col = if line.selected { WHITE } else { WARM };
            text.draw(&mut c, &line.label, left + u(24.0), ty, size, true, col, 1.0, false);
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
        let name = live::fit(text, &line.label, size, line.selected, name_max);
        put(text, &mut c, &name, left + u(24.0), size, name_col, false, line.selected && !line.dim);
        if line.dim {
            // Not usable yet: the note, dim, where the value would be.
            let note = live::fit(text, &line.note, small, false, width - u(24.0) - name_max - u(30.0));
            put(text, &mut c, &note, left + width - u(14.0), small, TEXT_DIM, true, false);
            continue;
        }
        if line.plain {
            let value = live::fit(text, &line.value, size, true, box_right - box_left);
            put(text, &mut c, &value, box_left, size, if line.pending { WARM } else { value_col }, false, true);
        } else {
            put(text, &mut c, "<", box_left, size, ACCENT, false, true);
            put(text, &mut c, ">", box_right, size, ACCENT, true, true);
            let value = live::fit(text, &line.value, size, true, box_right - box_left - u(40.0));
            let (vw, _) = text.measure(&value, size, true);
            put(text, &mut c, &value, (box_left + box_right) / 2.0 - vw as f32 / 2.0, size, value_col, false, true);
        }
        if !line.default.is_empty() && v.tab != Tab::Cheats {
            let default = live::fit(text, &format!("default {}", line.default), small, false, u(140.0));
            put(text, &mut c, &default, default_right, small, dim, true, false);
        }
        if line.restart {
            let col = if line.pending { WARM } else { dim };
            put(text, &mut c, "R", mark_right, small, col, true, true);
        }
    }
    scrollbar(&mut c, scale, (w - u(11.0), top + u(6.0), row_h * live::ROWS as f32 - u(12.0)), v.rows, live::ROWS, v.scroll);
    if let Some(r) = &v.mem {
        paint_readouts(text, &mut c, scale, (left, top + row_h * v.lines.len() as f32 + u(10.0), width), r);
    }

    // Footer: the description, the frame rate, the keys.
    let foot = top + row_h * live::ROWS as f32 + u(gap);
    rule(&mut c, scale, left, foot - u(5.0), width);
    let lines = live::wrap2(text, &v.desc, px(15.0), width - u(4.0));
    text.draw(&mut c, &lines[0], left + u(2.0), foot + u(1.0), px(15.0), false, TEXT, 1.0, false);
    text.draw(&mut c, &lines[1], left + u(2.0), foot + u(22.0), px(15.0), false, TEXT, 1.0, false);
    text.draw(&mut c, &v.env, left + width - u(2.0), foot + u(44.0), px(12.0), false, TEXT_DIM, 1.0, true);
    let stats = format!("{:.0} FPS     {:.2} ms a frame", v.fps_x10 as f32 / 10.0, v.ms_x100 as f32 / 100.0);
    text.draw(&mut c, &stats, left + u(2.0), foot + u(46.0), px(15.0), true, TEXT_SOFT, 1.0, false);
    let note = live::fit(text, &v.note, px(14.0), true, width - u(4.0));
    text.draw(&mut c, &note, left + u(2.0), foot + u(66.0), px(14.0), true, WARM, 1.0, false);
    let keys = if v.tab == Tab::Controls {
        "Tab next page      Up Down select      Enter press a new key      Right add a key      Del default (Shift: section)      PgUp PgDn section      Esc or F10 close"
    } else {
        "Tab next page      Shift+Tab previous      Up Down select      Left Right change      Del default (Shift: page)      Esc or F10 close"
    };
    text.draw(&mut c, keys, left + u(2.0), foot + u(88.0), px(12.0), false, TEXT_DIM, 1.0, false);
    let last = match v.tab {
        Tab::Home | Tab::Graphics | Tab::Performance => "R: needs a restart, a value waiting is marked !      Enter on the last lines saves and restarts, saves, resets or closes",
        Tab::Cheats => "Dim lines are not wired to the game yet: a press on one shows why",
        Tab::Controls => "Keys apply at once; the last lines save them into the controls file (a .bak copy first)      * marks a line changed from its default",
        _ => "These apply at once and are kept in the live settings file      F2 shows and hides the overlay",
    };
    text.draw(&mut c, last, left + u(2.0), foot + u(103.0), px(12.0), false, TEXT_DIM, 1.0, false);
    c
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The launcher settings are process-wide: the tests that change them take turns.
    fn turn() -> std::sync::MutexGuard<'static, ()> {
        static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
        TURN.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn var_of(item: &Item) -> &'static str {
        match item.src {
            Src::Env(v) => v,
            _ => panic!("not a launcher line"),
        }
    }

    /// A PNG file (stored, not compressed) of an 8-bit RGB picture, for the previews.
    pub(crate) fn encode_png(w: u32, h: u32, rgb: &[u8]) -> Vec<u8> {
        fn crc(bytes: &[u8]) -> u32 {
            let mut c = !0u32;
            for &b in bytes {
                c ^= u32::from(b);
                for _ in 0..8 {
                    c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                }
            }
            !c
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend((data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend(data);
            out.extend(&body);
            out.extend(crc(&body).to_be_bytes());
        }
        let mut raw = Vec::with_capacity((w as usize * 3 + 1) * h as usize);
        for row in rgb.chunks(w as usize * 3).take(h as usize) {
            raw.push(0);
            raw.extend(row);
        }
        let mut z = vec![0x78, 0x01];
        let mut chunks = raw.chunks(65535).peekable();
        while let Some(c) = chunks.next() {
            z.push(u8::from(chunks.peek().is_none()));
            z.extend((c.len() as u16).to_le_bytes());
            z.extend((!(c.len() as u16)).to_le_bytes());
            z.extend(c);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &x in &raw {
            a = (a + u32::from(x)) % 65521;
            b = (b + a) % 65521;
        }
        z.extend(((b << 16) | a).to_be_bytes());
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut head = Vec::new();
        head.extend(w.to_be_bytes());
        head.extend(h.to_be_bytes());
        head.extend([8, 2, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &head);
        chunk(&mut out, b"IDAT", &z);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn every_line_of_the_old_f1_menu_is_on_a_page() {
        let mut vars: Vec<&str> = Vec::new();
        for items in [home_items(), graphics_items(), performance_items()] {
            for item in &items {
                if let Src::Env(v) = item.src {
                    assert!(!item.values.is_empty() && !item.desc.is_empty(), "{v}");
                    vars.push(v);
                }
            }
        }
        // The old menu listed these (the frame rate line and the F3 panel became the overlay).
        for want in [
            "VICTORY_MENU_DLSS", "VICTORY_MENU_FRAMEGEN", "VICTORY_MENU_FPS", "VICTORY_FULLSCREEN", "VICTORY_MOUSE_CAPTURE",
            "VICTORY_IME_ENGLISH", "VICTORY_MENU_CONSOLE", "VICTORY_MENU_RES", "VICTORY_DLSS_PRESET", "VICTORY_AMBIENT",
            "VICTORY_AMBIENT_SPECULAR", "VICTORY_AMBIENT_AIR", "VICTORY_BVH_BUILDER", "VICTORY_WATER_LIGHT",
            "VICTORY_LEAVE_OUT", "VICTORY_EARLY_DEPTH", "VICTORY_WC_NO_BACK", "VICTORY_SOFT_VIEWS",
            "VICTORY_TEXTURE_NO_WAIT", "VICTORY_RELAY", "VICTORY_CARD_STEPS", "VICTORY_INDIRECT_BATCH",
            "VICTORY_CARD_BUFFERS_MIN_KB", "VICTORY_PORT_CONSTANTS",
        ] {
            assert_eq!(vars.iter().filter(|v| **v == want).count(), 1, "{want}");
        }
        assert_eq!(vars.len(), 24);
        assert!(matches!(home_items()[2].src, Src::PanelShown));
    }

    #[test]
    fn home_has_the_frame_cap_frame_generation_and_the_overlay_switch() {
        let items = home_items();
        let labels: Vec<&str> = items.iter().map(|i| i.label).collect();
        assert_eq!(&labels[..3], ["FRAME RATE CAP", "FRAME GENERATION", "OVERLAY"]);
        let page = Curated::new(Tab::Home);
        assert_eq!(page.rows(), items.len() + 4);
        let mut v = page.view(58.2, 17.2);
        v.mem = Some(Readouts { gpu_used: 21_300 << 20, gpu_budget: 24_170 << 20, fps: 58.2, frame_ms: 17.2 });
        assert_eq!(v.tally, "");
        assert!(v.lines[0].selected && v.desc.contains("Frames begin"), "{}", v.desc);
        let mut text = Text::new().expect("gdi");
        let canvas = paint(&mut text, &v, 1.0);
        assert_eq!((canvas.w as u32, canvas.h as u32), live::panel_size(1.0));
        // The readout's bar is under the list: the used share of the card's memory is filled (accent) from the left.
        let list_end = (live::HEADER + live::GAP + v.lines.len() as f32 * live::ROW) as usize;
        let bar_y = list_end + 10 + 12 + 25;
        let filled = canvas.px[bar_y * canvas.w + 60];
        assert!(filled[3] > 0.9 && filled[0] > 0.6, "{filled:?}");
        // The list and the readout stay clear of the footer.
        assert!(bar_y + 4 < (live::HEADER + live::GAP + live::ROW * live::ROWS as f32) as usize);
    }

    #[test]
    fn values_match_by_text_and_step_round() {
        let items = home_items();
        let cap = &items[0];
        assert_eq!(shown(RegKind::Bool, cap.values, "60"), "60");
        assert_eq!(shown(RegKind::Bool, cap.values, "30"), "30 (THE GAME)");
        assert_eq!(shown(RegKind::Bool, cap.values, "75"), "75");
        assert_eq!(step_value(RegKind::Bool, cap.values, "1000", 1), Some("120"));
        assert_eq!(step_value(RegKind::Bool, cap.values, "30", 1), Some("1000"));
        assert_eq!(step_value(RegKind::Bool, cap.values, "1000", -1), Some("30"));
        // An unset value is a value too (the lines that are off when unset).
        let unset = [("1", "ON"), ("", "OFF")];
        assert_eq!(shown(RegKind::Bool, &unset, ""), "OFF");
        assert_eq!(step_value(RegKind::Bool, &unset, "", 1), Some("1"));
        assert_eq!(step_value(RegKind::Bool, &unset, "1", 1), Some(""));
        let f = RegKind::Float { min: 0.0, max: 10.0 };
        assert_eq!(shown(f, &[("0", "OFF"), ("1", "1")], "1.0"), "1");
        assert_eq!(step_value(RegKind::Bool, &[], "2", 1), None);
    }

    #[test]
    fn a_launcher_line_waits_as_pending_and_goes_back() {
        let _turn = turn();
        let mut page = Curated::new(Tab::Performance);
        let n = page.items.len();
        assert_eq!(page.rows(), n + 4);
        let at = page.items.iter().position(|i| i.label == "TEXTURE NO WAIT").unwrap();
        let var = var_of(&page.items[at]);
        page.selected = at;
        let before = menu_env::current(var);
        page.step(Action::Right);
        let want = if before.is_empty() { "1" } else { "" };
        assert_eq!(menu_env::current(var), want);
        let v = page.view(0.0, 0.0);
        let line = v.lines.iter().find(|l| l.label == "TEXTURE NO WAIT").unwrap();
        assert!(line.restart && line.pending && line.selected);
        assert!(v.desc.contains("needs a restart") && v.env == var, "{}", v.desc);
        // The next press goes back to what the run started with.
        page.step(Action::Right);
        assert_eq!(menu_env::current(var), before);
        assert!(!menu_env::changed(var));
        // The last lines are the actions, in the fork's order, and Enter asks for them.
        page.selected = n;
        assert_eq!(page.step(Action::Enter), Some(Act::SaveRestart));
        page.step(Action::Down);
        assert_eq!(page.step(Action::Enter), Some(Act::Save));
        page.step(Action::Down);
        assert_eq!(page.step(Action::Enter), Some(Act::ResetAll));
        page.step(Action::Down);
        assert_eq!(page.step(Action::Enter), Some(Act::Close));
        assert_eq!(page.step(Action::Right), None);
        page.step(Action::Down);
        assert_eq!(page.selected, 0);
        page.step(Action::Up);
        assert_eq!(page.selected, n + 3);
        let v = page.view(0.0, 0.0);
        assert!(v.lines.last().unwrap().action && v.lines.last().unwrap().label == "CLOSE");
        assert_eq!(v.desc, ACTIONS[3].1);
        // Graphics and Home have the same four lines; Overlay and Cheats have none.
        assert_eq!(Curated::new(Tab::Graphics).rows(), graphics_items().len() + 4);
        for tab in [Tab::Overlay, Tab::Cheats] {
            let page = Curated::new(tab);
            assert_eq!(page.rows(), page.items.len(), "{tab:?}");
        }
    }

    #[test]
    fn save_writes_the_launchers_file_and_reset_forgets_the_page() {
        let _turn = turn();
        let dir = tempfile::tempdir_in(std::env::current_exe().unwrap().parent().unwrap()).unwrap();
        let path = dir.path().join("victory-settings.txt");
        std::fs::write(&path, "# old\r\nVICTORY_MENU_FPS=60\r\n").unwrap();
        // SAFETY: only these tests read the variable, one at a time.
        unsafe { std::env::set_var("VICTORY_MENU_FILE", &path) };
        let mut page = Curated::new(Tab::Performance);
        let at = page.items.iter().position(|i| i.label == "CARD STEPS").unwrap();
        let var = var_of(&page.items[at]);
        page.selected = at;
        page.step(Action::Right);
        assert!(!page.run(Act::Save));
        assert!(page.note.starts_with("Saved"), "{}", page.note);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&format!("{var}=")) && text.contains("VICTORY_MENU_FPS=60"), "{text}");
        assert!(!menu_env::changed(var));
        assert!(page.run(Act::Close));
        // Reset all puts the page's lines back to the play values and forgets them in the file.
        assert!(!page.run(Act::ResetAll));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(var) && text.contains("VICTORY_MENU_FPS=60"), "{text}");
        // SAFETY: as above.
        unsafe { std::env::remove_var("VICTORY_MENU_FILE") };
        assert!(!menu_env::save().0, "no file, no save");
    }

    #[test]
    fn the_cheats_page_is_dim_with_notes_until_something_is_plugged_in() {
        let _turn = cheats::test_turn();
        cheats::clear();
        let mut page = Curated::new(Tab::Cheats);
        assert_eq!(page.rows(), 19);
        let v = page.view(0.0, 0.0);
        assert_eq!(v.tally, "0 of 19 available");
        assert!(v.lines.iter().all(|l| l.dim && !l.action && !l.restart));
        assert_eq!(v.lines[0].label, "TIME OF DAY");
        assert_eq!(v.lines[0].note, "Start with VICTORY_STORY_CLOCK=1");
        assert_eq!(v.lines[2].note, "Start with VICTORY_CHEATS=1");
        // The help line says why; a press shows the note too.
        assert!(v.desc.contains("Not available yet: Start with VICTORY_STORY_CLOCK=1"), "{}", v.desc);
        assert!(v.note.is_empty());
        page.step(Action::Right);
        assert_eq!(page.view(0.0, 0.0).note, "Start with VICTORY_STORY_CLOCK=1");
        page.step(Action::Down);
        page.step(Action::Down);
        page.step(Action::Enter);
        assert_eq!(page.view(0.0, 0.0).note, "Start with VICTORY_CHEATS=1");
        // Another key clears it.
        page.step(Action::Down);
        assert!(page.view(0.0, 0.0).note.is_empty());
        // Plug a clock in: the time of day answers and steps by an hour.
        use std::sync::atomic::{AtomicU8, Ordering};
        static HOUR: AtomicU8 = AtomicU8::new(21);
        cheats::register_clock(|| Some((HOUR.load(Ordering::Relaxed), 5)), |h, _| {
            HOUR.store(h, Ordering::Relaxed);
            true
        });
        let mut page = Curated::new(Tab::Cheats);
        let v = page.view(0.0, 0.0);
        assert!(!v.lines[0].dim && v.lines[0].value == "21:05" && v.lines[1].dim);
        assert_eq!(v.tally, "1 of 19 available");
        page.step(Action::Right);
        assert_eq!(HOUR.load(Ordering::Relaxed), 22);
        page.step(Action::Left);
        page.step(Action::Left);
        assert_eq!(HOUR.load(Ordering::Relaxed), 20);
        assert!(page.view(0.0, 0.0).note.is_empty());
        cheats::clear();
    }

    #[test]
    fn the_pages_are_painted_at_the_advanced_pages_size() {
        let mut text = Text::new().expect("gdi");
        for tab in [Tab::Home, Tab::Graphics, Tab::Performance, Tab::Overlay, Tab::Cheats] {
            let v = Curated::new(tab).view(59.6, 16.8);
            for scale in [0.75f32, 1.0, 2.0] {
                let canvas = paint(&mut text, &v, scale);
                assert_eq!((canvas.w as u32, canvas.h as u32), live::panel_size(scale));
                let drawn = canvas.px.iter().filter(|p| p[3] > 0.05).count();
                assert!(drawn > canvas.px.len() / 2, "{tab:?}");
            }
        }
        // With an action line selected the page paints too.
        let mut page = Curated::new(Tab::Graphics);
        page.selected = page.items.len() + 1;
        page.scroll = live::follow(page.selected, 0, page.rows());
        let v = page.view(60.0, 16.7);
        assert!(v.lines.iter().any(|l| l.action && l.selected));
        let canvas = paint(&mut text, &v, 1.0);
        assert_eq!((canvas.w as u32, canvas.h as u32), live::panel_size(1.0));
    }

    /// The pages the way they look over a picture: every page painted and
    /// composited by the CPU stand-in of the card's glass (the picture under
    /// the panel blurred, darkened and tinted, with a grain, the panel on top),
    /// saved for review as `menu-<page>.png`.
    #[test]
    fn the_previews_are_saved() {
        use super::super::tests::backdrop;
        let _turn = cheats::test_turn();
        cheats::clear();
        // Part of the cheats plugged in, so the page shows both looks.
        cheats::register_clock(|| Some((14, 0)), |_, _| true);
        let mut text = Text::new().unwrap();
        let dir = std::path::Path::new(r"D:\.devops\.temp\sxo-port");
        std::fs::create_dir_all(dir).unwrap();
        let (bw, bh) = (1920usize, 1080usize);
        let scale = 1.0;
        let readouts = Readouts { gpu_used: 21_300 << 20, gpu_budget: 24_170 << 20, fps: 58.2, frame_ms: 17.2 };
        let save = |name: &str, canvas: &Canvas| {
            let mut pic = backdrop(bw, bh);
            let top = (bh - canvas.h) / 2;
            canvas.composite(&mut pic, bw, 48, top, 1.0, scale);
            let png = encode_png(bw as u32, bh as u32, &pic);
            assert!(png.len() > 10_000);
            std::fs::write(dir.join(format!("menu-{name}.png")), png).unwrap();
        };
        for (name, tab, selected) in [
            ("home", Tab::Home, 0),
            ("graphics", Tab::Graphics, 4),
            ("performance", Tab::Performance, 2),
            ("overlay", Tab::Overlay, 1),
            ("cheats", Tab::Cheats, 0),
        ] {
            let mut page = Curated::new(tab);
            page.selected = selected;
            let mut v = page.view(readouts.fps, readouts.frame_ms);
            if tab == Tab::Home {
                v.mem = Some(readouts);
            }
            save(name, &paint(&mut text, &v, scale));
        }
        // The Controls page: a line being rebound, a changed line, and the note of a key taken from another action.
        {
            use super::super::controls;
            let _registry = controls::test_turn();
            victory_core::keybinds::install(controls::test_setup());
            victory_core::keybinds::set_namer(controls::test_namer);
            victory_core::keybinds::assign(victory_core::keybinds::Layout::OnFoot, "Jump", "C", true).unwrap();
            let mut page = controls::Page::default();
            let at = victory_core::keybinds::lines(victory_core::keybinds::Layout::OnFoot).iter().position(|l| l.label == "Sprint").unwrap();
            for _ in 0..=at {
                page.step(Action::Down);
            }
            page.step(Action::Enter);
            let mut v = page.view(readouts.fps, readouts.frame_ms);
            v.note = "Space was Jump, which now has no key there".to_owned();
            save("controls", &paint(&mut text, &v, scale));
            victory_core::keybinds::clear();
        }
        // The Advanced page: the whole registry, its first group.
        let mut live_page = live::Page::default();
        live_page.ensure();
        save("advanced", &live::paint(&mut text, &live_page.view(readouts.fps, readouts.frame_ms), scale));
        // A scrolled Graphics page with an action line selected, at a 4K picture's scale.
        let mut page = Curated::new(Tab::Graphics);
        page.selected = page.items.len() + 1;
        page.scroll = live::follow(page.selected, 0, page.rows());
        let big = paint(&mut text, &page.view(readouts.fps, readouts.frame_ms), 2.0);
        let mut pic = backdrop(3840, 2160);
        big.composite(&mut pic, 3840, 96, (2160 - big.h) / 2, 1.0, 2.0);
        std::fs::write(dir.join("menu-graphics-4k-actions.png"), encode_png(3840, 2160, &pic)).unwrap();
        cheats::clear();
    }
}
