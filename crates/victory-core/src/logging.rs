//! One tracing setup for every binary.
//!
//! Filter with `VICTORY_LOG` (falls back to `RUST_LOG`), e.g.
//! `VICTORY_LOG=victory_loader=trace,info`. Debug and trace output exist
//! only in builds with the `diagnostics` feature; a plain release build
//! logs info and above whatever the filter asks for.
//!
//! `VICTORY_LOG_FILE=<path>`: every line is added to that file too, without
//! colours (`victory play` names one for each start).
//!
//! # One file per subject (`VICTORY_LOG_DIR`)
//!
//! With `VICTORY_LOG_DIR=<folder>` set, the runtime also writes UTF-8 files
//! there, besides stderr (which stays as it was):
//!
//! | file         | what goes in it                                                        |
//! |--------------|------------------------------------------------------------------------|
//! | `all.log`    | everything `VICTORY_LOG` lets through, plus everything any subject takes |
//! | `gpu-crash.log` | device removed, TDR, breadcrumbs, "when the host device went", panics |
//! | `memory.log` | target `victory::memory` and `victory::settings`, residency, sweep, pool, budget and eviction lines of the GPU memory modules, and the 5 s memory line |
//! | `textures.log` | texture upload, changed, neutral and levels lines                    |
//! | `shaders.log` | the shader translator, pipeline creation, refused and dropped         |
//! | `perf.log`   | frame-rate lines and the 5 s perf line (target `victory::perf`)        |
//! | `title.log`  | what the title prints ("title said: ...", logged at WARN so they show by default) |
//! | `hle.log`    | `victory_hle`                                                          |
//! | `input.log`  | input targets                                                          |
//!
//! The routing table is [`RULES`]; the first matching rule wins and an event
//! goes to its subject's file and to `all.log`, never to two subject files.
//!
//! ## Convention for code that logs
//!
//! * GPU memory events (texture pool, eviction, budget): target
//!   `victory::memory`, level info, e.g.
//!   `tracing::info!(target: "victory::memory", "evicted {n} textures")`.
//! * Settings changes: target `victory::settings` (goes to `memory.log`).
//! * Periodic numbers: [`publish_perf`] and [`publish_memory`] (once a second
//!   from the host; the files get one line every 5 s and one at a device
//!   removal, [`device_removed`]). More memory numbers: fill
//!   `memory_extra()` in `victory-gpu/src/logstats.rs`.
//! * Anything that must reach `gpu-crash.log`: target `victory::crash`.
//!
//! ## Settings
//!
//! * `VICTORY_LOG_SUBJECTS`: per-subject levels, e.g.
//!   `memory=info,textures=warn,gpu-crash=trace`. Subject names are the file
//!   names without `.log` (and `other` for what no rule takes: only in
//!   `all.log`). Defaults: gpu-crash trace, memory info, perf info, the rest
//!   warn. (A build without the `diagnostics` feature still cannot log below
//!   info.) These levels work whatever `VICTORY_LOG` says.
//! * Every line: local time with ms, frame number, level, target, message and
//!   its fields.
//! * A message template (target, level and message text with digits ignored)
//!   seen more than 20 times in 10 s is folded in the files: the first 5 in
//!   full, then one line when the 10 s are over: "repeated N times in last
//!   10 s (first: <fields>, last: <fields>)". Stderr is never folded.
//! * Files are flushed every second, at once for errors and `gpu-crash.log`,
//!   and on a panic and a device removal.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Context, Filter, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

/// The most verbose level this build can log.
const CAP: LevelFilter = if cfg!(feature = "diagnostics") {
    LevelFilter::TRACE
} else {
    LevelFilter::INFO
};

fn env_filter(default_level: &str) -> EnvFilter {
    std::env::var("VICTORY_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .ok()
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new(default_level))
}

/// `VICTORY_LOG_FILE=<path>`: a plain text layer onto that file (without
/// colours), or none. (Opened again right after it was made: a virus scanner
/// may hold a new file for a moment, so a refusal is tried again a few times.)
fn log_file_layer<S>() -> Option<impl Layer<S>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let path = std::env::var_os("VICTORY_LOG_FILE").filter(|path| !path.is_empty())?;
    let mut tries = 0;
    let file = loop {
        match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => break file,
            Err(_) if tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                eprintln!("VICTORY_LOG_FILE: no log file ({e}); this run logs to this window only");
                return None;
            }
        }
    };
    Some(
        tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_ansi(false)
            .with_writer(Mutex::new(file)),
    )
}

pub fn init(default_level: &str) {
    let dir = std::env::var_os("VICTORY_LOG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    if let Some(dir) = dir {
        match Sink::open(&dir, parse_subjects(std::env::var("VICTORY_LOG_SUBJECTS").ok().as_deref())) {
            Ok(sink) => {
                let levels = sink.levels;
                let _ = SINK.set(Mutex::new(sink));
                install_panic_hook();
                start_ticker();
                let _ = tracing_subscriber::registry()
                    .with(CAP)
                    // Only with VICTORY_MCP set (this also starts the debug server); None otherwise.
                    .with(victory_mcp::init_from_env().with_filter(env_filter(default_level)))
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_target(true)
                            .with_writer(std::io::stderr)
                            .with_filter(env_filter(default_level)),
                    )
                    .with(log_file_layer().with_filter(env_filter(default_level)))
                    .with(
                        FileLayer { env: env_filter(default_level) }
                            .with_filter(FileFilter { env: env_filter(default_level), levels }),
                    )
                    .try_init();
                return;
            }
            Err(e) => eprintln!("VICTORY_LOG_DIR {}: {e}; no log files", dir.display()),
        }
    }
    let filter = env_filter(default_level);
    let _ = tracing_subscriber::registry()
        .with(CAP)
        .with(filter)
        // Only with VICTORY_MCP set (this also starts the debug server); None otherwise.
        .with(victory_mcp::init_from_env())
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_writer(std::io::stderr),
        )
        .with(log_file_layer())
        .try_init();
}

// ---------------------------------------------------------------------
// Subjects and routing
// ---------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Subject {
    GpuCrash,
    Memory,
    Textures,
    Shaders,
    Perf,
    Title,
    Hle,
    Input,
    /// Draws the host did not run (`VICTORY_DROP_REPORT=1`).
    Draws,
    /// No rule took it: only `all.log`.
    Other,
}

impl Subject {
    const ALL: [Subject; 10] = [
        Subject::GpuCrash,
        Subject::Memory,
        Subject::Textures,
        Subject::Shaders,
        Subject::Perf,
        Subject::Title,
        Subject::Hle,
        Subject::Input,
        Subject::Draws,
        Subject::Other,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(9)
    }

    pub fn name(self) -> &'static str {
        match self {
            Subject::GpuCrash => "gpu-crash",
            Subject::Memory => "memory",
            Subject::Textures => "textures",
            Subject::Shaders => "shaders",
            Subject::Perf => "perf",
            Subject::Title => "title",
            Subject::Hle => "hle",
            Subject::Input => "input",
            Subject::Draws => "draws",
            Subject::Other => "other",
        }
    }

    fn default_level(self) -> LevelFilter {
        match self {
            Subject::GpuCrash => LevelFilter::TRACE,
            Subject::Memory | Subject::Perf | Subject::Draws => LevelFilter::INFO,
            _ => LevelFilter::WARN,
        }
    }
}

/// A level per subject, indexed by [`Subject::index`].
type Levels = [LevelFilter; 10];

fn default_levels() -> Levels {
    Subject::ALL.map(Subject::default_level)
}

/// Applies `VICTORY_LOG_SUBJECTS` (`memory=info,textures=warn`) to the
/// defaults; entries it cannot read are left out.
fn parse_subjects(text: Option<&str>) -> Levels {
    let mut levels = default_levels();
    for part in text.unwrap_or("").split(',') {
        let Some((name, level)) = part.split_once('=') else { continue };
        let (name, level) = (name.trim(), level.trim());
        if let (Some(s), Ok(l)) = (
            Subject::ALL.iter().find(|s| s.name() == name),
            level.parse::<LevelFilter>(),
        ) {
            levels[s.index()] = l;
        }
    }
    levels
}

/// How a rule matches an event.
#[derive(Clone, Copy)]
enum Mode {
    /// The target starts with one of `targets`.
    Target,
    /// The message holds one of `words` (any case), for WARN and above only
    /// when `severe`.
    Words { severe: bool },
    /// Both.
    TargetAndWords,
}

struct Rule {
    subject: Subject,
    mode: Mode,
    targets: &'static [&'static str],
    words: &'static [&'static str],
}

/// The routing table: the first rule that matches takes the event.
const RULES: &[Rule] = &[
    Rule { subject: Subject::GpuCrash, mode: Mode::Target, targets: &["victory::crash"], words: &[] },
    // Before every word rule: the report text starts "dropped draws in the
    // last", which the Shaders word "dropped draw" would take (draws.log
    // stayed empty for that reason).
    Rule { subject: Subject::Draws, mode: Mode::Target, targets: &["victory::draws"], words: &[] },
    Rule {
        subject: Subject::GpuCrash,
        mode: Mode::Words { severe: true },
        targets: &[],
        words: &[
            "device removed",
            "device was removed",
            "device_removed",
            "device-removed",
            "device lost",
            "tdr",
            "breadcrumb",
            "host device went",
            "dred",
            "panicked",
            "panic on thread",
        ],
    },
    Rule { subject: Subject::Memory, mode: Mode::Target, targets: &["victory::memory", "victory::settings"], words: &[] },
    Rule {
        subject: Subject::Textures,
        mode: Mode::Words { severe: false },
        targets: &[],
        words: &["picked by index", "after its upload", "neutral", "texture upload", "uploaded again", "texture levels", "mip levels"],
    },
    Rule { subject: Subject::Textures, mode: Mode::Target, targets: &["victory_gpu::texture", "victory::textures"], words: &[] },
    Rule {
        subject: Subject::Memory,
        mode: Mode::TargetAndWords,
        targets: &["victory_gpu::resource", "victory_gpu::pool", "victory_gpu::shadow", "victory_gpu::residency", "victory_gpu::tiles"],
        words: &["residency", "sweep", "pool", "budget", "evict", "let go", "idle", "memory", "card"],
    },
    Rule {
        subject: Subject::Shaders,
        mode: Mode::Target,
        targets: &["victory_shader", "victory_pipeline", "victory_gpu::pipeline", "victory_gpu::shader_cache", "victory::shaders"],
        words: &[],
    },
    Rule {
        subject: Subject::Shaders,
        mode: Mode::Words { severe: false },
        targets: &[],
        words: &["pipeline creat", "pipeline made", "shader translat", "translation refused", "shader refused", "dropped draw"],
    },
    Rule {
        subject: Subject::Perf,
        mode: Mode::Words { severe: false },
        targets: &[],
        words: &[" fps,", "frames a second", "frame rate"],
    },
    Rule { subject: Subject::Perf, mode: Mode::Target, targets: &["victory::perf"], words: &[] },
    Rule { subject: Subject::Title, mode: Mode::Target, targets: &["victory::title"], words: &[] },
    Rule { subject: Subject::Title, mode: Mode::Words { severe: false }, targets: &[], words: &["title said:"] },
    Rule { subject: Subject::Hle, mode: Mode::Target, targets: &["victory_hle"], words: &[] },
    Rule { subject: Subject::Input, mode: Mode::Target, targets: &["victory::input", "victory_input", "victory_hle::modules::input"], words: &[] },
];

/// Which subject takes an event. `lower` is the message in lower case.
fn route(target: &str, level: Level, lower: &str) -> Subject {
    let target_hit = |r: &Rule| r.targets.iter().any(|t| target.starts_with(t));
    let words_hit = |r: &Rule| r.words.iter().any(|w| lower.contains(w));
    for r in RULES {
        let hit = match r.mode {
            Mode::Target => target_hit(r),
            Mode::Words { severe } => (!severe || level <= Level::WARN) && words_hit(r),
            Mode::TargetAndWords => target_hit(r) && words_hit(r),
        };
        if hit {
            return r.subject;
        }
    }
    Subject::Other
}

/// The most verbose level any rule could give an event of this target: what
/// the per-layer filter lets through to be routed (the message is not known
/// yet).
fn possible_max(target: &str, levels: &Levels) -> LevelFilter {
    let mut max = LevelFilter::OFF;
    for r in RULES {
        let level = levels[r.subject.index()];
        let reach = match r.mode {
            Mode::Target | Mode::TargetAndWords => {
                if r.targets.iter().any(|t| target.starts_with(t)) {
                    level
                } else {
                    continue;
                }
            }
            Mode::Words { severe: true } => level.min(LevelFilter::WARN),
            Mode::Words { severe: false } => level,
        };
        max = max.max(reach);
    }
    max.max(levels[Subject::Other.index()])
}

// ---------------------------------------------------------------------
// The layer
// ---------------------------------------------------------------------

struct FileFilter {
    env: EnvFilter,
    levels: Levels,
}

impl<S> Filter<S> for FileFilter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn enabled(&self, meta: &Metadata<'_>, cx: &Context<'_, S>) -> bool {
        *meta.level() <= possible_max(meta.target(), &self.levels) || Filter::enabled(&self.env, meta, cx)
    }
}

struct FileLayer {
    env: EnvFilter,
}

#[derive(Default)]
struct Visitor {
    message: String,
    fields: String,
}

impl Visitor {
    fn field(&mut self, name: &str, value: &dyn std::fmt::Display) {
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        let _ = write!(self.fields, "{name}={value}");
    }
}

impl Visit for Visitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            self.field(field.name(), &value);
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let mut text = String::new();
            let _ = write!(text, "{value:?}");
            self.field(field.name(), &text);
        }
    }
}

impl<S> Layer<S> for FileLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let Some(sink) = SINK.get() else { return };
        let meta = event.metadata();
        let mut v = Visitor::default();
        event.record(&mut v);
        let lower = v.message.to_ascii_lowercase();
        let subject = route(meta.target(), *meta.level(), &lower);
        let mut sink = sink.lock().unwrap_or_else(|e| e.into_inner());
        let to_subject = subject != Subject::Other && *meta.level() <= sink.levels[subject.index()];
        let to_other = subject == Subject::Other && *meta.level() <= sink.levels[subject.index()];
        let to_all = to_subject || to_other || Layer::<S>::enabled(&self.env, meta, ctx);
        if !(to_subject || to_all) {
            return;
        }
        sink.emit(Entry {
            now: Instant::now(),
            stamp: &stamp(),
            subject,
            to_subject,
            to_all,
            level: *meta.level(),
            target: meta.target(),
            message: &v.message,
            fields: &v.fields,
        });
    }
}

// ---------------------------------------------------------------------
// Files, folding
// ---------------------------------------------------------------------

/// Messages with the same template seen more than this many times in a window
/// are folded.
const LIMIT: u32 = 20;
/// The ones shown in full before folding.
const SHOW: u32 = 5;
const WINDOW: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Fold {
    start: Option<Instant>,
    count: u32,
    shown: u32,
    /// Lines 6 to 20 of the window, held until it is known they are not part of a flood.
    pending: Vec<String>,
    first: Option<String>,
    last: String,
    head: String,
}

impl Fold {
    /// Takes one more message; the lines to write now.
    fn push(&mut self, now: Instant, stamp: &str, head: &str, fields: &str, line: String) -> Vec<String> {
        let mut out = Vec::new();
        if self.count > 0 && self.start.is_some_and(|s| now.duration_since(s) >= WINDOW) {
            out.extend(self.close(stamp));
        }
        if self.count == 0 {
            self.start = Some(now);
        }
        self.count += 1;
        if self.count <= SHOW {
            self.shown += 1;
            out.push(line);
            return out;
        }
        if self.first.is_none() {
            self.first = Some(fields.to_owned());
            self.head = head.to_owned();
        }
        self.last = fields.to_owned();
        if self.count <= LIMIT {
            self.pending.push(line);
        } else {
            self.pending.clear();
        }
        out
    }

    /// Ends the window: the held lines, or the summary of a flood.
    fn close(&mut self, stamp: &str) -> Vec<String> {
        let mut out = Vec::new();
        if self.count > LIMIT {
            let n = self.count - self.shown;
            let none = String::from("-");
            let dash = |s: &str| if s.is_empty() { none.clone() } else { s.to_owned() };
            out.push(format!(
                "{stamp} {} | repeated {n} times in last 10 s (first: {}, last: {})",
                self.head,
                dash(self.first.as_deref().unwrap_or("")),
                dash(&self.last)
            ));
        } else {
            out.append(&mut self.pending);
        }
        *self = Fold::default();
        out
    }

    /// The held lines, written without ending the window (a crash is near).
    fn release(&mut self) -> Vec<String> {
        if self.count > LIMIT {
            return Vec::new();
        }
        self.shown += self.pending.len() as u32;
        self.first = None;
        self.last.clear();
        std::mem::take(&mut self.pending)
    }
}

struct Out {
    w: std::io::BufWriter<std::fs::File>,
    folds: HashMap<String, Fold>,
}

impl Out {
    fn write_all(&mut self, lines: Vec<String>) {
        for l in lines {
            let _ = writeln!(self.w, "{l}");
        }
    }

    fn tick(&mut self, now: Instant, stamp: &str) {
        let mut lines = Vec::new();
        for f in self.folds.values_mut() {
            if f.count > 0 && f.start.is_some_and(|s| now.duration_since(s) >= WINDOW) {
                lines.extend(f.close(stamp));
            }
        }
        self.folds.retain(|_, f| f.count > 0);
        self.write_all(lines);
    }

    fn release(&mut self) {
        let mut lines = Vec::new();
        for f in self.folds.values_mut() {
            lines.extend(f.release());
        }
        self.write_all(lines);
    }
}

/// What one event is, for the sink.
struct Entry<'a> {
    now: Instant,
    stamp: &'a str,
    subject: Subject,
    to_subject: bool,
    to_all: bool,
    level: Level,
    target: &'a str,
    message: &'a str,
    fields: &'a str,
}

struct Sink {
    /// 0 is `all.log`, then one per subject in [`Subject::ALL`] order (none for `Other`).
    outs: Vec<Option<Out>>,
    levels: Levels,
}

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

/// Whether the files are being written.
pub fn files_on() -> bool {
    SINK.get().is_some()
}

fn sink() -> Option<std::sync::MutexGuard<'static, Sink>> {
    SINK.get().map(|m| m.lock().unwrap_or_else(|e| e.into_inner()))
}

/// A message with its digit runs made `#`, so "frame 12" and "frame 13" are one template.
fn template(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut in_digits = false;
    for c in message.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
        } else {
            in_digits = false;
            out.push(c);
        }
    }
    out
}

impl Sink {
    fn open(dir: &Path, levels: Levels) -> std::io::Result<Sink> {
        std::fs::create_dir_all(dir)?;
        let open = |name: &str| -> std::io::Result<Out> {
            let f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(name))?;
            Ok(Out { w: std::io::BufWriter::new(f), folds: HashMap::new() })
        };
        let mut outs = vec![Some(open("all.log")?)];
        for s in Subject::ALL {
            outs.push(if s == Subject::Other { None } else { Some(open(&format!("{}.log", s.name()))?) });
        }
        Ok(Sink { outs, levels })
    }

    fn emit(&mut self, e: Entry<'_>) {
        let head = format!("{:<5} {}: {}", e.level, e.target, e.message);
        let line = if e.fields.is_empty() {
            format!("{} {head}", e.stamp)
        } else {
            format!("{} {head} {}", e.stamp, e.fields)
        };
        let key = format!("{}\u{1f}{}\u{1f}{}", e.level, e.target, template(e.message));
        let mut dests = [false; 11];
        dests[0] = e.to_all;
        dests[1 + e.subject.index()] = e.to_subject;
        let hard = e.level == Level::ERROR || e.subject == Subject::GpuCrash;
        for (i, on) in dests.iter().enumerate() {
            if !*on {
                continue;
            }
            let Some(out) = self.outs.get_mut(i).and_then(Option::as_mut) else { continue };
            let lines = out
                .folds
                .entry(key.clone())
                .or_default()
                .push(e.now, e.stamp, &head, e.fields, line.clone());
            out.write_all(lines);
            if hard {
                let _ = out.w.flush();
            }
        }
    }

    fn tick(&mut self, now: Instant) {
        let stamp = stamp();
        for out in self.outs.iter_mut().flatten() {
            out.tick(now, &stamp);
            let _ = out.w.flush();
        }
    }

    /// Writes what folding holds back and flushes everything (a crash).
    fn flush_all(&mut self) {
        for out in self.outs.iter_mut().flatten() {
            out.release();
            let _ = out.w.flush();
        }
    }

    /// An event that does not come through tracing.
    fn direct(&mut self, subject: Subject, level: Level, target: &str, message: &str) {
        let to_subject = level <= self.levels[subject.index()];
        if !to_subject {
            return;
        }
        self.emit(Entry {
            now: Instant::now(),
            stamp: &stamp(),
            subject,
            to_subject: true,
            to_all: true,
            level,
            target,
            message,
            fields: "",
        });
    }
}

fn start_ticker() {
    let _ = std::thread::Builder::new().name("victory-log".into()).spawn(|| {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            if let Some(mut s) = sink() {
                s.tick(Instant::now());
            }
        }
    });
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(mut s) = sink() {
            let thread = std::thread::current();
            let text = format!("panic on thread {}: {info}", thread.name().unwrap_or("?"));
            s.direct(Subject::GpuCrash, Level::ERROR, "victory::crash", &text);
            s.flush_all();
        }
        previous(info);
    }));
}

// ---------------------------------------------------------------------
// Time and frame
// ---------------------------------------------------------------------

static FRAME: OnceLock<fn() -> u64> = OnceLock::new();

/// Where the frame number in every line comes from (the host's present
/// counter); set once.
pub fn set_frame_source(f: fn() -> u64) {
    let _ = FRAME.set(f);
}

struct Parts {
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    milli: u32,
}

#[cfg(windows)]
fn now_parts() -> Parts {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milli: u16,
    }
    unsafe extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }
    let mut t = SystemTime::default();
    // SAFETY: fills the structure we own, laid out as SYSTEMTIME.
    unsafe { GetLocalTime(&mut t) };
    Parts {
        year: t.year.into(),
        month: t.month.into(),
        day: t.day.into(),
        hour: t.hour.into(),
        minute: t.minute.into(),
        second: t.second.into(),
        milli: t.milli.into(),
    }
}

#[cfg(not(windows))]
fn now_parts() -> Parts {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    utc_parts(ms)
}

/// UTC parts of a Unix time in milliseconds.
#[cfg_attr(windows, allow(dead_code))]
fn utc_parts(unix_ms: u64) -> Parts {
    let days = (unix_ms / 86_400_000) as i64;
    let rest = (unix_ms % 86_400_000) as u32;
    // Civil date from days since 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as u32;
    Parts {
        year,
        month,
        day,
        hour: rest / 3_600_000,
        minute: rest / 60_000 % 60,
        second: rest / 1000 % 60,
        milli: rest % 1000,
    }
}

fn format_stamp(p: &Parts, frame: Option<u64>) -> String {
    let frame = frame.map_or_else(|| "-".to_owned(), |f| f.to_string());
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} f={frame}",
        p.year, p.month, p.day, p.hour, p.minute, p.second, p.milli
    )
}

/// Local time with milliseconds and the frame number.
fn stamp() -> String {
    format_stamp(&now_parts(), FRAME.get().map(|f| f()))
}

// ---------------------------------------------------------------------
// Direct entries: the title's lines and crash texts
// ---------------------------------------------------------------------

/// A line the title printed (`title.log`; logged at WARN so it shows by default).
pub fn title_line(line: &str) {
    if let Some(mut s) = sink() {
        s.direct(Subject::Title, Level::WARN, "victory::title", &format!("title said: {line}"));
    }
}

/// A crash report text (`gpu-crash.log`), flushed at once.
pub fn crash_text(text: &str) {
    if let Some(mut s) = sink() {
        s.direct(Subject::GpuCrash, Level::ERROR, "victory::crash", text);
        s.flush_all();
    }
}

// ---------------------------------------------------------------------
// Periodic lines
// ---------------------------------------------------------------------

/// The frame meter's numbers of the last second.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PerfSample {
    pub fps: f64,
    pub ms_per_frame: f64,
    pub draws_per_frame: f64,
    pub dropped_per_frame: f64,
    pub submissions_per_frame: f64,
    pub preparing_ms: f64,
}

/// Memory numbers, in bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemSample {
    /// The card's own memory as the system accounts it: budget, in use.
    pub gpu_local: Option<[u64; 2]>,
    /// The PC's memory shared with the card: budget, in use.
    pub gpu_non_local: Option<[u64; 2]>,
    /// The process: working set, committed.
    pub process: Option<[u64; 2]>,
    /// Named byte counts of the runtime's own accounting, shown in MB.
    pub bytes: Vec<(&'static str, u64)>,
    /// Further named raw numbers, shown as they are.
    pub extra: Vec<(&'static str, u64)>,
}

fn mb(bytes: u64) -> String {
    format!("{} MB", bytes >> 20)
}

/// The perf line, without stamp and target.
pub fn format_perf(p: &PerfSample, age: Option<Duration>) -> String {
    let mut s = format!(
        "fps {:.1}, {:.2} ms/frame, draws/frame {:.0}, dropped/frame {:.1}, submissions/frame {:.1}, preparing {:.2} ms",
        p.fps, p.ms_per_frame, p.draws_per_frame, p.dropped_per_frame, p.submissions_per_frame, p.preparing_ms
    );
    if let Some(a) = age {
        let _ = write!(s, " (sample {} ms old)", a.as_millis());
    }
    s
}

/// The memory line, without stamp and target.
pub fn format_memory(m: &MemSample, age: Option<Duration>) -> String {
    let mut s = String::new();
    let mut part = |text: String| {
        if !s.is_empty() {
            s.push_str(", ");
        }
        s.push_str(&text);
    };
    if let Some([b, u]) = m.gpu_local {
        part(format!("gpu local budget {} used {}", mb(b), mb(u)));
    }
    if let Some([b, u]) = m.gpu_non_local {
        part(format!("gpu non-local budget {} used {}", mb(b), mb(u)));
    }
    if let Some([w, c]) = m.process {
        part(format!("process working set {} commit {}", mb(w), mb(c)));
    }
    for (name, v) in &m.bytes {
        part(format!("{name} {}", mb(*v)));
    }
    for (name, v) in &m.extra {
        part(format!("{name} {v}"));
    }
    if let Some(a) = age {
        let _ = write!(s, " (sample {} ms old)", a.as_millis());
    }
    s
}

const PERIOD: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Latest {
    perf: Option<(PerfSample, Instant)>,
    memory: Option<(MemSample, Instant)>,
    emitted: Option<Instant>,
}

static LATEST: Mutex<Latest> = Mutex::new(Latest { perf: None, memory: None, emitted: None });
static REMOVED: AtomicBool = AtomicBool::new(false);

fn latest() -> std::sync::MutexGuard<'static, Latest> {
    LATEST.lock().unwrap_or_else(|e| e.into_inner())
}

/// The frame meter's numbers (the host calls it once a second); the files
/// get a line of them every 5 s.
pub fn publish_perf(sample: PerfSample) {
    if !files_on() {
        return;
    }
    latest().perf = Some((sample, Instant::now()));
}

/// The memory numbers (once a second, after [`publish_perf`]); the files get
/// a line every 5 s.
pub fn publish_memory(sample: MemSample) {
    if !files_on() {
        return;
    }
    let due = {
        let mut l = latest();
        l.memory = Some((sample, Instant::now()));
        let due = l.emitted.is_none_or(|t| t.elapsed() >= PERIOD);
        if due {
            l.emitted = Some(Instant::now());
        }
        due
    };
    if due {
        emit_periodic(false);
    }
}

/// Writes the last numbers now and flushes every file: the host device was
/// found removed. Once.
pub fn device_removed() {
    if !files_on() || REMOVED.swap(true, Ordering::Relaxed) {
        return;
    }
    if let Some(mut s) = sink() {
        s.direct(Subject::GpuCrash, Level::ERROR, "victory::crash", "host device removal detected; the last periodic numbers follow in perf.log and memory.log");
    }
    emit_periodic(true);
    if let Some(mut s) = sink() {
        s.flush_all();
    }
}

fn emit_periodic(with_age: bool) {
    let (perf, memory) = {
        let l = latest();
        (l.perf.clone(), l.memory.clone())
    };
    let Some(mut s) = sink() else { return };
    let age = |at: Instant| with_age.then(|| at.elapsed());
    if let Some((p, at)) = perf {
        let text = format_perf(&p, age(at));
        s.direct(Subject::Perf, Level::INFO, "victory::perf", &text);
    }
    if let Some((m, at)) = memory {
        let text = format_memory(&m, age(at));
        s.direct(Subject::Memory, Level::INFO, "victory::memory", &text);
    }
    if with_age {
        // Perf and memory lines of a removal also belong next to the crash.
        if let Some((m, _)) = latest().memory.clone() {
            let text = format!("last memory numbers: {}", format_memory(&m, None));
            s.direct(Subject::GpuCrash, Level::ERROR, "victory::crash", &text);
        }
        if let Some((p, _)) = latest().perf.clone() {
            let text = format!("last perf numbers: {}", format_perf(&p, None));
            s.direct(Subject::GpuCrash, Level::ERROR, "victory::crash", &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_by_target_and_words() {
        let r = |t: &str, l: Level, m: &str| route(t, l, &m.to_ascii_lowercase());
        assert_eq!(r("victory_gpu::host", Level::ERROR, "when the host device went: x"), Subject::GpuCrash);
        assert_eq!(r("victory_gpu::draw", Level::WARN, "a texture picked by index changed after its upload"), Subject::Textures);
        assert_eq!(r("victory::memory", Level::INFO, "evicted 3"), Subject::Memory);
        assert_eq!(r("victory::settings", Level::INFO, "budget now 9000"), Subject::Memory);
        assert_eq!(r("victory_gpu::pool", Level::INFO, "pool trimmed"), Subject::Memory);
        assert_eq!(r("victory_gpu::resource", Level::WARN, "residency sweep let go 4"), Subject::Memory);
        assert_eq!(r("victory_shader::emit", Level::WARN, "refused"), Subject::Shaders);
        assert_eq!(r("victory_gpu::host", Level::INFO, "59.9 fps, 16.7 ms/frame"), Subject::Perf);
        assert_eq!(r("victory::title", Level::WARN, "title said: hi"), Subject::Title);
        assert_eq!(r("victory_hle::modules::x", Level::WARN, "hello"), Subject::Hle);
        assert_eq!(r("victory_gpu::host", Level::INFO, "something else"), Subject::Other);
        // The drop report text holds "dropped draw" (a Shaders word) and still goes to draws.log.
        assert_eq!(r("victory::draws", Level::INFO, "dropped draws in the last 5.0 s: 3"), Subject::Draws);
        assert_eq!(r("victory_gpu::pipeline", Level::INFO, "dropped draw of x"), Subject::Shaders);
        // Crash words only count at WARN and above.
        assert_eq!(r("victory_gpu::host", Level::INFO, "breadcrumb noted"), Subject::Other);
        assert_eq!(r("victory_gpu::host", Level::WARN, "breadcrumb noted"), Subject::GpuCrash);
    }

    #[test]
    fn subject_levels_parse() {
        let d = default_levels();
        assert_eq!(d[Subject::Memory.index()], LevelFilter::INFO);
        assert_eq!(d[Subject::GpuCrash.index()], LevelFilter::TRACE);
        assert_eq!(d[Subject::Textures.index()], LevelFilter::WARN);
        let l = parse_subjects(Some("memory=warn, textures=debug,bogus=info,perf=nonsense,hle"));
        assert_eq!(l[Subject::Memory.index()], LevelFilter::WARN);
        assert_eq!(l[Subject::Textures.index()], LevelFilter::DEBUG);
        assert_eq!(l[Subject::Perf.index()], LevelFilter::INFO);
    }

    #[test]
    fn possible_max_reaches_subject_levels() {
        let l = default_levels();
        // Info passes for memory, and for any target (perf words are info).
        assert!(Level::INFO <= possible_max("victory::memory", &l));
        // A debug event of an unrelated target needs a debug rule: none.
        assert!(Level::DEBUG > possible_max("victory_gpu::draw", &l));
        let l = parse_subjects(Some("textures=trace,perf=warn"));
        assert!(Level::TRACE <= possible_max("victory_gpu::texture", &l));
    }

    #[test]
    fn templates_ignore_digits() {
        assert_eq!(template("frame 12 of 300"), template("frame 9 of 4"));
        assert_ne!(template("frame 12"), template("slot 12"));
    }

    #[test]
    fn stamp_has_time_and_frame() {
        let p = Parts { year: 2026, month: 11, day: 26, hour: 9, minute: 5, second: 3, milli: 7 };
        assert_eq!(format_stamp(&p, Some(42)), "2026-11-26 09:05:03.007 f=42");
        assert_eq!(format_stamp(&p, None), "2026-11-26 09:05:03.007 f=-");
        let u = utc_parts(1_795_000_000_123);
        assert_eq!((u.year, u.month, u.day), (2026, 11, 18));
        assert_eq!((u.hour, u.minute, u.second, u.milli), (11, 6, 40, 123));
    }

    fn push(f: &mut Fold, now: Instant, n: u32) -> Vec<String> {
        f.push(now, "T", "WARN t: m", &format!("slot={n}"), format!("line {n}"))
    }

    #[test]
    fn a_flood_is_folded_after_five() {
        let t0 = Instant::now();
        let mut f = Fold::default();
        let mut written = Vec::new();
        for n in 1..=100 {
            written.extend(push(&mut f, t0, n));
        }
        assert_eq!(written, (1..=5).map(|n| format!("line {n}")).collect::<Vec<_>>());
        let summary = f.close("T2");
        assert_eq!(summary.len(), 1);
        assert_eq!(
            summary[0],
            "T2 WARN t: m | repeated 95 times in last 10 s (first: slot=6, last: slot=100)"
        );
    }

    #[test]
    fn a_small_burst_is_written_in_full() {
        let t0 = Instant::now();
        let mut f = Fold::default();
        let mut written = Vec::new();
        for n in 1..=20 {
            written.extend(push(&mut f, t0, n));
        }
        assert_eq!(written.len(), 5);
        let rest = f.close("T");
        assert_eq!(rest.len(), 15);
        assert_eq!(rest[0], "line 6");
    }

    #[test]
    fn a_window_ends_after_ten_seconds() {
        let t0 = Instant::now();
        let mut f = Fold::default();
        for n in 1..=30 {
            push(&mut f, t0, n);
        }
        let out = push(&mut f, t0 + Duration::from_secs(11), 31);
        assert_eq!(out.len(), 2);
        assert!(out[0].contains("repeated 25 times"));
        assert_eq!(out[1], "line 31");
    }

    #[test]
    fn released_lines_are_not_counted_twice() {
        let t0 = Instant::now();
        let mut f = Fold::default();
        for n in 1..=10 {
            push(&mut f, t0, n);
        }
        assert_eq!(f.release().len(), 5);
        for n in 11..=30 {
            push(&mut f, t0, n);
        }
        let out = f.close("T");
        assert!(out[0].contains("repeated 20 times"), "{}", out[0]);
    }

    #[test]
    fn perf_and_memory_lines() {
        let p = PerfSample {
            fps: 59.94,
            ms_per_frame: 16.68,
            draws_per_frame: 1234.4,
            dropped_per_frame: 0.5,
            submissions_per_frame: 3.0,
            preparing_ms: 4.25,
        };
        assert_eq!(
            format_perf(&p, None),
            "fps 59.9, 16.68 ms/frame, draws/frame 1234, dropped/frame 0.5, submissions/frame 3.0, preparing 4.25 ms"
        );
        let m = MemSample {
            gpu_local: Some([22 << 30, 21 << 30]),
            gpu_non_local: Some([30 << 30, 1 << 30]),
            process: Some([5 << 30, 9 << 30]),
            bytes: vec![("texture host copies", 3 << 30)],
            extra: vec![("tiles", 7)],
        };
        assert_eq!(
            format_memory(&m, Some(Duration::from_millis(400))),
            "gpu local budget 22528 MB used 21504 MB, gpu non-local budget 30720 MB used 1024 MB, \
             process working set 5120 MB commit 9216 MB, texture host copies 3072 MB, tiles 7 (sample 400 ms old)"
        );
    }

    #[test]
    fn the_sink_writes_subject_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = Sink::open(dir.path(), default_levels()).unwrap();
        let now = Instant::now();
        let put = |sink: &mut Sink, subject: Subject, to_subject: bool, level: Level, msg: &str| {
            sink.emit(Entry { now, stamp: "S", subject, to_subject, to_all: true, level, target: "t", message: msg, fields: "k=v" });
        };
        put(&mut sink, Subject::Memory, true, Level::INFO, "pool trimmed");
        put(&mut sink, Subject::Other, false, Level::WARN, "plain");
        for n in 0..40 {
            put(&mut sink, Subject::Textures, true, Level::WARN, &format!("changed {n}"));
        }
        sink.direct(Subject::GpuCrash, Level::ERROR, "victory::crash", "device removed");
        sink.flush_all();
        let read = |n: &str| std::fs::read_to_string(dir.path().join(n)).unwrap();
        assert_eq!(read("memory.log"), "S INFO  t: pool trimmed k=v\n");
        assert!(read("gpu-crash.log").contains("device removed"));
        assert_eq!(read("textures.log").lines().count(), 5);
        assert!(read("all.log").contains("plain"));
        assert!(!dir.path().join("other.log").exists());
    }
}
