//! `victory preload`, and the start screen's flow inside `victory play`.
//!
//! A start whose cache is not complete for this build and this driver (the
//! title's shaders translated, the host pipelines the runs drew with made)
//! shows the preload screen before the title starts; see
//! `victory_core::preload` for the decisions and `victory_gpu::preload_screen`
//! for the screen.
//!
//! The launcher never makes the host device itself (the cache's fingerprint
//! holds the driver, and a device made here would live on into the title's
//! run): it asks `victory preload --check` and starts `victory preload` as
//! the worker, in processes of their own, at below normal priority and with at
//! most two thirds of the cores. The worker tells its progress in `@ ` lines
//! on its output. Stopping it (Esc) loses nothing: shaders are in the cache
//! and the pipelines made are in `warm-done.txt`, so the next start goes on.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use victory_core::preload::{Event, Mode, Plan, Progress, Status, decide, shaders_marker, shaders_marker_valid};

use crate::{translate, warm};

const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x4000;
const IDLE_PRIORITY_CLASS: u32 = 0x40;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How long the background worker waits before it starts making pipelines.
const BACKGROUND_DELAY: Duration = Duration::from_secs(60);

#[derive(clap::Args, Clone, Debug)]
pub(crate) struct Args {
    /// Folders to look for shaders in (default: the folder of `--exe`, else
    /// the configured game and test titles).
    paths: Vec<PathBuf>,
    /// The title's executable: its folder is looked through, its size and
    /// write time say whether the shaders were done for this copy.
    #[arg(long)]
    exe: Option<PathBuf>,
    /// Only print what is ready (`@ status ...`) and end.
    #[arg(long)]
    check: bool,
    /// The worker for while the game runs: after a delay, one process at idle
    /// priority makes the pipelines the runs did not draw with; ends when its
    /// input closes (the game's end).
    #[arg(long)]
    background: bool,
    /// Make every known pipeline, not only those earlier runs drew with.
    #[arg(long)]
    all: bool,
    /// Processes at once (default: two thirds of the logical cores).
    #[arg(long)]
    jobs: Option<usize>,
}

fn two_thirds() -> usize {
    (std::thread::available_parallelism().map_or(4, |n| n.get()) * 2 / 3).max(1)
}

/// The title's size and write time in seconds (0, 0 without it).
fn exe_stamp(exe: Option<&Path>) -> (u64, u64) {
    let meta = exe.and_then(|e| std::fs::metadata(e).ok());
    let size = meta.as_ref().map_or(0, |m| m.len());
    let stamp = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    (size, stamp)
}

fn marker_file() -> Result<PathBuf> {
    Ok(warm::folder()?.join("preload-shaders.txt"))
}

/// Says one event; the worker ends when whoever reads it is gone.
fn say(event: &Event) {
    use std::io::Write;
    let mut out = std::io::stdout();
    if writeln!(out, "{}", event.line()).and_then(|()| out.flush()).is_err() {
        std::process::exit(1);
    }
}

/// Lets an event through at most every quarter second.
struct Throttle(Mutex<Instant>);

impl Throttle {
    fn new() -> Throttle {
        Throttle(Mutex::new(Instant::now() - Duration::from_secs(1)))
    }

    fn due(&self) -> bool {
        let mut last = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if last.elapsed() < Duration::from_millis(250) {
            return false;
        }
        *last = Instant::now();
        true
    }
}

/// `victory preload`.
pub(crate) fn run(a: Args) -> Result<()> {
    let exe = a.exe.as_deref();
    let paths = match (a.paths.is_empty(), exe) {
        (true, Some(exe)) => vec![exe.parent().map(Path::to_path_buf).unwrap_or_default()],
        _ => crate::default_paths(a.paths.clone())?,
    };
    let (size, stamp) = exe_stamp(exe);
    let used = !a.all;
    let first = warm::plan(None, false, used)?;
    let marker = shaders_marker(&first.fingerprint, size, stamp);
    let marker_file = marker_file()?;
    let shaders_done = shaders_marker_valid(&std::fs::read_to_string(&marker_file).unwrap_or_default(), &marker);
    let status = Status { shaders_done, todo: first.todo.len(), scope: first.scope };
    if a.check {
        println!("{}", status.line());
        return Ok(());
    }
    let jobs = a.jobs.unwrap_or_else(two_thirds);
    if a.background {
        return background(shaders_done);
    }
    if !shaders_done {
        let total = translate::precompile_total(&paths);
        say(&Event::ShaderStage { total });
        let throttle = Throttle::new();
        let report = |done: u64, total: u64, count: u64| {
            if done >= total || throttle.due() {
                say(&Event::Shaders { done, total, count });
            }
        };
        let whole = translate::precompile_with(paths, Some(jobs), false, Some(&report), None)?;
        if whole {
            use std::io::Write;
            if let Some(dir) = marker_file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&marker_file).context("preload-shaders.txt")?;
            writeln!(file, "{marker}")?;
        }
    }
    // (The shaders are in the cache now: the pipelines of the book can be made.)
    let todo = warm::plan(None, false, used)?.todo.len() as u64;
    say(&Event::PipelineStage { total: todo });
    if todo > 0 {
        let throttle = Throttle::new();
        let report = |done: usize, total: usize| {
            if throttle.due() {
                say(&Event::Pipelines { done: done as u64, total: total as u64 });
            }
        };
        let options = warm::Options { report: Some(&report), priority: BELOW_NORMAL_PRIORITY_CLASS, chatty: false, stop: None };
        warm::run_with(Some(jobs), None, false, used, &options)?;
        say(&Event::Pipelines { done: todo, total: todo });
    }
    say(&Event::Done);
    Ok(())
}

/// The worker for while the game runs.
fn background(shaders_done: bool) -> Result<()> {
    // Ends with the game: its end closes this input.
    victory_core::threads::spawn("victory-preload-bg", || {
        let mut sink = Vec::new();
        let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut sink);
        std::process::exit(0);
    });
    if !shaders_done {
        return Ok(());
    }
    std::thread::sleep(BACKGROUND_DELAY);
    let options = warm::Options { report: None, priority: IDLE_PRIORITY_CLASS, chatty: false, stop: None };
    warm::run_with(Some(1), None, false, false, &options)
}

fn env_text(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn env_on(name: &str) -> bool {
    matches!(env_text(name).trim().to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes")
}

/// A process of this program with `args`, for the preload: no console window
/// and none of the log file settings (it would write into the run's log).
fn child(args: &[&std::ffi::OsStr], priority: u32) -> Command {
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("victory.exe"));
    let mut c = Command::new(exe);
    c.args(args).env_remove("VICTORY_LOG_FILE").env_remove("VICTORY_LOG_DIR").stderr(Stdio::null());
    c.creation_flags(priority | CREATE_NO_WINDOW);
    c
}

/// What `victory preload --check` says; `None` when it could not.
fn check(exe: &Path) -> Option<Status> {
    let out = child(&["preload".as_ref(), "--check".as_ref(), "--exe".as_ref(), exe.as_os_str()], 0)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(Status::parse)
}

/// Ends a process and the ones it started.
fn kill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

/// The screen with a worker behind it; whether the title is to start.
fn screen(exe: &Path, wait: bool, status: Status) -> bool {
    let began = Instant::now();
    let progress = Arc::new(Mutex::new(Progress::new(&status)));
    let spawned = child(&["preload".as_ref(), "--exe".as_ref(), exe.as_os_str()], BELOW_NORMAL_PRIORITY_CLASS)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn();
    let mut worker = match spawned {
        Ok(w) => w,
        Err(e) => {
            println!("Preload: the worker did not start ({e}); starting the game.");
            return true;
        }
    };
    let Some(out) = worker.stdout.take() else { return true };
    let ended = Arc::new(AtomicBool::new(false));
    let (reader_progress, reader_ended) = (progress.clone(), ended.clone());
    victory_core::threads::spawn("victory-preload-read", move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(event) = Event::parse(&line) {
                reader_progress.lock().unwrap_or_else(|e| e.into_inner()).apply(&event, began.elapsed().as_secs_f64());
            }
        }
        // The worker is gone, done or not: what it made stays, the title starts.
        reader_progress.lock().unwrap_or_else(|e| e.into_inner()).apply(&Event::Done, began.elapsed().as_secs_f64());
        reader_ended.store(true, Ordering::Relaxed);
    });
    let outcome = victory_gpu::preload_screen::show(wait, || {
        let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
        p.tick(began.elapsed().as_secs_f64());
        p.clone()
    });
    if !ended.load(Ordering::Relaxed) {
        kill_tree(worker.id());
    }
    let _ = worker.wait();
    outcome != victory_gpu::preload_screen::Outcome::Quit
}

/// The worker for while the game runs, started from the launcher.
fn start_background(exe: &Path) {
    let spawned = child(&["preload".as_ref(), "--background".as_ref(), "--exe".as_ref(), exe.as_os_str()], IDLE_PRIORITY_CLASS)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn();
    if let Ok(mut worker) = spawned {
        // Held open until this process ends: that is the worker's sign to end too.
        std::mem::forget(worker.stdin.take());
        std::mem::forget(worker);
    }
}

/// What a start does before the title: the screen, if the cache is not
/// complete and `VICTORY_PRELOAD` (or `forced`, the menu's "PRELOAD NOW")
/// says so, and the background worker if `VICTORY_PRELOAD_BACKGROUND` is on.
/// Returns false when the window was closed: the launcher ends.
pub(crate) fn start(exe: &Path, forced: bool) -> bool {
    let mode = Mode::parse(&env_text("VICTORY_PRELOAD"));
    let background = env_on("VICTORY_PRELOAD_BACKGROUND");
    if mode == Mode::Off && !forced && !background {
        return true;
    }
    let status = check(exe);
    if status.is_none() {
        println!("Preload: could not read what is ready; starting the game.");
    }
    let mut go = true;
    if let Some(status) = status
        && let Plan::Show { wait } = decide(mode, forced, &status)
    {
        println!("Preload: {} of {} pipelines to make, shaders {}.", status.todo, status.scope, if status.shaders_done { "ready" } else { "not all translated" });
        go = screen(exe, wait, status);
    }
    if go && background && status.is_some() {
        start_background(exe);
    }
    go
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Wrap {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn the_command_line_reads_its_switches() {
        let a = Wrap::parse_from(["preload", "--check", "--exe", r"C:	itle\game.exe", "--jobs", "3"]).args;
        assert!(a.check && !a.background && !a.all);
        assert_eq!(a.jobs, Some(3));
        assert_eq!(a.exe.as_deref(), Some(Path::new(r"C:	itle\game.exe")));
        let b = Wrap::parse_from(["preload", "--background", "--all"]).args;
        assert!(b.background && b.all && b.exe.is_none());
    }

    #[test]
    fn the_stamp_follows_the_copy_of_the_title() {
        assert_eq!(exe_stamp(None), (0, 0));
        assert_eq!(exe_stamp(Some(Path::new(r"C:
o\suchile.exe"))), (0, 0));
        let dir = tempfile_dir();
        let file = dir.join("game.exe");
        std::fs::write(&file, [1u8; 100]).unwrap();
        let (size, stamp) = exe_stamp(Some(&file));
        assert_eq!(size, 100);
        assert!(stamp > 1_600_000_000);
        std::fs::write(&file, [1u8; 101]).unwrap();
        assert_ne!(exe_stamp(Some(&file)).0, size);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("victory-preload-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn progress_lines_pass_through_the_pipe_text_and_noise_is_dropped() {
        // What the launcher's reader does with a worker's output.
        let output = "log noise
@ stage shaders 100
@ shaders 50 100 7
@ done
";
        let mut p = Progress::new(&Status { shaders_done: false, todo: 0, scope: 0 });
        for (i, line) in output.lines().enumerate() {
            if let Some(e) = Event::parse(line) {
                p.apply(&e, i as f64);
            }
        }
        assert!(p.finished());
        assert_eq!(p.shader_count, 7);
    }

    #[test]
    fn the_throttle_lets_one_event_through_a_quarter_second() {
        let t = Throttle::new();
        assert!(t.due());
        assert!(!t.due());
    }
}
