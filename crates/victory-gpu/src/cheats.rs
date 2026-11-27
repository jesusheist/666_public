//! The cheats of the in-game menu (F10, "Cheats" page): a registry of entries
//! (name, kind, whether it can be used now, a note) and the plugs that make
//! an entry usable.
//!
//! Nothing here knows where the title keeps its values. An entry is
//! "available" once something registers a [`Backend`] for it: a pair of
//! functions that read the current value and write a new one (a later memory
//! provider registers each entry with its image-plus-offset reads and writes).
//! The time of day has its own, friendlier plug: [`register_clock`] takes the
//! game's clock as `get() -> Option<(hour, minute)>` and `set(hour, minute) ->
//! bool`, and [`register_clock_freeze`] adds the freeze (the clock's hook may
//! hold the time still). Until something registers, an entry is drawn dim with
//! its note and a press only shows the note.
//!
//! Values: a toggle is 0 or 1, an action ignores its value (it runs once), a
//! choice is a whole number (the hour, 0 to 23, for the time of day).

use std::sync::Mutex;

/// Which entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Id {
    TimeOfDay,
    FreezeTime,
    InfiniteHealth,
    NoWanted,
    MaxMoney,
    InfiniteAmmo,
    /// One of the title's own cheat functions, by its number in the title's table
    /// (the loader runs it on the game thread; needs VICTORY_CHEATS=1).
    Title(u8),
}

/// How an entry is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// On or off.
    Toggle,
    /// Does something once when pressed.
    Action,
    /// One of several values (the hour).
    Choice,
}

/// An entry of the registry.
#[derive(Clone, Copy, Debug)]
pub struct Cheat {
    pub id: Id,
    pub name: &'static str,
    pub kind: Kind,
    /// Shown when it cannot be used yet.
    pub note: &'static str,
}

const NOT_FOUND: &str = "Address not found yet";
const NO_CLOCK: &str = "Start with VICTORY_STORY_CLOCK=1";
const NO_CHEATS: &str = "Start with VICTORY_CHEATS=1";

const fn title(n: u8, name: &'static str, kind: Kind) -> Cheat {
    Cheat { id: Id::Title(n), name, kind, note: NO_CHEATS }
}

/// Every entry, in the order of the page.
pub const CHEATS: [Cheat; 19] = [
    Cheat { id: Id::TimeOfDay, name: "TIME OF DAY", kind: Kind::Choice, note: NO_CLOCK },
    Cheat { id: Id::FreezeTime, name: "FREEZE TIME", kind: Kind::Toggle, note: NOT_FOUND },
    title(0, "CHEAT 0", Kind::Action),
    title(2, "CHEAT 2 (REFILL, UNSURE)", Kind::Action),
    title(3, "WANTED LEVEL UP (CHEAT 3)", Kind::Action),
    title(4, "WANTED DOWN + FLAG (CHEAT 4)", Kind::Toggle),
    title(6, "CHEAT 6 (NEXT WEATHER?)", Kind::Action),
    title(7, "SPAWN VEHICLE A", Kind::Action),
    title(8, "SPAWN VEHICLE B", Kind::Action),
    title(9, "SPAWN VEHICLE C", Kind::Action),
    title(10, "SPAWN VEHICLE D", Kind::Action),
    title(11, "SPAWN VEHICLE E", Kind::Action),
    title(13, "SPAWN VEHICLE F", Kind::Action),
    title(14, "SPAWN VEHICLE G", Kind::Action),
    title(15, "SPAWN VEHICLE H", Kind::Action),
    Cheat { id: Id::InfiniteHealth, name: "INFINITE HEALTH", kind: Kind::Toggle, note: NOT_FOUND },
    Cheat { id: Id::NoWanted, name: "NO WANTED LEVEL", kind: Kind::Toggle, note: NOT_FOUND },
    Cheat { id: Id::MaxMoney, name: "MAX MONEY", kind: Kind::Action, note: NOT_FOUND },
    Cheat { id: Id::InfiniteAmmo, name: "INFINITE AMMO", kind: Kind::Toggle, note: NOT_FOUND },
];

/// The entry `id`.
pub fn cheat(id: Id) -> &'static Cheat {
    CHEATS.iter().find(|c| c.id == id).unwrap_or(&CHEATS[0])
}

/// What makes an entry usable: reads its current value and writes a new one
/// (`write` answers whether it took).
#[derive(Clone, Copy)]
pub struct Backend {
    pub read: fn() -> Option<i64>,
    pub write: fn(i64) -> bool,
}

/// The game's clock: `get` gives (hour, minute), `set` takes them.
#[derive(Clone, Copy)]
struct Clock {
    get: fn() -> Option<(u8, u8)>,
    set: fn(u8, u8) -> bool,
    freeze: Option<fn(bool) -> bool>,
}

struct State {
    backends: [Option<Backend>; CHEATS.len()],
    clock: Option<Clock>,
    /// What each toggle was last set to by the menu.
    on: [bool; CHEATS.len()],
}

static STATE: Mutex<State> = Mutex::new(State { backends: [None; CHEATS.len()], clock: None, on: [false; CHEATS.len()] });

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn at(id: Id) -> usize {
    CHEATS.iter().position(|c| c.id == id).unwrap_or(0)
}

/// Makes the entry `id` usable through `backend`.
pub fn register(id: Id, backend: Backend) {
    state().backends[at(id)] = Some(backend);
}

/// Plugs in the game's clock: `get` its (hour, minute) (None while it is not
/// readable), `set` them (true when the game took them). Makes TIME OF DAY usable.
pub fn register_clock(get: fn() -> Option<(u8, u8)>, set: fn(u8, u8) -> bool) {
    let mut s = state();
    let freeze = s.clock.and_then(|c| c.freeze);
    s.clock = Some(Clock { get, set, freeze });
}

/// Adds the freeze to the clock: `freeze(true)` holds the time, `freeze(false)`
/// lets it run (true when the game took it). Makes FREEZE TIME usable once a
/// clock is registered.
pub fn register_clock_freeze(freeze: fn(bool) -> bool) {
    let mut s = state();
    if let Some(c) = s.clock.as_mut() {
        c.freeze = Some(freeze);
    } else {
        // Remembered with a clock that does nothing until the real one comes.
        s.clock = Some(Clock { get: || None, set: |_, _| false, freeze: Some(freeze) });
    }
}

/// Sets the game's hour (minute 0); false where there is no clock or it did not take.
/// Without a plugged-in clock the story clock's request is used (VICTORY_STORY_CLOCK=1).
pub fn set_hour(hour: u8) -> bool {
    let clock = state().clock;
    match clock {
        Some(c) => (c.set)(hour.min(23), 0),
        None => victory_core::story_clock::request(u32::from(hour.min(23)) * 60),
    }
}

/// The game's time, if a clock is plugged in (or the story clock runs) and readable.
pub fn time() -> Option<(u8, u8)> {
    let clock = state().clock;
    match clock {
        Some(c) => (c.get)(),
        None => victory_core::story_clock::current().map(|m| ((m / 60) as u8, (m % 60) as u8)),
    }
}

/// Whether `id` can be used now.
pub fn available(id: Id) -> bool {
    let s = state();
    match id {
        Id::TimeOfDay => match s.clock {
            Some(c) => (c.get)().is_some(),
            None => victory_core::story_clock::current().is_some(),
        },
        Id::Title(n) => victory_core::cheats::ready() && usize::from(n) < victory_core::cheats::COUNT,
        Id::FreezeTime => s.clock.is_some_and(|c| c.freeze.is_some()),
        _ => s.backends[at(id)].is_some(),
    }
}

/// The text shown as the entry's value.
pub fn value_text(id: Id) -> String {
    let entry = cheat(id);
    if !available(id) {
        return "N/A".to_owned();
    }
    match (id, entry.kind) {
        (Id::TimeOfDay, _) => time().map_or_else(|| "N/A".to_owned(), |(h, m)| format!("{h:02}:{m:02}")),
        (_, Kind::Toggle) => if state().on[at(id)] { "ON" } else { "OFF" }.to_owned(),
        (_, Kind::Action) => "RUN".to_owned(),
        (_, Kind::Choice) => {
            let read = state().backends[at(id)].map(|b| b.read);
            read.and_then(|r| r()).map_or_else(|| "N/A".to_owned(), |v| v.to_string())
        }
    }
}

/// Whether a toggle is on (the menu's last setting).
pub fn is_on(id: Id) -> bool {
    state().on[at(id)]
}

/// Presses the entry `id` one step `dir` (1 up or Enter, -1 down): a toggle
/// flips, an action runs, a choice moves (the hour by one, round the clock).
/// Err carries what to tell where it cannot be done.
pub fn step(id: Id, dir: i32) -> Result<(), &'static str> {
    let entry = cheat(id);
    if !available(id) {
        return Err(entry.note);
    }
    match id {
        Id::TimeOfDay => {
            let (hour, _) = time().ok_or(entry.note)?;
            let next = (i32::from(hour) + dir).rem_euclid(24) as u8;
            if set_hour(next) { Ok(()) } else { Err("The game did not take the time") }
        }
        Id::FreezeTime => {
            let (freeze, want) = {
                let s = state();
                (s.clock.and_then(|c| c.freeze), !s.on[at(id)])
            };
            let freeze = freeze.ok_or(entry.note)?;
            if freeze(want) {
                state().on[at(id)] = want;
                Ok(())
            } else {
                Err("The game did not take it")
            }
        }
        Id::Title(n) => {
            let toggle = entry.kind == Kind::Toggle;
            victory_core::cheats::queue(n)?;
            if toggle {
                let mut s = state();
                let i = at(id);
                s.on[i] = !s.on[i];
            }
            Ok(())
        }
        _ => {
            let backend = state().backends[at(id)].ok_or(entry.note)?;
            match entry.kind {
                Kind::Toggle => {
                    let want = !state().on[at(id)];
                    if (backend.write)(i64::from(want)) {
                        state().on[at(id)] = want;
                        Ok(())
                    } else {
                        Err("The game did not take it")
                    }
                }
                Kind::Action => if (backend.write)(0) { Ok(()) } else { Err("The game did not take it") },
                Kind::Choice => {
                    let now = (backend.read)().ok_or(entry.note)?;
                    if (backend.write)(now + i64::from(dir)) { Ok(()) } else { Err("The game did not take it") }
                }
            }
        }
    }
}

/// Puts the entry back to off (toggles that are on are switched off; the others stay).
pub fn reset(id: Id) {
    if cheat(id).kind == Kind::Toggle && is_on(id) && available(id) {
        let _ = step(id, -1);
    }
}

/// How many entries can be used now.
pub fn available_count() -> usize {
    CHEATS.iter().filter(|c| available(c.id)).count()
}

/// Forgets every registration (tests).
#[cfg(test)]
pub(crate) fn clear() {
    *state() = State { backends: [None; CHEATS.len()], clock: None, on: [false; CHEATS.len()] };
}

/// Tests that register plugs take turns (the registry is process-wide).
#[cfg(test)]
pub(crate) fn test_turn() -> std::sync::MutexGuard<'static, ()> {
    static TURN: Mutex<()> = Mutex::new(());
    TURN.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};

    static HOUR: AtomicU8 = AtomicU8::new(9);
    static HEALTH: AtomicI64 = AtomicI64::new(0);

    fn get() -> Option<(u8, u8)> {
        Some((HOUR.load(Ordering::Relaxed), 30))
    }

    fn set(h: u8, _m: u8) -> bool {
        HOUR.store(h, Ordering::Relaxed);
        true
    }

    #[test]
    fn the_registry_lists_its_entries_all_unavailable_at_first() {
        let _turn = test_turn();
        clear();
        let names: Vec<&str> = CHEATS.iter().map(|c| c.name).collect();
        assert_eq!(names.len(), 19);
        assert_eq!(&names[..3], ["TIME OF DAY", "FREEZE TIME", "CHEAT 0"]);
        assert_eq!(&names[15..], ["INFINITE HEALTH", "NO WANTED LEVEL", "MAX MONEY", "INFINITE AMMO"]);
        assert_eq!(available_count(), 0);
        for c in CHEATS {
            assert!(!available(c.id));
            assert_eq!(step(c.id, 1), Err(c.note));
            assert_eq!(value_text(c.id), "N/A");
        }
        assert_eq!(cheat(Id::InfiniteHealth).note, "Address not found yet");
        assert!(!set_hour(12));
        assert_eq!(cheat(Id::TimeOfDay).note, "Start with VICTORY_STORY_CLOCK=1");
        assert_eq!(cheat(Id::Title(7)).note, "Start with VICTORY_CHEATS=1");
    }

    #[test]
    fn the_title_cheats_queue_their_number_once_ready_and_cheat_4_toggles() {
        let _turn = test_turn();
        clear();
        victory_core::cheats::set_ready(false);
        assert!(!available(Id::Title(7)));
        assert_eq!(step(Id::Title(7), 1), Err("Start with VICTORY_CHEATS=1"));
        victory_core::cheats::set_ready(true);
        assert!(available(Id::Title(7)) && available(Id::Title(4)));
        assert_eq!(value_text(Id::Title(7)), "RUN");
        assert_eq!(step(Id::Title(7), 1), Ok(()));
        assert_eq!(value_text(Id::Title(4)), "OFF");
        assert_eq!(step(Id::Title(4), 1), Ok(()));
        assert_eq!(value_text(Id::Title(4)), "ON");
        let mut got = Vec::new();
        victory_core::cheats::drain(|n| got.push(n));
        assert_eq!(got, [4, 7]);
        victory_core::cheats::set_ready(false);
        clear();
    }

    #[test]
    fn a_clock_makes_the_time_of_day_usable_and_steps_round() {
        let _turn = test_turn();
        clear();
        register_clock(get, set);
        assert!(available(Id::TimeOfDay) && !available(Id::FreezeTime));
        assert_eq!(value_text(Id::TimeOfDay), "09:30");
        assert_eq!(step(Id::TimeOfDay, 1), Ok(()));
        assert_eq!(HOUR.load(Ordering::Relaxed), 10);
        HOUR.store(23, Ordering::Relaxed);
        assert_eq!(step(Id::TimeOfDay, 1), Ok(()));
        assert_eq!(HOUR.load(Ordering::Relaxed), 0);
        assert_eq!(step(Id::TimeOfDay, -1), Ok(()));
        assert_eq!(HOUR.load(Ordering::Relaxed), 23);
        assert!(set_hour(6) && HOUR.load(Ordering::Relaxed) == 6);
        clear();
    }

    #[test]
    fn the_freeze_needs_its_own_plug_and_a_toggle_remembers_its_state() {
        let _turn = test_turn();
        clear();
        register_clock(get, set);
        fn freeze(_on: bool) -> bool {
            true
        }
        register_clock_freeze(freeze);
        assert!(available(Id::FreezeTime));
        assert_eq!(value_text(Id::FreezeTime), "OFF");
        assert_eq!(step(Id::FreezeTime, 1), Ok(()));
        assert!(is_on(Id::FreezeTime));
        assert_eq!(value_text(Id::FreezeTime), "ON");
        reset(Id::FreezeTime);
        assert!(!is_on(Id::FreezeTime));
        clear();
    }

    #[test]
    fn a_backend_makes_any_entry_usable() {
        let _turn = test_turn();
        clear();
        fn read() -> Option<i64> {
            Some(HEALTH.load(Ordering::Relaxed))
        }
        fn write(v: i64) -> bool {
            HEALTH.store(v, Ordering::Relaxed);
            true
        }
        register(Id::InfiniteHealth, Backend { read, write });
        register(Id::MaxMoney, Backend { read, write: |_| true });
        assert_eq!(available_count(), 2);
        assert_eq!(step(Id::InfiniteHealth, 1), Ok(()));
        assert_eq!(HEALTH.load(Ordering::Relaxed), 1);
        assert_eq!(value_text(Id::InfiniteHealth), "ON");
        assert_eq!(step(Id::InfiniteHealth, 1), Ok(()));
        assert_eq!(HEALTH.load(Ordering::Relaxed), 0);
        assert_eq!(value_text(Id::MaxMoney), "RUN");
        assert_eq!(step(Id::MaxMoney, 1), Ok(()));
        assert!(!available(Id::NoWanted));
        clear();
    }
}
