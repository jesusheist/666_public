//! Live tunables: one registry for the `VICTORY_*` switches, readable and
//! changeable while the game runs (debug MCP server, in-game panel, the
//! `[tunables]` table of the live settings file).
//!
//! Declare one in [`known`] (or any module):
//!
//! ```ignore
//! tunable!(pub VIEW_GUARD: bool = true, "VICTORY_VIEW_GUARD", "Guard the other views of a page", live = Safe);
//! tunable!(pub WIDE_SPIN: i64 = 0, 0..=100000, "VICTORY_WIDE_SPIN", "Spin time in us", live = Safe);
//! tunable!(pub SCALE: f64 = 1.0, 0.25..=4.0, "VICTORY_SCALE", "Scale", live = NextFrame);
//! tunable!(pub MODE: enum = "fast", ["fast", "slow"], "VICTORY_MODE", "Mode", live = Restart);
//! ```
//!
//! and read it with `VIEW_GUARD.get()`: one `Once` check (an acquire load) and
//! one relaxed atomic load. `MODE.get()` is the index in the list, `MODE.get_str()` the text.
//!
//! Start value: the environment variable, else the `[tunables]` table of the
//! live file (key: the lowercase name, `view_guard`), else the default.
//! Out-of-range numbers from the environment or the file are clamped; a
//! value that does not parse is ignored (with a warning).
//!
//! Live class: [`Live::Safe`] applies at once, [`Live::NextFrame`] is kept
//! as pending until [`frame_boundary`] (the GPU host calls it once per frame),
//! [`Live::Restart`] is kept in the file and reported as needing a restart.
//!
//! The registry knows a tunable once it is read or [`Tunable::register`]ed;
//! [`known`] lists them all in `register_all`.

use std::collections::BTreeMap;
use std::io;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, Once};
use std::time::{Duration, SystemTime};

use victory_mcp::Json;

pub mod known;

/// How a change reaches the running game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Live {
    /// Applied at once.
    Safe,
    /// Kept as pending; applied by [`frame_boundary`].
    NextFrame,
    /// Kept in the file; takes effect at the next start.
    Restart,
}

impl Live {
    pub fn as_str(self) -> &'static str {
        match self {
            Live::Safe => "safe",
            Live::NextFrame => "next_frame",
            Live::Restart => "restart",
        }
    }
}

/// What a tunable holds and may be.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Bool,
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    Choice(&'static [&'static str]),
}

/// What one declaration says.
#[derive(Clone, Copy, Debug)]
pub struct Meta {
    /// The Rust name as declared (`VIEW_GUARD`); the key is its lowercase form.
    pub ident: &'static str,
    /// The environment variable.
    pub env: &'static str,
    pub desc: &'static str,
    pub live: Live,
    pub kind: Kind,
    /// The default as stored bits.
    pub default: u64,
}

/// What a set did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    /// Kept until the next frame boundary.
    PendingNextFrame,
    /// Kept in the file; needs a restart.
    NeedsRestart,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Applied => "applied",
            Outcome::PendingNextFrame => "pending_next_frame",
            Outcome::NeedsRestart => "needs_restart",
        }
    }
}

/// A type a tunable reads as.
pub trait Value: Copy + Send + Sync + 'static {
    fn from_bits(bits: u64) -> Self;
    fn to_bits(self) -> u64;
}

impl Value for bool {
    fn from_bits(bits: u64) -> Self {
        bits != 0
    }
    fn to_bits(self) -> u64 {
        u64::from(self)
    }
}
impl Value for i64 {
    fn from_bits(bits: u64) -> Self {
        bits as i64
    }
    fn to_bits(self) -> u64 {
        self as u64
    }
}
impl Value for f64 {
    fn from_bits(bits: u64) -> Self {
        f64::from_bits(bits)
    }
    fn to_bits(self) -> u64 {
        f64::to_bits(self)
    }
}
/// A choice: the index in the allowed list.
impl Value for usize {
    fn from_bits(bits: u64) -> Self {
        bits as usize
    }
    fn to_bits(self) -> u64 {
        self as u64
    }
}

const SRC_DEFAULT: u8 = 0;
const SRC_FILE: u8 = 1;
const SRC_ENV: u8 = 2;
const SRC_LIVE: u8 = 3;

/// The type-free part of a tunable: what the registry holds.
pub struct Slot {
    meta: Meta,
    cur: AtomicU64,
    pending: AtomicU64,
    has_pending: AtomicBool,
    source: AtomicU8,
    env_locked: AtomicBool,
    once: Once,
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tunable({})", self.meta.ident)
    }
}

/// A declared tunable. Use [`tunable!`].
pub struct Tunable<T: Value> {
    slot: Slot,
    _t: PhantomData<fn() -> T>,
}

impl<T: Value> Tunable<T> {
    pub const fn new(meta: Meta) -> Self {
        Tunable {
            slot: Slot {
                meta,
                cur: AtomicU64::new(meta.default),
                pending: AtomicU64::new(0),
                has_pending: AtomicBool::new(false),
                source: AtomicU8::new(SRC_DEFAULT),
                env_locked: AtomicBool::new(false),
                once: Once::new(),
            },
            _t: PhantomData,
        }
    }

    /// The value now.
    #[inline]
    pub fn get(&'static self) -> T {
        self.slot.ready();
        T::from_bits(self.slot.cur.load(Relaxed))
    }

    /// Makes the registry know it (and reads its start value).
    pub fn register(&'static self) {
        self.slot.ready();
    }

    pub fn slot(&'static self) -> &'static Slot {
        self.slot.ready();
        &self.slot
    }

    /// Sets it (same rules as the text form: range checked, live class).
    pub fn set(&'static self, value: T) -> Result<Outcome, String> {
        self.slot.ready();
        self.slot.set_text(&fmt(&self.slot.meta, value.to_bits()))
    }

    /// Runs `f` with the new value each time it changes (after it applied).
    pub fn on_change(&'static self, f: impl Fn(T) + Send + Sync + 'static) {
        self.slot.ready();
        let cb: Callback = Arc::new(move |s: &Slot| f(T::from_bits(s.cur.load(Relaxed))));
        lock(&CALLBACKS).push((Some(&self.slot as *const Slot as usize), cb));
    }

    /// Test hook: reads the start value from the given environment and file lookups instead of the real ones.
    #[doc(hidden)]
    pub fn init_with(&'static self, env: &dyn Fn(&str) -> Option<String>, file: &dyn Fn(&str) -> Option<String>) {
        self.slot.once.call_once(|| self.slot.init_inner(env, file));
    }
}

impl Tunable<usize> {
    /// The chosen text.
    pub fn get_str(&'static self) -> &'static str {
        let i = self.get();
        match self.slot.meta.kind {
            Kind::Choice(list) => list.get(i).copied().unwrap_or(""),
            _ => "",
        }
    }
}

type Callback = Arc<dyn Fn(&Slot) + Send + Sync>;
static CALLBACKS: Mutex<Vec<(Option<usize>, Callback)>> = Mutex::new(Vec::new());
static REGISTRY: Mutex<Vec<&'static Slot>> = Mutex::new(Vec::new());
static GENERATION: AtomicU64 = AtomicU64::new(0);
static PENDING_NEXT: AtomicBool = AtomicBool::new(false);
static DIRTY: AtomicBool = AtomicBool::new(false);
static WATCHING: AtomicBool = AtomicBool::new(false);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Bumped on every change of any tunable.
pub fn generation() -> u64 {
    GENERATION.load(Relaxed)
}

/// Runs `f(key, new value text)` on every change of any tunable.
pub fn on_any_change(f: impl Fn(&str, &str) + Send + Sync + 'static) {
    let cb: Callback = Arc::new(move |s: &Slot| f(&s.key(), &s.value_text()));
    lock(&CALLBACKS).push((None, cb));
}

/// Applies the changes kept as pending for [`Live::NextFrame`]. The GPU host
/// calls it once per frame; with nothing pending it is one relaxed load.
#[inline]
pub fn frame_boundary() {
    if !PENDING_NEXT.load(Relaxed) {
        return;
    }
    PENDING_NEXT.store(false, Relaxed);
    let slots: Vec<&'static Slot> = lock(&REGISTRY).clone();
    for s in slots {
        if s.meta.live == Live::NextFrame && s.has_pending.swap(false, Relaxed) {
            s.apply_bits(s.pending.load(Relaxed), SRC_LIVE);
        }
    }
}

// ---------------------------------------------------------------- text

/// `const` index of `s` in `list`; a compile error when it is not there.
pub const fn choice_index(list: &[&str], s: &str) -> usize {
    let mut i = 0;
    while i < list.len() {
        if const_eq(list[i], s) {
            return i;
        }
        i += 1;
    }
    panic!("the default is not in the list of choices")
}

const fn const_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn unquote(text: &str) -> &str {
    text.trim().trim_matches('"').trim()
}

/// Text to stored bits. `strict`: out of range is an error; else it is clamped.
fn parse(meta: &Meta, text: &str, strict: bool) -> Result<u64, String> {
    let t = unquote(text);
    let key = meta.ident.to_ascii_lowercase();
    match meta.kind {
        Kind::Bool => match t.to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => Ok(1),
            "0" | "false" | "off" | "no" => Ok(0),
            _ => Err(format!("{key} is on/off (true or false), not {text:?}")),
        },
        Kind::Int { min, max } => {
            let v: i64 = t.parse().map_err(|_| format!("{key} is a whole number, not {text:?}"))?;
            if (min..=max).contains(&v) {
                Ok(v as u64)
            } else if strict {
                Err(format!("{key} must be {min} to {max}, not {v}"))
            } else {
                Ok(v.clamp(min, max) as u64)
            }
        }
        Kind::Float { min, max } => {
            let v: f64 = t.parse().map_err(|_| format!("{key} is a number, not {text:?}"))?;
            if !v.is_finite() {
                return Err(format!("{key} is a finite number, not {text:?}"));
            }
            if (min..=max).contains(&v) {
                Ok(v.to_bits())
            } else if strict {
                Err(format!("{key} must be {min} to {max}, not {v}"))
            } else {
                Ok(v.clamp(min, max).to_bits())
            }
        }
        Kind::Choice(list) => list
            .iter()
            .position(|c| c.eq_ignore_ascii_case(t))
            .map(|i| i as u64)
            .ok_or_else(|| format!("{key} is one of {}, not {text:?}", list.join(", "))),
    }
}

/// Stored bits to the text kept in the file.
fn fmt(meta: &Meta, bits: u64) -> String {
    match meta.kind {
        Kind::Bool => (bits != 0).to_string(),
        Kind::Int { .. } => (bits as i64).to_string(),
        Kind::Float { .. } => format!("{:?}", f64::from_bits(bits)),
        Kind::Choice(list) => list.get(bits as usize).copied().unwrap_or("").to_owned(),
    }
}

fn to_json(meta: &Meta, bits: u64) -> Json {
    match meta.kind {
        Kind::Bool => Json::Bool(bits != 0),
        Kind::Int { .. } => Json::Int(bits as i64),
        Kind::Float { .. } => Json::num(f64::from_bits(bits)),
        Kind::Choice(_) => Json::str(fmt(meta, bits)),
    }
}

// ---------------------------------------------------------------- slot

impl Slot {
    #[inline]
    fn ready(&'static self) {
        self.once.call_once(|| self.init_inner(&|n| std::env::var(n).ok(), &file_value));
    }

    fn init_inner(&'static self, env: &dyn Fn(&str) -> Option<String>, file: &dyn Fn(&str) -> Option<String>) {
        let key = self.key();
        let mut bits = self.meta.default;
        let mut source = SRC_DEFAULT;
        if let Some(text) = file(&key) {
            match parse(&self.meta, &text, false) {
                Ok(b) => (bits, source) = (b, SRC_FILE),
                Err(e) => tracing::warn!(target: "victory::tunables", "live file: {e}; ignored"),
            }
        }
        if let Some(text) = env(self.meta.env) {
            match parse(&self.meta, &text, false) {
                Ok(b) => {
                    (bits, source) = (b, SRC_ENV);
                    self.env_locked.store(true, Relaxed);
                }
                Err(e) => tracing::warn!(target: "victory::tunables", "{}: {e}; ignored", self.meta.env),
            }
        }
        self.cur.store(bits, Relaxed);
        self.source.store(source, Relaxed);
        let mut reg = lock(&REGISTRY);
        if !reg.iter().any(|s| std::ptr::eq(*s, self)) {
            reg.push(self);
        }
    }

    /// The lowercase name: the key in the file and in the tools.
    pub fn key(&self) -> String {
        self.meta.ident.to_ascii_lowercase()
    }

    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    pub fn value_text(&self) -> String {
        fmt(&self.meta, self.cur.load(Relaxed))
    }

    pub fn default_text(&self) -> String {
        fmt(&self.meta, self.meta.default)
    }

    pub fn needs_restart(&self) -> bool {
        self.meta.live == Live::Restart && self.has_pending.load(Relaxed)
    }

    /// The value waiting for the next frame or the next start, if there is one.
    pub fn pending_text(&self) -> Option<String> {
        self.has_pending.load(Relaxed).then(|| fmt(&self.meta, self.pending.load(Relaxed)))
    }

    fn apply_bits(&self, bits: u64, source: u8) {
        let was = self.cur.swap(bits, Relaxed);
        self.source.store(source, Relaxed);
        if was != bits {
            GENERATION.fetch_add(1, Relaxed);
            self.fire();
        }
    }

    fn fire(&self) {
        let me = self as *const Slot as usize;
        let list: Vec<Callback> = lock(&CALLBACKS).iter().filter(|(a, _)| a.is_none_or(|a| a == me)).map(|(_, f)| f.clone()).collect();
        for f in list {
            f(self);
        }
    }

    /// Stores a value by its live class.
    fn store(&self, bits: u64, source: u8) -> Outcome {
        match self.meta.live {
            Live::Safe => {
                self.apply_bits(bits, source);
                Outcome::Applied
            }
            Live::NextFrame => {
                if bits == self.cur.load(Relaxed) {
                    self.has_pending.store(false, Relaxed);
                    Outcome::Applied
                } else {
                    self.pending.store(bits, Relaxed);
                    self.has_pending.store(true, Relaxed);
                    PENDING_NEXT.store(true, Relaxed);
                    GENERATION.fetch_add(1, Relaxed);
                    Outcome::PendingNextFrame
                }
            }
            Live::Restart => {
                if bits == self.cur.load(Relaxed) {
                    self.has_pending.store(false, Relaxed);
                    Outcome::Applied
                } else {
                    self.pending.store(bits, Relaxed);
                    self.has_pending.store(true, Relaxed);
                    GENERATION.fetch_add(1, Relaxed);
                    Outcome::NeedsRestart
                }
            }
        }
    }

    /// Sets from text: the tool and panel path. Range checked; kept in the file.
    pub fn set_text(&'static self, text: &str) -> Result<Outcome, String> {
        self.ready();
        let bits = parse(&self.meta, text, true)?;
        let outcome = self.store(bits, SRC_LIVE);
        record(&self.key(), Some(fmt(&self.meta, bits)));
        Ok(outcome)
    }

    /// Back to the default; the file forgets the key.
    pub fn reset(&'static self) -> Outcome {
        self.ready();
        let outcome = self.store(self.meta.default, SRC_DEFAULT);
        record(&self.key(), None);
        outcome
    }

    pub fn to_json(&'static self) -> Json {
        self.ready();
        let m = &self.meta;
        let cur = self.cur.load(Relaxed);
        let mut o: Vec<(String, Json)> = Vec::new();
        let mut put = |k: &str, v: Json| o.push((k.to_owned(), v));
        put("name", Json::str(self.key()));
        put("env", Json::str(m.env));
        let ty = match m.kind {
            Kind::Bool => "bool",
            Kind::Int { .. } => "int",
            Kind::Float { .. } => "float",
            Kind::Choice(_) => "enum",
        };
        put("type", Json::str(ty));
        put("value", to_json(m, cur));
        put("default", to_json(m, m.default));
        match m.kind {
            Kind::Int { min, max } => {
                put("min", Json::Int(min));
                put("max", Json::Int(max));
            }
            Kind::Float { min, max } => {
                put("min", Json::num(min));
                put("max", Json::num(max));
            }
            Kind::Choice(list) => put("allowed", Json::Arr(list.iter().map(|c| Json::str(*c)).collect())),
            Kind::Bool => {}
        }
        put("live", Json::str(m.live.as_str()));
        put("description", Json::str(m.desc));
        put("changed_from_default", Json::Bool(cur != m.default));
        put(
            "source",
            Json::str(match self.source.load(Relaxed) {
                SRC_FILE => "file",
                SRC_ENV => "env",
                SRC_LIVE => "live",
                _ => "default",
            }),
        );
        put("pending", if self.has_pending.load(Relaxed) { to_json(m, self.pending.load(Relaxed)) } else { Json::Null });
        put("needs_restart", Json::Bool(self.needs_restart()));
        Json::Obj(o)
    }
}

// ---------------------------------------------------------------- declaring

/// Declares a tunable (see the module text).
#[macro_export]
macro_rules! tunable {
    ($vis:vis $name:ident : bool = $d:expr, $env:literal, $desc:literal, live = $live:ident) => {
        $vis static $name: $crate::tunables::Tunable<bool> = $crate::tunables::Tunable::new($crate::tunables::Meta {
            ident: stringify!($name),
            env: $env,
            desc: $desc,
            live: $crate::tunables::Live::$live,
            kind: $crate::tunables::Kind::Bool,
            default: ($d) as u64,
        });
    };
    ($vis:vis $name:ident : i64 = $d:expr, $min:literal ..= $max:literal, $env:literal, $desc:literal, live = $live:ident) => {
        $vis static $name: $crate::tunables::Tunable<i64> = $crate::tunables::Tunable::new($crate::tunables::Meta {
            ident: stringify!($name),
            env: $env,
            desc: $desc,
            live: $crate::tunables::Live::$live,
            kind: $crate::tunables::Kind::Int { min: $min, max: $max },
            default: ($d) as i64 as u64,
        });
    };
    ($vis:vis $name:ident : f64 = $d:expr, $min:literal ..= $max:literal, $env:literal, $desc:literal, live = $live:ident) => {
        $vis static $name: $crate::tunables::Tunable<f64> = $crate::tunables::Tunable::new($crate::tunables::Meta {
            ident: stringify!($name),
            env: $env,
            desc: $desc,
            live: $crate::tunables::Live::$live,
            kind: $crate::tunables::Kind::Float { min: $min, max: $max },
            default: f64::to_bits($d),
        });
    };
    ($vis:vis $name:ident : enum = $d:literal, [$($v:literal),+ $(,)?], $env:literal, $desc:literal, live = $live:ident) => {
        $vis static $name: $crate::tunables::Tunable<usize> = $crate::tunables::Tunable::new($crate::tunables::Meta {
            ident: stringify!($name),
            env: $env,
            desc: $desc,
            live: $crate::tunables::Live::$live,
            kind: $crate::tunables::Kind::Choice(&[$($v),+]),
            default: $crate::tunables::choice_index(&[$($v),+], $d) as u64,
        });
    };
}

/// Makes `pub fn register_all()` that registers the listed tunables.
#[macro_export]
macro_rules! tunable_list {
    ($vis:vis fn $f:ident : $($name:path),* $(,)?) => {
        $vis fn $f() {
            $( $name.register(); )*
        }
    };
}

// ---------------------------------------------------------------- registry

fn known_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(known::register_all);
}

fn all() -> Vec<&'static Slot> {
    known_once();
    let mut v = lock(&REGISTRY).clone();
    v.sort_by_key(|s| s.key());
    v
}

fn find(name: &str) -> Result<&'static Slot, String> {
    let n = name.trim();
    let list = all();
    if let Some(s) = list.iter().find(|s| s.meta.ident.eq_ignore_ascii_case(n) || s.meta.env.eq_ignore_ascii_case(n)) {
        return Ok(s);
    }
    let low = n.to_ascii_lowercase();
    let near: Vec<String> = list.iter().filter(|s| !low.is_empty() && s.key().contains(&low)).take(5).map(|s| s.key()).collect();
    if near.is_empty() {
        Err(format!("unknown tunable {name:?}"))
    } else {
        Err(format!("unknown tunable {name:?}; did you mean {}", near.join(", ")))
    }
}

/// Every registered tunable, sorted by name.
pub fn list() -> Vec<&'static Slot> {
    all()
}

/// The tunable `name` (the lowercase name, the declared name or the variable).
pub fn get(name: &str) -> Result<&'static Slot, String> {
    find(name)
}

/// Sets `name` from text.
pub fn set(name: &str, text: &str) -> Result<Outcome, String> {
    find(name)?.set_text(text)
}

/// Back to the default.
pub fn reset(name: &str) -> Result<Outcome, String> {
    Ok(find(name)?.reset())
}

/// `list_tunables`.
pub fn list_json(filter: &str) -> Json {
    let f = filter.trim().to_ascii_lowercase();
    let items: Vec<Json> = all()
        .into_iter()
        .filter(|s| f.is_empty() || s.key().contains(&f) || s.meta.env.to_ascii_lowercase().contains(&f) || s.meta.desc.to_ascii_lowercase().contains(&f))
        .map(Slot::to_json)
        .collect();
    Json::obj([
        ("count", Json::uint(items.len() as u64)),
        ("generation", Json::uint(generation())),
        ("file", file_path().map_or(Json::Null, |p| Json::str(p.display().to_string()))),
        ("tunables", Json::Arr(items)),
    ])
}

/// `get_tunable`.
pub fn get_json(name: &str) -> Result<Json, String> {
    Ok(find(name)?.to_json())
}

/// `set_tunable`: the tunable after the change, and what the change did.
pub fn set_json(name: &str, text: &str) -> Result<Json, String> {
    let slot = find(name)?;
    let outcome = slot.set_text(text)?;
    Ok(with_outcome(slot, outcome))
}

/// `reset_tunable`.
pub fn reset_json(name: &str) -> Result<Json, String> {
    let slot = find(name)?;
    let outcome = slot.reset();
    Ok(with_outcome(slot, outcome))
}

fn with_outcome(slot: &'static Slot, outcome: Outcome) -> Json {
    let Json::Obj(mut o) = slot.to_json() else { unreachable!() };
    o.push(("outcome".to_owned(), Json::str(outcome.as_str())));
    Json::Obj(o)
}

/// Starts the file watcher, reads the known tunables and gives the debug
/// server its tools. The runtime calls it once, before the first frame.
pub fn init() {
    known_once();
    victory_mcp::set_tunable_hooks(victory_mcp::TunableHooks { list: list_json, get: get_json, set: set_json, reset: reset_json });
    start_watcher();
}

// ---------------------------------------------------------------- the file

struct State {
    loaded: bool,
    path: Option<PathBuf>,
    values: BTreeMap<String, String>,
    mtime: Option<SystemTime>,
}

static STATE: Mutex<State> = Mutex::new(State { loaded: false, path: None, values: BTreeMap::new(), mtime: None });

/// Where the settings are kept: the file `VICTORY_LIVE_SETTINGS` names, else
/// `victory-live.toml` in the parent of `VICTORY_DATA_DIR`, else the default
/// place if its folder is there.
pub fn live_path(file: Option<&std::ffi::OsStr>, data_dir: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    if let Some(file) = file.filter(|f| !f.is_empty()) {
        return Some(PathBuf::from(file));
    }
    if let Some(parent) = data_dir.filter(|d| !d.is_empty()).and_then(|d| Path::new(d).parent().map(PathBuf::from))
        && !parent.as_os_str().is_empty()
    {
        return Some(parent.join("victory-live.toml"));
    }
    let default = PathBuf::from(r"K:\Leonida\cool_fox");
    default.is_dir().then(|| default.join("victory-live.toml"))
}

fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn ensure_loaded(st: &mut State) {
    if st.loaded {
        return;
    }
    st.loaded = true;
    st.path = live_path(std::env::var_os("VICTORY_LIVE_SETTINGS").as_deref(), std::env::var_os("VICTORY_DATA_DIR").as_deref());
    if let Some(p) = &st.path {
        st.mtime = mtime_of(p);
        st.values = std::fs::read_to_string(p).map(|t| parse_section(&t)).unwrap_or_default();
    }
}

fn file_value(key: &str) -> Option<String> {
    let mut st = lock(&STATE);
    ensure_loaded(&mut st);
    st.values.get(key).cloned()
}

/// The file in use.
pub fn file_path() -> Option<PathBuf> {
    let mut st = lock(&STATE);
    ensure_loaded(&mut st);
    st.path.clone()
}

/// Uses another file (tests, tools); reads its `[tunables]`.
pub fn set_live_path(path: Option<PathBuf>) {
    let mut st = lock(&STATE);
    st.loaded = true;
    st.mtime = path.as_deref().and_then(mtime_of);
    st.values = path.as_deref().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse_section(&t)).unwrap_or_default();
    st.path = path;
}

const HEADER: &str = "[tunables]";

fn is_header(line: &str) -> bool {
    line.trim_start().starts_with('[')
}

/// The `key = value` pairs of the `[tunables]` table.
pub fn parse_section(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut inside = false;
    for line in text.lines() {
        if is_header(line) {
            inside = line.split('#').next().unwrap_or("").trim() == HEADER;
            continue;
        }
        if !inside {
            continue;
        }
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some((k, v)) = line.split_once('=') {
            out.insert(k.trim().to_ascii_lowercase(), unquote(v).to_owned());
        }
    }
    out
}

fn section_text(values: &BTreeMap<String, String>, meta_quote: impl Fn(&str, &str) -> String) -> String {
    let mut s = format!("{HEADER}\n");
    for (k, v) in values {
        s.push_str(&format!("{k} = {}\n", meta_quote(k, v)));
    }
    s
}

/// A value as the file holds it: choices in quotes, the rest bare.
fn file_text(key: &str, value: &str) -> String {
    let is_choice = lock(&REGISTRY).iter().any(|s| s.key() == key && matches!(s.meta.kind, Kind::Choice(_)));
    if is_choice { format!("\"{value}\"") } else { value.to_owned() }
}

/// `text` with its `[tunables]` table replaced by `values` (appended when it had none); the rest is kept as it is.
pub fn splice_section(text: &str, values: &BTreeMap<String, String>) -> String {
    let new = if values.is_empty() { String::new() } else { section_text(values, file_text) };
    let mut out = String::new();
    let mut inside = false;
    let mut placed = false;
    for line in text.split_inclusive('\n') {
        if is_header(line) {
            inside = line.split('#').next().unwrap_or("").trim() == HEADER;
            if inside {
                if !placed {
                    out.push_str(&new);
                    placed = true;
                }
                continue;
            }
        }
        if !inside {
            out.push_str(line);
        }
    }
    if !placed && !new.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&new);
    }
    out
}

/// The text of the `[tunables]` table in `text`, header included.
pub fn extract_section(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut inside = false;
    for line in text.split_inclusive('\n') {
        if is_header(line) {
            inside = line.split('#').next().unwrap_or("").trim() == HEADER;
        }
        if inside {
            out.push_str(line);
        }
    }
    (!out.is_empty()).then_some(out)
}

fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Writes the live settings file with `top` (the runtime's own keys) and
/// keeps the `[tunables]` table that is in the file now.
pub fn write_settings_text(path: &Path, top: &str) -> io::Result<()> {
    let mut st = lock(&STATE);
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut text = top.to_owned();
    if let Some(section) = extract_section(&existing) {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
        text.push_str(&section);
    }
    write_atomic(path, &text)?;
    if st.path.as_deref() == Some(path) {
        st.mtime = mtime_of(path);
    }
    Ok(())
}

/// Keeps `key = value` (or forgets the key) and saves, now or soon.
fn record(key: &str, value: Option<String>) {
    {
        let mut st = lock(&STATE);
        ensure_loaded(&mut st);
        match value {
            Some(v) => st.values.insert(key.to_owned(), v),
            None => st.values.remove(key),
        };
    }
    if WATCHING.load(Relaxed) {
        DIRTY.store(true, Relaxed);
    } else {
        flush();
    }
}

/// Writes the `[tunables]` table now (the other keys of the file stay).
pub fn flush() {
    DIRTY.store(false, Relaxed);
    let mut st = lock(&STATE);
    let Some(path) = st.path.clone() else { return };
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let text = splice_section(&existing, &st.values);
    if text == existing {
        return;
    }
    match write_atomic(&path, &text) {
        Ok(()) => st.mtime = mtime_of(&path),
        Err(e) => tracing::warn!(target: "victory::tunables", file = %path.display(), error = %e, "tunables not saved"),
    }
}

/// Reads the file again and applies the values that changed in it (a value
/// from the environment stays). Returns how many tunables took a new value.
pub fn reload_file() -> usize {
    let (old, new) = {
        let mut st = lock(&STATE);
        ensure_loaded(&mut st);
        let Some(path) = st.path.clone() else { return 0 };
        st.mtime = mtime_of(&path);
        let Ok(text) = std::fs::read_to_string(&path) else { return 0 };
        let new = parse_section(&text);
        (std::mem::replace(&mut st.values, new.clone()), new)
    };
    let mut n = 0;
    let slots = lock(&REGISTRY).clone();
    for s in slots {
        let key = s.key();
        let Some(value) = new.get(&key) else { continue };
        if old.get(&key) == Some(value) || s.env_locked.load(Relaxed) {
            continue;
        }
        match parse(&s.meta, value, false) {
            Ok(bits) => {
                let before = (s.cur.load(Relaxed), s.has_pending.load(Relaxed));
                s.store(bits, SRC_FILE);
                if before != (s.cur.load(Relaxed), s.has_pending.load(Relaxed)) {
                    n += 1;
                }
                tracing::info!(target: "victory::tunables", name = %key, value = %value, "tunable changed in the file");
            }
            Err(e) => tracing::warn!(target: "victory::tunables", "live file: {e}; ignored"),
        }
    }
    n
}

fn start_watcher() {
    if WATCHING.swap(true, Relaxed) {
        return;
    }
    let spawned = std::thread::Builder::new().name("victory tunables".into()).spawn(|| {
        loop {
            std::thread::sleep(Duration::from_millis(500));
            if DIRTY.load(Relaxed) {
                flush();
                continue;
            }
            let (path, seen) = {
                let st = lock(&STATE);
                (st.path.clone(), st.mtime)
            };
            if let Some(p) = path
                && mtime_of(&p) != seen
            {
                reload_file();
            }
        }
    });
    if spawned.is_err() {
        WATCHING.store(false, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        lock(&SERIAL)
    }

    fn none(_: &str) -> Option<String> {
        None
    }

    crate::tunable!(T_BOOL: bool = true, "VICTORY_T_BOOL", "a bool", live = Safe);
    crate::tunable!(T_INT: i64 = 5, 0..=10, "VICTORY_T_INT", "an int", live = Safe);
    crate::tunable!(T_FLOAT: f64 = 1.5, 0.0..=4.0, "VICTORY_T_FLOAT", "a float", live = Safe);
    crate::tunable!(T_ENUM: enum = "b", ["a", "b", "c"], "VICTORY_T_ENUM", "an enum", live = Safe);
    crate::tunable!(T_PRIO: i64 = 1, 0..=100, "VICTORY_T_PRIO", "priority", live = Safe);
    crate::tunable!(T_NEXT: i64 = 1, 0..=100, "VICTORY_T_NEXT", "next frame", live = NextFrame);
    crate::tunable!(T_RESTART: i64 = 1, 0..=100, "VICTORY_T_RESTART", "restart only", live = Restart);
    crate::tunable!(T_FILE: i64 = 1, 0..=100, "VICTORY_T_FILE", "file round trip", live = Safe);
    crate::tunable!(T_CB: i64 = 1, 0..=100, "VICTORY_T_CB", "callback", live = Safe);

    fn up(t: &'static Tunable<impl Value>) {
        t.init_with(&none, &none);
    }

    #[test]
    fn parse_and_range_errors() {
        up(&T_BOOL);
        up(&T_INT);
        up(&T_FLOAT);
        up(&T_ENUM);
        let _g = serial();
        set_live_path(None);
        assert_eq!(T_INT.set(11).unwrap_err(), "t_int must be 0 to 10, not 11");
        assert!(T_INT.slot().set_text("abc").unwrap_err().contains("whole number"));
        assert!(T_INT.slot().set_text("-1").unwrap_err().contains("0 to 10"));
        assert!(T_BOOL.slot().set_text("maybe").unwrap_err().contains("on/off"));
        assert!(T_FLOAT.slot().set_text("9").unwrap_err().contains("0 to 4"));
        assert!(T_FLOAT.slot().set_text("nan").unwrap_err().contains("finite"));
        assert!(T_ENUM.slot().set_text("z").unwrap_err().contains("a, b, c"));
        assert_eq!(T_INT.get(), 5, "a refused value changes nothing");
        assert_eq!(T_BOOL.slot().set_text("off"), Ok(Outcome::Applied));
        assert!(!T_BOOL.get());
        assert_eq!(T_ENUM.slot().set_text("C"), Ok(Outcome::Applied));
        assert_eq!((T_ENUM.get(), T_ENUM.get_str()), (2, "c"));
        assert_eq!(T_FLOAT.slot().set_text("2.25"), Ok(Outcome::Applied));
        assert_eq!(T_FLOAT.get(), 2.25);
        let e = get("nope_x").unwrap_err();
        assert!(e.contains("unknown tunable"), "{e}");
        assert!(get("t_in").unwrap_err().contains("t_int"), "near names are offered");
        assert!(get("VICTORY_T_INT").is_ok());
        assert!(get("T_INT").is_ok());
    }

    #[test]
    fn env_beats_file_beats_default_and_numbers_clamp() {
        let _g = serial();
        let env = |n: &str| (n == "VICTORY_T_PRIO").then(|| "7".to_owned());
        let file = |k: &str| (k == "t_prio").then(|| "3".to_owned());
        T_PRIO.init_with(&env, &file);
        assert_eq!(T_PRIO.get(), 7);
        assert_eq!(T_PRIO.slot().to_json().get("source").and_then(Json::as_str), Some("env"));
        crate::tunable!(P2: i64 = 1, 0..=100, "VICTORY_P2", "p2", live = Safe);
        P2.init_with(&none, &|k| (k == "p2").then(|| "3".to_owned()));
        assert_eq!(P2.get(), 3);
        crate::tunable!(P3: i64 = 1, 0..=100, "VICTORY_P3", "p3", live = Safe);
        P3.init_with(&|_| Some("500".into()), &none);
        assert_eq!(P3.get(), 100, "out of range from the environment is clamped");
        crate::tunable!(P4: i64 = 4, 0..=100, "VICTORY_P4", "p4", live = Safe);
        P4.init_with(&|_| Some("junk".into()), &|_| Some("junk".into()));
        assert_eq!(P4.get(), 4, "unparsable values are ignored");
        // The file does not override the environment when it changes.
        assert!(T_PRIO.slot().env_locked.load(Relaxed));
    }

    #[test]
    fn next_frame_is_pending_until_the_boundary() {
        let _g = serial();
        set_live_path(None);
        up(&T_NEXT);
        let g = generation();
        assert_eq!(T_NEXT.set(9), Ok(Outcome::PendingNextFrame));
        assert_eq!(T_NEXT.get(), 1);
        assert!(generation() > g);
        let j = T_NEXT.slot().to_json();
        assert_eq!(j.get("pending"), Some(&Json::Int(9)));
        frame_boundary();
        assert_eq!(T_NEXT.get(), 9);
        assert_eq!(T_NEXT.slot().to_json().get("pending"), Some(&Json::Null));
        frame_boundary();
        assert_eq!(T_NEXT.get(), 9);
    }

    #[test]
    fn restart_is_kept_and_reported() {
        let _g = serial();
        set_live_path(None);
        up(&T_RESTART);
        assert_eq!(T_RESTART.set(40), Ok(Outcome::NeedsRestart));
        assert_eq!(T_RESTART.get(), 1);
        assert!(T_RESTART.slot().needs_restart());
        frame_boundary();
        assert_eq!(T_RESTART.get(), 1);
        let j = set_json("t_restart", "50").unwrap();
        assert_eq!(j.get("outcome").and_then(Json::as_str), Some("needs_restart"));
        assert_eq!(j.get("pending"), Some(&Json::Int(50)));
        assert_eq!(T_RESTART.set(1), Ok(Outcome::Applied));
        assert!(!T_RESTART.slot().needs_restart());
    }

    #[test]
    fn file_round_trip_keeps_the_other_keys_and_reloads_edits() {
        let _g = serial();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("victory-live.toml");
        std::fs::write(&path, "texture_pool_mb = 8192\nroom_percent = 80\nevict = true\n").unwrap();
        set_live_path(Some(path.clone()));
        up(&T_FILE);
        up(&T_ENUM);
        assert_eq!(T_FILE.set(42), Ok(Outcome::Applied));
        T_ENUM.slot().set_text("a").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("texture_pool_mb = 8192\nroom_percent = 80\nevict = true\n"), "{text}");
        assert!(text.contains("[tunables]\n"), "{text}");
        assert!(text.contains("t_file = 42\n"), "{text}");
        assert!(text.contains("t_enum = \"a\"\n"), "{text}");
        let parsed = parse_section(&text);
        assert_eq!(parsed.get("t_file").map(String::as_str), Some("42"));
        assert_eq!(parsed.get("t_enum").map(String::as_str), Some("a"));
        // The runtime's own writer keeps the table.
        write_settings_text(&path, "texture_pool_mb = 4096\nevict = false\n").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("texture_pool_mb = 4096\n") && text.contains("t_file = 42"), "{text}");
        // A start reads it back.
        crate::tunable!(R_FILE: i64 = 1, 0..=100, "VICTORY_R_FILE", "r", live = Safe);
        R_FILE.init_with(&none, &file_value);
        assert_eq!(R_FILE.get(), 1);
        assert_eq!(file_value("t_file").as_deref(), Some("42"));
        // Edited in an editor: applies on reload.
        let edited = text.replace("t_file = 42", "t_file = 77");
        std::fs::write(&path, edited).unwrap();
        assert!(reload_file() >= 1);
        assert_eq!(T_FILE.get(), 77);
        assert_eq!(reload_file(), 0, "nothing new, nothing changes");
        // Reset forgets the key.
        assert_eq!(reset("t_file"), Ok(Outcome::Applied));
        assert_eq!(T_FILE.get(), 1);
        assert!(!parse_section(&std::fs::read_to_string(&path).unwrap()).contains_key("t_file"));
        set_live_path(None);
    }

    #[test]
    fn splice_and_extract() {
        let mut m = BTreeMap::new();
        m.insert("a".to_owned(), "1".to_owned());
        let t = splice_section("x = 1\n[other]\ny = 2\n", &m);
        assert_eq!(t, "x = 1\n[other]\ny = 2\n\n[tunables]\na = 1\n");
        m.insert("b".to_owned(), "2".to_owned());
        let t2 = splice_section(&t, &m);
        assert_eq!(t2, "x = 1\n[other]\ny = 2\n\n[tunables]\na = 1\nb = 2\n");
        assert_eq!(splice_section(&t2, &BTreeMap::new()), "x = 1\n[other]\ny = 2\n\n");
        assert_eq!(extract_section(&t2).as_deref(), Some("[tunables]\na = 1\nb = 2\n"));
        assert_eq!(parse_section("a = 3\n[tunables]\nk = \"v\" # c\n[z]\nq = 1\n").get("k").map(String::as_str), Some("v"));
    }

    #[test]
    fn callbacks_and_generation() {
        let _g = serial();
        set_live_path(None);
        up(&T_CB);
        static SEEN: AtomicU64 = AtomicU64::new(0);
        T_CB.on_change(|v| SEEN.store(v as u64, Relaxed));
        let g = generation();
        T_CB.set(33).unwrap();
        assert_eq!(SEEN.load(Relaxed), 33);
        assert!(generation() > g);
        let g = generation();
        T_CB.set(33).unwrap();
        assert_eq!(generation(), g, "the same value is no change");
    }

    #[test]
    fn list_json_shape() {
        let _g = serial();
        set_live_path(None);
        up(&T_INT);
        up(&T_ENUM);
        up(&T_BOOL);
        let l = list_json("t_int");
        assert_eq!(l.get("count"), Some(&Json::Int(1)));
        let Some(Json::Arr(items)) = l.get("tunables") else { panic!() };
        let t = &items[0];
        assert_eq!(t.get("name").and_then(Json::as_str), Some("t_int"));
        assert_eq!(t.get("env").and_then(Json::as_str), Some("VICTORY_T_INT"));
        assert_eq!(t.get("type").and_then(Json::as_str), Some("int"));
        assert_eq!(t.get("default"), Some(&Json::Int(5)));
        assert_eq!((t.get("min"), t.get("max")), (Some(&Json::Int(0)), Some(&Json::Int(10))));
        assert_eq!(t.get("live").and_then(Json::as_str), Some("safe"));
        assert_eq!(t.get("description").and_then(Json::as_str), Some("an int"));
        assert!(matches!(t.get("changed_from_default"), Some(Json::Bool(_))));
        let e = get_json("t_enum").unwrap();
        assert_eq!(e.get("allowed"), Some(&Json::Arr(["a", "b", "c"].map(Json::str).to_vec())));
        assert_eq!(e.get("type").and_then(Json::as_str), Some("enum"));
        let s = set_json("t_bool", "false").unwrap();
        assert_eq!(s.get("value"), Some(&Json::Bool(false)));
        assert_eq!(s.get("changed_from_default"), Some(&Json::Bool(true)));
        assert_eq!(s.get("outcome").and_then(Json::as_str), Some("applied"));
        assert_eq!(s.get("source").and_then(Json::as_str), Some("live"));
        assert!(set_json("t_int", "99").is_err());
        reset_json("t_bool").unwrap();
        assert_eq!(get_json("t_bool").unwrap().get("changed_from_default"), Some(&Json::Bool(false)));
        // Always parseable by the debug server's writer.
        assert!(victory_mcp::parse_json(l.text().as_bytes()).is_ok());
    }

    #[test]
    fn the_known_ones_are_listed_with_their_defaults() {
        let _g = serial();
        set_live_path(None);
        let l = list_json("");
        let Some(Json::Arr(items)) = l.get("tunables") else { panic!() };
        let names: Vec<_> = items.iter().filter_map(|t| t.get("name").and_then(Json::as_str)).collect();
        for n in ["view_guard", "wide_spin", "wide_threads", "audio_boost"] {
            assert!(names.contains(&n), "{n}");
        }
    }
}
