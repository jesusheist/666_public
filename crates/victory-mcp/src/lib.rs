//! A debug MCP server inside the runtime.
//!
//! Off unless `VICTORY_MCP=<port>` is set: then no thread, no socket and no
//! memory are taken, and the places that feed it cost one relaxed atomic
//! load ([`enabled`], [`take_shot_request`]). On, it listens on
//! 127.0.0.1 only and answers MCP (JSON-RPC 2.0 over Streamable HTTP, a
//! POST to `/mcp`) with read-only tools: the frame meter's numbers, recent
//! log lines, pipeline events, the device-lost report and a screenshot.
//!
//! The server thread never touches the game's threads: numbers and lines
//! reach it through values published under locks held for microseconds. A
//! screenshot raises a flag that the present path takes once
//! ([`take_shot_request`]), reads the frame back and hands it over a channel.

pub mod badpix;
mod http;
mod json;
mod layer;
pub mod memscan;
mod protocol;
pub mod remote_pad;
mod ring;
mod server;
pub mod shot;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub use json::{Json, parse as parse_json};
pub use layer::LogLayer;
pub use memscan::{MemHooks, MemRegion, set_mem_hooks};
pub use protocol::{PROTOCOL_VERSION, handle_body, Outcome};
pub use server::{ServerHandle, start};
pub use badpix::{ScanParams, ScanReport};
pub use shot::RawFrame;

use ring::Ring;

/// Ports 59340-59450 belong to another tool on the same PC.
pub const RESERVED_PORTS: std::ops::RangeInclusive<u16> = 59340..=59450;
/// Log lines kept.
const LOG_LINES: usize = 4096;
/// Pipeline events kept.
const EVENTS: usize = 4096;

/// Set once the server runs; read by [`enabled`].
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Set while a screenshot waits for the present path; read by
/// [`take_shot_request`].
static SHOT_FLAG: AtomicBool = AtomicBool::new(false);

/// The bad-pixel capture's phase ([`scan_edge`], [`scan_recording`]): the
/// one value the game's code reads, with a relaxed load, while no capture
/// is asked for.
static SCAN_PHASE: AtomicU8 = AtomicU8::new(SCAN_IDLE);
/// No capture asked for.
const SCAN_IDLE: u8 = 0;
/// Asked for; waits for the next present, which begins it.
const SCAN_ARMED: u8 = 1;
/// Recording: commands recorded now are scanned until the next present.
const SCAN_RECORDING: u8 = 2;
/// The caller gave up: the next present drops the capture.
const SCAN_CANCELLED: u8 = 3;
static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// What the runtime's memory code gives the memory tools: each is a plain
/// function that reads atomics and the system's counters, so a tool call
/// never takes a lock the game's threads wait on.
#[derive(Clone, Copy)]
pub struct MemoryHooks {
    /// `get_memory`: the snapshot, the settings and the eviction totals.
    pub memory: fn() -> Json,
    /// `get_settings`: every setting, its value and what it may be.
    pub settings: fn() -> Json,
    /// `set_setting`: sets one by name; the new settings, or why not.
    pub set: fn(&str, &str) -> Result<Json, String>,
}

static MEMORY_HOOKS: OnceLock<MemoryHooks> = OnceLock::new();

/// Registers the memory tools' functions (the runtime does it once, when
/// its live settings are first read). The first call wins.
pub fn set_memory_hooks(hooks: MemoryHooks) {
    let _ = MEMORY_HOOKS.set(hooks);
}

/// What the runtime's tunable registry gives the tunable tools (the runtime
/// registers it once at start, `victory_core::tunables::init`). Each is a plain
/// function; the arguments are a filter text, or a name and a value text.
#[derive(Clone, Copy)]
pub struct TunableHooks {
    /// `list_tunables`: all (or those matching the filter) as one object.
    pub list: fn(&str) -> Json,
    /// `get_tunable`: one by name, or why not.
    pub get: fn(&str) -> Result<Json, String>,
    /// `set_tunable`: sets one by name from text; it after the change, or why not.
    pub set: fn(&str, &str) -> Result<Json, String>,
    /// `reset_tunable`: back to the default.
    pub reset: fn(&str) -> Result<Json, String>,
}

static TUNABLE_HOOKS: OnceLock<TunableHooks> = OnceLock::new();

/// Registers the tunable tools' functions. The first call wins.
pub fn set_tunable_hooks(hooks: TunableHooks) {
    let _ = TUNABLE_HOOKS.set(hooks);
}

pub(crate) fn tunable_hooks() -> Option<&'static TunableHooks> {
    TUNABLE_HOOKS.get()
}

static DROPS_HOOK: OnceLock<fn() -> Json> = OnceLock::new();

/// Registers the function behind `get_dropped_draws` (the runtime does it
/// when `VICTORY_DROP_REPORT=1`). The first call wins.
pub fn set_drops_hook(hook: fn() -> Json) {
    let _ = DROPS_HOOK.set(hook);
}

pub(crate) fn drops_hook() -> Option<fn() -> Json> {
    DROPS_HOOK.get().copied()
}

pub(crate) fn memory_hooks() -> Option<&'static MemoryHooks> {
    MEMORY_HOOKS.get()
}

/// Whether the server runs: the one check the game's code makes before it
/// publishes anything.
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub struct Config {
    /// Where screenshots are written.
    pub shots_dir: PathBuf,
    /// How long a screenshot request waits for a present.
    pub shot_timeout: Duration,
    /// How long a bad-pixel capture waits: two presents and the frame it
    /// scans, which runs slowly.
    pub scan_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            shots_dir: PathBuf::from(r"K:\Leonida\avatarmanz\shots"),
            shot_timeout: Duration::from_secs(10),
            scan_timeout: Duration::from_secs(120),
        }
    }
}

/// The frame meter's values of the last second, as the game measured them.
/// Per-frame values are means over that second's frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    /// Frames begun so far.
    pub frame: u64,
    /// Frames presented in the second.
    pub frames: u32,
    pub fps: f64,
    pub ms_per_frame: f64,
    /// Preparing submissions on the CPU, and its parts.
    pub preparing_ms: f64,
    pub copies_ms: f64,
    pub copies_mb: f64,
    pub textures_ms: f64,
    pub textures_mb: f64,
    /// Waiting for the GPU.
    pub gpu_ms: f64,
    pub copy_back_ms: f64,
    pub copy_back_mb: f64,
    pub draws_per_frame: f64,
    pub dropped_per_frame: f64,
    pub indirect_per_frame: f64,
    pub submissions_per_frame: f64,
}

pub const KIND_SHADER_TRANSLATED: &str = "shader_translated";
pub const KIND_FIRST_BIND: &str = "first_bind";
pub const KIND_PIPELINE_LOADED: &str = "pipeline_loaded";
pub const KIND_PIPELINE_MADE: &str = "pipeline_made";
pub const KIND_LIBRARY_LOADED: &str = "library_loaded";
pub const KINDS: [&str; 5] =
    [KIND_SHADER_TRANSLATED, KIND_FIRST_BIND, KIND_PIPELINE_LOADED, KIND_PIPELINE_MADE, KIND_LIBRARY_LOADED];

#[derive(Debug)]
pub struct PipelineEvent {
    pub kind: &'static str,
    /// The shader's hash, or a pipeline's name in the library.
    pub hash: String,
    pub stage: Option<String>,
    pub detail: Option<String>,
    pub frame: u64,
    /// Milliseconds since the server started.
    pub at_ms: f64,
    pub duration_ms: Option<f64>,
}

/// A screenshot asked for, to be answered by the present path.
pub struct ShotRequest {
    reply: mpsc::Sender<Result<RawFrame, String>>,
}

impl ShotRequest {
    /// Hands over the frame read back.
    pub fn deliver(self, frame: RawFrame) {
        let _ = self.reply.send(Ok(frame));
    }

    /// Says why no frame can be had.
    pub fn fail(self, why: impl Into<String>) {
        let _ = self.reply.send(Err(why.into()));
    }
}

/// A bad-pixel capture asked for, to be answered after the frame it
/// records has been presented.
pub struct ScanRequest {
    pub params: ScanParams,
    reply: mpsc::Sender<Result<ScanReport, String>>,
}

impl ScanRequest {
    /// Hands over what the frame's scans found.
    pub fn deliver(self, report: ScanReport) {
        let _ = self.reply.send(Ok(report));
    }

    /// Says why no report can be had.
    pub fn fail(self, why: impl Into<String>) {
        let _ = self.reply.send(Err(why.into()));
    }
}

/// What the present path is told about a capture ([`scan_edge`]).
pub enum ScanEdge {
    /// A capture begins with the frame after this present; the request
    /// goes with it.
    Begin(ScanRequest),
    /// The frame recorded since the last present is over: the capture ends
    /// with this present (the frame is read back at once).
    End,
    /// The caller gave up: drop what was recorded.
    Cancel,
}

/// Everything the tools read.
pub struct Shared {
    pub(crate) started: Instant,
    pub(crate) config: Config,
    stats: Mutex<Option<(Stats, Instant)>>,
    pub(crate) log: Ring<Arc<str>>,
    pub(crate) events: Ring<Arc<PipelineEvent>>,
    device_lost: Mutex<Option<Arc<str>>>,
    shot: Mutex<Option<ShotRequest>>,
    /// One screenshot at a time.
    shot_gate: Mutex<()>,
    scan: Mutex<Option<ScanRequest>>,
    /// One capture at a time.
    scan_gate: Mutex<()>,
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    pub fn new(config: Config) -> Self {
        Shared {
            started: Instant::now(),
            config,
            stats: Mutex::new(None),
            log: Ring::new(LOG_LINES),
            events: Ring::new(EVENTS),
            device_lost: Mutex::new(None),
            shot: Mutex::new(None),
            shot_gate: Mutex::new(()),
            scan: Mutex::new(None),
            scan_gate: Mutex::new(()),
        }
    }

    pub fn uptime_s(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub fn publish_stats(&self, stats: Stats) {
        *locked(&self.stats) = Some((stats, Instant::now()));
    }

    /// The last published values and how long ago they were.
    pub fn stats(&self) -> Option<(Stats, Duration)> {
        locked(&self.stats).as_ref().map(|(s, at)| (s.clone(), at.elapsed()))
    }

    pub fn push_log(&self, line: &str) {
        self.log.push(Arc::from(layer::cut(line.to_owned(), 4096)));
    }

    pub fn record_event(
        &self,
        kind: &'static str,
        hash: &str,
        stage: Option<&str>,
        detail: Option<&str>,
        frame: u64,
        duration_ms: Option<f64>,
    ) {
        self.events.push(Arc::new(PipelineEvent {
            kind,
            hash: hash.to_owned(),
            stage: stage.map(str::to_owned),
            detail: detail.map(str::to_owned),
            frame,
            at_ms: self.started.elapsed().as_secs_f64() * 1000.0,
            duration_ms,
        }));
    }

    /// Keeps the report of the first device loss (a later one adds nothing).
    pub fn set_device_lost(&self, report: String) {
        let mut slot = locked(&self.device_lost);
        if slot.is_none() {
            *slot = Some(Arc::from(report));
        }
    }

    pub fn device_lost(&self) -> Option<Arc<str>> {
        locked(&self.device_lost).clone()
    }

    /// For the present path: the screenshot waiting, if any.
    pub fn take_shot(&self) -> Option<ShotRequest> {
        let taken = locked(&self.shot).take();
        SHOT_FLAG.store(false, Ordering::Relaxed);
        taken
    }

    /// Asks the present path for the next frame and writes it as a PNG.
    pub fn screenshot(&self, scale: u32) -> Result<shot::Saved, String> {
        let _one = self
            .shot_gate
            .try_lock()
            .map_err(|_| "another screenshot is being taken; try again in a moment".to_owned())?;
        let (reply, answer) = mpsc::channel();
        *locked(&self.shot) = Some(ShotRequest { reply });
        SHOT_FLAG.store(true, Ordering::Relaxed);
        let frame = match answer.recv_timeout(self.config.shot_timeout) {
            Ok(Ok(frame)) => frame,
            Ok(Err(why)) => return Err(why),
            Err(RecvTimeoutError::Timeout) => {
                // Not taken: withdraw it so a later present does not pay for it.
                drop(locked(&self.shot).take());
                SHOT_FLAG.store(false, Ordering::Relaxed);
                return Err(format!(
                    "no frame was presented within {} s (is the game drawing?)",
                    self.config.shot_timeout.as_secs()
                ));
            }
            Err(RecvTimeoutError::Disconnected) => return Err("the request was dropped".into()),
        };
        shot::save(frame, scale, &self.config.shots_dir)
    }

    /// For the present path: the next step of the capture, if one is
    /// asked for.
    fn scan_edge(&self) -> Option<ScanEdge> {
        let phase = SCAN_PHASE.load(Ordering::Relaxed);
        match phase {
            SCAN_ARMED => {
                let request = locked(&self.scan).take()?;
                SCAN_PHASE.store(SCAN_RECORDING, Ordering::Relaxed);
                Some(ScanEdge::Begin(request))
            }
            SCAN_RECORDING => {
                SCAN_PHASE.store(SCAN_IDLE, Ordering::Relaxed);
                Some(ScanEdge::End)
            }
            SCAN_CANCELLED => {
                SCAN_PHASE.store(SCAN_IDLE, Ordering::Relaxed);
                Some(ScanEdge::Cancel)
            }
            _ => None,
        }
    }

    /// Arms a capture of the frame after the next present and waits for its
    /// report (`timeout`: for the two presents and the frame's own time),
    /// then writes it as JSON. Returns the summary and the file.
    pub fn find_bad_pixels(&self, params: ScanParams, timeout: Duration) -> Result<(ScanReport, PathBuf, String), String> {
        let _one = self
            .scan_gate
            .try_lock()
            .map_err(|_| "another bad-pixel capture is running; try again in a moment".to_owned())?;
        let (reply, answer) = mpsc::channel();
        *locked(&self.scan) = Some(ScanRequest { params, reply });
        SCAN_PHASE.store(SCAN_ARMED, Ordering::Relaxed);
        let report = match answer.recv_timeout(timeout) {
            Ok(Ok(report)) => report,
            Ok(Err(why)) => return Err(why),
            Err(RecvTimeoutError::Timeout) => {
                // Withdraw: not begun, or begun and to be dropped at the next present.
                drop(locked(&self.scan).take());
                let _ = SCAN_PHASE.compare_exchange(SCAN_ARMED, SCAN_IDLE, Ordering::Relaxed, Ordering::Relaxed);
                let _ = SCAN_PHASE.compare_exchange(SCAN_RECORDING, SCAN_CANCELLED, Ordering::Relaxed, Ordering::Relaxed);
                return Err(format!(
                    "no frame was captured within {} s (the game must present two frames; is it drawing?)",
                    timeout.as_secs()
                ));
            }
            Err(RecvTimeoutError::Disconnected) => return Err("the request was dropped".into()),
        };
        let path = badpix::save(&report, &self.config.shots_dir)?;
        let text = badpix::summary(&report);
        Ok((report, path, text))
    }
}

/// The address `VICTORY_MCP` names: a port, or `127.0.0.1:<port>`. Any other
/// address, a reserved port or a port below 1024 is refused.
pub fn parse_listen(value: &str) -> Result<SocketAddr, String> {
    let value = value.trim();
    let port = value.strip_prefix("127.0.0.1:").unwrap_or(value);
    let port: u16 = port
        .parse()
        .map_err(|_| format!("VICTORY_MCP={value:?}: expected a port number, or 127.0.0.1:<port> (only loopback is served)"))?;
    if port < 1024 {
        return Err(format!("VICTORY_MCP: port {port} is below 1024"));
    }
    if RESERVED_PORTS.contains(&port) {
        return Err(format!("VICTORY_MCP: ports 59340-59450 are reserved, not {port}"));
    }
    Ok(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
}

/// Starts the server and returns the log layer when `VICTORY_MCP` is set
/// and good; `None` (nothing started, nothing taken) otherwise. Called once,
/// where logging is set up.
pub fn init_from_env() -> Option<LogLayer> {
    let value = std::env::var_os("VICTORY_MCP")?;
    let addr = match parse_listen(&value.to_string_lossy()) {
        Ok(addr) => addr,
        Err(why) => {
            eprintln!("debug MCP server not started: {why}");
            return None;
        }
    };
    match init(addr, Config::default()) {
        Ok(layer) => Some(layer),
        Err(e) => {
            eprintln!("debug MCP server not started: {e}");
            None
        }
    }
}

/// Starts the server for the process on `addr`.
pub fn init(addr: SocketAddr, config: Config) -> std::io::Result<LogLayer> {
    if SHARED.get().is_some() {
        return Err(std::io::Error::other("already started"));
    }
    let shared = Arc::new(Shared::new(config));
    let handle = start(shared.clone(), addr)?;
    let bound = handle.addr();
    // The server thread keeps running for the life of the process.
    std::mem::forget(handle);
    if SHARED.set(shared.clone()).is_err() {
        return Err(std::io::Error::other("already started"));
    }
    ENABLED.store(true, Ordering::Relaxed);
    eprintln!("debug MCP server: http://{bound}/mcp (loopback only)");
    Ok(LogLayer { shared })
}

/// The frame meter's numbers of the last second.
pub fn publish_stats(stats: Stats) {
    if let Some(shared) = SHARED.get() {
        shared.publish_stats(stats);
    }
}

/// A shader or pipeline event. Callers check [`enabled`] first, so what they
/// format for the arguments is only made when the server runs.
pub fn pipeline_event(
    kind: &'static str,
    hash: &str,
    stage: Option<&str>,
    detail: Option<&str>,
    frame: u64,
    duration_ms: Option<f64>,
) {
    if let Some(shared) = SHARED.get() {
        shared.record_event(kind, hash, stage, detail, frame, duration_ms);
    }
}

/// The text of the device-lost report.
pub fn publish_device_lost(report: String) {
    if let Some(shared) = SHARED.get() {
        shared.set_device_lost(report);
    }
}

/// For the present path, once a frame: the screenshot waiting for this
/// frame, if any. One relaxed load when none is.
#[inline]
pub fn take_shot_request() -> Option<ShotRequest> {
    if !SHOT_FLAG.load(Ordering::Relaxed) {
        return None;
    }
    SHARED.get()?.take_shot()
}

/// For the present path, once a frame: the next step of the bad-pixel
/// capture (`find_bad_pixels`), if one is asked for. One relaxed load when
/// none is.
#[inline]
pub fn scan_edge() -> Option<ScanEdge> {
    if SCAN_PHASE.load(Ordering::Relaxed) == SCAN_IDLE {
        return None;
    }
    SHARED.get()?.scan_edge()
}

/// For the places that record draws and dispatches: whether a capture is
/// recording now. One relaxed load; false whenever no capture is asked for.
#[inline]
pub fn scan_recording() -> bool {
    SCAN_PHASE.load(Ordering::Relaxed) == SCAN_RECORDING
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_addresses() {
        assert_eq!(parse_listen("47820").unwrap(), "127.0.0.1:47820".parse().unwrap());
        assert_eq!(parse_listen(" 127.0.0.1:47820 ").unwrap(), "127.0.0.1:47820".parse().unwrap());
        for bad in ["", "0", "80", "0.0.0.0:47820", "192.168.1.5:47820", "localhost:47820", "59340", "59450", "59400", "65536", "-1", "abc"] {
            assert!(parse_listen(bad).is_err(), "{bad:?}");
        }
        assert!(parse_listen("59339").is_ok() && parse_listen("59451").is_ok());
    }

    #[test]
    fn nothing_runs_until_started() {
        // (No other test in this crate calls `init`.)
        assert!(!enabled());
        assert!(take_shot_request().is_none());
        publish_stats(Stats::default());
        pipeline_event(KIND_FIRST_BIND, "h", None, None, 1, None);
        publish_device_lost("x".into());
        assert!(SHARED.get().is_none());
    }

    #[test]
    fn the_first_device_loss_is_kept() {
        let shared = Shared::new(Config::default());
        assert!(shared.device_lost().is_none());
        shared.set_device_lost("first".into());
        shared.set_device_lost("second".into());
        assert_eq!(&*shared.device_lost().unwrap(), "first");
    }

    /// The capture's phase is one value for the process: tests that arm it
    /// run one at a time.
    pub(crate) static SCAN_TESTS: Mutex<()> = Mutex::new(());

    #[test]
    fn a_capture_runs_begin_then_end_and_answers_with_the_report() {
        use crate::badpix::{DrawScan, ScanReport};
        let _serial = SCAN_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let shared = Arc::new(Shared::new(Config { shots_dir: dir.path().to_owned(), ..Config::default() }));
        assert!(shared.scan_edge().is_none());
        assert!(!scan_recording());
        let asker = {
            let shared = shared.clone();
            std::thread::spawn(move || shared.find_bad_pixels(ScanParams { max_draws: 7, ..Default::default() }, Duration::from_secs(20)))
        };
        // The first present begins it; recording is on until the second.
        let request = loop {
            if let Some(ScanEdge::Begin(r)) = shared.scan_edge() {
                break r;
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(request.params.max_draws, 7);
        assert!(scan_recording());
        assert!(matches!(shared.scan_edge(), Some(ScanEdge::End)));
        assert!(!scan_recording());
        assert!(shared.scan_edge().is_none());
        request.deliver(ScanReport { frame: 12, draws: vec![DrawScan { index: 0, kind: "draw".into(), ..Default::default() }], ..Default::default() });
        let (report, path, text) = asker.join().unwrap().unwrap();
        assert_eq!(report.frame, 12);
        assert!(path.starts_with(dir.path()) && path.to_string_lossy().ends_with("-f12.json"));
        assert!(text.contains("1 draws and dispatches scanned"), "{text}");
    }

    #[test]
    fn a_caller_that_gives_up_while_recording_has_the_capture_dropped() {
        let _serial = SCAN_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let shared = Arc::new(Shared::new(Config::default()));
        let asker = {
            let shared = shared.clone();
            std::thread::spawn(move || shared.find_bad_pixels(ScanParams::default(), Duration::from_millis(150)))
        };
        let request = loop {
            if let Some(ScanEdge::Begin(r)) = shared.scan_edge() {
                break r;
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        assert!(asker.join().unwrap().unwrap_err().contains("no frame was captured"));
        // The next present is told to drop it, and then nothing is asked for.
        assert!(matches!(shared.scan_edge(), Some(ScanEdge::Cancel)));
        assert!(!scan_recording());
        assert!(shared.scan_edge().is_none());
        drop(request);
    }

    #[test]
    fn a_second_capture_while_one_runs_is_refused() {
        let _serial = SCAN_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let shared = Arc::new(Shared::new(Config::default()));
        let first = {
            let shared = shared.clone();
            std::thread::spawn(move || shared.find_bad_pixels(ScanParams::default(), Duration::from_millis(300)))
        };
        // Wait until the first has armed.
        while SCAN_PHASE.load(Ordering::Relaxed) == SCAN_IDLE {
            std::thread::sleep(Duration::from_millis(2));
        }
        let err = shared.find_bad_pixels(ScanParams::default(), Duration::from_millis(10)).unwrap_err();
        assert!(err.contains("another bad-pixel capture"), "{err}");
        assert!(first.join().unwrap().is_err());
        assert!(shared.scan_edge().is_none());
        SCAN_PHASE.store(SCAN_IDLE, Ordering::Relaxed);
    }

    #[test]
    fn a_screenshot_with_no_present_times_out_and_withdraws() {
        let shared = Shared::new(Config { shot_timeout: Duration::from_millis(50), ..Config::default() });
        let err = shared.screenshot(1).unwrap_err();
        assert!(err.contains("no frame was presented"), "{err}");
        assert!(shared.take_shot().is_none());
    }
}
