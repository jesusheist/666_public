//! `find_bad_pixels`: what one scanned frame found, and how it is reported.
//!
//! The game's present path arms a capture of one frame; for every draw and
//! dispatch recorded in it, the GPU crate scans the render targets (and, if
//! asked, the texture UAVs) the command wrote, before and after it, for NaN
//! and infinite values. It fills a [`ScanReport`] and hands it back
//! ([`crate::ScanRequest::deliver`]); this module turns the report into the
//! JSON that is written to disk and the short summary the tool returns. It
//! holds no GPU code, so all of it is tested without a card.

use crate::json::Json;
use std::path::{Path, PathBuf};

/// Most draws a capture may scan, as the tool's argument bounds have it.
pub const MAX_DRAWS_LIMIT: u32 = 100_000;
pub const DEFAULT_MAX_DRAWS: u32 = 20_000;

/// What a one-frame capture is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureMode {
    /// `find_bad_pixels`: scan float targets for NaN and Inf.
    #[default]
    BadPixels,
    /// `frame_commands`: list every draw and dispatch, scan nothing.
    Commands,
    /// `trace_pixel`: read one texel of every target around every command.
    Trace,
    /// `dump_target`: save one target after one command.
    Dump,
}

/// Which target to save after which command (`dump_target`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DumpSpec {
    /// The command's index in the capture.
    pub after_command: Option<u32>,
    /// Or: the command whose pipeline has this shader hash (`occurrence`
    /// counts the matches from 0).
    pub after_shader: Option<String>,
    pub occurrence: u32,
    /// The title resource to save (default: the command's first target).
    pub resource: Option<u64>,
    /// Or the role of one of the command's targets (`rt0`, `uav ...`).
    pub role: Option<String>,
    pub subresource: u32,
}

/// What the caller asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanParams {
    pub mode: CaptureMode,
    /// `frame_commands`, `trace_pixel`: keep only commands whose shaders
    /// have a hash containing one of these.
    pub shader_filter: Vec<String>,
    pub dump: Option<DumpSpec>,
    /// Draws and dispatches scanned at most (the rest are counted).
    pub max_draws: u32,
    /// A screen pixel whose value is followed through the frame, on the
    /// targets that are the size of the back buffer.
    pub pixel: Option<(u32, u32)>,
    /// Also scan texture UAVs that dispatches and pixel shaders write.
    pub include_uav: bool,
}

impl Default for ScanParams {
    fn default() -> Self {
        ScanParams {
            mode: CaptureMode::BadPixels,
            shader_filter: Vec::new(),
            dump: None,
            max_draws: DEFAULT_MAX_DRAWS,
            pixel: None,
            include_uav: false,
        }
    }
}

/// What one scan of one subresource found. The numbers are as the shader
/// stored them; `first` is the bad pixel nearest the top left in row order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counts {
    /// The scan ran on the card before the frame was read back.
    pub ran: bool,
    /// Pixels with a NaN in any channel.
    pub nan: u32,
    /// Pixels with an infinity in any channel.
    pub inf: u32,
    /// The bits of the largest finite absolute value.
    pub max_bits: u32,
    /// The order the scans ran in on the card (the draws' order).
    pub order: u32,
    pub first: Option<(u32, u32)>,
    /// The raw float bits of the first bad pixel's four channels.
    pub first_value: Option<[u32; 4]>,
    /// The raw float bits of the followed pixel, if this target shows it.
    pub pixel: Option<[u32; 4]>,
}

impl Counts {
    pub fn bad(&self) -> bool {
        self.ran && self.nan + self.inf > 0
    }

    pub fn max_finite(&self) -> f32 {
        f32::from_bits(self.max_bits)
    }
}

/// One target a draw or dispatch wrote.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TargetScan {
    /// `rt0`, `rt1`, ... or `uav`.
    pub role: String,
    /// The title resource's address, which names the same texture in every
    /// draw of the frame.
    pub resource: u64,
    /// A DXGI_FORMAT value (of the view the draw wrote through).
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub subresource: u32,
    pub pre: Option<Counts>,
    pub post: Option<Counts>,
    /// `trace_pixel`: the texel read (in this target's own size) and its
    /// bytes (as many as the format's texel has) before and after the command.
    pub texel: Option<(u32, u32)>,
    pub texel_pre: Option<[u8; 16]>,
    pub texel_post: Option<[u8; 16]>,
    /// Why it was not scanned (a format with no NaN, multisampled, too large).
    pub skipped: Option<String>,
}

impl TargetScan {
    fn key(&self) -> (u64, u32) {
        (self.resource, self.subresource)
    }

    /// Bad before the command wrote it.
    pub fn bad_before(&self) -> bool {
        self.pre.is_some_and(|c| c.bad())
    }

    pub fn bad_after(&self) -> bool {
        self.post.is_some_and(|c| c.bad())
    }

    /// The command made a clean target bad: it is the first writer.
    pub fn introduced(&self) -> bool {
        self.bad_after() && self.pre.is_some_and(|c| c.ran && !c.bad())
    }

    /// The followed pixel changed in this command.
    pub fn pixel_changed(&self) -> bool {
        match (self.pre.and_then(|c| c.pixel), self.post.and_then(|c| c.pixel)) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        }
    }
}

/// One draw or dispatch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawScan {
    /// The place among those recorded in the frame (recording order, which
    /// is not always the order they ran in: see `Counts::order`).
    pub index: u32,
    /// `draw`, `indexed`, `dispatch`, `mesh` or `indirect`.
    pub kind: String,
    /// The host pipeline's name (its shaders).
    pub pipeline: String,
    /// (stage, hash) as the log prints them (`hash=...`).
    pub shaders: Vec<(String, String)>,
    pub targets: Vec<TargetScan>,
    /// Dispatch: groups x, y, z. Draw: vertices, instances, first vertex,
    /// first instance. Indexed: indices, instances, first index, base vertex
    /// (as bits). Mesh: groups.
    pub counts: [u32; 4],
    /// The host command list it was recorded on (its address).
    pub list: u64,
}

impl DrawScan {
    /// Where the draw ran among the others: its first scan's order, or
    /// `u32::MAX` if none ran.
    pub fn order(&self) -> u32 {
        self.targets
            .iter()
            .filter_map(|t| t.pre.filter(|c| c.ran).or(t.post.filter(|c| c.ran)))
            .map(|c| c.order)
            .min()
            .unwrap_or(u32::MAX)
    }

    fn interesting(&self, pixel: bool) -> bool {
        self.targets.iter().any(|t| t.bad_before() || t.bad_after() || (pixel && t.pixel_changed()))
    }
}

/// One target as it was after a command (`dump_target`), as read back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DumpImage {
    pub after_command: u32,
    pub kind: String,
    pub shaders: Vec<(String, String)>,
    pub role: String,
    pub resource: u64,
    pub format: u32,
    pub subresource: u32,
    pub width: u32,
    pub height: u32,
    /// Bytes from one row to the next in `data`.
    pub pitch: u32,
    pub data: Vec<u8>,
}

/// What the capture of one frame found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanReport {
    /// `dump_target`: the saved target, if the command was reached.
    pub dump: Option<DumpImage>,
    /// Commands the capture saw, listed or not (the next index).
    pub commands_seen: u32,
    pub frame: u64,
    pub back_buffer: (u32, u32),
    pub params: ScanParams,
    pub draws: Vec<DrawScan>,
    pub slots_used: u32,
    pub slots_total: u32,
    /// Commands not scanned because `max_draws` was reached.
    pub over_cap: u32,
    /// Commands not scanned because the result slots were used up.
    pub over_slots: u32,
    pub notes: Vec<String>,
}

/// A target that went from clean to bad in one command.
#[derive(Clone, Debug, PartialEq)]
pub struct Introducer {
    /// Positions in `ScanReport::draws` and in that draw's `targets`.
    pub draw: usize,
    pub target: usize,
    /// The first command to make this texture bad in the capture.
    pub first_for_target: bool,
}

/// The commands (by the order they ran in) that made a clean target bad, and
/// the targets the capture first saw already bad.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    pub introducers: Vec<Introducer>,
    /// Targets bad before the first command of the capture that wrote them:
    /// (draw position, target position). The bad pixels come from an earlier
    /// frame, or from a writer the capture could not scan.
    pub inherited: Vec<(usize, usize)>,
    pub scans_not_run: u32,
}

/// The draws in the order they ran on the card.
pub fn run_order(report: &ScanReport) -> Vec<usize> {
    let mut order: Vec<usize> = (0..report.draws.len()).collect();
    order.sort_by_key(|&i| (report.draws[i].order(), report.draws[i].index));
    order
}

pub fn analyse(report: &ScanReport) -> Analysis {
    let mut a = Analysis::default();
    let mut seen: std::collections::HashMap<(u64, u32), bool> = std::collections::HashMap::new();
    for i in run_order(report) {
        let draw = &report.draws[i];
        for (j, t) in draw.targets.iter().enumerate() {
            for c in [t.pre, t.post].into_iter().flatten() {
                if !c.ran {
                    a.scans_not_run += 1;
                }
            }
            let known = seen.get(&t.key()).copied();
            if t.introduced() {
                a.introducers.push(Introducer { draw: i, target: j, first_for_target: known != Some(true) });
            } else if known.is_none() && t.bad_before() {
                a.inherited.push((i, j));
            }
            if t.bad_before() || t.bad_after() {
                seen.insert(t.key(), true);
            } else if known.is_none() && (t.pre.is_some() || t.post.is_some()) {
                seen.insert(t.key(), false);
            }
        }
    }
    a
}

/// The name of a DXGI_FORMAT value the scans meet; the number otherwise.
pub fn format_name(format: u32) -> String {
    let name = match format {
        1 => "R32G32B32A32_TYPELESS",
        2 => "R32G32B32A32_FLOAT",
        6 => "R32G32B32_FLOAT",
        9 => "R16G16B16A16_TYPELESS",
        10 => "R16G16B16A16_FLOAT",
        15 => "R32G32_TYPELESS",
        16 => "R32G32_FLOAT",
        26 => "R11G11B10_FLOAT",
        33 => "R16G16_TYPELESS",
        34 => "R16G16_FLOAT",
        39 => "R32_TYPELESS",
        41 => "R32_FLOAT",
        53 => "R16_TYPELESS",
        54 => "R16_FLOAT",
        _ => return format!("format {format}"),
    };
    format!("{name} ({format})")
}

/// A float's bits as JSON: a number if finite, else text that says which
/// (NaN keeps its bits).
fn float_json(bits: u32) -> Json {
    let v = f32::from_bits(bits);
    if v.is_finite() {
        Json::num(f64::from(v))
    } else if v.is_nan() {
        Json::str(format!("NaN(0x{bits:08x})"))
    } else if v > 0.0 {
        Json::str("+Inf")
    } else {
        Json::str("-Inf")
    }
}

fn value_json(v: [u32; 4]) -> Json {
    Json::Arr(v.iter().map(|&b| float_json(b)).collect())
}

fn counts_json(c: &Option<Counts>) -> Json {
    let Some(c) = c else { return Json::Null };
    if !c.ran {
        return Json::obj([("ran", Json::Bool(false))]);
    }
    let point = |p: Option<(u32, u32)>| p.map_or(Json::Null, |(x, y)| Json::obj([("x", Json::Int(i64::from(x))), ("y", Json::Int(i64::from(y)))]));
    Json::obj([
        ("ran", Json::Bool(true)),
        ("nan", Json::uint(u64::from(c.nan))),
        ("inf", Json::uint(u64::from(c.inf))),
        ("max_finite_abs", float_json(c.max_bits)),
        ("first_bad_pixel", point(c.first)),
        ("first_bad_value", c.first_value.map_or(Json::Null, value_json)),
        ("pixel", c.pixel.map_or(Json::Null, value_json)),
    ])
}

fn target_json(t: &TargetScan) -> Json {
    let mut fields = vec![
        ("role", Json::str(t.role.as_str())),
        ("resource", Json::str(format!("{:#x}", t.resource))),
        ("format", Json::str(format_name(t.format))),
        ("width", Json::Int(i64::from(t.width))),
        ("height", Json::Int(i64::from(t.height))),
        ("subresource", Json::Int(i64::from(t.subresource))),
    ];
    if let Some(why) = &t.skipped {
        fields.push(("skipped", Json::str(why.as_str())));
    } else {
        fields.push(("bad_before", Json::Bool(t.bad_before())));
        fields.push(("introduced_here", Json::Bool(t.introduced())));
        fields.push(("pre", counts_json(&t.pre)));
        fields.push(("post", counts_json(&t.post)));
    }
    Json::Obj(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn draw_json(d: &DrawScan) -> Json {
    let order = d.order();
    Json::obj([
        ("index", Json::uint(u64::from(d.index))),
        ("run_order", if order == u32::MAX { Json::Null } else { Json::uint(u64::from(order)) }),
        ("kind", Json::str(d.kind.as_str())),
        ("pipeline", Json::str(d.pipeline.as_str())),
        (
            "shaders",
            Json::Arr(
                d.shaders
                    .iter()
                    .map(|(stage, hash)| Json::obj([("stage", Json::str(stage.as_str())), ("hash", Json::str(hash.as_str()))]))
                    .collect(),
            ),
        ),
        ("targets", Json::Arr(d.targets.iter().map(target_json).collect())),
    ])
}

fn introducer_json(report: &ScanReport, i: &Introducer) -> Json {
    let d = &report.draws[i.draw];
    let t = &d.targets[i.target];
    Json::obj([
        ("index", Json::uint(u64::from(d.index))),
        ("run_order", Json::uint(u64::from(d.order()))),
        ("first_for_target", Json::Bool(i.first_for_target)),
        ("kind", Json::str(d.kind.as_str())),
        ("pipeline", Json::str(d.pipeline.as_str())),
        (
            "shaders",
            Json::Arr(d.shaders.iter().map(|(s, h)| Json::str(format!("{s} {h}"))).collect()),
        ),
        ("target", target_json(t)),
    ])
}

/// The whole report as JSON (the file the tool writes). Lists the commands
/// that matter (a bad target before or after, or a followed pixel that
/// changed) in the order they ran on the card.
pub fn to_json(report: &ScanReport) -> Json {
    let a = analyse(report);
    let order = run_order(report);
    let pixel = report.params.pixel;
    let listed: Vec<Json> = order.iter().map(|&i| &report.draws[i]).filter(|d| d.interesting(pixel.is_some())).map(draw_json).collect();
    let history: Vec<Json> = if pixel.is_some() {
        order
            .iter()
            .flat_map(|&i| {
                let d = &report.draws[i];
                d.targets.iter().filter(|t| t.pixel_changed()).map(move |t| (d, t))
            })
            .map(|(d, t)| {
                Json::obj([
                    ("index", Json::uint(u64::from(d.index))),
                    ("run_order", Json::uint(u64::from(d.order()))),
                    ("kind", Json::str(d.kind.as_str())),
                    ("shaders", Json::Arr(d.shaders.iter().map(|(s, h)| Json::str(format!("{s} {h}"))).collect())),
                    ("target", Json::str(format!("{} {:#x} {}", t.role, t.resource, format_name(t.format)))),
                    ("before", t.pre.and_then(|c| c.pixel).map_or(Json::Null, value_json)),
                    ("after", t.post.and_then(|c| c.pixel).map_or(Json::Null, value_json)),
                ])
            })
            .collect()
    } else {
        Vec::new()
    };
    Json::obj([
        ("frame", Json::uint(report.frame)),
        (
            "back_buffer",
            Json::obj([("width", Json::Int(i64::from(report.back_buffer.0))), ("height", Json::Int(i64::from(report.back_buffer.1)))]),
        ),
        (
            "params",
            Json::obj([
                ("max_draws", Json::uint(u64::from(report.params.max_draws))),
                (
                    "pixel",
                    pixel.map_or(Json::Null, |(x, y)| Json::obj([("x", Json::Int(i64::from(x))), ("y", Json::Int(i64::from(y)))])),
                ),
                ("include_uav", Json::Bool(report.params.include_uav)),
            ]),
        ),
        ("draws_scanned", Json::uint(report.draws.len() as u64)),
        ("slots_used", Json::uint(u64::from(report.slots_used))),
        ("slots_total", Json::uint(u64::from(report.slots_total))),
        ("not_scanned_max_draws", Json::uint(u64::from(report.over_cap))),
        ("not_scanned_slots_used_up", Json::uint(u64::from(report.over_slots))),
        ("scans_not_run_before_readback", Json::uint(u64::from(a.scans_not_run))),
        (
            "first_bad_writers",
            Json::Arr(a.introducers.iter().map(|i| introducer_json(report, i)).collect()),
        ),
        (
            "bad_before_the_capture_saw_a_writer",
            Json::Arr(
                a.inherited
                    .iter()
                    .map(|&(d, t)| {
                        let draw = &report.draws[d];
                        Json::obj([("index", Json::uint(u64::from(draw.index))), ("target", target_json(&draw.targets[t]))])
                    })
                    .collect(),
            ),
        ),
        ("draws_listed", Json::uint(listed.len() as u64)),
        ("draws", Json::Arr(listed)),
        ("pixel_history", Json::Arr(history)),
        ("notes", Json::Arr(report.notes.iter().map(|n| Json::str(n.as_str())).collect())),
    ])
}

/// The few lines the tool answers with.
pub fn summary(report: &ScanReport) -> String {
    let a = analyse(report);
    let mut text = format!(
        "frame {}: {} draws and dispatches scanned ({} of {} result slots)",
        report.frame,
        report.draws.len(),
        report.slots_used,
        report.slots_total
    );
    if report.over_cap + report.over_slots > 0 {
        text.push_str(&format!("; {} not scanned (max_draws), {} not scanned (slots used up)", report.over_cap, report.over_slots));
    }
    if a.scans_not_run > 0 {
        text.push_str(&format!("; {} scans had not run when the frame was read back", a.scans_not_run));
    }
    text.push_str(". ");
    match a.introducers.iter().find(|i| i.first_for_target) {
        None if a.inherited.is_empty() => text.push_str("No NaN or Inf found in any scanned target."),
        None => text.push_str(&format!(
            "{} target(s) were already bad when the capture first saw them (no clean-to-bad writer in this frame).",
            a.inherited.len()
        )),
        Some(first) => {
            let ran_first = a.introducers.iter().min_by_key(|i| report.draws[i.draw].order()).unwrap_or(first);
            let d = &report.draws[ran_first.draw];
            let t = &d.targets[ran_first.target];
            let after = t.post.unwrap_or_default();
            text.push_str(&format!(
                "First writer of bad pixels: {} #{} (run order {}) {} -> {} {}x{} {}: {} NaN, {} Inf, first at {}. {} writer(s) in all.",
                d.kind,
                d.index,
                d.order(),
                d.shaders.iter().map(|(s, h)| format!("{s} {h}")).collect::<Vec<_>>().join(" + "),
                t.role,
                t.width,
                t.height,
                format_name(t.format),
                after.nan,
                after.inf,
                after.first.map_or("?".to_owned(), |(x, y)| format!("({x},{y})")),
                a.introducers.len()
            ));
        }
    }
    text
}

/// Writes the report as `badpixels-<timestamp>-f<frame>.json` in `dir`.
pub fn save(report: &ScanReport, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let stamp = crate::shot::timestamp();
    let mut path = dir.join(format!("badpixels-{stamp}-f{}.json", report.frame));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("badpixels-{stamp}-f{}-{n}.json", report.frame));
        n += 1;
    }
    std::fs::write(&path, to_json(report).text()).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAN: u32 = 0x7fc0_0000;
    const INF: u32 = 0x7f80_0000;

    fn counts(order: u32, nan: u32, inf: u32) -> Counts {
        Counts {
            ran: true,
            nan,
            inf,
            max_bits: 1.5f32.to_bits(),
            order,
            first: (nan + inf > 0).then_some((3, 4)),
            first_value: (nan + inf > 0).then_some([NAN, 0, 0, 0]),
            pixel: None,
        }
    }

    fn target(resource: u64, pre: Counts, post: Counts) -> TargetScan {
        TargetScan {
            role: "rt0".into(),
            resource,
            format: 10,
            width: 64,
            height: 32,
            subresource: 0,
            pre: Some(pre),
            post: Some(post),
            skipped: None,
            ..Default::default()
        }
    }

    fn draw(index: u32, pipeline: &str, targets: Vec<TargetScan>) -> DrawScan {
        DrawScan {
            index,
            kind: "draw".into(),
            pipeline: pipeline.into(),
            shaders: vec![("vertex".into(), format!("v{index}")), ("pixel".into(), format!("p{index}"))],
            targets,
            ..Default::default()
        }
    }

    fn report(draws: Vec<DrawScan>) -> ScanReport {
        ScanReport { frame: 77, back_buffer: (64, 32), draws, slots_used: 8, slots_total: 100, ..Default::default() }
    }

    #[test]
    fn a_clean_to_bad_target_is_the_first_writer_and_later_draws_inherit() {
        // Recorded 0, 1, 2; ran 1 (clean), 0 (makes it bad), 2 (still bad).
        let r = report(vec![
            draw(0, "a", vec![target(9, counts(2, 0, 0), counts(3, 5, 0))]),
            draw(1, "b", vec![target(9, counts(0, 0, 0), counts(1, 0, 0))]),
            draw(2, "c", vec![target(9, counts(4, 5, 0), counts(5, 5, 2))]),
        ]);
        let a = analyse(&r);
        assert_eq!(a.introducers, vec![Introducer { draw: 0, target: 0, first_for_target: true }]);
        assert!(a.inherited.is_empty());
        assert_eq!(run_order(&r), vec![1, 0, 2]);
        let text = summary(&r);
        assert!(text.contains("First writer of bad pixels: draw #0 (run order 2) vertex v0 + pixel p0 -> rt0 64x32 R16G16B16A16_FLOAT (10): 5 NaN, 0 Inf, first at (3,4)"), "{text}");
    }

    #[test]
    fn a_target_already_bad_at_first_sight_is_inherited_not_blamed() {
        let r = report(vec![draw(0, "a", vec![target(9, counts(0, 1, 0), counts(1, 1, 0))])]);
        let a = analyse(&r);
        assert!(a.introducers.is_empty());
        assert_eq!(a.inherited, vec![(0, 0)]);
        assert!(summary(&r).contains("already bad when the capture first saw them"));
    }

    #[test]
    fn a_second_clean_to_bad_in_another_texture_is_its_own_first_writer() {
        let r = report(vec![
            draw(0, "a", vec![target(9, counts(0, 0, 0), counts(1, 0, 3))]),
            draw(1, "b", vec![target(10, counts(2, 0, 0), counts(3, 2, 0))]),
            // Texture 9 is cleared (clean) and made bad again: not the first for it.
            draw(2, "c", vec![target(9, counts(4, 0, 3), counts(5, 0, 0))]),
            draw(3, "d", vec![target(9, counts(6, 0, 0), counts(7, 1, 0))]),
        ]);
        let a = analyse(&r);
        let found: Vec<_> = a.introducers.iter().map(|i| (i.draw, i.first_for_target)).collect();
        assert_eq!(found, vec![(0, true), (1, true), (3, false)]);
    }

    #[test]
    fn clean_frames_and_scans_that_did_not_run_say_so() {
        let mut t = target(9, counts(0, 0, 0), counts(1, 0, 0));
        let r = report(vec![draw(0, "a", vec![t.clone()])]);
        assert!(summary(&r).contains("No NaN or Inf found"));
        t.post = Some(Counts::default());
        let r = report(vec![draw(0, "a", vec![t])]);
        assert!(analyse(&r).introducers.is_empty());
        assert!(summary(&r).contains("1 scans had not run"));
    }

    #[test]
    fn json_lists_only_what_matters_and_spells_non_finite_values() {
        let mut bad = target(9, counts(0, 0, 0), counts(1, 4, 0));
        bad.post.as_mut().unwrap().pixel = Some([NAN, INF, 0.5f32.to_bits(), (-f32::INFINITY).to_bits()]);
        bad.pre.as_mut().unwrap().pixel = Some([0, 0, 0, 0]);
        let mut r = report(vec![draw(0, "clean", vec![target(8, counts(2, 0, 0), counts(3, 0, 0))]), draw(1, "dirty", vec![bad])]);
        r.params.pixel = Some((10, 12));
        r.notes.push("a note".into());
        let j = to_json(&r);
        let parsed = crate::json::parse(j.text().as_bytes()).unwrap();
        assert_eq!(parsed.get("draws_scanned"), Some(&Json::Int(2)));
        assert_eq!(parsed.get("draws_listed"), Some(&Json::Int(1)));
        let Some(Json::Arr(draws)) = parsed.get("draws") else { panic!() };
        assert_eq!(draws[0].get("pipeline").and_then(Json::as_str), Some("dirty"));
        let target = match draws[0].get("targets") {
            Some(Json::Arr(t)) => t[0].clone(),
            _ => panic!(),
        };
        assert_eq!(target.get("introduced_here"), Some(&Json::Bool(true)));
        assert_eq!(target.get("format").and_then(Json::as_str), Some("R16G16B16A16_FLOAT (10)"));
        assert_eq!(target.get("post").and_then(|p| p.get("nan")), Some(&Json::Int(4)));
        assert_eq!(target.get("post").and_then(|p| p.get("max_finite_abs")), Some(&Json::Num(1.5)));
        let Some(Json::Arr(history)) = parsed.get("pixel_history") else { panic!() };
        assert_eq!(history.len(), 1);
        let Some(Json::Arr(after)) = history[0].get("after") else { panic!() };
        assert_eq!(after[0].as_str(), Some("NaN(0x7fc00000)"));
        assert_eq!(after[1].as_str(), Some("+Inf"));
        assert_eq!(after[2], Json::Num(0.5));
        assert_eq!(after[3].as_str(), Some("-Inf"));
        let Some(Json::Arr(writers)) = parsed.get("first_bad_writers") else { panic!() };
        assert_eq!(writers.len(), 1);
    }

    #[test]
    fn skipped_targets_are_named_and_unscanned_counts_are_reported() {
        let skipped = TargetScan { role: "uav".into(), resource: 1, format: 28, skipped: Some("format R8G8B8A8_UNORM has no NaN".into()), ..Default::default() };
        let mut r = report(vec![draw(0, "a", vec![skipped])]);
        r.over_cap = 5;
        r.over_slots = 2;
        let text = summary(&r);
        assert!(text.contains("5 not scanned (max_draws), 2 not scanned (slots used up)"), "{text}");
        assert_eq!(format_name(10), "R16G16B16A16_FLOAT (10)");
        assert_eq!(format_name(999), "format 999");
    }

    #[test]
    fn the_file_is_named_by_time_and_frame() {
        let dir = tempfile::tempdir().unwrap();
        let r = report(vec![draw(0, "a", vec![target(9, counts(0, 0, 0), counts(1, 1, 0))])]);
        let path = save(&r, &dir.path().join("shots")).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("badpixels-") && name.ends_with("-f77.json"), "{name}");
        let back = crate::json::parse(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back.get("frame"), Some(&Json::Int(77)));
        let again = save(&r, &dir.path().join("shots")).unwrap();
        assert_ne!(again, path);
    }
}
