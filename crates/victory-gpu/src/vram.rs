//! The card's memory: what the system says is in use, what we hold of it by
//! kind, the settings that change while the game runs (the texture pool, the
//! share of the card textures may fill, eviction on or off), and the eviction
//! that keeps the card from filling up.
//!
//! The runtime used to only LIMIT new copies of textures ([`crate::resource::room`]);
//! once the card was full nothing went out of it, and on a 24 GB card the game
//! ran to 23.5 of 23.6 GB and the device was removed. [`tick`] lets go of the
//! host copies of textures that nothing has used for longest when the pool
//! setting is lowered below what they take or the card's use passes its share,
//! a bounded amount a frame, and tells the system which copies are cold.
//!
//! Switches (all off is the runtime as it was): `VICTORY_EVICT=0` (no
//! eviction; `VICTORY_RESIDENCY_PRIORITY=1` adds residency priorities, off
//! by default), `VICTORY_MENU=0` (no in-game
//! menu, [`crate::menu`]), the pool setting at Auto (0).
//!
//! Settings are atomics read where they are used, every time. They begin as
//! the variable says (`VICTORY_TEXTURE_BUDGET_MB`, `VICTORY_TEXTURE_ROOM_PERCENT`,
//! `VICTORY_EVICT`), else as the live settings file says (`VICTORY_LIVE_SETTINGS`,
//! else `victory-live.toml` beside the data directory, else
//! `K:\Leonida\cool_fox\victory-live.toml`), else as the runtime decides.
//! A change is written to that file and logged to `victory::settings`;
//! eviction is logged to `victory::memory`.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use victory_mcp::Json;

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// The most one tick lets go of, in bytes: a frame's worth, so that the
/// frame does not stutter.
pub(crate) const EVICT_PER_FRAME: u64 = 256 * MIB;
/// How long what was let go counts as freed already, in milliseconds: the
/// host copy is dropped once every submission in the air has finished, and
/// the pool gives a block back some seconds after that.
const PENDING_MS: u64 = 4000;
/// Residency priorities ([`crate::resource::residency_pass`]).
pub(crate) const PRIO_NORMAL: u8 = 1;
pub(crate) const PRIO_LOW: u8 = 2;
pub(crate) const PRIO_HIGH: u8 = 3;

/// The texture pool setting in megabytes (0: Auto).
static POOL_MB: AtomicU64 = AtomicU64::new(0);
/// The share of the card's budget textures fill, in percent.
static ROOM_PERCENT: AtomicU64 = AtomicU64::new(DEFAULT_ROOM_PERCENT);
static EVICT: AtomicBool = AtomicBool::new(true);

pub(crate) const DEFAULT_ROOM_PERCENT: u64 = 90;
/// The smallest pool setting that is taken, in megabytes.
pub(crate) const MIN_POOL_MB: u64 = 1024;

/// Eviction since the start: batches, copies, bytes, ticks that found
/// nothing to let go.
static EVICTED: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
/// Residency priorities told, and refused by the system.
static PRIORITIES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
/// The reason of the last batch.
static LAST_REASON: Mutex<&'static str> = Mutex::new("none yet");
/// What was let go lately (when, bytes): counted as freed for [`PENDING_MS`].
static RECENT: Mutex<VecDeque<(u64, u64)>> = Mutex::new(VecDeque::new());
/// Until when the pool gives empty blocks back at once (milliseconds).
static TRIM_UNTIL: AtomicU64 = AtomicU64::new(0);

struct Live {
    /// Where the settings are kept, if anywhere.
    path: Option<PathBuf>,
    /// `VICTORY_EVICT=0`: eviction is off and cannot be turned on.
    evict_allowed: bool,
    save: Mutex<()>,
}

static LIVE: OnceLock<Live> = OnceLock::new();

/// What a settings file says; `None` for what it does not.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Parsed {
    pub pool_mb: Option<u64>,
    pub room_percent: Option<u64>,
    pub evict: Option<bool>,
}

/// The values a setting may take, as text: a number, `auto`, `on` or `off`.
fn flag(text: &str) -> Option<bool> {
    match text.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => Some(true),
        "off" | "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// A pool setting in megabytes from text: `auto` or 0 is 0, else 1,024 or
/// more (the value of an environment variable or a file, or a tool's).
pub(crate) fn pool_from(text: &str) -> Result<u64, String> {
    let t = text.trim().trim_matches('"');
    if t.eq_ignore_ascii_case("auto") {
        return Ok(0);
    }
    match t.parse::<u64>() {
        Ok(0) => Ok(0),
        Ok(mb) if (MIN_POOL_MB..=1 << 20).contains(&mb) => Ok(mb),
        _ => Err(format!("texture_pool_mb must be auto, 0 or {MIN_POOL_MB} and more megabytes, not {text:?}")),
    }
}

/// A share of the card's budget from text: 50 to 97.
pub(crate) fn percent_from(text: &str) -> Result<u64, String> {
    match text.trim().trim_matches('"').parse::<u64>() {
        Ok(p) if (50..=97).contains(&p) => Ok(p),
        _ => Err(format!("room_percent must be 50 to 97, not {text:?}")),
    }
}

/// The settings file's lines: `name = value`, `#` begins a comment.
pub(crate) fn parse(text: &str) -> Parsed {
    let mut out = Parsed::default();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((name, value)) = line.split_once('=') else { continue };
        match name.trim() {
            "texture_pool_mb" => out.pool_mb = pool_from(value).ok(),
            "room_percent" => out.room_percent = percent_from(value).ok(),
            "evict" => out.evict = flag(value),
            _ => {}
        }
    }
    out
}

/// The settings file's text.
pub(crate) fn render(pool_mb: u64, room_percent: u64, evict: bool) -> String {
    format!(
        "# VIctory live settings: written by the in-game menu and the debug server's set_setting, read at start.\n\
         # texture_pool_mb: 0 is Auto, else megabytes of host copies of textures the card may hold.\n\
         texture_pool_mb = {pool_mb}\n\
         room_percent = {room_percent}\n\
         evict = {evict}\n"
    )
}

/// Where the settings are kept: the file `VICTORY_LIVE_SETTINGS` names, else
/// `victory-live.toml` in the parent of `VICTORY_DATA_DIR`, else the default
/// place if its folder is there.
pub(crate) fn live_path(file: Option<&std::ffi::OsStr>, data_dir: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    victory_core::tunables::live_path(file, data_dir)
}

/// What the three settings begin as: the variables first, then the file.
pub(crate) fn initial(
    file: &Parsed,
    env_pool: Option<&str>,
    env_room: Option<&str>,
    env_evict: Option<&str>,
) -> (u64, u64, bool, bool) {
    let pool = env_pool.and_then(|v| pool_from(v).ok()).or(file.pool_mb).unwrap_or(0);
    let room = env_room.and_then(|v| percent_from(v).ok()).or(file.room_percent).unwrap_or(DEFAULT_ROOM_PERCENT);
    // The variable wins over the file, both ways: 0 is off for good, any other
    // value is on; unset, the file says (on without one).
    let env = env_evict.map(|v| v.trim() != "0");
    let allowed = env != Some(false);
    let evict = env.unwrap_or_else(|| file.evict.unwrap_or(true));
    (pool, room, evict, allowed)
}

fn live() -> &'static Live {
    LIVE.get_or_init(|| {
        let var = |name: &str| std::env::var(name).ok();
        let path = live_path(std::env::var_os("VICTORY_LIVE_SETTINGS").as_deref(), std::env::var_os("VICTORY_DATA_DIR").as_deref());
        // Tests never touch the real file.
        #[cfg(test)]
        let path = {
            let _real = path;
            let dir = std::env::temp_dir().join("victory-test-mem");
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join("test-live.toml");
            let _ = std::fs::remove_file(&file);
            Some(file)
        };
        let file = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse(&t)).unwrap_or_default();
        let (pool, room, evict, allowed) = initial(
            &file,
            var("VICTORY_TEXTURE_BUDGET_MB").as_deref(),
            var("VICTORY_TEXTURE_ROOM_PERCENT").as_deref(),
            var("VICTORY_EVICT").as_deref(),
        );
        POOL_MB.store(pool, Ordering::Relaxed);
        ROOM_PERCENT.store(room, Ordering::Relaxed);
        EVICT.store(evict, Ordering::Relaxed);
        victory_mcp::set_memory_hooks(victory_mcp::MemoryHooks { memory: memory_json, settings: settings_json, set: set_json });
        tracing::info!(
            target: "victory::settings",
            texture_pool_mb = pool,
            room_percent = room,
            evict,
            evict_allowed = allowed,
            residency_priority = residency_priority_on(),
            file = ?path,
            "live settings at start"
        );
        Live { path, evict_allowed: allowed, save: Mutex::new(()) }
    })
}

/// Reads the live settings if that is not done yet (the runtime does it when
/// the host is made, so the debug server's tools are there from the first
/// frame).
pub(crate) fn init() {
    live();
}

/// The texture pool setting in bytes: `None` for Auto.
pub(crate) fn pool_limit() -> Option<u64> {
    live();
    let mb = POOL_MB.load(Ordering::Relaxed);
    (mb > 0).then(|| mb * MIB)
}

/// The pool setting in megabytes (0: Auto).
pub(crate) fn pool_mb() -> u64 {
    live();
    POOL_MB.load(Ordering::Relaxed)
}

/// The share of the card's budget, in percent, up to which textures are made.
pub(crate) fn room_percent() -> u64 {
    live();
    ROOM_PERCENT.load(Ordering::Relaxed)
}

/// Whether eviction runs.
pub(crate) fn evict_on() -> bool {
    live();
    EVICT.load(Ordering::Relaxed)
}

/// Whether eviction may be turned on (`VICTORY_EVICT=0` says no).
pub(crate) fn evict_allowed() -> bool {
    live().evict_allowed
}

fn save() {
    let live = live();
    let Some(path) = &live.path else { return };
    let _one = live.save.lock().unwrap_or_else(|e| e.into_inner());
    let text = render(POOL_MB.load(Ordering::Relaxed), ROOM_PERCENT.load(Ordering::Relaxed), EVICT.load(Ordering::Relaxed));
    // The file's [tunables] table stays as it is (the tunables registry owns it).
    if let Err(e) = victory_core::tunables::write_settings_text(path, &text) {
        tracing::warn!(target: "victory::settings", file = %path.display(), error = %e, "live settings not written");
    }
}

fn changed(name: &str, was: String, now: String) {
    if was != now {
        tracing::info!(target: "victory::settings", setting = name, was, now, "setting changed");
        save();
    }
}

fn pool_text(mb: u64) -> String {
    if mb == 0 { "auto".to_owned() } else { format!("{mb} MB") }
}

/// Sets the pool, in megabytes (0: Auto).
pub(crate) fn set_pool_mb(mb: u64) {
    live();
    let was = POOL_MB.swap(mb, Ordering::Relaxed);
    changed("texture_pool_mb", pool_text(was), pool_text(mb));
}

/// Sets the share of the card textures may fill, 50 to 97.
pub(crate) fn set_room_percent(percent: u64) {
    live();
    let percent = percent.clamp(50, 97);
    let was = ROOM_PERCENT.swap(percent, Ordering::Relaxed);
    changed("room_percent", was.to_string(), percent.to_string());
}

/// Turns eviction on or off; on does nothing under `VICTORY_EVICT=0`.
pub(crate) fn set_evict(on: bool) {
    let on = on && live().evict_allowed;
    let was = EVICT.swap(on, Ordering::Relaxed);
    changed("evict", was.to_string(), on.to_string());
}

/// Sets a setting by name from text (the debug server's `set_setting`).
pub(crate) fn set_by_name(name: &str, value: &str) -> Result<(), String> {
    match name {
        "texture_pool_mb" => set_pool_mb(pool_from(value)?),
        "room_percent" => set_room_percent(percent_from(value)?),
        "evict" => {
            let on = flag(value).ok_or_else(|| format!("evict must be on or off, not {value:?}"))?;
            if on && !live().evict_allowed {
                return Err("eviction is off for good: the process was started with VICTORY_EVICT=0".into());
            }
            set_evict(on);
        }
        _ => return Err(format!("no setting {name:?}; they are texture_pool_mb, room_percent, evict")),
    }
    Ok(())
}

/// The card's memory and what we hold of it, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    /// The system's budget and use for the card's own memory, then for the
    /// PC's memory shared with the card.
    pub local_budget: u64,
    pub local_usage: u64,
    pub shared_budget: u64,
    pub shared_usage: u64,
    /// Host copies of textures the title fills: how many, their bytes.
    pub texture_copies: u64,
    pub texture_bytes: u64,
    /// Tiles given to textures made level by level (a part of the pool's use).
    pub tile_bytes: u64,
    /// The host's own render targets, depth and unordered access textures.
    pub target_bytes: u64,
    /// Card copies of title buffers, and of windows of title memory.
    pub card_buffer_bytes: u64,
    pub window_bytes: u64,
    /// Texture pool blocks no texture holds.
    pub pool_free_bytes: u64,
}

impl Snapshot {
    /// The numbers as they are now: two queries of the system and some
    /// atomics. `with_system` false leaves the system's four at 0.
    pub(crate) fn take(with_system: bool) -> Snapshot {
        let (texture_copies, texture_bytes, target_bytes) = crate::resource::copy_totals();
        let mut s = Snapshot {
            texture_copies,
            texture_bytes,
            tile_bytes: crate::tiles::given_total(),
            target_bytes,
            card_buffer_bytes: crate::shadow::card_buffer_bytes(),
            window_bytes: crate::shadow::window_card_bytes(),
            pool_free_bytes: crate::pool::texture_room(),
            ..Snapshot::default()
        };
        if with_system && let Some([own, shared]) = crate::host::made_host_any().and_then(|h| h.video_memory_groups()) {
            (s.local_budget, s.local_usage, s.shared_budget, s.shared_usage) = (own[0], own[1], shared[0], shared[1]);
        }
        s
    }

    /// The pool's use: copies of textures the title fills, with their tiles.
    pub(crate) fn pool_used(&self) -> u64 {
        self.texture_bytes + self.tile_bytes
    }
}

/// What eviction has to let go of now, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Need {
    pub bytes: u64,
    pub reason: &'static str,
}

/// Whether bytes must go, given the pool limit (`None`: Auto), the bytes the
/// pool holds, the card's budget and net use, the share textures may fill,
/// what was let go lately and is not freed yet, and whether textures wait for
/// room (`demand`). Over the pool limit, down to it; or past the share plus 5
/// points (98 at most), down to the share.
///
/// With `demand`, also room for what waits: `room()` refuses new copies from
/// the limit (or the share) on, so with eviction asked for only above it the
/// card sat between the two for good and the textures that were needed
/// stayed black (runs 20261126-155246 and -160258: pool 8 and 10 GB against
/// 13 GB wanted). A step ([`EVICT_PER_FRAME`]) of room is kept below the line
/// while they wait.
pub(crate) fn plan(
    pool_limit: Option<u64>,
    pool_used: u64,
    budget: u64,
    used_net: u64,
    percent: u64,
    pending: u64,
    demand: bool,
) -> Option<Need> {
    let mut need: Option<Need> = None;
    let mut ask = |bytes: u64, reason: &'static str| {
        if bytes > 0 && need.is_none_or(|n| bytes > n.bytes) {
            need = Some(Need { bytes, reason });
        }
    };
    let step = if demand { EVICT_PER_FRAME } else { 0 };
    if let Some(limit) = pool_limit {
        if pool_used > limit {
            ask(pool_used - limit, "pool setting");
        } else if demand {
            ask((pool_used + step).saturating_sub(limit), "textures wait for room in the pool");
        }
    }
    if budget > 0 {
        let high = budget / 100 * (percent + 5).min(98);
        let low = budget / 100 * percent;
        if used_net > high {
            ask(used_net - low, "card use past its share");
        } else if demand {
            ask((used_net + step).saturating_sub(low), "textures wait for room on the card");
        }
    }
    need.and_then(|n| {
        let bytes = n.bytes.saturating_sub(pending);
        (bytes > 0).then_some(Need { bytes, ..n })
    })
}

/// When textures last waited for room (milliseconds; 0: never).
static DEMAND_AT: AtomicU64 = AtomicU64::new(0);

/// The pass that makes textures picked by index found the card without room
/// for them: they wait, and eviction makes room for a second.
pub(crate) fn note_demand() {
    DEMAND_AT.store(crate::host::now_ms().max(1), Ordering::Relaxed);
}

fn demand(now: u64) -> bool {
    let at = DEMAND_AT.load(Ordering::Relaxed);
    at != 0 && now <= at + 1000
}

/// Frames a host copy must have gone unused before it may be evicted
/// (`VICTORY_EVICT_MIN_FRAMES`, 120): a copy bound lately is wanted again
/// soon, and made and uploaded again it would only churn.
pub(crate) fn min_idle_frames() -> u64 {
    victory_core::tunables::known::EVICT_MIN_FRAMES.get() as u64
}

fn pending(now: u64) -> u64 {
    let mut recent = RECENT.lock().unwrap_or_else(|e| e.into_inner());
    while recent.front().is_some_and(|&(at, _)| now > at + PENDING_MS) {
        recent.pop_front();
    }
    recent.iter().map(|&(_, bytes)| bytes).sum()
}

/// Whether the texture pool gives its empty blocks back now (some seconds
/// after eviction let go of copies): the pool keeps them while the card has
/// room, and eviction wants the room.
pub(crate) fn trimming() -> bool {
    crate::host::now_ms() < TRIM_UNTIL.load(Ordering::Relaxed)
}

/// Once a submission, before it starts, by the thread that submits (the
/// sweep's place: nothing named by a list not yet reset is let go): eviction
/// when the pool setting or the card's use asks for it, a part of at most
/// [`EVICT_PER_FRAME`] bytes, and the residency priorities once a second.
pub(crate) fn tick() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    static BACKOFF: AtomicU64 = AtomicU64::new(0);
    static PRIORITY_AT: AtomicU64 = AtomicU64::new(0);
    if !evict_on() {
        return;
    }
    let now = crate::host::now_ms();
    // Once a frame at most (a title at 60 a second submits many times).
    if now < NEXT.load(Ordering::Relaxed) {
        return;
    }
    NEXT.store(now + 12, Ordering::Relaxed);
    let Some(host) = crate::host::made_host_any() else { return };
    let memory = host.video_memory();
    if now >= BACKOFF.load(Ordering::Relaxed) {
        let [budget, used, ..] = memory.unwrap_or_default();
        let used_net = used.saturating_sub(crate::pool::texture_room());
        let need = plan(pool_limit(), crate::resource::filled_bytes(), budget, used_net, room_percent(), pending(now), demand(now));
        if let Some(need) = need {
            let (count, bytes) = crate::resource::evict_lru(need.bytes.min(EVICT_PER_FRAME));
            if count == 0 {
                // Everything left is in use: look again in half a second.
                EVICTED[3].fetch_add(1, Ordering::Relaxed);
                BACKOFF.store(now + 500, Ordering::Relaxed);
            } else {
                EVICTED[0].fetch_add(1, Ordering::Relaxed);
                EVICTED[1].fetch_add(count, Ordering::Relaxed);
                EVICTED[2].fetch_add(bytes, Ordering::Relaxed);
                *LAST_REASON.lock().unwrap_or_else(|e| e.into_inner()) = need.reason;
                RECENT.lock().unwrap_or_else(|e| e.into_inner()).push_back((now, bytes));
                TRIM_UNTIL.store(now + 10_000, Ordering::Relaxed);
                tracing::info!(
                    target: "victory::memory",
                    count,
                    mb = bytes / MIB,
                    still_needed_mb = need.bytes.saturating_sub(bytes) / MIB,
                    reason = need.reason,
                    card_used_mb = used >> 20,
                    card_budget_mb = budget >> 20,
                    pool_mb = crate::resource::filled_bytes() >> 20,
                    pool_setting_mb = pool_mb(),
                    frame = crate::frame::current(),
                    "eviction batch: host copies of textures let go, longest unused first"
                );
            }
        }
    }
    if residency_priority_on() && now >= PRIORITY_AT.load(Ordering::Relaxed) {
        PRIORITY_AT.store(now + 1000, Ordering::Relaxed);
        prioritise(host);
    }
}

/// Whether residency priorities are told (`VICTORY_RESIDENCY_PRIORITY=1`; off
/// by default). Run 20261126-154946: the device was removed ("invalid call",
/// 0x887A0001) 18 ms after eviction was turned on, with no batch yet, so the
/// only thing it had started was the first priority pass.
pub(crate) fn residency_priority_on() -> bool {
    victory_core::tunables::known::RESIDENCY_PRIORITY.get()
}

/// Tells the system which host copies are cold and which are render targets
/// ([`crate::resource::residency_pass`]): `SetResidencyPriority`, on the
/// device's second interface. Where the system refuses (a texture placed in a
/// heap of ours is no object of its own to it) the refusals are counted.
fn prioritise(host: &crate::host::Host) {
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::core::Interface;
    static DEVICE: OnceLock<Option<ID3D12Device1>> = OnceLock::new();
    let Some(device) = DEVICE.get_or_init(|| host.device.cast::<ID3D12Device1>().ok()).as_ref() else { return };
    crate::resource::residency_pass(|texture, prio| {
        let level = match prio {
            PRIO_LOW => D3D12_RESIDENCY_PRIORITY_LOW,
            PRIO_HIGH => D3D12_RESIDENCY_PRIORITY_HIGH,
            _ => D3D12_RESIDENCY_PRIORITY_NORMAL,
        };
        let Ok(object) = texture.cast::<ID3D12Pageable>() else { return false };
        // SAFETY: one live object and one priority.
        let told = unsafe { device.SetResidencyPriority(1, &Some(object), &level) }.is_ok();
        PRIORITIES[usize::from(!told)].fetch_add(1, Ordering::Relaxed);
        told
    });
}

/// What eviction has done since the start: batches, copies, megabytes, ticks
/// that found nothing to let go, and the last reason.
pub(crate) fn evicted() -> ([u64; 4], &'static str) {
    let [a, b, c, d] = [0, 1, 2, 3].map(|i| EVICTED[i].load(Ordering::Relaxed));
    ([a, b, c >> 20, d], *LAST_REASON.lock().unwrap_or_else(|e| e.into_inner()))
}

fn mb(bytes: u64) -> Json {
    Json::num((bytes as f64 / MIB as f64 * 10.0).round() / 10.0)
}

/// `get_settings`.
pub(crate) fn settings_json() -> Json {
    let live = live();
    Json::obj([
        ("texture_pool_mb", Json::obj([
            ("value", Json::uint(pool_mb())),
            ("auto", Json::Bool(pool_mb() == 0)),
            ("allowed", Json::str("auto or 0, else 1024 and more megabytes")),
        ])),
        ("room_percent", Json::obj([
            ("value", Json::uint(room_percent())),
            ("allowed", Json::str("50 to 97")),
        ])),
        ("evict", Json::obj([
            ("value", Json::Bool(evict_on())),
            ("allowed", Json::str(if live.evict_allowed { "on or off" } else { "off for good (VICTORY_EVICT=0)" })),
        ])),
        ("file", live.path.as_ref().map_or(Json::Null, |p| Json::str(p.display().to_string()))),
    ])
}

/// `set_setting`.
pub(crate) fn set_json(name: &str, value: &str) -> Result<Json, String> {
    set_by_name(name, value)?;
    Ok(settings_json())
}

/// `get_memory`.
pub(crate) fn memory_json() -> Json {
    let s = Snapshot::take(true);
    let ([batches, copies, evicted_mb, empty], reason) = evicted();
    Json::obj([
        ("card_own", Json::obj([("budget_mb", mb(s.local_budget)), ("used_mb", mb(s.local_usage))])),
        ("pc_shared", Json::obj([("budget_mb", mb(s.shared_budget)), ("used_mb", mb(s.shared_usage))])),
        ("held_mb", Json::obj([
            ("texture_copies", mb(s.texture_bytes)),
            ("texture_copy_count", Json::uint(s.texture_copies)),
            ("tiles", mb(s.tile_bytes)),
            ("render_targets", mb(s.target_bytes)),
            ("card_buffer_copies", mb(s.card_buffer_bytes)),
            ("title_memory_windows_on_card", mb(s.window_bytes)),
            ("free_pool_blocks", mb(s.pool_free_bytes)),
        ])),
        ("pool", Json::obj([
            ("setting_mb", Json::uint(pool_mb())),
            ("auto", Json::Bool(pool_mb() == 0)),
            ("used_mb", mb(s.pool_used())),
            ("room_percent", Json::uint(room_percent())),
        ])),
        ("eviction", Json::obj([
            ("on", Json::Bool(evict_on())),
            ("allowed", Json::Bool(evict_allowed())),
            ("batches", Json::uint(batches)),
            ("copies_let_go", Json::uint(copies)),
            ("mb_let_go", Json::uint(evicted_mb)),
            ("ticks_with_nothing_to_let_go", Json::uint(empty)),
            ("skipped_gpu_written", Json::obj([
                ("copied_into_by_gpu", Json::uint(crate::resource::EVICT_SKIPPED[0].load(Ordering::Relaxed))),
                ("shares_memory_with_gpu_written", Json::uint(crate::resource::EVICT_SKIPPED[1].load(Ordering::Relaxed))),
                ("never_uploaded", Json::uint(crate::resource::EVICT_SKIPPED[2].load(Ordering::Relaxed))),
            ])),
            ("last_reason", Json::str(reason)),
            ("priorities_told", Json::uint(PRIORITIES[0].load(Ordering::Relaxed))),
            ("priorities_refused", Json::uint(PRIORITIES[1].load(Ordering::Relaxed))),
        ])),
    ])
}

/// For the periodic memory line ([`crate::logstats`]): name and value
/// (megabytes unless the name says another unit).
pub(crate) fn memory_extra() -> Vec<(&'static str, u64)> {
    let s = Snapshot::take(true);
    let ([batches, copies, evicted_mb, empty], _) = evicted();
    vec![
        ("vram_local_budget_mb", s.local_budget >> 20),
        ("vram_local_used_mb", s.local_usage >> 20),
        ("vram_shared_used_mb", s.shared_usage >> 20),
        ("texture_copy_mb", s.texture_bytes >> 20),
        ("texture_copy_n", s.texture_copies),
        ("tile_mb", s.tile_bytes >> 20),
        ("render_target_mb", s.target_bytes >> 20),
        ("card_buffer_mb", s.card_buffer_bytes >> 20),
        ("title_window_card_mb", s.window_bytes >> 20),
        ("free_pool_block_mb", s.pool_free_bytes >> 20),
        ("pool_setting_mb", pool_mb()),
        ("evict_on", u64::from(evict_on())),
        ("evict_batches", batches),
        ("evict_copies", copies),
        ("evict_mb", evicted_mb),
        ("evict_idle_ticks", empty),
        ("evict_skip_gpu_copied", crate::resource::EVICT_SKIPPED[0].load(Ordering::Relaxed)),
        ("evict_skip_shares_gpu_memory", crate::resource::EVICT_SKIPPED[1].load(Ordering::Relaxed)),
        ("evict_skip_never_uploaded", crate::resource::EVICT_SKIPPED[2].load(Ordering::Relaxed)),
        ("priorities_told", PRIORITIES[0].load(Ordering::Relaxed)),
        ("priorities_refused", PRIORITIES[1].load(Ordering::Relaxed)),
        ("dyn_desc_refused", crate::descriptor::host::DYNAMIC_REFUSED.load(Ordering::Relaxed)),
        ("dyn_desc_expanded", crate::descriptor::host::DYNAMIC_EXPANDED.load(Ordering::Relaxed)),
    ]
}

/// The pool settings the menu offers, in gigabytes: Auto first, then 4 to
/// 20 by two, those that fit within the card's budget less 1.5 GB (`budget`
/// 0: all of them).
pub(crate) fn pool_choices(budget: u64) -> Vec<u64> {
    std::iter::once(0)
        .chain((4..=20).step_by(2).filter(|gb| budget == 0 || gb * GIB + GIB + GIB / 2 <= budget))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_is_read_and_written_back_the_same() {
        let text = render(8192, 92, false);
        assert_eq!(parse(&text), Parsed { pool_mb: Some(8192), room_percent: Some(92), evict: Some(false) });
        let a = "texture_pool_mb = auto # as it decides\nroom_percent = \"75\"\nevict = on\nsomething = else\n";
        assert_eq!(parse(a), Parsed { pool_mb: Some(0), room_percent: Some(75), evict: Some(true) });
    }

    #[test]
    fn bad_values_in_the_file_are_not_taken() {
        let p = parse("texture_pool_mb = 12\nroom_percent = 10\nevict = maybe\n\n# nothing\n=\n");
        assert_eq!(p, Parsed::default());
    }

    #[test]
    fn pool_and_percent_text() {
        assert_eq!(pool_from("auto"), Ok(0));
        assert_eq!(pool_from("0"), Ok(0));
        assert_eq!(pool_from(" 8192 "), Ok(8192));
        assert!(pool_from("100").is_err());
        assert!(pool_from("-5").is_err());
        assert_eq!(percent_from("97"), Ok(97));
        assert!(percent_from("98").is_err());
        assert!(percent_from("49").is_err());
    }

    #[test]
    fn variables_begin_the_settings_then_the_file_then_the_defaults() {
        let file = Parsed { pool_mb: Some(6144), room_percent: Some(80), evict: Some(false) };
        assert_eq!(initial(&file, None, None, None), (6144, 80, false, true));
        assert_eq!(initial(&file, Some("10240"), Some("93"), None), (10240, 93, false, true));
        assert_eq!(initial(&Parsed::default(), None, None, None), (0, 90, true, true));
        // A variable that is no good is passed over.
        assert_eq!(initial(&file, Some("lots"), Some("5"), None), (6144, 80, false, true));
        // VICTORY_EVICT=0 turns it off whatever the file says, for good.
        let on = Parsed { evict: Some(true), ..Parsed::default() };
        assert_eq!(initial(&on, None, None, Some("0")), (0, 90, false, false));
        assert_eq!(initial(&on, None, None, Some("1")), (0, 90, true, true));
        // VICTORY_EVICT=1 beats a file that says off (run 20261126-154946).
        let off = Parsed { evict: Some(false), ..Parsed::default() };
        assert_eq!(initial(&off, None, None, Some("1")), (0, 90, true, true));
        assert_eq!(initial(&off, None, None, None), (0, 90, false, true));
    }

    #[test]
    fn the_file_lies_where_it_is_told_or_beside_the_data_directory() {
        use std::ffi::OsStr;
        assert_eq!(live_path(Some(OsStr::new(r"X:\a\b.toml")), Some(OsStr::new(r"K:\Leonida\x\data"))), Some(PathBuf::from(r"X:\a\b.toml")));
        assert_eq!(
            live_path(None, Some(OsStr::new(r"K:\Leonida\cool_fox\data"))),
            Some(PathBuf::from(r"K:\Leonida\cool_fox").join("victory-live.toml"))
        );
        assert_eq!(live_path(Some(OsStr::new("")), Some(OsStr::new(r"K:\Leonida\cool_fox\data"))), Some(PathBuf::from(r"K:\Leonida\cool_fox").join("victory-live.toml")));
    }

    #[test]
    fn nothing_goes_while_there_is_room() {
        // Auto, 23.6 GB budget, 15 GB in use at 90 percent.
        assert_eq!(plan(None, 9 * GIB, 24 * GIB, 15 * GIB, 90, 0, false), None);
        // Under a pool setting.
        assert_eq!(plan(Some(10 * GIB), 9 * GIB, 24 * GIB, 15 * GIB, 90, 0, false), None);
        // No budget known and no pool setting.
        assert_eq!(plan(None, 50 * GIB, 0, 50 * GIB, 90, 0, false), None);
    }

    #[test]
    fn a_lowered_pool_asks_for_the_difference() {
        let n = plan(Some(8 * GIB), 12 * GIB, 24 * GIB, 15 * GIB, 90, 0, false).unwrap();
        assert_eq!((n.bytes, n.reason), (4 * GIB, "pool setting"));
    }

    #[test]
    fn past_the_share_down_to_the_share() {
        let budget = 100 * GIB;
        // 95 percent is not past 90 + 5; 96 is.
        assert_eq!(plan(None, 10 * GIB, budget, 95 * GIB, 90, 0, false), None);
        let n = plan(None, 10 * GIB, budget, 96 * GIB, 90, 0, false).unwrap();
        assert_eq!((n.bytes, n.reason), (6 * GIB, "card use past its share"));
        // The high mark never goes past 98.
        assert_eq!(plan(None, 10 * GIB, budget, 98 * GIB, 97, 0, false), None);
        assert!(plan(None, 10 * GIB, budget, 99 * GIB, 97, 0, false).is_some());
    }

    #[test]
    fn the_larger_need_wins_and_what_is_let_go_lately_counts() {
        let n = plan(Some(8 * GIB), 9 * GIB, 100 * GIB, 99 * GIB, 90, 0, false).unwrap();
        assert_eq!((n.bytes, n.reason), (9 * GIB, "card use past its share"));
        let n = plan(Some(8 * GIB), 9 * GIB, 100 * GIB, 99 * GIB, 90, 4 * GIB, false).unwrap();
        assert_eq!(n.bytes, 5 * GIB);
        assert_eq!(plan(Some(8 * GIB), 9 * GIB, 100 * GIB, 50 * GIB, 90, GIB, false), None);
    }

    #[test]
    fn waiting_textures_get_room_below_the_line() {
        let (limit, budget) = (10 * GIB, 100 * GIB);
        // At the pool's line with textures waiting: a step of room is made.
        let n = plan(Some(limit), limit, budget, 50 * GIB, 90, 0, true).unwrap();
        assert_eq!((n.bytes, n.reason), (EVICT_PER_FRAME, "textures wait for room in the pool"));
        // Without anything waiting the card may sit at the line.
        assert_eq!(plan(Some(limit), limit, budget, 50 * GIB, 90, 0, false), None);
        // Far below the line nothing is let go even with demand.
        assert_eq!(plan(Some(limit), 5 * GIB, budget, 50 * GIB, 90, 0, true), None);
        // On the card's share (90 of 100 GB), between the share and the high mark.
        let n = plan(None, 0, budget, 90 * GIB, 90, 0, true).unwrap();
        assert_eq!((n.bytes, n.reason), (EVICT_PER_FRAME, "textures wait for room on the card"));
        assert_eq!(plan(None, 0, budget, 90 * GIB, 90, 0, false), None);
        // What was let go lately counts.
        assert_eq!(plan(Some(limit), limit, budget, 50 * GIB, 90, EVICT_PER_FRAME, true), None);
    }

    #[test]
    fn demand_lasts_a_second() {
        DEMAND_AT.store(0, Ordering::Relaxed);
        assert!(!demand(5000));
        note_demand();
        let at = DEMAND_AT.load(Ordering::Relaxed);
        assert!(demand(at) && demand(at + 1000) && !demand(at + 1001));
        DEMAND_AT.store(0, Ordering::Relaxed);
    }

    #[test]
    fn copies_bound_lately_stay_for_120_frames_by_default() {
        assert_eq!(min_idle_frames(), 120);
    }

    #[test]
    fn what_was_let_go_counts_for_four_seconds() {
        let mut recent = RECENT.lock().unwrap();
        recent.clear();
        recent.push_back((1000, 100));
        recent.push_back((3000, 50));
        drop(recent);
        assert_eq!(pending(4000), 150);
        assert_eq!(pending(5500), 50);
        assert_eq!(pending(7500), 0);
    }

    #[test]
    fn the_menu_offers_what_fits() {
        assert_eq!(pool_choices(0), vec![0, 4, 6, 8, 10, 12, 14, 16, 18, 20]);
        // A 4090: budget about 23.6 GB; 22.1 is the most, so 20 stays.
        assert_eq!(pool_choices(236 * GIB / 10), vec![0, 4, 6, 8, 10, 12, 14, 16, 18, 20]);
        // 12 GB card: budget 11 GB, less 1.5 is 9.5: 4, 6, 8.
        assert_eq!(pool_choices(11 * GIB), vec![0, 4, 6, 8]);
        assert_eq!(pool_choices(2 * GIB), vec![0]);
    }

    #[test]
    fn settings_are_set_by_name_and_refused_by_reason() {
        let _serial = SETTINGS_TEST.lock().unwrap_or_else(|e| e.into_inner());
        init();
        let (pool, room, evict) = (POOL_MB.load(Ordering::Relaxed), ROOM_PERCENT.load(Ordering::Relaxed), EVICT.load(Ordering::Relaxed));
        assert!(set_by_name("texture_pool_mb", "12").is_err());
        assert!(set_by_name("room_percent", "99").is_err());
        assert!(set_by_name("evict", "perhaps").is_err());
        assert!(set_by_name("nothing", "1").unwrap_err().contains("no setting"));
        // Nothing changed by the refused ones.
        assert_eq!((POOL_MB.load(Ordering::Relaxed), ROOM_PERCENT.load(Ordering::Relaxed), EVICT.load(Ordering::Relaxed)), (pool, room, evict));
        set_by_name("texture_pool_mb", "8192").unwrap();
        assert_eq!(pool_limit(), Some(8192 * MIB));
        set_by_name("room_percent", "93").unwrap();
        assert_eq!(room_percent(), 93);
        // The change is in the file, as the next start reads it.
        let kept = parse(&std::fs::read_to_string(live().path.as_ref().unwrap()).unwrap());
        assert_eq!((kept.pool_mb, kept.room_percent), (Some(8192), Some(93)));
        set_by_name("texture_pool_mb", "auto").unwrap();
        assert_eq!(pool_limit(), None);
        set_by_name("room_percent", &room.to_string()).unwrap();
        set_pool_mb(pool);
        set_evict(evict);
    }

    static SETTINGS_TEST: Mutex<()> = Mutex::new(());

    #[test]
    fn the_snapshot_adds_up() {
        let s = Snapshot { local_usage: 20 * GIB, pool_free_bytes: 2 * GIB, texture_bytes: 9 * GIB, tile_bytes: GIB, ..Snapshot::default() };
        assert_eq!(s.pool_used(), 10 * GIB);
    }

    #[test]
    fn the_extra_lines_name_their_units() {
        let extra = memory_extra();
        assert!(extra.iter().any(|(k, _)| *k == "evict_batches"));
        assert!(extra.iter().all(|(k, _)| k.ends_with("_mb") || k.ends_with("_n") || k.contains("evict") || k.contains("priorities")));
        let mut names: Vec<_> = extra.iter().map(|(k, _)| *k).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), extra.len());
    }

    #[test]
    fn the_tools_json_has_the_settings_and_the_totals() {
        let m = memory_json();
        for key in ["card_own", "pc_shared", "held_mb", "pool", "eviction"] {
            assert!(m.get(key).is_some(), "{key}");
        }
        let s = settings_json();
        for key in ["texture_pool_mb", "room_percent", "evict"] {
            assert!(s.get(key).and_then(|v| v.get("value")).is_some(), "{key}");
        }
    }
}
