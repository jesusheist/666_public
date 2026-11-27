//! The settings the launcher owns (`victory play`): DLSS mode, frame generation,
//! frame rate cap, console, picture size and the layer's own switches the fork's
//! F1 menu listed. They are not registry tunables: the launcher reads them from
//! the settings file `VICTORY_MENU_FILE` names (`victory-settings.txt`) at every
//! start and turns the `VICTORY_MENU_...` ones into the layer's switches, so the
//! menu saves them there and "Save and restart" runs the launcher again
//! (`VICTORY_MENU_PLAY` with `VICTORY_MENU_ARGS`). Frame generation also changes
//! at once when the run started with it ready.
//!
//! A value chosen here waits in memory until it is saved; what the run started
//! with is the environment the launcher gave it.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

#[derive(Default)]
struct State {
    /// The values chosen in this run, by variable.
    chosen: BTreeMap<String, String>,
    /// The values saved to the file in this run (what the next start has), by variable.
    saved: BTreeMap<String, String>,
}

static STATE: Mutex<State> = Mutex::new(State { chosen: BTreeMap::new(), saved: BTreeMap::new() });

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn env(var: &str) -> String {
    std::env::var(var).unwrap_or_default()
}

/// What the line holds now: the value chosen, else what the next start has.
pub(crate) fn current(var: &str) -> String {
    let s = state();
    s.chosen.get(var).cloned().unwrap_or_else(|| started(&s, var))
}

/// What the next start has: the value saved in this run, else what this run started with.
fn started(s: &State, var: &str) -> String {
    s.saved.get(var).cloned().unwrap_or_else(|| env(var))
}

/// A value was chosen and not saved yet.
pub(crate) fn changed(var: &str) -> bool {
    let s = state();
    s.chosen.get(var).is_some_and(|v| *v != started(&s, var))
}

/// Whether a change of `var` shows at once. Frame generation does when this run started with
/// it ready (the launcher asks for that whenever Streamline is there).
pub(crate) fn live(var: &str) -> bool {
    var == "VICTORY_MENU_FRAMEGEN" && crate::streamline::framegen_live()
}

/// Chooses `value` for `var` (an empty value: not set).
pub(crate) fn set(var: &str, value: &str) {
    state().chosen.insert(var.to_owned(), value.to_owned());
    if live(var) {
        // Off (0) or the frames shown for each one rendered, from the next present on.
        crate::streamline::set_framegen(value.parse().unwrap_or(0));
    }
}

/// Whether anything chosen is not saved.
pub(crate) fn any_changed() -> bool {
    let s = state();
    s.chosen.iter().any(|(k, v)| *v != started(&s, k))
}

fn file() -> Option<std::ffi::OsString> {
    std::env::var_os("VICTORY_MENU_FILE")
}

/// The file's text after `values` were put over its `VICTORY_` lines.
pub(crate) fn file_text(old: &str, values: &[(String, String)]) -> String {
    let mut kept: Vec<(String, String)> = old
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())))
        .filter(|(k, _)| k.starts_with("VICTORY_"))
        .collect();
    for (k, v) in values {
        kept.retain(|(name, _)| name != k);
        kept.push((k.clone(), v.clone()));
    }
    let mut text = String::from("# Saved by the menu; applied over the play settings at every start (an empty value: unset).\r\n");
    for (k, v) in &kept {
        text.push_str(&format!("{k}={v}\r\n"));
    }
    text
}

/// Writes the values chosen and not saved over those in the settings file; whether it could, and what to tell.
pub(crate) fn save() -> (bool, String) {
    let Some(path) = file() else {
        return (false, "No settings file: start with victory play".to_owned());
    };
    let mut s = state();
    let todo: Vec<(String, String)> = s.chosen.iter().filter(|(k, v)| **v != started(&s, k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    let text = file_text(&std::fs::read_to_string(&path).unwrap_or_default(), &todo);
    match std::fs::write(&path, text) {
        Ok(()) => {
            tracing::info!(path = %std::path::Path::new(&path).display(), "settings menu: saved");
            for (k, v) in todo {
                s.saved.insert(k, v);
            }
            (true, "Saved, applies at the next start".to_owned())
        }
        Err(e) => {
            tracing::warn!(error = %e, "settings menu: not saved");
            (false, "Could not save the settings file".to_owned())
        }
    }
}

/// Puts the lines `vars` (variable, what the launcher starts with) back to their play value and
/// forgets them in the settings file.
pub(crate) fn reset(vars: &[(&'static str, &'static str)]) {
    {
        let mut s = state();
        for (var, default) in vars {
            s.chosen.insert((*var).to_owned(), (*default).to_owned());
            s.saved.insert((*var).to_owned(), (*default).to_owned());
        }
    }
    for (var, default) in vars {
        if live(var) {
            crate::streamline::set_framegen(default.parse().unwrap_or(0));
        }
    }
    let Some(path) = file() else { return };
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let text: String = old
        .lines()
        .filter(|l| l.split_once('=').is_none_or(|(k, _)| !vars.iter().any(|(var, _)| *var == k.trim())))
        .map(|l| format!("{l}\r\n"))
        .collect();
    if text.lines().all(|l| l.trim().is_empty() || l.starts_with('#')) {
        let _ = std::fs::remove_file(&path);
    } else {
        let _ = std::fs::write(&path, text);
    }
}

/// Starts the launcher again in a new console, a few seconds on, and ends this run. Returns
/// only when it could not, with what to tell.
pub(crate) fn restart() -> Option<String> {
    restart_with("")
}

/// [`restart`] with `extra` after the arguments (" --preload": the start shows the preload screen).
pub(crate) fn restart_with(extra: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let Some(play) = std::env::var_os("VICTORY_MENU_PLAY") else {
        return Some("Saved: start the launcher again yourself".to_owned());
    };
    let play = play.to_string_lossy().replace('\'', "''");
    let args = format!("{}{extra}", env("VICTORY_MENU_ARGS"));
    let command = format!("Start-Sleep 3; & '{play}' {args}");
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    match std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &command])
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
    {
        Ok(_) => {
            tracing::info!(command, "settings menu: restarting");
            // Ended at once: std::process::exit runs the DLLs' unloading, which with
            // Streamline loaded did not come back (a windowless leftover process).
            // SAFETY: this process, with nothing of its own left to save.
            unsafe {
                use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
                TerminateProcess(GetCurrentProcess(), 0);
            }
            std::process::exit(0);
        }
        Err(e) => {
            tracing::warn!(error = %e, "settings menu: restart failed");
            Some("Saved, but the restart failed: start it again yourself".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_keeps_other_lines_and_replaces_a_value() {
        let old = "# a comment\r\nVICTORY_MENU_FPS=60\r\nVICTORY_BVH_BUILDER=\r\nOTHER=1\r\n";
        let text = file_text(old, &[("VICTORY_MENU_FPS".to_owned(), "120".to_owned()), ("VICTORY_RELAY".to_owned(), "1".to_owned())]);
        assert!(text.contains("VICTORY_MENU_FPS=120\r\n"));
        assert!(!text.contains("VICTORY_MENU_FPS=60"));
        assert!(text.contains("VICTORY_BVH_BUILDER=\r\n"));
        assert!(text.contains("VICTORY_RELAY=1\r\n"));
        assert!(!text.contains("OTHER"));
    }
}
