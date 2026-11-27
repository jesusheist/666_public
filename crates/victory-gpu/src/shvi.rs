//! ScriptHook VI cheats on the CHEATS page ("SCRIPTHOOK" section).
//!
//! ScriptHook VI's core (`shvi_core.dll`, loaded through VICTORY_LOAD_DLLS) exports `shvi_get_api`: a
//! table of C functions. Natives may only run on the game's script thread, so the menu never calls
//! one. A press on a line queues a request; a per-frame callback registered with the table's
//! `tick_register` (it runs on the game thread, natives allowed) takes the requests and runs them.
//! Toggles that are on are applied again every so often from the same callback, because the game's
//! own scripts set the same state.
//!
//! The lines are the features of this table whose natives the running game binds (`native_exists`):
//! nothing is listed that cannot run. With no ScriptHook loaded the page shows one line saying so.

use std::sync::Mutex;

/// Which feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fid {
    God,
    NeverWanted,
    PoliceIgnore,
    KeepHealth,
    FastRun,
    NoRagdoll,
    Heal,
    Vitals,
    ClearWanted,
    SetWanted,
    RepairVehicle,
    Waypoint,
    Weather,
    TimePreset,
    /// A vehicle from [`VEHICLES`] (its place in the list).
    Spawn(u8),
}

/// How a feature is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Toggle,
    Action,
    /// One of several values (the labels); a press steps to the next and applies it.
    Choice(&'static [&'static str]),
}

/// Extra condition besides the natives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    None,
    /// Vehicle creation: ScriptHook must have it switched on (SHVI_EXPERIMENTAL_VEHICLE=1) and offer the script call and the pins.
    Spawn,
}

/// A line of the section.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Feature {
    pub fid: Fid,
    pub label: &'static str,
    pub kind: Kind,
    /// Every native it calls.
    pub needs: &'static [&'static str],
    pub desc: &'static str,
    /// A toggle that is on is applied again after this many milliseconds (0: every frame).
    pub every_ms: u64,
    pub gate: Gate,
}

/// Vehicles of the spawn lines: label and the game's 64-bit model key (ScriptHook's list of build 1.0.2.1).
pub(crate) const VEHICLES: [(&str, u64); 6] = [
    ("SULTAN", 0x17E4_7FD6_41C9_4862),
    ("BANSHEE", 0x4892_BE1F_9E76_DE37),
    ("BUFFALO", 0x4A10_7770_FE63_D53A),
    ("COMET 7", 0xF061_C096_18DF_81F2),
    ("INFERNUS 2", 0x5416_13F9_417C_82BA),
    ("RUINER", 0xFD75_43F3_1248_CA3A),
];

/// Weather types the game's scripts pass (hash, label); five have a known name.
pub(crate) const WEATHERS: [(u32, &str); 14] = [
    (0x614A_1F91, "SUNNY"),
    (0xBB89_8D2D, "OVERCAST"),
    (0xD61B_DE01, "FOG"),
    (0x7C1C_4A13, "THUNDERSTORM"),
    (0x320D_0951, "HURRICANE"),
    (0xCF3A_0F21, "OTHER 1"),
    (0x57F5_3C65, "OTHER 2"),
    (0x6563_BA05, "OTHER 3"),
    (0xF144_4588, "OTHER 4"),
    (0x9033_FF87, "OTHER 5"),
    (0xA073_0CC0, "OTHER 6"),
    (0xE606_165B, "OTHER 7"),
    (0x0365_3F47, "OTHER 8"),
    (0x5EBC_0DF0, "OTHER 9"),
];

const WEATHER_LABELS: &[&str] = &[
    "SUNNY", "OVERCAST", "FOG", "THUNDERSTORM", "HURRICANE", "OTHER 1", "OTHER 2", "OTHER 3", "OTHER 4", "OTHER 5", "OTHER 6", "OTHER 7", "OTHER 8", "OTHER 9",
];
/// Time presets: label and (hour, minute).
pub(crate) const TIMES: [(&str, i32, i32); 6] = [("DAWN 06:00", 6, 0), ("MORNING 09:00", 9, 0), ("NOON 12:00", 12, 0), ("AFTERNOON 16:00", 16, 0), ("DUSK 19:30", 19, 30), ("MIDNIGHT 00:00", 0, 0)];
const TIME_LABELS: &[&str] = &["DAWN 06:00", "MORNING 09:00", "NOON 12:00", "AFTERNOON 16:00", "DUSK 19:30", "MIDNIGHT 00:00"];
const WANTED_LABELS: &[&str] = &["0 STARS", "1 STAR", "2 STARS", "3 STARS", "4 STARS", "5 STARS"];

const fn feature(fid: Fid, label: &'static str, kind: Kind, needs: &'static [&'static str], desc: &'static str) -> Feature {
    Feature { fid, label, kind, needs, desc, every_ms: 750, gate: Gate::None }
}

const fn spawn(n: u8, label: &'static str) -> Feature {
    Feature {
        fid: Fid::Spawn(n),
        label,
        kind: Kind::Action,
        needs: &["PLAYER_PED_ID", "GET_ENTITY_COORDS", "GET_ENTITY_FORWARD_VECTOR", "GET_ENTITY_HEADING"],
        desc: "Creates the vehicle 6 m ahead of the player through ScriptHook (Enter).",
        every_ms: 0,
        gate: Gate::Spawn,
    }
}

/// How many features there are.
pub(crate) const COUNT: usize = 20;

/// Every feature, in the order of the section.
pub(crate) const FEATURES: [Feature; COUNT] = [
    feature(Fid::God, "GOD MODE", Kind::Toggle, &["PLAYER_PED_ID", "SET_ENTITY_INVINCIBLE"], "The player cannot die while on (re-applied every moment)."),
    feature(Fid::NeverWanted, "NEVER WANTED", Kind::Toggle, &["PLAYER_PED_ID", "SET_MAX_WANTED_LEVEL", "GET_PLAYER_WANTED_LEVEL", "CLEAR_PLAYER_WANTED_LEVEL"], "Caps the wanted level at none and clears any stars while on; off puts the cap back to 5."),
    feature(Fid::PoliceIgnore, "POLICE IGNORE PLAYER", Kind::Toggle, &["PLAYER_PED_ID", "SET_POLICE_IGNORE_PLAYER"], "The police pay no attention to the player while on."),
    feature(Fid::KeepHealth, "KEEP HEALTH FULL", Kind::Toggle, &["PLAYER_PED_ID", "GET_ENTITY_HEALTH", "GET_ENTITY_MAX_HEALTH", "SET_ENTITY_HEALTH"], "Tops the player's health up whenever it is below the maximum."),
    Feature {
        every_ms: 0,
        ..feature(Fid::FastRun, "FAST RUN", Kind::Toggle, &["PLAYER_PED_ID", "SET_PED_MOVE_RATE_OVERRIDE"], "Runs and walks 1.5 times faster while on.")
    },
    feature(Fid::NoRagdoll, "NO RAGDOLL", Kind::Toggle, &["PLAYER_PED_ID", "SET_PED_CAN_RAGDOLL"], "The player is not knocked over while on."),
    feature(Fid::Heal, "HEAL", Kind::Action, &["PLAYER_PED_ID", "GET_ENTITY_MAX_HEALTH", "SET_ENTITY_HEALTH"], "Sets the player's health to the maximum (Enter)."),
    feature(Fid::Vitals, "RESTORE ALL VITALS", Kind::Action, &["PLAYER_PED_ID", "VI_GET_PED_VITAL_MAX", "VI_SET_PED_VITAL"], "Fills the player's health, armour and the other vital pools (Enter)."),
    feature(Fid::ClearWanted, "CLEAR WANTED LEVEL", Kind::Action, &["PLAYER_PED_ID", "CLEAR_PLAYER_WANTED_LEVEL"], "Takes away every star (Enter)."),
    feature(Fid::SetWanted, "SET WANTED LEVEL", Kind::Choice(WANTED_LABELS), &["PLAYER_PED_ID", "CLEAR_PLAYER_WANTED_LEVEL", "VI_RAISE_PLAYER_WANTED_LEVEL"], "Sets the stars (Left, Right). ScriptHook has not shown that the raise works in the game yet."),
    feature(Fid::RepairVehicle, "REPAIR VEHICLE", Kind::Action, &["PLAYER_PED_ID", "GET_VEHICLE_PED_IS_USING", "SET_VEHICLE_FIXED", "SET_VEHICLE_ENGINE_HEALTH", "SET_VEHICLE_BODY_HEALTH", "SET_VEHICLE_PETROL_TANK_HEALTH"], "Repairs the vehicle the player is in (Enter)."),
    feature(Fid::Waypoint, "TELEPORT TO WAYPOINT", Kind::Action, &["PLAYER_PED_ID", "IS_WAYPOINT_ACTIVE", "_GET_WAYPOINT_COORDS", "GET_ENTITY_COORDS", "SET_ENTITY_COORDS", "GET_VEHICLE_PED_IS_USING"], "Moves the player, or the vehicle, to the map waypoint (Enter). The waypoint's name is a guess in ScriptHook."),
    feature(Fid::Weather, "WEATHER", Kind::Choice(WEATHER_LABELS), &["SET_WEATHER_TYPE"], "Sets the weather: Right is the next type, Left the one before."),
    feature(Fid::TimePreset, "TIME OF DAY (SCRIPTHOOK)", Kind::Choice(TIME_LABELS), &["SET_CLOCK_TIME"], "Sets the game's clock to a time of day (Left, Right)."),
    spawn(0, "SPAWN SULTAN"),
    spawn(1, "SPAWN BANSHEE"),
    spawn(2, "SPAWN BUFFALO"),
    spawn(3, "SPAWN COMET 7"),
    spawn(4, "SPAWN INFERNUS 2"),
    spawn(5, "SPAWN RUINER"),
];

/// The engine hash ScriptHook binds a native under: `0x5649 << 48` plus the case-folded
/// one-at-a-time hash of its name (`shvi_engine_hash` of the C header).
pub(crate) fn engine_hash(name: &str) -> u64 {
    let mut h: u32 = 0;
    for b in name.bytes() {
        let c = match b {
            b'A'..=b'Z' => b + 32,
            b'\\' => b'/',
            other => other,
        };
        h = h.wrapping_add(u32::from(c));
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h = h.wrapping_add(h << 15);
    0x5649_0000_0000_0000 | u64::from(h)
}

/// What the cheats need from ScriptHook. The real one is the C table; the tests use a fake.
pub(crate) trait Host {
    /// Whether the running game binds the native.
    fn exists(&self, name: &str) -> bool;
    /// Calls a native (game thread only): the cells it takes, the first four cells it returns.
    fn call(&self, name: &str, args: &[u64]) -> [u64; 4];
    /// Whether vehicle creation can be asked for.
    fn can_spawn(&self) -> bool;
    /// Runs a script function (program name hash, code offset, arguments); the cell it returns.
    fn script_call(&self, program: u32, offset: u32, args: &[u64]) -> Result<u64, i32>;
    /// Keeps a script object alive; the id to give back to `unpin`.
    fn pin(&self, object: u64) -> Option<u64>;
    fn unpin(&self, pin: u64);
    /// Writes a line to ScriptHook's log.
    fn log(&self, text: &str);
}

/// The features whose natives are bound and whose gate is open.
pub(crate) fn available(host: &dyn Host) -> Vec<Feature> {
    FEATURES
        .iter()
        .filter(|f| f.needs.iter().all(|n| host.exists(n)) && (f.gate != Gate::Spawn || (host.can_spawn() && host.exists("VI_CREATE_VEHICLE_ASYNC"))))
        .copied()
        .collect()
}

fn index_of(fid: Fid) -> usize {
    FEATURES.iter().position(|f| f.fid == fid).unwrap_or(0)
}

/// The feature `fid`.
pub(crate) fn feature_of(fid: Fid) -> &'static Feature {
    &FEATURES[index_of(fid)]
}

/// A request for the game thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub fid: Fid,
    /// A toggle: 1 on, 0 off. A choice: the place of the label. An action: 0.
    pub value: i32,
}

struct State {
    on: [bool; COUNT],
    /// The label last chosen (-1: none yet).
    choice: [i32; COUNT],
    queue: Vec<Request>,
    /// When a toggle was last applied (ms).
    applied: [u64; COUNT],
    /// What the last request did, for the line under the list.
    status: String,
    /// Tasks held alive after a vehicle request: (pin, until ms).
    pins: Vec<(u64, u64)>,
}

static STATE: Mutex<State> = Mutex::new(State { on: [false; COUNT], choice: [-1; COUNT], queue: Vec::new(), applied: [0; COUNT], status: String::new(), pins: Vec::new() });

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The text shown as the line's value.
pub(crate) fn value_text(fid: Fid) -> String {
    let f = feature_of(fid);
    let i = index_of(fid);
    let s = state();
    match f.kind {
        Kind::Toggle => if s.on[i] { "ON" } else { "OFF" }.to_owned(),
        Kind::Action => "RUN".to_owned(),
        Kind::Choice(labels) => usize::try_from(s.choice[i]).ok().and_then(|c| labels.get(c)).map_or_else(|| "PICK".to_owned(), |l| (*l).to_owned()),
    }
}

/// Whether the line differs from its start (a toggle on, a value chosen).
pub(crate) fn is_changed(fid: Fid) -> bool {
    let i = index_of(fid);
    let s = state();
    s.on[i] || s.choice[i] >= 0
}

/// What the last request did.
pub(crate) fn status() -> String {
    state().status.clone()
}

/// Presses the line: a toggle flips, an action runs, a choice steps `dir` (1 next, -1 before). The
/// request waits for the game thread; Err is what to tell where nothing was queued.
pub(crate) fn step(fid: Fid, dir: i32) -> Result<(), &'static str> {
    if !ready() {
        return Err("ScriptHook is not running");
    }
    let f = feature_of(fid);
    let i = index_of(fid);
    let mut s = state();
    let value = match f.kind {
        Kind::Toggle => {
            s.on[i] = !s.on[i];
            i32::from(s.on[i])
        }
        Kind::Action => 0,
        Kind::Choice(labels) => {
            let n = labels.len() as i32;
            let next = if s.choice[i] < 0 { if dir < 0 { n - 1 } else { 0 } } else { (s.choice[i] + dir.signum()).rem_euclid(n) };
            s.choice[i] = next;
            next
        }
    };
    s.queue.push(Request { fid, value });
    Ok(())
}

/// Puts the line back to its start: a toggle that is on is switched off (the request runs once), a choice is forgotten.
pub(crate) fn reset(fid: Fid) {
    let i = index_of(fid);
    let mut s = state();
    match feature_of(fid).kind {
        Kind::Toggle if s.on[i] => {
            s.on[i] = false;
            s.queue.push(Request { fid, value: 0 });
        }
        Kind::Choice(_) => s.choice[i] = -1,
        _ => {}
    }
}

/// Runs one request. Err says why nothing happened.
pub(crate) fn apply(host: &dyn Host, req: Request, now: u64) -> Result<(), String> {
    let ped = player(host)?;
    let on = req.value != 0;
    let f = |x: f32| u64::from(x.to_bits());
    let int = |x: i32| i64::from(x) as u64;
    let flag = |b: bool| u64::from(b);
    let float_of = |cells: [u64; 4]| f32::from_bits(cells[0] as u32);
    let max_health = |host: &dyn Host| int_of(host.call("GET_ENTITY_MAX_HEALTH", &[ped[0], ped[1], 0])[0]);
    match req.fid {
        Fid::God => {
            host.call("SET_ENTITY_INVINCIBLE", &[ped[0], ped[1], flag(on)]);
        }
        Fid::NeverWanted => {
            if on {
                host.call("SET_MAX_WANTED_LEVEL", &[int(0)]);
                if int_of(host.call("GET_PLAYER_WANTED_LEVEL", &[ped[0], ped[1]])[0]) > 0 {
                    host.call("CLEAR_PLAYER_WANTED_LEVEL", &[ped[0], ped[1]]);
                }
            } else {
                // The stock story maximum.
                host.call("SET_MAX_WANTED_LEVEL", &[int(5)]);
            }
        }
        Fid::PoliceIgnore => {
            host.call("SET_POLICE_IGNORE_PLAYER", &[ped[0], ped[1], flag(on)]);
        }
        Fid::KeepHealth => {
            if on {
                let max = max_health(host);
                if int_of(host.call("GET_ENTITY_HEALTH", &[ped[0], ped[1]])[0]) < max {
                    host.call("SET_ENTITY_HEALTH", &[ped[0], ped[1], int(max), 0, 0, 0, 0]);
                }
            }
        }
        Fid::FastRun => {
            host.call("SET_PED_MOVE_RATE_OVERRIDE", &[ped[0], ped[1], f(if on { 1.5 } else { 1.0 })]);
        }
        Fid::NoRagdoll => {
            host.call("SET_PED_CAN_RAGDOLL", &[ped[0], ped[1], flag(!on)]);
        }
        Fid::Heal => {
            let max = max_health(host);
            host.call("SET_ENTITY_HEALTH", &[ped[0], ped[1], int(max), 0, 0, 0, 0]);
        }
        Fid::Vitals => {
            // The engine accepts pool 0 to 5.
            for pool in 0..6 {
                let max = float_of(host.call("VI_GET_PED_VITAL_MAX", &[ped[0], ped[1], int(pool)]));
                if max > 0.0 {
                    host.call("VI_SET_PED_VITAL", &[ped[0], ped[1], f(max), int(pool)]);
                }
            }
        }
        Fid::ClearWanted => {
            host.call("CLEAR_PLAYER_WANTED_LEVEL", &[ped[0], ped[1]]);
        }
        Fid::SetWanted => {
            // The raise only raises, so the stars are cleared first.
            host.call("CLEAR_PLAYER_WANTED_LEVEL", &[ped[0], ped[1]]);
            if req.value > 0 {
                host.call("VI_RAISE_PLAYER_WANTED_LEVEL", &[ped[0], ped[1], int(req.value)]);
            }
        }
        Fid::RepairVehicle => {
            let veh = host.call("GET_VEHICLE_PED_IS_USING", &[ped[0], ped[1], 0]);
            if veh[0] == 0 && veh[1] == 0 {
                return Err("Not in a vehicle".to_owned());
            }
            host.call("SET_VEHICLE_FIXED", &[veh[0], veh[1]]);
            // The healths run 0 to 1 in this game.
            for native in ["SET_VEHICLE_ENGINE_HEALTH", "SET_VEHICLE_BODY_HEALTH", "SET_VEHICLE_PETROL_TANK_HEALTH"] {
                host.call(native, &[veh[0], veh[1], f(1.0)]);
            }
        }
        Fid::Waypoint => {
            if host.call("IS_WAYPOINT_ACTIVE", &[])[0] & 0xFF == 0 {
                return Err("No waypoint on the map".to_owned());
            }
            let w = host.call("_GET_WAYPOINT_COORDS", &[]);
            let veh = host.call("GET_VEHICLE_PED_IS_USING", &[ped[0], ped[1], 0]);
            let target = if veh[0] != 0 || veh[1] != 0 { [veh[0], veh[1]] } else { ped };
            let here = host.call("GET_ENTITY_COORDS", &[target[0], target[1], 1]);
            let z = f32::from_bits(w[1] as u32);
            let z = if z != 0.0 { z } else { f32::from_bits(here[1] as u32) };
            host.call("SET_ENTITY_COORDS", &[target[0], target[1], w[0], u64::from(z.to_bits()), 0, 0, 0, 0, f(-1.0)]);
        }
        Fid::Weather => {
            let (hash, _) = WEATHERS.get(req.value as usize).ok_or("No such weather")?;
            // The form the game's scripts use most: (hash, 1, 1, 0, seconds 0, 0, 1).
            host.call("SET_WEATHER_TYPE", &[u64::from(*hash), 1, 1, 0, f(0.0), 0, 1]);
        }
        Fid::TimePreset => {
            let (_, hour, minute) = TIMES.get(req.value as usize).ok_or("No such time")?;
            host.call("SET_CLOCK_TIME", &[int(*hour), int(*minute), 0, 0]);
        }
        Fid::Spawn(n) => spawn_vehicle(host, ped, n, now)?,
    }
    Ok(())
}

fn int_of(cell: u64) -> i32 {
    cell as u32 as i32
}

fn player(host: &dyn Host) -> Result<[u64; 2], String> {
    let r = host.call("PLAYER_PED_ID", &[]);
    if r[0] == 0 && r[1] == 0 { Err("No player yet".to_owned()) } else { Ok([r[0], r[1]]) }
}

/// ScriptHook's own wrapper of the creation native (program of the "native" module, sentinel code offset).
const NATIVE_PROGRAM: u32 = 0x4839_1B87;
const CREATE_VEHICLE_OFFSET: u32 = 0xFFFF_FF01;
/// How long the creation task is held alive, in ms.
const PIN_MS: u64 = 10_000;

fn spawn_vehicle(host: &dyn Host, ped: [u64; 2], n: u8, now: u64) -> Result<(), String> {
    let (_, model) = *VEHICLES.get(usize::from(n)).ok_or("No such vehicle")?;
    let at = host.call("GET_ENTITY_COORDS", &[ped[0], ped[1], 1]);
    let dir = host.call("GET_ENTITY_FORWARD_VECTOR", &[ped[0], ped[1]]);
    let heading = host.call("GET_ENTITY_HEADING", &[ped[0], ped[1]])[0] as u32;
    let x = f32::from_bits(at[0] as u32) + f32::from_bits(dir[0] as u32) * 6.0;
    let y = f32::from_bits((at[0] >> 32) as u32) + f32::from_bits((dir[0] >> 32) as u32) * 6.0;
    let z = f32::from_bits(at[1] as u32);
    let mut args = [0u64; 13];
    args[0] = model;
    args[3] = 0xFFFF_FFFF << 32;
    args[4] = u64::from(x.to_bits()) | (u64::from(y.to_bits()) << 32);
    args[5] = u64::from(z.to_bits());
    args[6] = u64::from(heading);
    args[7] = 1 << 16;
    args[8] = 1 << 40;
    args[10] = u64::from((-1.0f32).to_bits()) << 32;
    let task = host.script_call(NATIVE_PROGRAM, CREATE_VEHICLE_OFFSET, &args).map_err(|rc| format!("ScriptHook refused the vehicle (status {rc})"))?;
    if task == 0 {
        return Err("ScriptHook gave no task".to_owned());
    }
    // The script runtime drops a task nothing refers to within a few frames; the request dies with it.
    match host.pin(task) {
        Some(pin) => {
            state().pins.push((pin, now + PIN_MS));
            Ok(())
        }
        None => Err("The vehicle task could not be held".to_owned()),
    }
}

/// One frame on the game thread: runs what was asked, applies the toggles that are on again, lets go of old tasks.
pub(crate) fn tick(host: &dyn Host, now: u64) {
    let (requests, due, expired) = {
        let mut s = state();
        let requests = std::mem::take(&mut s.queue);
        let mut due = Vec::new();
        for (i, f) in FEATURES.iter().enumerate() {
            if f.kind != Kind::Toggle {
                continue;
            }
            if requests.iter().any(|r| r.fid == f.fid) {
                s.applied[i] = now;
            } else if s.on[i] && now.saturating_sub(s.applied[i]) >= f.every_ms {
                s.applied[i] = now;
                due.push(Request { fid: f.fid, value: 1 });
            }
        }
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut s.pins).into_iter().partition(|(_, until)| *until <= now);
        s.pins = kept;
        (requests, due, gone)
    };
    for (pin, _) in expired {
        host.unpin(pin);
    }
    // A press is logged; the repeat of a toggle only when it fails.
    for (req, repeat) in requests.into_iter().map(|r| (r, false)).chain(due.into_iter().map(|r| (r, true))) {
        let label = feature_of(req.fid).label;
        let result = apply(host, req, now);
        let text = match &result {
            Ok(()) => format!("{label}: done"),
            Err(why) => format!("{label}: {why}"),
        };
        if !repeat || result.is_err() {
            host.log(&format!("victory cheats: {text}"));
        }
        if !repeat || result.is_err() {
            state().status = text;
        }
    }
}

// ---------------------------------------------------------------------------
// The real ScriptHook (Windows): found through its loaded module, called through its C table.

#[cfg(windows)]
mod real {
    use super::{Host, engine_hash};
    use std::ffi::{CString, c_char, c_void};
    use std::sync::Mutex;
    use std::time::Instant;

    /// `shvi_api` of ABI 1.7 (shvi.h): append-only, so a prefix is enough.
    #[repr(C)]
    pub(super) struct Api {
        size: u32,
        abi_major: u16,
        abi_minor: u16,
        log: unsafe extern "C" fn(*const c_char),
        game_name: usize,
        game_build: usize,
        is_blocked: unsafe extern "C" fn() -> i32,
        script_register: usize,
        module_unregister: usize,
        script_wait: usize,
        native_init: unsafe extern "C" fn(u64),
        native_push64: unsafe extern "C" fn(u64),
        native_call: unsafe extern "C" fn() -> *const u64,
        global_ptr: usize,
        keyboard_register: usize,
        tick_register: unsafe extern "C" fn(*mut c_void, unsafe extern "C" fn(*mut c_void), *mut c_void) -> i32,
        world_get_all: usize,
        entity_ptr: usize,
        static_ptr: usize,
        present_register: usize,
        present_unregister: usize,
        native_exists: unsafe extern "C" fn(u64) -> i32,
        script_function_call: unsafe extern "C" fn(u32, u32, *const u64, u32, *mut u64, u32) -> i32,
        script_object_pin: unsafe extern "C" fn(u64, *mut u64) -> i32,
        script_object_unpin: unsafe extern "C" fn(u64) -> i32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }

    /// The table, once found and the frame callback registered.
    static FOUND: Mutex<Option<usize>> = Mutex::new(None);
    /// Any address of this program: the owner the callback is registered under.
    static ANCHOR: u8 = 0;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// The names of ScriptHook's core as loaded: its usual file name and every file name of VICTORY_LOAD_DLLS.
    fn module_names() -> Vec<String> {
        let mut names = vec!["shvi_core.dll".to_owned()];
        if let Ok(list) = std::env::var("VICTORY_LOAD_DLLS") {
            for path in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
                if let Some(name) = path.rsplit(['\\', '/']).next()
                    && !names.iter().any(|n| n.eq_ignore_ascii_case(name))
                {
                    names.push(name.to_owned());
                }
            }
        }
        names
    }

    fn find() -> Option<&'static Api> {
        for name in module_names() {
            let w = wide(&name);
            // SAFETY: a NUL-terminated name; the module stays loaded for the run.
            let module = unsafe { GetModuleHandleW(w.as_ptr()) };
            if module.is_null() {
                continue;
            }
            // SAFETY: a NUL-terminated name and a loaded module.
            let entry = unsafe { GetProcAddress(module, c"shvi_get_api".as_ptr().cast()) };
            if entry.is_null() {
                continue;
            }
            // SAFETY: the export's documented signature (ABI major in, the table out).
            let get: unsafe extern "C" fn(u32) -> *const Api = unsafe { std::mem::transmute(entry) };
            // SAFETY: as above.
            let api = unsafe { get(1) };
            // SAFETY: a table of the host's, valid for the run; `size` says how much of it exists.
            let api = unsafe { api.as_ref() }?;
            if api.abi_major == 1 && api.size as usize >= std::mem::size_of::<Api>() {
                return Some(api);
            }
        }
        None
    }

    /// The table, found and with the frame callback registered (once); None where ScriptHook is not loaded.
    pub(super) fn api() -> Option<&'static Api> {
        let mut found = FOUND.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = *found {
            // SAFETY: stored from a reference to the host's table, which lives for the run.
            return Some(unsafe { &*(at as *const Api) });
        }
        let api = find()?;
        // SAFETY: the table's function; the owner is an address of this program, the callback never unwinds.
        let rc = unsafe { (api.tick_register)(std::ptr::addr_of!(ANCHOR).cast_mut().cast(), frame, std::ptr::null_mut()) };
        if rc != 0 {
            return None;
        }
        *found = Some(std::ptr::from_ref(api) as usize);
        Some(api)
    }

    fn now_ms() -> u64 {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    /// The per-frame callback, on the game thread.
    unsafe extern "C" fn frame(_: *mut c_void) {
        let Some(at) = *FOUND.lock().unwrap_or_else(|e| e.into_inner()) else { return };
        // SAFETY: see `api`.
        let api = unsafe { &*(at as *const Api) };
        // SAFETY: the table's function.
        if unsafe { (api.is_blocked)() } != 0 {
            return;
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| super::tick(&Real(api), now_ms())));
    }

    pub(super) struct Real(pub &'static Api);

    impl Host for Real {
        fn exists(&self, name: &str) -> bool {
            // SAFETY: the table's function, callable from any thread.
            unsafe { (self.0.native_exists)(engine_hash(name)) != 0 }
        }

        fn call(&self, name: &str, args: &[u64]) -> [u64; 4] {
            let mut out = [0u64; 4];
            // SAFETY: only reached from the frame callback, the game thread the table's natives need.
            unsafe {
                (self.0.native_init)(engine_hash(name));
                for a in args {
                    (self.0.native_push64)(*a);
                }
                let r = (self.0.native_call)();
                if !r.is_null() {
                    // The return slots hold at least four cells (the C++ wrappers read up to 32 bytes).
                    out.copy_from_slice(std::slice::from_raw_parts(r, 4));
                }
            }
            out
        }

        fn can_spawn(&self) -> bool {
            std::env::var("SHVI_EXPERIMENTAL_VEHICLE").is_ok_and(|v| v == "1")
        }

        fn script_call(&self, program: u32, offset: u32, args: &[u64]) -> Result<u64, i32> {
            let mut ret = [0u64; 1];
            // SAFETY: the frame callback is a script callback; the arrays outlive the call.
            let rc = unsafe { (self.0.script_function_call)(program, offset, args.as_ptr(), args.len() as u32, ret.as_mut_ptr(), 1) };
            if rc == 0 { Ok(ret[0]) } else { Err(rc) }
        }

        fn pin(&self, object: u64) -> Option<u64> {
            let mut pin = 0u64;
            // SAFETY: a script callback; `pin` outlives the call.
            let rc = unsafe { (self.0.script_object_pin)(object, &mut pin) };
            (rc == 0).then_some(pin)
        }

        fn unpin(&self, pin: u64) {
            // SAFETY: a script callback.
            unsafe { (self.0.script_object_unpin)(pin) };
        }

        fn log(&self, text: &str) {
            if let Ok(c) = CString::new(text.replace('\0', " ")) {
                // SAFETY: a NUL-terminated string.
                unsafe { (self.0.log)(c.as_ptr()) };
            }
        }
    }
}

#[cfg(test)]
fn test_lines() -> Option<Vec<Feature>> {
    test_host::lines()
}
#[cfg(not(test))]
fn test_lines() -> Option<Vec<Feature>> {
    None
}

/// Whether ScriptHook is loaded and its frame callback is in (looks for it each time until found).
pub(crate) fn ready() -> bool {
    test_lines().is_some() || found()
}

#[cfg(windows)]
fn found() -> bool {
    real::api().is_some()
}

#[cfg(not(windows))]
fn found() -> bool {
    false
}

/// The lines to show: None where ScriptHook is not loaded, else the features whose natives are bound.
pub(crate) fn lines() -> Option<Vec<Feature>> {
    if let Some(list) = test_lines() {
        return Some(list);
    }
    found_lines()
}

#[cfg(windows)]
fn found_lines() -> Option<Vec<Feature>> {
    real::api().map(|api| available(&real::Real(api)))
}

#[cfg(not(windows))]
fn found_lines() -> Option<Vec<Feature>> {
    None
}

/// The page's switch for the tests: pretend ScriptHook is loaded with these lines.
#[cfg(test)]
pub(crate) mod test_host {
    use super::{Feature, FEATURES};
    use std::sync::Mutex;

    static LINES: Mutex<Option<Vec<Feature>>> = Mutex::new(None);

    pub(crate) fn lines() -> Option<Vec<Feature>> {
        LINES.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Pretends ScriptHook is loaded and offers `n` of the features (all when `n` is large).
    pub(crate) fn set(n: Option<usize>) {
        *LINES.lock().unwrap_or_else(|e| e.into_inner()) = n.map(|n| FEATURES.iter().take(n).copied().collect());
    }

    /// Clears what a test did to the shared state.
    pub(crate) fn reset_state() {
        let mut s = super::state();
        s.on = [false; super::COUNT];
        s.choice = [-1; super::COUNT];
        s.queue.clear();
        s.applied = [0; super::COUNT];
        s.status.clear();
        s.pins.clear();
    }

    /// Tests that touch the shared state take turns.
    pub(crate) fn turn() -> std::sync::MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        TURN.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::test_host::{reset_state, set, turn};
    use super::*;
    use std::cell::RefCell;

    /// A fake ScriptHook: records the calls, answers what it is told.
    struct Fake {
        calls: RefCell<Vec<(String, Vec<u64>)>>,
        missing: Vec<&'static str>,
        spawn: bool,
        wanted: u64,
        in_vehicle: bool,
        waypoint: bool,
        pins: RefCell<Vec<u64>>,
        unpinned: RefCell<Vec<u64>>,
        logs: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake { calls: RefCell::default(), missing: vec![], spawn: true, wanted: 0, in_vehicle: true, waypoint: true, pins: RefCell::default(), unpinned: RefCell::default(), logs: RefCell::default() }
        }
        fn names(&self) -> Vec<String> {
            self.calls.borrow().iter().map(|(n, _)| n.clone()).collect()
        }
        fn args_of(&self, name: &str) -> Vec<u64> {
            self.calls.borrow().iter().find(|(n, _)| n == name).map(|(_, a)| a.clone()).unwrap_or_default()
        }
    }

    impl Host for Fake {
        fn exists(&self, name: &str) -> bool {
            !self.missing.contains(&name)
        }
        fn call(&self, name: &str, args: &[u64]) -> [u64; 4] {
            self.calls.borrow_mut().push((name.to_owned(), args.to_vec()));
            match name {
                "PLAYER_PED_ID" => [7, 8, 0, 0],
                "GET_ENTITY_MAX_HEALTH" => [200, 0, 0, 0],
                "GET_ENTITY_HEALTH" => [150, 0, 0, 0],
                "GET_PLAYER_WANTED_LEVEL" => [self.wanted, 0, 0, 0],
                "GET_VEHICLE_PED_IS_USING" => if self.in_vehicle { [3, 4, 0, 0] } else { [0; 4] },
                "IS_WAYPOINT_ACTIVE" => [u64::from(self.waypoint), 0, 0, 0],
                "_GET_WAYPOINT_COORDS" => [u64::from(100.0f32.to_bits()) | (u64::from(200.0f32.to_bits()) << 32), 0, 0, 0],
                "GET_ENTITY_COORDS" => [u64::from(1.0f32.to_bits()) | (u64::from(2.0f32.to_bits()) << 32), u64::from(30.0f32.to_bits()), 0, 0],
                "GET_ENTITY_FORWARD_VECTOR" => [u64::from(1.0f32.to_bits()), 0, 0, 0],
                "GET_ENTITY_HEADING" => [u64::from(0.5f32.to_bits()), 0, 0, 0],
                "VI_GET_PED_VITAL_MAX" => [u64::from(if args[2] < 2 { 100.0f32 } else { -1.0 }.to_bits()), 0, 0, 0],
                _ => [0; 4],
            }
        }
        fn can_spawn(&self) -> bool {
            self.spawn
        }
        fn script_call(&self, program: u32, offset: u32, args: &[u64]) -> Result<u64, i32> {
            self.calls.borrow_mut().push((format!("script {program:X} {offset:X}"), args.to_vec()));
            Ok(0x1234)
        }
        fn pin(&self, object: u64) -> Option<u64> {
            self.pins.borrow_mut().push(object);
            Some(55)
        }
        fn unpin(&self, pin: u64) {
            self.unpinned.borrow_mut().push(pin);
        }
        fn log(&self, text: &str) {
            self.logs.borrow_mut().push(text.to_owned());
        }
    }

    #[test]
    fn the_engine_hash_is_the_tagged_case_folded_one_at_a_time_hash() {
        // The vector of ScriptHook's own tests.
        assert_eq!(engine_hash("adder"), 0x5649_0000_B779_A091);
        assert_eq!(engine_hash("ADDER"), engine_hash("adder"));
        assert_eq!(engine_hash("x") >> 48, 0x5649);
    }

    #[test]
    fn the_table_has_unique_features_and_every_choice_has_its_labels() {
        for (i, a) in FEATURES.iter().enumerate() {
            assert!(!a.needs.is_empty() && !a.desc.is_empty() && !a.label.is_empty());
            assert!(FEATURES.iter().skip(i + 1).all(|b| b.fid != a.fid && b.label != a.label), "{}", a.label);
            assert!(a.label.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == ' ' || c == '+' || c == '(' || c == ')'), "{}", a.label);
        }
        assert_eq!(WEATHER_LABELS.len(), WEATHERS.len());
        assert!(WEATHERS.iter().zip(WEATHER_LABELS).all(|((_, a), b)| a == b));
        assert!(TIMES.iter().zip(TIME_LABELS).all(|((a, ..), b)| a == b));
        assert_eq!(WANTED_LABELS.len(), 6);
        assert_eq!(FEATURES.iter().filter(|f| f.gate == Gate::Spawn).count(), VEHICLES.len());
        assert!(FEATURES.iter().all(|f| f.label.len() <= 32));
        assert_eq!(index_of(Fid::Weather), 12);
    }

    #[test]
    fn only_features_whose_natives_are_bound_are_listed() {
        let mut fake = Fake::new();
        assert_eq!(available(&fake).len(), COUNT);
        fake.missing = vec!["SET_WEATHER_TYPE", "SET_VEHICLE_FIXED"];
        let labels: Vec<&str> = available(&fake).iter().map(|f| f.label).collect();
        assert!(!labels.contains(&"WEATHER") && !labels.contains(&"REPAIR VEHICLE") && labels.contains(&"GOD MODE"));
        // Vehicle creation needs ScriptHook's switch and its creation native.
        let mut fake = Fake::new();
        fake.spawn = false;
        assert_eq!(available(&fake).len(), COUNT - VEHICLES.len());
        let mut fake = Fake::new();
        fake.missing = vec!["VI_CREATE_VEHICLE_ASYNC"];
        assert_eq!(available(&fake).len(), COUNT - VEHICLES.len());
    }

    #[test]
    fn a_press_queues_and_the_game_thread_runs_it() {
        let _turn = turn();
        reset_state();
        set(Some(COUNT));
        assert_eq!(value_text(Fid::God), "OFF");
        assert_eq!(step(Fid::God, 1), Ok(()));
        assert_eq!(value_text(Fid::God), "ON");
        assert!(is_changed(Fid::God));
        let fake = Fake::new();
        // Nothing ran until the game thread's frame.
        assert!(fake.names().is_empty());
        tick(&fake, 1000);
        assert_eq!(fake.args_of("SET_ENTITY_INVINCIBLE"), [7, 8, 1]);
        assert_eq!(status(), "GOD MODE: done");
        // Switching off applies the off value once, and nothing is repeated after.
        assert_eq!(step(Fid::God, 1), Ok(()));
        let fake = Fake::new();
        tick(&fake, 1100);
        assert_eq!(fake.args_of("SET_ENTITY_INVINCIBLE"), [7, 8, 0]);
        let fake = Fake::new();
        tick(&fake, 5000);
        assert!(fake.names().is_empty());
        reset_state();
        set(None);
    }

    #[test]
    fn a_toggle_that_is_on_is_applied_again_on_its_interval() {
        let _turn = turn();
        reset_state();
        set(Some(COUNT));
        step(Fid::God, 1).unwrap();
        step(Fid::FastRun, 1).unwrap();
        let fake = Fake::new();
        tick(&fake, 10_000);
        assert_eq!(fake.names().iter().filter(|n| *n == "SET_ENTITY_INVINCIBLE").count(), 1);
        // 100 ms later: the every-frame one only.
        let fake = Fake::new();
        tick(&fake, 10_100);
        assert_eq!(fake.names().iter().filter(|n| *n == "SET_ENTITY_INVINCIBLE").count(), 0);
        assert_eq!(fake.args_of("SET_PED_MOVE_RATE_OVERRIDE"), [7, 8, u64::from(1.5f32.to_bits())]);
        // After the interval it comes again.
        let fake = Fake::new();
        tick(&fake, 10_800);
        assert_eq!(fake.names().iter().filter(|n| *n == "SET_ENTITY_INVINCIBLE").count(), 1);
        reset_state();
        set(None);
    }

    #[test]
    fn a_choice_starts_unset_steps_round_and_applies_each_step() {
        let _turn = turn();
        reset_state();
        set(Some(COUNT));
        assert_eq!(value_text(Fid::Weather), "PICK");
        assert!(!is_changed(Fid::Weather));
        step(Fid::Weather, 1).unwrap();
        assert_eq!(value_text(Fid::Weather), "SUNNY");
        step(Fid::Weather, -1).unwrap();
        assert_eq!(value_text(Fid::Weather), "OTHER 9");
        step(Fid::Weather, 1).unwrap();
        assert_eq!(value_text(Fid::Weather), "SUNNY");
        let fake = Fake::new();
        tick(&fake, 1);
        let weather: Vec<Vec<u64>> = fake.calls.borrow().iter().filter(|(n, _)| n == "SET_WEATHER_TYPE").map(|(_, a)| a.clone()).collect();
        assert_eq!(weather.len(), 3);
        assert_eq!(weather[0][0], 0x614A_1F91);
        assert_eq!(weather[1][0], 0x5EBC_0DF0);
        assert_eq!(weather[0].len(), 7);
        reset(Fid::Weather);
        assert_eq!(value_text(Fid::Weather), "PICK");
        reset_state();
        set(None);
    }

    #[test]
    fn heal_vitals_and_wanted_use_their_natives() {
        let mut fake = Fake::new();
        apply(&fake, Request { fid: Fid::Heal, value: 0 }, 0).unwrap();
        assert_eq!(fake.args_of("SET_ENTITY_HEALTH"), [7, 8, 200, 0, 0, 0, 0]);
        apply(&fake, Request { fid: Fid::Vitals, value: 0 }, 0).unwrap();
        let sets = fake.calls.borrow().iter().filter(|(n, _)| n == "VI_SET_PED_VITAL").count();
        assert_eq!(sets, 2, "only the pools that exist are filled");
        fake.calls.borrow_mut().clear();
        apply(&fake, Request { fid: Fid::SetWanted, value: 3 }, 0).unwrap();
        assert_eq!(fake.names(), ["PLAYER_PED_ID", "CLEAR_PLAYER_WANTED_LEVEL", "VI_RAISE_PLAYER_WANTED_LEVEL"]);
        assert_eq!(fake.args_of("VI_RAISE_PLAYER_WANTED_LEVEL"), [7, 8, 3]);
        fake.calls.borrow_mut().clear();
        apply(&fake, Request { fid: Fid::SetWanted, value: 0 }, 0).unwrap();
        assert_eq!(fake.names(), ["PLAYER_PED_ID", "CLEAR_PLAYER_WANTED_LEVEL"]);
        // Never wanted: caps at none and clears only when there are stars; off restores the cap.
        fake.calls.borrow_mut().clear();
        apply(&fake, Request { fid: Fid::NeverWanted, value: 1 }, 0).unwrap();
        assert_eq!(fake.args_of("SET_MAX_WANTED_LEVEL"), [0]);
        assert!(!fake.names().contains(&"CLEAR_PLAYER_WANTED_LEVEL".to_owned()));
        fake.wanted = 2;
        fake.calls.borrow_mut().clear();
        apply(&fake, Request { fid: Fid::NeverWanted, value: 1 }, 0).unwrap();
        assert!(fake.names().contains(&"CLEAR_PLAYER_WANTED_LEVEL".to_owned()));
        fake.calls.borrow_mut().clear();
        apply(&fake, Request { fid: Fid::NeverWanted, value: 0 }, 0).unwrap();
        assert_eq!(fake.args_of("SET_MAX_WANTED_LEVEL"), [5]);
    }

    #[test]
    fn repair_and_the_waypoint_need_a_vehicle_or_a_waypoint() {
        let mut fake = Fake::new();
        apply(&fake, Request { fid: Fid::RepairVehicle, value: 0 }, 0).unwrap();
        assert_eq!(fake.args_of("SET_VEHICLE_FIXED"), [3, 4]);
        assert_eq!(fake.args_of("SET_VEHICLE_BODY_HEALTH"), [3, 4, u64::from(1.0f32.to_bits())]);
        fake.in_vehicle = false;
        assert_eq!(apply(&fake, Request { fid: Fid::RepairVehicle, value: 0 }, 0), Err("Not in a vehicle".to_owned()));
        // The waypoint moves the vehicle when driving, else the player; a zero height keeps the current one.
        fake.in_vehicle = true;
        fake.calls.borrow_mut().clear();
        apply(&fake, Request { fid: Fid::Waypoint, value: 0 }, 0).unwrap();
        let to = fake.args_of("SET_ENTITY_COORDS");
        assert_eq!(&to[..2], [3, 4]);
        assert_eq!(f32::from_bits(to[2] as u32), 100.0);
        assert_eq!(f32::from_bits((to[2] >> 32) as u32), 200.0);
        assert_eq!(f32::from_bits(to[3] as u32), 30.0);
        assert_eq!(to.len(), 9);
        fake.waypoint = false;
        assert_eq!(apply(&fake, Request { fid: Fid::Waypoint, value: 0 }, 0), Err("No waypoint on the map".to_owned()));
    }

    #[test]
    fn a_vehicle_is_asked_for_through_the_script_call_and_its_task_pinned_then_let_go() {
        let _turn = turn();
        reset_state();
        let fake = Fake::new();
        apply(&fake, Request { fid: Fid::Spawn(1), value: 0 }, 1000).unwrap();
        let call = fake.calls.borrow().iter().find(|(n, _)| n.starts_with("script")).cloned().unwrap();
        assert_eq!(call.0, "script 48391B87 FFFFFF01");
        assert_eq!(call.1.len(), 13);
        assert_eq!(call.1[0], 0x4892_BE1F_9E76_DE37);
        let (x, y) = (f32::from_bits(call.1[4] as u32), f32::from_bits((call.1[4] >> 32) as u32));
        assert_eq!((x, y), (7.0, 2.0));
        assert_eq!(f32::from_bits(call.1[5] as u32), 30.0);
        assert_eq!(fake.pins.borrow().as_slice(), [0x1234]);
        // Still held a moment later, let go after PIN_MS.
        let later = Fake::new();
        tick(&later, 5000);
        assert!(later.unpinned.borrow().is_empty());
        tick(&later, 1000 + PIN_MS);
        assert_eq!(later.unpinned.borrow().as_slice(), [55]);
        reset_state();
    }

    #[test]
    fn a_failed_request_says_why_and_is_logged() {
        let _turn = turn();
        reset_state();
        set(Some(COUNT));
        step(Fid::RepairVehicle, 1).unwrap();
        let mut fake = Fake::new();
        fake.in_vehicle = false;
        tick(&fake, 1);
        assert_eq!(status(), "REPAIR VEHICLE: Not in a vehicle");
        assert_eq!(fake.logs.borrow().len(), 1);
        // No player yet: nothing is called past the ped.
        struct NoPlayer;
        impl Host for NoPlayer {
            fn exists(&self, _: &str) -> bool {
                true
            }
            fn call(&self, _: &str, _: &[u64]) -> [u64; 4] {
                [0; 4]
            }
            fn can_spawn(&self) -> bool {
                false
            }
            fn script_call(&self, _: u32, _: u32, _: &[u64]) -> Result<u64, i32> {
                Err(-2)
            }
            fn pin(&self, _: u64) -> Option<u64> {
                None
            }
            fn unpin(&self, _: u64) {}
            fn log(&self, _: &str) {}
        }
        assert_eq!(apply(&NoPlayer, Request { fid: Fid::Heal, value: 0 }, 0), Err("No player yet".to_owned()));
        reset_state();
        set(None);
    }

    #[test]
    fn without_scripthook_nothing_is_queued() {
        let _turn = turn();
        reset_state();
        set(None);
        // (The real lookup finds no shvi_core.dll in the test program.)
        assert_eq!(step(Fid::God, 1), Err("ScriptHook is not running"));
        assert!(!is_changed(Fid::God));
        assert!(lines().is_none());
    }

    #[test]
    fn reset_switches_a_toggle_off_once() {
        let _turn = turn();
        reset_state();
        set(Some(COUNT));
        step(Fid::NoRagdoll, 1).unwrap();
        tick(&Fake::new(), 1);
        reset(Fid::NoRagdoll);
        assert!(!is_changed(Fid::NoRagdoll));
        let fake = Fake::new();
        tick(&fake, 100_000);
        assert_eq!(fake.args_of("SET_PED_CAN_RAGDOLL"), [7, 8, 1]);
        reset_state();
        set(None);
    }
}
