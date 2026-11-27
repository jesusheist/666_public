//! The keyboard and mouse bindings the in-game menu's CONTROLS page edits.
//!
//! `victory-hle` owns the real key layouts (`modules/controls.rs`) and fills
//! this registry at start: for each layout (on foot, vehicle, aircraft) the
//! list of actions the page shows, the default bindings and the bindings in
//! force (the defaults with the controls file laid over them). The menu
//! (`victory-gpu`, which must not depend on `victory-hle`) reads and edits
//! the registry; `victory-hle` sees the change through [`generation`] and
//! builds its layouts again from [`tables`], so a rebinding applies at once.
//! [`save`] writes the bindings into the controls file the run uses.
//!
//! A binding is the text the controls file takes: a key name maps to a list
//! of binding strings (`Sprint`, `tap:Holster`, `macro:Y 60, wait 100, A 60`).
//! A key "has" an action when one of its strings, without a `style:` prefix,
//! is the action's name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Key name to its binding strings (none: the key is unbound).
pub type Table = BTreeMap<String, Vec<String>>;

/// The three key layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    OnFoot,
    Vehicle,
    Aircraft,
}

impl Layout {
    pub const ALL: [Layout; 3] = [Layout::OnFoot, Layout::Vehicle, Layout::Aircraft];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The name shown on the page.
    pub fn label(self) -> &'static str {
        match self {
            Layout::OnFoot => "ON FOOT",
            Layout::Vehicle => "VEHICLE",
            Layout::Aircraft => "AIRCRAFT",
        }
    }

    /// The table's name in the controls file (`[controls.<name>]`).
    pub fn toml_name(self) -> &'static str {
        match self {
            Layout::OnFoot => "on_foot",
            Layout::Vehicle => "vehicle",
            Layout::Aircraft => "aircraft",
        }
    }

    pub fn next(self) -> Layout {
        Layout::ALL[(self.index() + 1) % 3]
    }

    pub fn prev(self) -> Layout {
        Layout::ALL[(self.index() + 2) % 3]
    }
}

/// An action the page lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Def {
    /// The name the controls file uses (`Sprint`, `Handbrake`, `A`).
    pub id: String,
    /// What the page shows (`Enter / exit vehicle`).
    pub label: String,
    /// What the pad does for it (`A`, `LS click`).
    pub pad: String,
}

impl Def {
    pub fn new(id: &str, label: &str, pad: &str) -> Def {
        Def { id: id.to_owned(), label: label.to_owned(), pad: pad.to_owned() }
    }
}

/// What `victory-hle` hands over at start.
#[derive(Clone, Debug, Default)]
pub struct Setup {
    pub defs: [Vec<Def>; 3],
    pub defaults: [Table; 3],
    pub current: [Table; 3],
    /// The controls file the run was started from.
    pub source: Option<PathBuf>,
    /// The vehicle layout in force is the aircraft one.
    pub aircraft_active: bool,
    /// The keyboard and mouse controls are on (changes show in the game).
    pub active: bool,
}

/// An action line as the page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub id: String,
    pub label: String,
    pub pad: String,
    pub keys: Vec<String>,
    pub default: Vec<String>,
    /// Other actions (by label) that are on one of this line's keys too.
    pub shared: Vec<String>,
    pub changed: bool,
}

/// Whether the binding string `text` is the action `id` (any `style:` prefix aside).
fn holds(text: &str, id: &str) -> bool {
    let bare = text.split_once(':').map_or(text, |(_, rest)| rest);
    bare.trim().eq_ignore_ascii_case(id)
}

/// The model: all of the logic, apart from the process-wide registry below.
#[derive(Clone, Debug, Default)]
pub struct Binds {
    defs: [Vec<Def>; 3],
    defaults: [Table; 3],
    current: [Table; 3],
    saved: [Table; 3],
    source: Option<PathBuf>,
    aircraft_active: bool,
    active: bool,
}

impl Binds {
    pub fn new(setup: Setup) -> Binds {
        Binds {
            defs: setup.defs,
            defaults: setup.defaults,
            saved: setup.current.clone(),
            current: setup.current,
            source: setup.source,
            aircraft_active: setup.aircraft_active,
            active: setup.active,
        }
    }

    pub fn defs(&self, layout: Layout) -> &[Def] {
        &self.defs[layout.index()]
    }

    fn label_of(&self, layout: Layout, text: &str) -> String {
        let bare = text.split_once(':').map_or(text, |(_, rest)| rest).trim();
        self.defs(layout).iter().find(|d| d.id.eq_ignore_ascii_case(bare)).map_or_else(|| text.to_owned(), |d| d.label.clone())
    }

    fn keys_in(table: &Table, id: &str) -> Vec<String> {
        table.iter().filter(|(_, v)| v.iter().any(|s| holds(s, id))).map(|(k, _)| k.clone()).collect()
    }

    /// The keys that have action `id` now.
    pub fn keys_of(&self, layout: Layout, id: &str) -> Vec<String> {
        Self::keys_in(&self.current[layout.index()], id)
    }

    /// The pairs (key, binding text) that hold `id`, for comparing with the defaults.
    fn pairs(table: &Table, id: &str) -> Vec<(String, String)> {
        table.iter().flat_map(|(k, v)| v.iter().filter(|s| holds(s, id)).map(move |s| (k.clone(), s.clone()))).collect()
    }

    pub fn is_changed(&self, layout: Layout, id: &str) -> bool {
        Self::pairs(&self.current[layout.index()], id) != Self::pairs(&self.defaults[layout.index()], id)
    }

    /// Every line of `layout`, in the registry's order.
    pub fn lines(&self, layout: Layout) -> Vec<Line> {
        let i = layout.index();
        self.defs[i]
            .iter()
            .map(|d| {
                let keys = self.keys_of(layout, &d.id);
                let mut shared = Vec::new();
                for key in &keys {
                    for other in self.others_on(layout, key, &d.id) {
                        if !shared.contains(&other) {
                            shared.push(other);
                        }
                    }
                }
                Line {
                    id: d.id.clone(),
                    label: d.label.clone(),
                    pad: d.pad.clone(),
                    default: Self::keys_in(&self.defaults[i], &d.id),
                    changed: self.is_changed(layout, &d.id),
                    keys,
                    shared,
                }
            })
            .collect()
    }

    fn find_key(table: &Table, key: &str) -> Option<String> {
        table.keys().find(|k| k.eq_ignore_ascii_case(key)).cloned()
    }

    /// The other actions (labels; a string that is no listed action, as written) on `key`.
    fn others_on(&self, layout: Layout, key: &str, id: &str) -> Vec<String> {
        let table = &self.current[layout.index()];
        let Some(name) = Self::find_key(table, key) else { return Vec::new() };
        table[&name].iter().filter(|s| !s.trim().is_empty() && !holds(s, id)).map(|s| self.label_of(layout, s)).collect()
    }

    /// The labels of the other actions of `layout` that already use `key` (what binding `id` there would take it from).
    pub fn conflicts(&self, layout: Layout, id: &str, key: &str) -> Vec<String> {
        self.others_on(layout, key, id)
    }

    /// Takes every string that holds `id` off every key; the first one found (it may carry a style).
    fn remove_action(&mut self, layout: Layout, id: &str) -> Option<String> {
        let i = layout.index();
        let mut first = None;
        let keys: Vec<String> = self.current[i].keys().cloned().collect();
        for key in keys {
            let Some(list) = self.current[i].get_mut(&key) else { continue };
            let before = list.len();
            for s in list.iter() {
                if holds(s, id) && first.is_none() {
                    first = Some(s.clone());
                }
            }
            list.retain(|s| !holds(s, id));
            if list.len() != before && list.is_empty() && !self.defaults[i].contains_key(&key) {
                self.current[i].remove(&key);
            }
        }
        first
    }

    /// Binds action `id` of `layout` to `key`: `add` keeps the keys it has, otherwise
    /// they are let go. The other actions on `key` lose it. Answers what happened.
    pub fn assign(&mut self, layout: Layout, id: &str, key: &str, add: bool) -> Result<String, String> {
        let i = layout.index();
        let def = self.defs[i].iter().find(|d| d.id.eq_ignore_ascii_case(id)).cloned().ok_or_else(|| format!("There is no action {id}"))?;
        let key = Self::find_key(&self.current[i], key).or_else(|| Self::find_key(&self.defaults[i], key)).unwrap_or_else(|| key.to_owned());
        let had = self.keys_of(layout, id);
        if add && had.iter().any(|k| k.eq_ignore_ascii_case(&key)) {
            return Ok(format!("{} is already on {key}", def.label));
        }
        let lost = self.others_on(layout, &key, id);
        let carried = if add {
            Self::pairs(&self.current[i], id).first().map(|(_, s)| s.clone())
        } else {
            self.remove_action(layout, id)
        };
        let carried = carried.unwrap_or_else(|| def.id.clone());
        let mut list = if add { self.current[i].get(&key).cloned().unwrap_or_default() } else { Vec::new() };
        // Whatever else was on the key goes: one key, one action.
        list.retain(|s| holds(s, id));
        list.push(carried);
        self.current[i].insert(key.clone(), list);
        Ok(if lost.is_empty() {
            format!("{} is now {key}", def.label)
        } else {
            format!("{key} was {}, which now has no key there", lost.join(", "))
        })
    }

    /// Action `id` back to its default keys.
    pub fn reset(&mut self, layout: Layout, id: &str) {
        let i = layout.index();
        self.remove_action(layout, id);
        for (key, text) in Self::pairs(&self.defaults[i], id) {
            let list = self.current[i].entry(key).or_default();
            if !list.contains(&text) {
                list.push(text);
            }
        }
    }

    /// The whole layout back to its defaults; how many lines were changed.
    pub fn reset_layout(&mut self, layout: Layout) -> usize {
        let n = self.defs(layout).iter().filter(|d| self.is_changed(layout, &d.id)).count();
        let i = layout.index();
        self.current[i] = self.defaults[i].clone();
        n
    }

    /// Whether anything differs from what the file had when the run started or was last saved.
    pub fn unsaved(&self) -> bool {
        self.current != self.saved
    }

    /// The keys of `layout` that differ from the defaults, as the file holds them (an empty list: unbound).
    pub fn diff(&self, layout: Layout) -> Table {
        let i = layout.index();
        let mut out = Table::new();
        let names: std::collections::BTreeSet<&String> = self.defaults[i].keys().chain(self.current[i].keys()).collect();
        for key in names {
            let now = self.current[i].get(key).cloned().unwrap_or_default();
            match self.defaults[i].get(key) {
                Some(default) if *default == now => {}
                None if now.is_empty() => {}
                _ => {
                    out.insert(key.clone(), now);
                }
            }
        }
        out
    }

    pub fn source(&self) -> Option<&Path> {
        self.source.as_deref()
    }

    /// Writes the bindings into the controls file: a `.bak` copy first, then the file again with only its
    /// `[controls.on_foot]`, `[controls.vehicle]` and `[controls.aircraft]` bodies replaced (comments and every
    /// other section stay). The result is read back before it is written; one that does not read back as
    /// the bindings is not written.
    pub fn save(&mut self) -> Result<String, String> {
        let path = self.source.clone().ok_or("No controls file was used for this run (start with victory.toml or VICTORY_CONFIG)")?;
        let old = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("Could not read {}: {e}", path.display())),
        };
        let diffs = Layout::ALL.map(|l| self.diff(l));
        let text = rewrite(old.as_deref().unwrap_or(""), &diffs);
        check(&text, &diffs)?;
        if let Some(old) = &old {
            let mut bak = path.clone().into_os_string();
            bak.push(".bak");
            std::fs::write(&bak, old).map_err(|e| format!("Could not write the .bak copy: {e}"))?;
        }
        std::fs::write(&path, &text).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
        self.saved = self.current.clone();
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        Ok(if old.is_some() { format!("Saved to {name} (copy in {name}.bak)") } else { format!("Saved to {name}") })
    }
}

// ---------------------------------------------------------------------------
// The controls file's text.

/// The line's `[`/`]` depth change, leaving out quoted text and a trailing comment.
fn depth_change(line: &str) -> i32 {
    let (mut depth, mut quote, mut escape) = (0, None::<char>, false);
    for c in line.chars() {
        match quote {
            Some(q) => {
                if escape {
                    escape = false;
                } else if c == '\\' && q == '"' {
                    escape = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '#' => break,
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            },
        }
    }
    depth
}

fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// One binding line of the file.
fn assignment(key: &str, list: &[String]) -> String {
    let list: Vec<&String> = list.iter().filter(|s| !s.trim().is_empty()).collect();
    match list.as_slice() {
        [] => format!("{key} = \"\""),
        [one] => format!("{key} = {}", quoted(one)),
        many => format!("{key} = [{}]", many.iter().map(|s| quoted(s)).collect::<Vec<_>>().join(", ")),
    }
}

/// The table's header (`[controls.on_foot]`)?
fn is_header(line: &str, name: &str) -> bool {
    let bare: String = line.split('#').next().unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect();
    bare == format!("[controls.{name}]")
}

fn starts_table(line: &str) -> bool {
    line.trim_start().starts_with('[')
}

fn rewrite_section(lines: &mut Vec<String>, name: &str, diff: &Table) {
    let new: Vec<String> = diff.iter().map(|(k, v)| assignment(k, v)).collect();
    let Some(start) = lines.iter().position(|l| is_header(l, name)) else {
        if new.is_empty() {
            return;
        }
        if lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("[controls.{name}]"));
        lines.extend(new);
        return;
    };
    // The body: up to the next table, minus its assignments (comments and blank lines stay).
    let mut end = start + 1;
    let mut kept: Vec<String> = Vec::new();
    let mut depth = 0;
    while end < lines.len() {
        let line = &lines[end];
        if depth == 0 && starts_table(line) {
            break;
        }
        let code = line.split('#').next().unwrap_or("");
        if depth > 0 || code.contains('=') {
            depth = (depth + depth_change(line)).max(0);
        } else {
            kept.push(line.clone());
        }
        end += 1;
    }
    let trailing = kept.iter().rev().take_while(|l| l.trim().is_empty()).count();
    let at = kept.len() - trailing;
    kept.splice(at..at, new);
    lines.splice(start + 1..end, kept);
}

/// `text` with the three tables' assignments replaced by `diffs`.
pub fn rewrite(text: &str, diffs: &[Table; 3]) -> String {
    let crlf = text.contains("\r\n");
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    for (layout, diff) in Layout::ALL.into_iter().zip(diffs) {
        rewrite_section(&mut lines, layout.toml_name(), diff);
    }
    let eol = if crlf { "\r\n" } else { "\n" };
    let mut out = lines.join(eol);
    if !out.is_empty() {
        out.push_str(eol);
    }
    out
}

/// Reads `text` back: it must be a toml file whose three binding tables are exactly `diffs`.
pub fn check(text: &str, diffs: &[Table; 3]) -> Result<(), String> {
    let parsed: toml::Table = toml::from_str(text).map_err(|e| format!("The controls file would not read back ({e}); nothing was written"))?;
    for (layout, diff) in Layout::ALL.into_iter().zip(diffs) {
        let found = parsed.get("controls").and_then(|c| c.get(layout.toml_name())).and_then(|t| t.as_table());
        let mut got = Table::new();
        for (key, value) in found.into_iter().flatten() {
            let list: Vec<String> = match value {
                toml::Value::String(s) => vec![s.clone()],
                toml::Value::Array(items) => items.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
                _ => return Err(format!("[controls.{}] has a value the menu cannot rewrite; nothing was written", layout.toml_name())),
            };
            got.insert(key.clone(), list.into_iter().filter(|s| !s.trim().is_empty()).collect());
        }
        let want: Table = diff.iter().map(|(k, v)| (k.clone(), v.iter().filter(|s| !s.trim().is_empty()).cloned().collect())).collect();
        let lower = |t: &Table| -> BTreeMap<String, Vec<String>> { t.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect() };
        if lower(&got) != lower(&want) {
            return Err(format!("[controls.{}] is written in a form the menu cannot rewrite; nothing was written", layout.toml_name()));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The process-wide registry.

static BINDS: Mutex<Option<Binds>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static NAMER: Mutex<Option<fn(u8) -> Option<String>>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut Binds) -> T) -> Option<T> {
    BINDS.lock().unwrap_or_else(|e| e.into_inner()).as_mut().map(f)
}

/// `victory-hle` fills the registry (before the title starts).
pub fn install(setup: Setup) {
    *BINDS.lock().unwrap_or_else(|e| e.into_inner()) = Some(Binds::new(setup));
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// Empties the registry (tests).
pub fn clear() {
    *BINDS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// Whether the registry is filled.
pub fn ready() -> bool {
    with(|_| ()).is_some()
}

/// Whether the keyboard and mouse controls are on in this run.
pub fn active() -> bool {
    with(|b| b.active).unwrap_or(false)
}

/// Counts every change of the bindings: the layouts are built again when it moves.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// The bindings in force: the on-foot table, and the vehicle table (the aircraft one when that layout is in force).
pub fn live_tables() -> Option<(Table, Table)> {
    with(|b| (b.current[0].clone(), b.current[if b.aircraft_active { 2 } else { 1 }].clone()))
}

pub fn lines(layout: Layout) -> Vec<Line> {
    with(|b| b.lines(layout)).unwrap_or_default()
}

pub fn assign(layout: Layout, id: &str, key: &str, add: bool) -> Result<String, String> {
    let out = with(|b| b.assign(layout, id, key, add)).unwrap_or_else(|| Err("The controls are not set up".to_owned()));
    GENERATION.fetch_add(1, Ordering::AcqRel);
    out
}

pub fn reset(layout: Layout, id: &str) {
    with(|b| b.reset(layout, id));
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

pub fn reset_layout(layout: Layout) -> usize {
    let n = with(|b| b.reset_layout(layout)).unwrap_or(0);
    GENERATION.fetch_add(1, Ordering::AcqRel);
    n
}

pub fn conflicts(layout: Layout, id: &str, key: &str) -> Vec<String> {
    with(|b| b.conflicts(layout, id, key)).unwrap_or_default()
}

pub fn unsaved() -> bool {
    with(|b| b.unsaved()).unwrap_or(false)
}

/// The controls file's name, for the page.
pub fn source_name() -> Option<String> {
    with(|b| b.source().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned())).flatten()
}

pub fn save() -> Result<String, String> {
    with(Binds::save).unwrap_or_else(|| Err("The controls are not set up".to_owned()))
}

/// `victory-hle` says how a pressed key (a virtual key) is named in the controls file, and
/// answers `None` for one that has no name there.
pub fn set_namer(namer: fn(u8) -> Option<String>) {
    *NAMER.lock().unwrap_or_else(|e| e.into_inner()) = Some(namer);
}

/// The controls file's name of the virtual key `vk`.
pub fn name_of(vk: u8) -> Option<String> {
    let namer = (*NAMER.lock().unwrap_or_else(|e| e.into_inner()))?;
    namer(vk)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[(&str, &[&str])]) -> Table {
        rows.iter().map(|(k, v)| ((*k).to_owned(), v.iter().map(|s| (*s).to_owned()).collect())).collect()
    }

    fn setup() -> Setup {
        let foot = table(&[("LShift", &["Sprint"]), ("Space", &["Jump"]), ("LCtrl", &["Crouch"]), ("C", &["Crouch"]), ("Tab", &["tap:Holster", "hold400:Loadout"]), ("W", &["MoveForward"])]);
        let vehicle = table(&[("Space", &["Handbrake"]), ("W", &["Accelerate"]), ("LShift", &[])]);
        Setup {
            defs: [
                vec![
                    Def::new("Sprint", "Sprint", "LS click"),
                    Def::new("Jump", "Jump", "A"),
                    Def::new("Crouch", "Crouch", "B"),
                    Def::new("Holster", "Holster", "LB"),
                    Def::new("MoveForward", "Move forward", "left stick up"),
                ],
                vec![Def::new("Handbrake", "Handbrake", "A"), Def::new("Accelerate", "Accelerate", "RT")],
                vec![],
            ],
            defaults: [foot.clone(), vehicle.clone(), Table::new()],
            current: [foot, vehicle, Table::new()],
            source: None,
            aircraft_active: false,
            active: true,
        }
    }

    #[test]
    fn lines_list_keys_defaults_and_whether_they_changed() {
        let b = Binds::new(setup());
        let lines = b.lines(Layout::OnFoot);
        let crouch = lines.iter().find(|l| l.id == "Crouch").unwrap();
        assert_eq!(crouch.keys, ["C", "LCtrl"]);
        assert_eq!(crouch.default, crouch.keys);
        assert!(!crouch.changed && crouch.shared.is_empty());
        // A styled binding counts as the action.
        assert_eq!(lines.iter().find(|l| l.id == "Holster").unwrap().keys, ["Tab"]);
        assert!(!b.unsaved());
    }

    #[test]
    fn assigning_replaces_the_keys_and_takes_the_key_from_the_other_action() {
        let mut b = Binds::new(setup());
        let note = b.assign(Layout::OnFoot, "Sprint", "Space", false).unwrap();
        assert_eq!(note, "Space was Jump, which now has no key there");
        assert_eq!(b.keys_of(Layout::OnFoot, "Sprint"), ["Space"]);
        assert!(b.keys_of(Layout::OnFoot, "Jump").is_empty());
        // LShift is let go, kept in the table as unbound so that the file can say so.
        assert_eq!(b.current[0]["LShift"], Vec::<String>::new());
        assert!(b.is_changed(Layout::OnFoot, "Sprint") && b.is_changed(Layout::OnFoot, "Jump"));
        assert!(b.unsaved());
        // Another layout is untouched.
        assert_eq!(b.keys_of(Layout::Vehicle, "Handbrake"), ["Space"]);
    }

    #[test]
    fn a_style_goes_with_the_action_and_a_new_key_is_an_unknown_key_in_the_table() {
        let mut b = Binds::new(setup());
        b.assign(Layout::OnFoot, "Holster", "Q", false).unwrap();
        assert_eq!(b.current[0]["Q"], ["tap:Holster"]);
        assert!(b.current[0]["Tab"].iter().all(|s| !holds(s, "Holster")));
        assert_eq!(b.current[0]["Tab"], ["hold400:Loadout"]);
        // Letting a key that was not a default go leaves no empty key behind.
        b.assign(Layout::OnFoot, "Holster", "E", false).unwrap();
        assert!(!b.current[0].contains_key("Q"));
    }

    #[test]
    fn add_keeps_the_old_keys_and_the_same_key_twice_is_said_not_done() {
        let mut b = Binds::new(setup());
        b.assign(Layout::OnFoot, "Jump", "J", true).unwrap();
        assert_eq!(b.keys_of(Layout::OnFoot, "Jump"), ["J", "Space"]);
        assert_eq!(b.assign(Layout::OnFoot, "Jump", "j", true).unwrap(), "Jump is already on J");
        assert_eq!(b.keys_of(Layout::OnFoot, "Jump"), ["J", "Space"]);
        assert!(b.assign(Layout::OnFoot, "Nothing", "X", false).is_err());
    }

    #[test]
    fn conflicts_name_the_actions_already_on_the_key() {
        let mut b = Binds::new(setup());
        assert_eq!(b.conflicts(Layout::OnFoot, "Sprint", "Space"), ["Jump"]);
        assert_eq!(b.conflicts(Layout::OnFoot, "Sprint", "space"), ["Jump"]);
        assert!(b.conflicts(Layout::OnFoot, "Sprint", "Z").is_empty());
        assert!(b.conflicts(Layout::OnFoot, "Jump", "Space").is_empty(), "its own key is no conflict");
        // A shared key shows on both lines (a file edited by hand can have one).
        b.current[0].get_mut("Space").unwrap().push("Sprint".to_owned());
        let lines = b.lines(Layout::OnFoot);
        let sprint = lines.iter().find(|l| l.id == "Sprint").unwrap();
        assert_eq!(sprint.shared, ["Jump"]);
        assert_eq!(lines.iter().find(|l| l.id == "Jump").unwrap().shared, ["Sprint"]);
    }

    #[test]
    fn reset_puts_an_action_or_a_layout_back() {
        let mut b = Binds::new(setup());
        b.assign(Layout::OnFoot, "Crouch", "X", false).unwrap();
        b.assign(Layout::OnFoot, "Sprint", "Space", false).unwrap();
        b.reset(Layout::OnFoot, "Crouch");
        assert_eq!(b.keys_of(Layout::OnFoot, "Crouch"), ["C", "LCtrl"]);
        assert!(!b.current[0].contains_key("X"));
        assert!(b.is_changed(Layout::OnFoot, "Sprint"));
        assert_eq!(b.reset_layout(Layout::OnFoot), 2);
        assert_eq!(b.current[0], b.defaults[0]);
        assert!(!b.unsaved());
    }

    #[test]
    fn the_diff_has_only_what_differs_and_unbound_keys_as_empty() {
        let mut b = Binds::new(setup());
        assert!(b.diff(Layout::OnFoot).is_empty());
        b.assign(Layout::OnFoot, "Sprint", "Z", false).unwrap();
        let d = b.diff(Layout::OnFoot);
        assert_eq!(d, table(&[("LShift", &[]), ("Z", &["Sprint"])]));
    }

    #[test]
    fn the_file_text_round_trips_and_keeps_comments_and_other_sections() {
        let original = "# my file\n[paths]\ngame_dir = 'K:\\g'\n\n[controls]\nenabled = true\nmouse_sensitivity = 2.5 # fast\n\n[controls.on_foot]\n# my keys\nLShift = \"Sprint\"\nTab = [\n  \"tap:Holster\",\n  \"hold400:Loadout\",\n]\n\n[controls.vehicle]\nSpace = \"Handbrake\"\n\n[other]\nx = 1\n";
        let diffs = [table(&[("LShift", &[]), ("Z", &["Sprint"]), ("Tab", &["tap:Holster", "hold400:Loadout"])]), table(&[("Space", &["Handbrake", "Horn"])]), Table::new()];
        let out = rewrite(original, &diffs);
        check(&out, &diffs).unwrap();
        assert!(out.contains("# my file") && out.contains("mouse_sensitivity = 2.5 # fast") && out.contains("# my keys"));
        assert!(out.contains("[paths]") && out.contains("[other]\nx = 1"));
        assert!(out.contains("LShift = \"\"") && out.contains("Z = \"Sprint\""));
        assert!(out.contains("Space = [\"Handbrake\", \"Horn\"]"));
        // Written once, in its place; the old multi-line array is gone.
        assert_eq!(out.matches("Tab = ").count(), 1);
        assert_eq!(out.matches("[controls.on_foot]").count(), 1);
        assert!(!out.contains("  \"tap:Holster\","));
        // Writing the same thing again changes nothing.
        assert_eq!(rewrite(&out, &diffs), out);
        // The tables are read back into the config's own type.
        #[derive(serde::Deserialize)]
        struct File {
            controls: crate::config::ControlsConfig,
        }
        let file: File = toml::from_str(&out).unwrap();
        assert_eq!(file.controls.on_foot.len(), 3);
        assert!(file.controls.aircraft.is_empty());
    }

    #[test]
    fn a_missing_table_is_added_at_the_end_and_an_empty_diff_empties_the_body() {
        let diffs = [table(&[("Z", &["Sprint"])]), Table::new(), table(&[("G", &[])])];
        let out = rewrite("[controls]\nenabled = true", &diffs);
        assert_eq!(out, "[controls]\nenabled = true\n\n[controls.on_foot]\nZ = \"Sprint\"\n\n[controls.aircraft]\nG = \"\"\n");
        check(&out, &diffs).unwrap();
        let none = [Table::new(), Table::new(), Table::new()];
        let back = rewrite(&out, &none);
        assert_eq!(back, "[controls]\nenabled = true\n\n[controls.on_foot]\n\n[controls.aircraft]\n");
        check(&back, &none).unwrap();
        // An empty file with nothing to write stays empty; CRLF files keep their line ends.
        assert_eq!(rewrite("", &none), "");
        assert_eq!(rewrite("a = 1\r\n", &diffs).matches("\r\n").count(), rewrite("a = 1\r\n", &diffs).matches('\n').count());
    }

    #[test]
    fn a_form_the_menu_cannot_rewrite_is_refused() {
        let diffs = [table(&[("Z", &["Sprint"])]), Table::new(), Table::new()];
        // The bindings written as dotted keys of [controls] would be a second table of the same name.
        let out = rewrite("[controls]\non_foot.W = \"Walk\"\n", &diffs);
        assert!(check(&out, &diffs).is_err());
        assert!(check("not = [toml", &diffs).unwrap_err().contains("nothing was written"));
    }

    #[test]
    fn save_writes_a_bak_first_and_only_the_binding_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("victory-pc.toml");
        let original = "[paths]\ngame_dir = 'K:\\g'\n\n[controls]\nenabled = true\n";
        std::fs::write(&path, original).unwrap();
        let mut s = setup();
        s.source = Some(path.clone());
        let mut b = Binds::new(s);
        b.assign(Layout::OnFoot, "Jump", "J", false).unwrap();
        let note = b.save().unwrap();
        assert_eq!(note, "Saved to victory-pc.toml (copy in victory-pc.toml.bak)");
        assert_eq!(std::fs::read_to_string(dir.path().join("victory-pc.toml.bak")).unwrap(), original);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(original.trim_end()) && text.contains("[controls.on_foot]") && text.contains("J = \"Jump\"") && text.contains("Space = \"\""), "{text}");
        assert!(!b.unsaved());
        // The file reads back through the config's own loader type.
        let again = Binds::new(Setup { source: Some(path), ..setup() });
        assert_eq!(again.source().unwrap().file_name().unwrap(), "victory-pc.toml");
        // No file named: nothing to write to.
        assert!(Binds::new(setup()).save().unwrap_err().contains("No controls file"));
    }

    #[test]
    fn the_registry_changes_the_generation_and_gives_the_tables() {
        static TURN: Mutex<()> = Mutex::new(());
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        assert!(!ready() && lines(Layout::OnFoot).is_empty() && live_tables().is_none());
        assert!(assign(Layout::OnFoot, "Jump", "J", false).is_err());
        install(setup());
        let g = generation();
        assert!(ready() && active() && !unsaved());
        assign(Layout::OnFoot, "Jump", "J", false).unwrap();
        assert!(generation() > g && unsaved());
        let (foot, vehicle) = live_tables().unwrap();
        assert_eq!(foot["J"], ["Jump"]);
        assert_eq!(vehicle["Space"], ["Handbrake"]);
        reset_layout(Layout::OnFoot);
        assert!(!unsaved());
        set_namer(|vk| (vk == 0x41).then(|| "A".to_owned()));
        assert_eq!((name_of(0x41).as_deref(), name_of(0x42)), (Some("A"), None));
        clear();
    }
}
