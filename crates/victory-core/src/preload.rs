//! The preload's logic without a window or a game: which pipelines are still
//! to make (resume), what a start does about it (`VICTORY_PRELOAD`), the lines
//! the worker reports its progress with, and the numbers the start screen
//! shows (stage, fraction, time left).
//!
//! The worker (`victory preload`) prints its progress as lines that start with
//! `@ `; the launcher reads them and feeds [`Progress`]; the screen
//! (`victory-gpu`'s `preload_screen`) paints what [`Progress`] says. Nothing
//! here touches the disk but the two small readers at the end.

use std::collections::{HashSet, VecDeque};

/// A start with this many pipelines or fewer still to make counts as complete:
/// runs add pipelines to the known list as they meet new places, and a screen
/// for a handful of them at every start would be a nuisance (the background
/// worker takes those).
pub const SMALL_REMAINDER: usize = 25;

/// The share of the bar the shader stage has when a pipeline stage follows.
pub const SHADER_WEIGHT: f32 = 0.2;

/// What `VICTORY_PRELOAD` says a start does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Show the screen when the cache is not complete (the default).
    #[default]
    Ask,
    /// Show the screen when it is not complete and wait for it without asking.
    AlwaysWait,
    /// Never.
    Off,
}

impl Mode {
    /// The mode a variable's text names; anything unknown is the default.
    pub fn parse(text: &str) -> Mode {
        match text.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "always_wait" => Mode::AlwaysWait,
            "off" | "0" | "false" | "never" => Mode::Off,
            _ => Mode::Ask,
        }
    }

    /// The text the variable holds.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::AlwaysWait => "always_wait",
            Mode::Off => "off",
        }
    }
}

/// What the cache holds for this build and driver, as `victory preload
/// --check` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    /// Every shader of the title is translated into the cache.
    pub shaders_done: bool,
    /// Pipelines of the preload's scope still to make.
    pub todo: usize,
    /// Pipelines of that scope in all.
    pub scope: usize,
}

impl Status {
    pub fn complete(&self) -> bool {
        self.shaders_done && self.todo <= SMALL_REMAINDER
    }

    /// The line `victory preload --check` prints.
    pub fn line(&self) -> String {
        format!("@ status shaders={} todo={} scope={}", u8::from(self.shaders_done), self.todo, self.scope)
    }

    pub fn parse(line: &str) -> Option<Status> {
        let rest = line.strip_prefix("@ status ")?;
        let (mut shaders, mut todo, mut scope) = (None, None, None);
        for word in rest.split(' ') {
            match word.split_once('=')? {
                ("shaders", v) => shaders = Some(v == "1"),
                ("todo", v) => todo = v.parse().ok(),
                ("scope", v) => scope = v.parse().ok(),
                _ => {}
            }
        }
        Some(Status { shaders_done: shaders?, todo: todo?, scope: scope? })
    }
}

/// What a start does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// The title starts at once.
    Skip,
    /// The screen shows; `wait` is the choice already made (the title then starts at 100%).
    Show { wait: bool },
}

/// The start's decision. `forced`: asked for by the menu's "PRELOAD NOW" or
/// `--preload`, which shows the screen even when nothing is left.
pub fn decide(mode: Mode, forced: bool, status: &Status) -> Plan {
    if forced {
        return Plan::Show { wait: mode == Mode::AlwaysWait };
    }
    match mode {
        Mode::Off => Plan::Skip,
        _ if status.complete() => Plan::Skip,
        Mode::Ask => Plan::Show { wait: false },
        Mode::AlwaysWait => Plan::Show { wait: true },
    }
}

/// The pipelines (numbers in the known list) a preload or warm has to make.
/// `keys`: every known pipeline's key. A key counts as done when a line of
/// `done_text` names it under `fingerprint`; a bad one (`bad_text`, first word
/// of each line) is never tried again. `used`: only those earlier runs drew
/// with (`early_text`). `again`: the done ones too. Those drawn with come
/// first, the others in key order, so a stopped preload has made the ones the
/// title needs first.
pub fn select_todo(
    keys: &[String],
    done_text: &str,
    bad_text: &str,
    early_text: &str,
    fingerprint: &str,
    again: bool,
    used: bool,
    limit: Option<usize>,
) -> Vec<usize> {
    let done = done_keys(done_text, fingerprint);
    let bad: HashSet<&str> = bad_text.lines().filter_map(|l| l.split(' ').next()).collect();
    let early: HashSet<&str> = early_text.lines().collect();
    let mut all: Vec<usize> = (0..keys.len())
        .filter(|&n| {
            let key = keys[n].as_str();
            (again || !done.contains(key)) && !bad.contains(key) && (!used || early.contains(key))
        })
        .collect();
    all.sort_by_key(|&n| (!early.contains(keys[n].as_str()), &keys[n]));
    all.truncate(limit.unwrap_or(usize::MAX));
    all
}

/// The keys `done_text` (lines `fingerprint key`) names under `fingerprint`.
pub fn done_keys<'a>(done_text: &'a str, fingerprint: &str) -> HashSet<&'a str> {
    done_text
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter(|(of, _)| *of == fingerprint)
        .map(|(_, key)| key)
        .collect()
}

/// The line that says every shader was translated for this translator and
/// driver (`fingerprint`) and this copy of the title (its size and write
/// time): written when the shader stage ends whole, so that a start does not
/// walk the title's files again.
pub fn shaders_marker(fingerprint: &str, exe_size: u64, exe_stamp: u64) -> String {
    format!("{fingerprint} {exe_size} {exe_stamp}")
}

/// Whether `file_text` holds `marker` as one of its lines.
pub fn shaders_marker_valid(file_text: &str, marker: &str) -> bool {
    file_text.lines().any(|l| l.trim() == marker)
}

/// Where the worker is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Nothing reported yet.
    #[default]
    Checking,
    /// Translating the title's shaders into the cache.
    Shaders,
    /// Making host pipelines.
    Pipelines,
    Done,
}

impl Stage {
    /// What the screen says of it.
    pub fn label(self) -> &'static str {
        match self {
            Stage::Checking => "CHECKING WHAT IS READY",
            Stage::Shaders => "TRANSLATING SHADERS",
            Stage::Pipelines => "MAKING PIPELINES",
            Stage::Done => "DONE",
        }
    }
}

/// One line of the worker's report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The shader stage begins: the bytes of the files to go through.
    ShaderStage { total: u64 },
    /// Bytes gone through of the total, and shaders in the cache so far.
    Shaders { done: u64, total: u64, count: u64 },
    /// The pipeline stage begins: how many are to make.
    PipelineStage { total: u64 },
    Pipelines { done: u64, total: u64 },
    Done,
}

impl Event {
    pub fn line(&self) -> String {
        match *self {
            Event::ShaderStage { total } => format!("@ stage shaders {total}"),
            Event::Shaders { done, total, count } => format!("@ shaders {done} {total} {count}"),
            Event::PipelineStage { total } => format!("@ stage pipelines {total}"),
            Event::Pipelines { done, total } => format!("@ pipelines {done} {total}"),
            Event::Done => "@ done".to_owned(),
        }
    }

    pub fn parse(line: &str) -> Option<Event> {
        let rest = line.trim_end().strip_prefix("@ ")?;
        let mut w = rest.split(' ');
        let n = |s: Option<&str>| s.and_then(|s| s.parse::<u64>().ok());
        match w.next()? {
            "stage" => match w.next()? {
                "shaders" => Some(Event::ShaderStage { total: n(w.next())? }),
                "pipelines" => Some(Event::PipelineStage { total: n(w.next())? }),
                _ => None,
            },
            "shaders" => Some(Event::Shaders { done: n(w.next())?, total: n(w.next())?, count: n(w.next())? }),
            "pipelines" => Some(Event::Pipelines { done: n(w.next())?, total: n(w.next())? }),
            "done" => Some(Event::Done),
            _ => None,
        }
    }
}

/// How long the rate of a stage is taken over.
const WINDOW: f64 = 30.0;

/// What the screen shows, built up from the worker's events.
#[derive(Clone, Debug)]
pub struct Progress {
    pub stage: Stage,
    pub shader_bytes: (u64, u64),
    pub shader_count: u64,
    pub pipelines: (u64, u64),
    /// The shaders' share of the bar (0 when they are done already, 1 when no pipelines follow).
    weight: f32,
    /// Seconds (any clock) at the first event, and at the last.
    began: Option<f64>,
    now: f64,
    /// (time, fraction of the stage) in the last [`WINDOW`] seconds.
    samples: VecDeque<(f64, f64)>,
}

impl Progress {
    /// Progress for a start with `status`: the bar's two shares follow from
    /// what is left to do.
    pub fn new(status: &Status) -> Progress {
        let weight = match (status.shaders_done, status.todo > 0) {
            (true, _) => 0.0,
            (false, true) => SHADER_WEIGHT,
            (false, false) => 1.0,
        };
        Progress {
            stage: Stage::Checking,
            shader_bytes: (0, 0),
            shader_count: 0,
            pipelines: (0, 0),
            weight,
            began: None,
            now: 0.0,
            samples: VecDeque::new(),
        }
    }

    /// Takes an event that arrived at `now` seconds.
    pub fn apply(&mut self, event: &Event, now: f64) {
        self.began.get_or_insert(now);
        self.now = now;
        let before = self.stage;
        match *event {
            Event::ShaderStage { total } => {
                self.stage = Stage::Shaders;
                self.shader_bytes = (0, total);
            }
            Event::Shaders { done, total, count } => {
                self.stage = self.stage.max(Stage::Shaders);
                self.shader_bytes = (done.min(total), total);
                self.shader_count = count;
            }
            Event::PipelineStage { total } => {
                self.stage = Stage::Pipelines;
                self.pipelines = (0, total);
            }
            Event::Pipelines { done, total } => {
                self.stage = self.stage.max(Stage::Pipelines);
                self.pipelines = (done.min(total), total);
            }
            Event::Done => self.stage = Stage::Done,
        }
        if self.stage != before {
            self.samples.clear();
        }
        let frac = self.stage_fraction();
        self.samples.push_back((now, frac));
        while self.samples.front().is_some_and(|(t, _)| now - t > WINDOW) {
            self.samples.pop_front();
        }
    }

    /// Moves the clock without an event (the screen's own ticks).
    pub fn tick(&mut self, now: f64) {
        self.now = self.now.max(now);
    }

    /// How far the current stage is, 0 to 1.
    pub fn stage_fraction(&self) -> f64 {
        let ratio = |(done, total): (u64, u64)| if total == 0 { 1.0 } else { done as f64 / total as f64 };
        match self.stage {
            Stage::Checking => 0.0,
            Stage::Shaders => ratio(self.shader_bytes),
            Stage::Pipelines => ratio(self.pipelines),
            Stage::Done => 1.0,
        }
    }

    /// The whole bar, 0 to 1: the stages by their shares.
    pub fn fraction(&self) -> f32 {
        let own = self.stage_fraction() as f32;
        let f = match self.stage {
            Stage::Checking => 0.0,
            Stage::Shaders => self.weight * own,
            Stage::Pipelines => self.weight + (1.0 - self.weight) * own,
            Stage::Done => 1.0,
        };
        f.clamp(0.0, 1.0)
    }

    /// Seconds left in the current stage at the rate of the last half minute;
    /// `None` until there is a rate to go by.
    pub fn eta_secs(&self) -> Option<f64> {
        if self.stage == Stage::Done {
            return Some(0.0);
        }
        let (t0, f0) = *self.samples.front()?;
        let (t1, f1) = *self.samples.back()?;
        let (dt, df) = (t1 - t0, f1 - f0);
        if dt < 1.0 || df <= 0.0 {
            return None;
        }
        Some(((1.0 - f1) / (df / dt)).max(0.0))
    }

    /// Seconds since the first event, at the clock of the last.
    pub fn elapsed(&self) -> f64 {
        self.began.map_or(0.0, |b| (self.now - b).max(0.0))
    }

    pub fn finished(&self) -> bool {
        self.stage == Stage::Done
    }
}

/// `1,234,567`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A span as the screen says it: `45 S`, `12 MIN`, `2 H 05 MIN`.
pub fn span(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    match s {
        0..=59 => format!("{s} S"),
        60..=3599 => format!("{} MIN {:02} S", s / 60, s % 60),
        _ => format!("{} H {:02} MIN", s / 3600, s % 3600 / 60),
    }
}

/// Bytes as `1.4 GB` (binary units, as the rest of the menu).
pub fn bytes(n: u64) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    const MB: f64 = (1u64 << 20) as f64;
    if n as f64 >= GB {
        format!("{:.1} GB", n as f64 / GB)
    } else {
        format!("{:.0} MB", n as f64 / MB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(shaders_done: bool, todo: usize) -> Status {
        Status { shaders_done, todo, scope: 1000 }
    }

    #[test]
    fn the_mode_reads_its_names_and_defaults_to_ask() {
        assert_eq!(Mode::parse("ask"), Mode::Ask);
        assert_eq!(Mode::parse(" ALWAYS_WAIT "), Mode::AlwaysWait);
        assert_eq!(Mode::parse("always-wait"), Mode::AlwaysWait);
        assert_eq!(Mode::parse("off"), Mode::Off);
        assert_eq!(Mode::parse(""), Mode::Ask);
        assert_eq!(Mode::parse("nonsense"), Mode::Ask);
        for m in [Mode::Ask, Mode::AlwaysWait, Mode::Off] {
            assert_eq!(Mode::parse(m.name()), m);
        }
    }

    #[test]
    fn a_start_shows_the_screen_only_when_the_cache_is_incomplete() {
        let complete = status(true, 0);
        let few = status(true, SMALL_REMAINDER);
        let many = status(true, SMALL_REMAINDER + 1);
        let no_shaders = status(false, 0);
        for m in [Mode::Ask, Mode::AlwaysWait, Mode::Off] {
            assert_eq!(decide(m, false, &complete), Plan::Skip, "{m:?}");
            assert_eq!(decide(m, false, &few), Plan::Skip, "{m:?}");
        }
        assert_eq!(decide(Mode::Ask, false, &many), Plan::Show { wait: false });
        assert_eq!(decide(Mode::Ask, false, &no_shaders), Plan::Show { wait: false });
        assert_eq!(decide(Mode::AlwaysWait, false, &many), Plan::Show { wait: true });
        assert_eq!(decide(Mode::Off, false, &many), Plan::Skip);
        assert_eq!(decide(Mode::Off, false, &no_shaders), Plan::Skip);
    }

    #[test]
    fn a_forced_preload_shows_even_when_complete_and_even_when_off() {
        let complete = status(true, 0);
        assert_eq!(decide(Mode::Ask, true, &complete), Plan::Show { wait: false });
        assert_eq!(decide(Mode::Off, true, &complete), Plan::Show { wait: false });
        assert_eq!(decide(Mode::AlwaysWait, true, &complete), Plan::Show { wait: true });
    }

    #[test]
    fn the_status_line_round_trips() {
        let s = Status { shaders_done: true, todo: 12, scope: 3456 };
        assert_eq!(s.line(), "@ status shaders=1 todo=12 scope=3456");
        assert_eq!(Status::parse(&s.line()), Some(s));
        assert_eq!(Status::parse("@ status shaders=0 todo=1"), None);
        assert_eq!(Status::parse("pipelines made"), None);
    }

    #[test]
    fn events_round_trip_and_noise_is_ignored() {
        let all = [
            Event::ShaderStage { total: 5_000_000_000 },
            Event::Shaders { done: 10, total: 20, count: 7 },
            Event::PipelineStage { total: 84_000 },
            Event::Pipelines { done: 1, total: 2 },
            Event::Done,
        ];
        for e in all {
            assert_eq!(Event::parse(&e.line()), Some(e), "{e:?}");
        }
        assert_eq!(Event::parse("1000 of 84000, 12 s"), None);
        assert_eq!(Event::parse("@ pipelines x 2"), None);
        assert_eq!(Event::parse("@ stage tea 2"), None);
        assert_eq!(Event::parse("@ pipelines 1 2\r\n"), Some(Event::Pipelines { done: 1, total: 2 }));
    }

    fn keys(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("k{i:02}")).collect()
    }

    #[test]
    fn resume_skips_what_this_build_made_and_what_is_bad() {
        let keys = keys(6);
        // k00 and k01 made under this build, k02 under another one, k03 is bad.
        let done = "fp1 k00\nfp1 k01\nfp0 k02\nfp1 k0";
        let bad = "k03 hung for 180 s\n";
        let todo = select_todo(&keys, done, bad, "", "fp1", false, false, None);
        // (A torn last line "fp1 k0" names no key.)
        assert_eq!(todo, [2, 4, 5]);
        // Everything again.
        assert_eq!(select_todo(&keys, done, bad, "", "fp1", true, false, None), [0, 1, 2, 4, 5]);
        // A new translator or driver (another fingerprint) starts over, bad ones stay out.
        assert_eq!(select_todo(&keys, done, bad, "", "fp2", false, false, None), [0, 1, 2, 4, 5]);
    }

    #[test]
    fn the_ones_the_title_drew_with_come_first_and_used_keeps_only_them() {
        let keys = keys(5);
        let early = "k04\nk02\n";
        assert_eq!(select_todo(&keys, "", "", early, "f", false, false, None), [2, 4, 0, 1, 3]);
        assert_eq!(select_todo(&keys, "", "", early, "f", false, true, None), [2, 4]);
        assert_eq!(select_todo(&keys, "", "", early, "f", false, false, Some(3)), [2, 4, 0]);
    }

    #[test]
    fn a_cancelled_preload_loses_nothing_and_the_next_one_goes_on() {
        let keys = keys(10);
        let early = "k05\nk06\nk07\n";
        let mut done = String::new();
        let first = select_todo(&keys, &done, "", early, "f", false, true, None);
        assert_eq!(first, [5, 6, 7]);
        // The first two are made, the preload is stopped (the last line is torn).
        done.push_str("f k05\nf k06\nf k0");
        let second = select_todo(&keys, &done, "", early, "f", false, true, None);
        assert_eq!(second, [7]);
        done.push_str("\nf k07\n");
        assert!(select_todo(&keys, &done, "", early, "f", false, true, None).is_empty());
    }

    #[test]
    fn the_shader_marker_is_per_build_and_per_copy_of_the_title() {
        let marker = shaders_marker("abc", 1000, 77);
        let file = format!("{}\n{}\n", shaders_marker("old", 1, 1), marker);
        assert!(shaders_marker_valid(&file, &marker));
        assert!(!shaders_marker_valid(&file, &shaders_marker("abc", 1001, 77)));
        assert!(!shaders_marker_valid(&file, &shaders_marker("abd", 1000, 77)));
        assert!(!shaders_marker_valid("", &marker));
    }

    #[test]
    fn the_bar_follows_the_stages_by_their_shares() {
        let mut p = Progress::new(&status(false, 500));
        assert_eq!((p.stage, p.fraction()), (Stage::Checking, 0.0));
        p.apply(&Event::ShaderStage { total: 1000 }, 0.0);
        p.apply(&Event::Shaders { done: 500, total: 1000, count: 40 }, 5.0);
        assert!((p.fraction() - SHADER_WEIGHT * 0.5).abs() < 1e-6);
        p.apply(&Event::PipelineStage { total: 200 }, 10.0);
        assert!((p.fraction() - SHADER_WEIGHT).abs() < 1e-6);
        p.apply(&Event::Pipelines { done: 100, total: 200 }, 20.0);
        assert!((p.fraction() - (SHADER_WEIGHT + (1.0 - SHADER_WEIGHT) * 0.5)).abs() < 1e-6);
        assert!(!p.finished());
        p.apply(&Event::Done, 30.0);
        assert_eq!(p.fraction(), 1.0);
        assert!(p.finished());
    }

    #[test]
    fn with_the_shaders_done_the_pipelines_have_the_whole_bar() {
        let mut p = Progress::new(&status(true, 300));
        p.apply(&Event::PipelineStage { total: 300 }, 0.0);
        p.apply(&Event::Pipelines { done: 150, total: 300 }, 1.0);
        assert!((p.fraction() - 0.5).abs() < 1e-6);
        // With no pipelines to make, the shaders have the whole bar.
        let mut p = Progress::new(&status(false, 0));
        p.apply(&Event::ShaderStage { total: 100 }, 0.0);
        p.apply(&Event::Shaders { done: 25, total: 100, count: 1 }, 1.0);
        assert!((p.fraction() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn an_empty_stage_is_whole_and_a_count_past_the_total_is_held() {
        let mut p = Progress::new(&status(true, 0));
        p.apply(&Event::PipelineStage { total: 0 }, 0.0);
        assert_eq!(p.stage_fraction(), 1.0);
        p.apply(&Event::Pipelines { done: 9, total: 4 }, 1.0);
        assert_eq!(p.pipelines, (4, 4));
    }

    #[test]
    fn the_time_left_comes_from_the_rate_of_the_stage() {
        let mut p = Progress::new(&status(true, 1000));
        p.apply(&Event::PipelineStage { total: 1000 }, 100.0);
        assert_eq!(p.eta_secs(), None, "no rate yet");
        p.apply(&Event::Pipelines { done: 100, total: 1000 }, 110.0);
        // 100 in 10 s: 900 left take 90 s.
        let eta = p.eta_secs().unwrap();
        assert!((eta - 90.0).abs() < 1e-6, "{eta}");
        assert!((p.elapsed() - 10.0).abs() < 1e-9);
        // A stage that stands still has no estimate.
        let mut q = Progress::new(&status(true, 1000));
        q.apply(&Event::PipelineStage { total: 1000 }, 0.0);
        q.apply(&Event::Pipelines { done: 0, total: 1000 }, 20.0);
        assert_eq!(q.eta_secs(), None);
        // The window forgets what is older than half a minute: the rate is the recent one.
        let mut r = Progress::new(&status(true, 1000));
        r.apply(&Event::PipelineStage { total: 1000 }, 0.0);
        r.apply(&Event::Pipelines { done: 500, total: 1000 }, 10.0);
        r.apply(&Event::Pipelines { done: 500, total: 1000 }, 60.0);
        r.apply(&Event::Pipelines { done: 600, total: 1000 }, 70.0);
        let eta = r.eta_secs().unwrap();
        assert!((eta - 40.0).abs() < 1e-6, "{eta}");
    }

    #[test]
    fn a_new_stage_starts_its_rate_over() {
        let mut p = Progress::new(&status(false, 10));
        p.apply(&Event::ShaderStage { total: 100 }, 0.0);
        p.apply(&Event::Shaders { done: 100, total: 100, count: 5 }, 10.0);
        assert!(p.eta_secs().is_some());
        p.apply(&Event::PipelineStage { total: 10 }, 10.5);
        assert_eq!(p.eta_secs(), None);
        assert_eq!(p.shader_count, 5);
    }

    #[test]
    fn numbers_read_as_the_screen_says_them() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(84_123_456), "84,123,456");
        assert_eq!(span(45.0), "45 S");
        assert_eq!(span(125.0), "2 MIN 05 S");
        assert_eq!(span(7_500.0), "2 H 05 MIN");
        assert_eq!(bytes(300 << 20), "300 MB");
        assert_eq!(bytes(3 << 30), "3.0 GB");
    }
}
