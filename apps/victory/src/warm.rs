//! `victory warm`: host pipelines made ahead of a run (`known_pipelines`), in
//! processes of their own: a compiler that crashes or hangs costs one pipeline.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use victory_hle::known_pipelines as known;

/// Pipelines a process makes before another takes its place: the compiler's
/// and the translations' memory goes with it.
const AT_ONCE: usize = 40;
/// A pipeline that takes longer is taken for one the compiler hangs on.
const LONGEST: Duration = Duration::from_secs(180);

/// As the runs count one that takes long.
const SLOW_MS: f64 = 1000.0;

pub(crate) fn folder() -> Result<PathBuf> {
    let cfg = victory_core::Config::load()?;
    Ok(cfg.cache_subdir("shaders")?.with_file_name("pipelines"))
}

/// What a run does besides making pipelines: who is told of progress, and the
/// priority the processes run at.
pub struct Options<'a> {
    /// Told after each pipeline: (made so far, to make in all).
    pub report: Option<&'a (dyn Fn(usize, usize) + Sync)>,
    /// Windows priority class of the processes (`0x40` idle, `0x4000` below normal).
    pub priority: u32,
    /// Print the summary lines.
    pub chatty: bool,
    /// Asked between pipelines: true ends the run (what is made stays).
    pub stop: Option<&'a (dyn Fn() -> bool + Sync)>,
}

const IDLE_PRIORITY_CLASS: u32 = 0x40;

impl Default for Options<'_> {
    fn default() -> Self {
        Options { report: None, priority: IDLE_PRIORITY_CLASS, chatty: true, stop: None }
    }
}

/// What a run has still to make: the translator's and the driver's fingerprint,
/// every known key and the ones to make (see [`victory_core::preload::select_todo`]).
pub struct Plan {
    pub dir: PathBuf,
    pub fingerprint: String,
    pub keys: Vec<String>,
    pub todo: Vec<usize>,
    pub made_before: usize,
    pub bad: usize,
    /// What the same selection holds with the made ones counted in.
    pub scope: usize,
}

/// Reads the cache's lists. Makes the host device (the fingerprint holds the
/// driver's), so a launcher asks a process of its own for this.
pub fn plan(limit: Option<usize>, again: bool, used: bool) -> Result<Plan> {
    let dir = folder()?;
    victory_hle::known_pipelines::use_translations(dir.with_file_name("shaders"));
    let fingerprint = known::fingerprint();
    let keys = known::keys(&dir);
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
    let (listed, bad, early) = (read("warm-done.txt"), read("warm-bad.txt"), read("early-pipelines.txt"));
    let todo = victory_core::preload::select_todo(&keys, &listed, &bad, &early, &fingerprint, again, used, limit);
    let scope = victory_core::preload::select_todo(&keys, &listed, &bad, &early, &fingerprint, true, used, None).len();
    let made_before = victory_core::preload::done_keys(&listed, &fingerprint).len();
    let bad = bad.lines().filter_map(|l| l.split(' ').next()).collect::<HashSet<_>>().len();
    Ok(Plan { dir, fingerprint, keys, todo, made_before, bad, scope })
}

pub fn run(jobs: Option<usize>, limit: Option<usize>, again: bool, used: bool, these: Option<String>) -> Result<()> {
    if let Some(these) = these {
        return make(&folder()?, &these);
    }
    run_with(jobs, limit, again, used, &Options::default())
}

/// [`run`] with `options`.
pub fn run_with(jobs: Option<usize>, limit: Option<usize>, again: bool, used: bool, options: &Options) -> Result<()> {
    let began = Instant::now();
    let Plan { dir, fingerprint, keys, todo: all, made_before, bad, .. } = plan(limit, again, used)?;
    let done_file = dir.join("warm-done.txt");
    let bad_file = dir.join("warm-bad.txt");
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let jobs = jobs.unwrap_or(cores * 2 / 3).max(1);
    if options.chatty {
        println!(
            "{} pipelines known, {} made before ({fingerprint}), {bad} bad; {} to make, {jobs} processes",
            keys.len(),
            made_before,
            all.len()
        );
    }
    let chunks: Vec<&[usize]> = all.chunks(AT_ONCE).collect();
    let next = AtomicUsize::new(0);
    let state = Mutex::new((Vec::<f64>::new(), 0usize, 0usize));
    let done_out = Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(&done_file)?);
    // (Read back by the runs: these are made beside the others, not before.)
    let slow_out = Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(dir.join("slow-pipelines.txt"))?);
    let bad_out = Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(&bad_file)?);
    std::thread::scope(|s| {
        for _ in 0..jobs.min(chunks.len()) {
            s.spawn(|| {
                while let Some(&chunk) = chunks.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let mut rest = chunk;
                    while !rest.is_empty() && !options.stop.is_some_and(|stop| stop()) {
                        let (made, ended) = child(rest, options.priority, |n, ms, there| {
                            if ms >= SLOW_MS {
                                let _ = writeln!(lock(&slow_out), "\n{} {ms:.0}", keys[n]);
                            }
                            let mut state = lock(&state);
                            state.0.push(ms);
                            state.1 += usize::from(!there);
                            let count = state.0.len();
                            if let Some(report) = options.report {
                                report(count, all.len());
                            }
                            if options.chatty && count % 1000 == 0 {
                                println!("{count} of {}, {:.0} s", all.len(), began.elapsed().as_secs_f64());
                            }
                            drop(state);
                            let _ = writeln!(lock(&done_out), "{fingerprint} {}", keys[n]);
                        });
                        rest = &rest[made.min(rest.len())..];
                        match (ended, rest.split_first()) {
                            (Some(why), Some((&n, after))) => {
                                // The one it was at.
                                println!("pipeline {n} {}: {why}", keys[n]);
                                let _ = writeln!(lock(&bad_out), "{} {why}", keys[n]);
                                lock(&state).2 += 1;
                                rest = after;
                            }
                            _ => break,
                        }
                    }
                }
            });
        }
    });
    let (mut times, refused, crashed) = state.into_inner().unwrap_or_else(|e| e.into_inner());
    if !options.chatty {
        return Ok(());
    }
    times.sort_by(f64::total_cmp);
    let at = |q: f64| times.get((times.len() as f64 * q) as usize).copied().unwrap_or(0.0);
    println!(
        "{} made in {:.0} s ({refused} have no host pipeline, {crashed} ended their process); ms each: median {:.1}, p90 {:.1}, p99 {:.1}, most {:.0}, sum {:.0} s",
        times.len(),
        began.elapsed().as_secs_f64(),
        at(0.5),
        at(0.9),
        at(0.99),
        times.last().copied().unwrap_or(0.0),
        times.iter().sum::<f64>() / 1e3
    );
    Ok(())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Has a process make `these`; how many it made, and how it ended if not well.
fn child(these: &[usize], priority: u32, mut each: impl FnMut(usize, f64, bool)) -> (usize, Option<String>) {
    let list: Vec<String> = these.iter().map(usize::to_string).collect();
    let spawned = std::env::current_exe().and_then(|exe| {
        use std::os::windows::process::CommandExt;
        Command::new(exe)
            .args(["warm", "--these", &list.join(",")])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(priority)
            .spawn()
    });
    let Ok(mut process) = spawned else { return (these.len(), None) };
    let Some(out) = process.stdout.take() else { return (these.len(), None) };
    let process = Mutex::new(process);
    let (heard, over) = (Mutex::new(Instant::now()), AtomicBool::new(false));
    let hung = AtomicBool::new(false);
    let mut made = 0;
    std::thread::scope(|s| {
        s.spawn(|| {
            while !over.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                if lock(&heard).elapsed() > LONGEST {
                    hung.store(true, Ordering::Relaxed);
                    let _ = lock(&process).kill();
                    return;
                }
            }
        });
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            *lock(&heard) = Instant::now();
            let mut words = line.split(' ');
            if words.next() == Some("made")
                && let (Some(n), Some(ms), Some(there)) = (words.next(), words.next(), words.next())
                && let (Ok(n), Ok(ms)) = (n.parse(), ms.parse())
            {
                made += 1;
                each(n, ms, there == "1");
            }
        }
        over.store(true, Ordering::Relaxed);
    });
    let status = lock(&process).wait();
    let well = status.as_ref().is_ok_and(|s| s.success());
    if well {
        return (these.len(), None);
    }
    let why = if hung.load(Ordering::Relaxed) {
        format!("hung for {} s", LONGEST.as_secs())
    } else {
        status.map_or_else(|e| e.to_string(), |s| format!("{s}"))
    };
    (made, Some(why))
}

/// What such a process does.
fn make(dir: &Path, these: &str) -> Result<()> {
    let these: Vec<usize> = these.split(',').map(|n| n.parse().context("--these")).collect::<Result<_>>()?;
    let cfg = victory_core::Config::load()?;
    victory_hle::known_pipelines::use_translations(cfg.cache_subdir("shaders")?);
    let mut out = std::io::stdout();
    known::make(dir, &these, |n, came| {
        let told = match came {
            None => writeln!(out, "at {n}"),
            Some((ms, there)) => writeln!(out, "made {n} {ms:.2} {}", u8::from(there)),
        };
        if told.and_then(|()| out.flush()).is_err() {
            // Whoever asked is gone.
            std::process::exit(1);
        }
    });
    // Without what a run's end does (nothing to save, the host's threads).
    let _ = out.flush();
    std::process::exit(0)
}
