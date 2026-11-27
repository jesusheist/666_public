//! `frame_commands`, `trace_pixel` and `dump_target`: what a one-frame
//! capture of another mode holds, and how it is read. No GPU code here, so
//! all of it is tested without a card.

use crate::badpix::{DrawScan, ScanReport, TargetScan, format_name};
use crate::json::Json;
use crate::shot::{encode_png, half_to_f32, linear_to_srgb8, timestamp};
use std::path::{Path, PathBuf};

/// Largest image `dump_target` saves, in pixels.
pub const MAX_DUMP_PIXELS: u64 = 64 << 20;

/// What the bytes of a texel say.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Texel {
    /// The channels (unused ones are zero); integers as numbers.
    pub ch: [f32; 4],
    /// How many channels the format has.
    pub n: usize,
    /// An 8-bit format whose values are sRGB-encoded.
    pub srgb: bool,
    /// A float format (values can be above 1; the dump tone-maps them).
    pub float: bool,
    /// An integer (non-normalized) format.
    pub uint: bool,
}

/// Bytes of a texel of a DXGI format that `decode_texel` knows.
pub fn texel_bytes(format: u32) -> Option<usize> {
    Some(match format {
        1..=4 => 16,
        5..=8 => 12,
        9..=14 | 15..=18 => 8,
        23..=26 | 27..=32 | 33..=38 | 39..=43 | 67 | 87..=93 => 4,
        48..=58 => 2,
        60..=65 => 1,
        85 | 86 => 2,
        _ => return None,
    })
}

fn unorm(v: u32, bits: u32) -> f32 {
    v as f32 / ((1u32 << bits) - 1) as f32
}

fn snorm(v: i32, bits: u32) -> f32 {
    (v as f32 / ((1i32 << (bits - 1)) - 1) as f32).max(-1.0)
}

fn small_float(e: u32, m: u32, mbits: u32) -> f32 {
    if e == 31 {
        return if m == 0 { f32::INFINITY } else { f32::NAN };
    }
    let mf = m as f32 / (1u32 << mbits) as f32;
    if e == 0 { mf * 2f32.powi(-14) } else { (1.0 + mf) * 2f32.powi(e as i32 - 15) }
}

/// Decodes the first texel of `bytes` as `format` (a view format, or the
/// texture's own: typeless 8-bit and 10-bit ones read as unorm, typeless
/// 16 and 32-bit ones as float). `None` for a format not known.
pub fn decode_texel(format: u32, bytes: &[u8]) -> Option<Texel> {
    let need = texel_bytes(format)?;
    if bytes.len() < need {
        return None;
    }
    let u16_at = |i: usize| u32::from(u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]));
    let u32_at = |i: usize| u32::from_le_bytes([bytes[4 * i], bytes[4 * i + 1], bytes[4 * i + 2], bytes[4 * i + 3]]);
    let f32_at = |i: usize| f32::from_bits(u32_at(i));
    let h16 = |i: usize| half_to_f32(u16_at(i) as u16);
    let mut t = Texel { ch: [0.0; 4], n: 0, srgb: false, float: false, uint: false };
    match format {
        // 32-bit channels: float (typeless reads as float), uint, sint.
        1 | 2 => (t.ch, t.n, t.float) = ([f32_at(0), f32_at(1), f32_at(2), f32_at(3)], 4, true),
        3 => (t.ch, t.n, t.uint) = ([u32_at(0) as f32, u32_at(1) as f32, u32_at(2) as f32, u32_at(3) as f32], 4, true),
        4 => (t.ch, t.n, t.uint) = ([u32_at(0) as i32 as f32, u32_at(1) as i32 as f32, u32_at(2) as i32 as f32, u32_at(3) as i32 as f32], 4, true),
        5 | 6 => (t.ch, t.n, t.float) = ([f32_at(0), f32_at(1), f32_at(2), 0.0], 3, true),
        7 => (t.ch, t.n, t.uint) = ([u32_at(0) as f32, u32_at(1) as f32, u32_at(2) as f32, 0.0], 3, true),
        8 => (t.ch, t.n, t.uint) = ([u32_at(0) as i32 as f32, u32_at(1) as i32 as f32, u32_at(2) as i32 as f32, 0.0], 3, true),
        // 16-bit channels x4.
        9 | 10 => (t.ch, t.n, t.float) = ([h16(0), h16(1), h16(2), h16(3)], 4, true),
        11 => (t.ch, t.n) = ([unorm(u16_at(0), 16), unorm(u16_at(1), 16), unorm(u16_at(2), 16), unorm(u16_at(3), 16)], 4),
        12 => (t.ch, t.n, t.uint) = ([u16_at(0) as f32, u16_at(1) as f32, u16_at(2) as f32, u16_at(3) as f32], 4, true),
        13 => {
            let s = |i: usize| snorm(i32::from(u16_at(i) as u16 as i16), 16);
            (t.ch, t.n) = ([s(0), s(1), s(2), s(3)], 4);
        }
        14 => {
            let s = |i: usize| f32::from(u16_at(i) as u16 as i16);
            (t.ch, t.n, t.uint) = ([s(0), s(1), s(2), s(3)], 4, true);
        }
        15 | 16 => (t.ch, t.n, t.float) = ([f32_at(0), f32_at(1), 0.0, 0.0], 2, true),
        17 | 18 => (t.ch, t.n, t.uint) = ([u32_at(0) as f32, u32_at(1) as f32, 0.0, 0.0], 2, true),
        // 10:10:10:2.
        23..=25 => {
            let w = u32_at(0);
            if format == 25 {
                (t.ch, t.n, t.uint) = ([(w & 1023) as f32, ((w >> 10) & 1023) as f32, ((w >> 20) & 1023) as f32, (w >> 30) as f32], 4, true);
            } else {
                (t.ch, t.n) = ([unorm(w & 1023, 10), unorm((w >> 10) & 1023, 10), unorm((w >> 20) & 1023, 10), unorm(w >> 30, 2)], 4);
            }
        }
        26 => {
            let w = u32_at(0);
            (t.ch, t.n, t.float) = (
                [small_float((w >> 6) & 31, w & 63, 6), small_float((w >> 17) & 31, (w >> 11) & 63, 6), small_float((w >> 27) & 31, (w >> 22) & 31, 5), 0.0],
                3,
                true,
            );
        }
        // 8-bit x4.
        27..=32 => {
            let b = |i: usize| u32::from(bytes[i]);
            match format {
                30 => (t.ch, t.n, t.uint) = ([b(0) as f32, b(1) as f32, b(2) as f32, b(3) as f32], 4, true),
                31 => {
                    let s = |i: usize| snorm(i32::from(bytes[i] as i8), 8);
                    (t.ch, t.n) = ([s(0), s(1), s(2), s(3)], 4);
                }
                32 => {
                    let s = |i: usize| f32::from(bytes[i] as i8);
                    (t.ch, t.n, t.uint) = ([s(0), s(1), s(2), s(3)], 4, true);
                }
                _ => {
                    (t.ch, t.n) = ([unorm(b(0), 8), unorm(b(1), 8), unorm(b(2), 8), unorm(b(3), 8)], 4);
                    t.srgb = format == 29;
                }
            }
        }
        // 16-bit x2.
        33 | 34 => (t.ch, t.n, t.float) = ([h16(0), h16(1), 0.0, 0.0], 2, true),
        35 => (t.ch, t.n) = ([unorm(u16_at(0), 16), unorm(u16_at(1), 16), 0.0, 0.0], 2),
        36 => (t.ch, t.n, t.uint) = ([u16_at(0) as f32, u16_at(1) as f32, 0.0, 0.0], 2, true),
        37 => {
            let s = |i: usize| snorm(i32::from(u16_at(i) as u16 as i16), 16);
            (t.ch, t.n) = ([s(0), s(1), 0.0, 0.0], 2);
        }
        38 => {
            let s = |i: usize| f32::from(u16_at(i) as u16 as i16);
            (t.ch, t.n, t.uint) = ([s(0), s(1), 0.0, 0.0], 2, true);
        }
        // 32-bit single channel.
        39 | 41 => (t.ch, t.n, t.float) = ([f32_at(0), 0.0, 0.0, 0.0], 1, true),
        42 => (t.ch, t.n, t.uint) = ([u32_at(0) as f32, 0.0, 0.0, 0.0], 1, true),
        43 => (t.ch, t.n, t.uint) = ([u32_at(0) as i32 as f32, 0.0, 0.0, 0.0], 1, true),
        // 8-bit x2 and 16-bit x1.
        48 | 49 => (t.ch, t.n) = ([unorm(u32::from(bytes[0]), 8), unorm(u32::from(bytes[1]), 8), 0.0, 0.0], 2),
        50 => (t.ch, t.n, t.uint) = ([f32::from(bytes[0]), f32::from(bytes[1]), 0.0, 0.0], 2, true),
        51 => (t.ch, t.n) = ([snorm(i32::from(bytes[0] as i8), 8), snorm(i32::from(bytes[1] as i8), 8), 0.0, 0.0], 2),
        52 => (t.ch, t.n, t.uint) = ([f32::from(bytes[0] as i8), f32::from(bytes[1] as i8), 0.0, 0.0], 2, true),
        53 | 54 => (t.ch, t.n, t.float) = ([h16(0), 0.0, 0.0, 0.0], 1, true),
        55 | 56 => (t.ch, t.n) = ([unorm(u16_at(0), 16), 0.0, 0.0, 0.0], 1),
        57 => (t.ch, t.n, t.uint) = ([u16_at(0) as f32, 0.0, 0.0, 0.0], 1, true),
        58 => (t.ch, t.n, t.uint) = ([u16_at(0) as u16 as i16 as f32, 0.0, 0.0, 0.0], 1, true),
        // 8-bit x1.
        60 | 61 => (t.ch, t.n) = ([unorm(u32::from(bytes[0]), 8), 0.0, 0.0, 0.0], 1),
        62 => (t.ch, t.n, t.uint) = ([f32::from(bytes[0]), 0.0, 0.0, 0.0], 1, true),
        63 => (t.ch, t.n) = ([snorm(i32::from(bytes[0] as i8), 8), 0.0, 0.0, 0.0], 1),
        64 => (t.ch, t.n, t.uint) = ([f32::from(bytes[0] as i8), 0.0, 0.0, 0.0], 1, true),
        65 => (t.ch, t.n) = ([unorm(u32::from(bytes[0]), 8), 0.0, 0.0, 0.0], 1),
        // Shared exponent.
        67 => {
            let w = u32_at(0);
            let e = ((w >> 27) & 31) as i32 - 15 - 9;
            let f = |m: u32| m as f32 * 2f32.powi(e);
            (t.ch, t.n, t.float) = ([f(w & 511), f((w >> 9) & 511), f((w >> 18) & 511), 0.0], 3, true);
        }
        // 5:6:5, and BGRA in the order a title reads it (b, g, r, a swapped to r, g, b, a).
        85 => {
            let w = u16_at(0);
            (t.ch, t.n) = ([unorm((w >> 11) & 31, 5), unorm((w >> 5) & 63, 6), unorm(w & 31, 5), 1.0], 3);
        }
        87..=93 => {
            let b = |i: usize| unorm(u32::from(bytes[i]), 8);
            let a = if matches!(format, 88 | 92 | 93) { 1.0 } else { b(3) };
            (t.ch, t.n) = ([b(2), b(1), b(0), a], 4);
            t.srgb = matches!(format, 91 | 93);
        }
        _ => return None,
    }
    Some(t)
}

impl Texel {
    /// A single number for "how bright": Rec. 709 weights on three or more
    /// channels, else the first channel.
    pub fn luma(&self) -> f32 {
        if self.n >= 3 { 0.2126 * self.ch[0] + 0.7152 * self.ch[1] + 0.0722 * self.ch[2] } else { self.ch[0] }
    }
}

fn float_json(v: f32) -> Json {
    if v.is_finite() {
        Json::num(f64::from(v))
    } else if v.is_nan() {
        Json::str("NaN")
    } else if v > 0.0 {
        Json::str("+Inf")
    } else {
        Json::str("-Inf")
    }
}

fn texel_json(t: &Texel) -> Json {
    Json::Arr(t.ch[..t.n.max(1)].iter().map(|&v| float_json(v)).collect())
}

fn view_format(t: &TargetScan) -> u32 {
    t.format
}

/// What a texel of a target decoded to before and after the command.
fn decoded(t: &TargetScan) -> Option<(Texel, Texel)> {
    let f = view_format(t);
    let a = decode_texel(f, &t.texel_pre?)?;
    let b = decode_texel(f, &t.texel_post?)?;
    Some((a, b))
}

/// One command that changed the followed texel of one target.
#[derive(Clone, Debug, PartialEq)]
pub struct PixelChange {
    pub draw: usize,
    pub target: usize,
    pub before: Texel,
    pub after: Texel,
    /// before - after (positive: darker).
    pub luma_drop: f32,
}

/// The commands that changed the followed texel, in recording order. A
/// texel whose bytes are the same before and after is not a change.
pub fn pixel_changes(report: &ScanReport) -> Vec<PixelChange> {
    let mut out = Vec::new();
    for (di, d) in report.draws.iter().enumerate() {
        for (ti, t) in d.targets.iter().enumerate() {
            let (Some(pre), Some(post)) = (t.texel_pre, t.texel_post) else { continue };
            let n = texel_bytes(view_format(t)).unwrap_or(16).min(16);
            if pre[..n] == post[..n] {
                continue;
            }
            let Some((before, after)) = decoded(t) else {
                // A format we cannot decode: the change is still reported.
                let blank = Texel { ch: [0.0; 4], n: 0, srgb: false, float: false, uint: false };
                out.push(PixelChange { draw: di, target: ti, before: blank, after: blank, luma_drop: 0.0 });
                continue;
            };
            let drop = before.luma() - after.luma();
            out.push(PixelChange { draw: di, target: ti, before, after, luma_drop: if drop.is_nan() { 0.0 } else { drop } });
        }
    }
    out
}

/// The changes that made the texel darker, biggest drop first.
pub fn darkening(changes: &[PixelChange]) -> Vec<&PixelChange> {
    let mut v: Vec<&PixelChange> = changes.iter().filter(|c| c.luma_drop > 0.0 && c.before.n > 0).collect();
    v.sort_by(|a, b| b.luma_drop.total_cmp(&a.luma_drop));
    v
}

fn shaders_json(d: &DrawScan) -> Json {
    Json::Arr(d.shaders.iter().map(|(s, h)| Json::obj([("stage", Json::str(s.as_str())), ("hash", Json::str(h.as_str()))])).collect())
}

fn counts_json(d: &DrawScan) -> Json {
    let [a, b, c, e] = d.counts;
    match d.kind.as_str() {
        "dispatch" => Json::obj([("groups_x", Json::uint(a.into())), ("groups_y", Json::uint(b.into())), ("groups_z", Json::uint(c.into()))]),
        "mesh" => Json::obj([("groups", Json::uint(a.into()))]),
        "indexed" => Json::obj([
            ("indices", Json::uint(a.into())),
            ("instances", Json::uint(b.into())),
            ("first_index", Json::uint(c.into())),
            ("base_vertex", Json::Int(i64::from(e as i32))),
        ]),
        _ => Json::obj([
            ("vertices", Json::uint(a.into())),
            ("instances", Json::uint(b.into())),
            ("first_vertex", Json::uint(c.into())),
            ("first_instance", Json::uint(e.into())),
        ]),
    }
}

fn target_json(t: &TargetScan) -> Json {
    let mut f = vec![
        ("role", Json::str(t.role.as_str())),
        ("resource", Json::str(format!("{:#x}", t.resource))),
        ("format", Json::str(format_name(t.format))),
        ("width", Json::Int(i64::from(t.width))),
        ("height", Json::Int(i64::from(t.height))),
        ("subresource", Json::Int(i64::from(t.subresource))),
    ];
    if let Some(why) = &t.skipped {
        f.push(("skipped", Json::str(why.as_str())));
    }
    Json::Obj(f.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn command_json(d: &DrawScan) -> Json {
    Json::obj([
        ("index", Json::uint(d.index.into())),
        ("kind", Json::str(d.kind.as_str())),
        ("pipeline", Json::str(d.pipeline.as_str())),
        ("shaders", shaders_json(d)),
        ("counts", counts_json(d)),
        ("list", Json::str(format!("{:#x}", d.list))),
        ("writes", Json::Arr(d.targets.iter().map(target_json).collect())),
    ])
}

/// Does a command run any shader whose hash contains one of `filter`?
pub fn matches_filter(d: &DrawScan, filter: &[String]) -> bool {
    filter.is_empty() || d.shaders.iter().any(|(_, h)| filter.iter().any(|f| h.to_ascii_lowercase().contains(&f.to_ascii_lowercase())))
}

fn write_json(dir: &Path, prefix: &str, frame: u64, json: &Json) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let stamp = timestamp();
    let mut path = dir.join(format!("{prefix}-{stamp}-f{frame}.json"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{prefix}-{stamp}-f{frame}-{n}.json"));
        n += 1;
    }
    std::fs::write(&path, json.text()).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

fn common_notes(report: &ScanReport) -> Vec<Json> {
    report.notes.iter().map(|n| Json::str(n.as_str())).collect()
}

// ---- frame_commands ----

/// What `frame_commands` writes and answers.
pub struct CommandsOut {
    pub file: Json,
    pub summary: Json,
}

pub fn commands_report(report: &ScanReport) -> CommandsOut {
    let filter = &report.params.shader_filter;
    let listed: Vec<&DrawScan> = report.draws.iter().filter(|d| matches_filter(d, filter)).collect();
    let mut by_kind: Vec<(String, u64)> = Vec::new();
    for d in &listed {
        match by_kind.iter_mut().find(|(k, _)| *k == d.kind) {
            Some((_, n)) => *n += 1,
            None => by_kind.push((d.kind.clone(), 1)),
        }
    }
    let hits: Vec<(String, Json)> = filter
        .iter()
        .map(|f| {
            let idx: Vec<Json> = listed
                .iter()
                .filter(|d| matches_filter(d, std::slice::from_ref(f)))
                .map(|d| Json::uint(d.index.into()))
                .collect();
            (f.clone(), Json::obj([("ran_in_this_frame", Json::Bool(!idx.is_empty())), ("times", Json::uint(idx.len() as u64)), ("command_indices", Json::Arr(idx.into_iter().take(200).collect()))]))
        })
        .collect();
    let counts = Json::Obj(by_kind.iter().map(|(k, n)| (k.clone(), Json::uint(*n))).collect());
    let head = |extra: Vec<(&str, Json)>| {
        let mut f = vec![
            ("frame", Json::uint(report.frame)),
            ("commands_in_frame", Json::uint(report.commands_seen.into())),
            ("commands_listed", Json::uint(listed.len() as u64)),
            ("not_listed_max_draws", Json::uint(report.over_cap.into())),
            ("by_kind", counts.clone()),
            ("shader_filter", Json::Arr(filter.iter().map(|f| Json::str(f.as_str())).collect())),
            ("shader_hits", Json::Obj(hits.clone().into_iter().collect())),
        ];
        f.extend(extra);
        Json::Obj(f.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    };
    let file = head(vec![
        ("commands", Json::Arr(listed.iter().map(|d| command_json(d)).collect())),
        ("notes", Json::Arr(common_notes(report))),
    ]);
    let summary = head(vec![("notes", Json::Arr(common_notes(report)))]);
    CommandsOut { file, summary }
}

pub fn save_commands(report: &ScanReport, dir: &Path) -> Result<(PathBuf, Json), String> {
    let out = commands_report(report);
    let path = write_json(dir, "framecommands", report.frame, &out.file)?;
    Ok((path, out.summary))
}

// ---- trace_pixel ----

fn change_json(report: &ScanReport, c: &PixelChange) -> Json {
    let d = &report.draws[c.draw];
    let t = &d.targets[c.target];
    Json::obj([
        ("index", Json::uint(d.index.into())),
        ("kind", Json::str(d.kind.as_str())),
        ("pipeline", Json::str(d.pipeline.as_str())),
        ("shaders", shaders_json(d)),
        ("counts", counts_json(d)),
        ("target", target_json(t)),
        ("texel", t.texel.map_or(Json::Null, |(x, y)| Json::obj([("x", Json::Int(i64::from(x))), ("y", Json::Int(i64::from(y)))]))),
        ("before", if c.before.n > 0 { texel_json(&c.before) } else { Json::str("format not decoded") }),
        ("after", if c.after.n > 0 { texel_json(&c.after) } else { Json::str("format not decoded") }),
        ("luma_before", float_json(c.before.luma())),
        ("luma_after", float_json(c.after.luma())),
        ("luma_drop", float_json(c.luma_drop)),
        ("srgb_encoded", Json::Bool(c.before.srgb)),
    ])
}

pub struct TraceOut {
    pub file: Json,
    pub summary: Json,
}

/// How many darkening commands the summary lists.
pub const TOP: usize = 15;

pub fn trace_report(report: &ScanReport) -> TraceOut {
    let changes = pixel_changes(report);
    let dark = darkening(&changes);
    let traced: Vec<(u64, u32)> = {
        let mut v: Vec<(u64, u32)> = Vec::new();
        for d in &report.draws {
            for t in &d.targets {
                if t.texel_pre.is_some() && !v.contains(&(t.resource, t.subresource)) {
                    v.push((t.resource, t.subresource));
                }
            }
        }
        v
    };
    let targets: Vec<Json> = traced
        .iter()
        .map(|&(resource, sub)| {
            let mut writers = 0u64;
            let mut changed = 0u64;
            let mut shape = None;
            for (di, d) in report.draws.iter().enumerate() {
                for (ti, t) in d.targets.iter().enumerate() {
                    if t.resource == resource && t.subresource == sub && t.texel_pre.is_some() {
                        writers += 1;
                        shape.get_or_insert_with(|| target_json(t));
                        if changes.iter().any(|c| c.draw == di && c.target == ti) {
                            changed += 1;
                        }
                    }
                }
            }
            Json::obj([("target", shape.unwrap_or(Json::Null)), ("commands_writing", Json::uint(writers)), ("commands_that_changed_the_texel", Json::uint(changed))])
        })
        .collect();
    let head = |extra: Vec<(&str, Json)>| {
        let (px, py) = report.params.pixel.unwrap_or((0, 0));
        let mut f = vec![
            ("frame", Json::uint(report.frame)),
            ("screen_pixel", Json::obj([("x", Json::uint(px.into())), ("y", Json::uint(py.into()))])),
            ("back_buffer", Json::obj([("width", Json::uint(report.back_buffer.0.into())), ("height", Json::uint(report.back_buffer.1.into()))])),
            ("commands_in_frame", Json::uint(report.commands_seen.into())),
            ("commands_traced", Json::uint(report.draws.len() as u64)),
            ("targets_traced", Json::uint(traced.len() as u64)),
            ("commands_that_changed_the_texel", Json::uint(changes.len() as u64)),
            ("commands_that_darkened_it", Json::uint(dark.len() as u64)),
            ("darkening_ranked", Json::Arr(dark.iter().take(TOP).map(|c| change_json(report, c)).collect())),
        ];
        f.extend(extra);
        Json::Obj(f.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    };
    let file = head(vec![
        ("targets", Json::Arr(targets)),
        ("all_changes_in_recording_order", Json::Arr(changes.iter().map(|c| change_json(report, c)).collect())),
        ("darkening_all", Json::Arr(dark.iter().map(|c| change_json(report, c)).collect())),
        ("notes", Json::Arr(common_notes(report))),
    ]);
    let summary = head(vec![("notes", Json::Arr(common_notes(report)))]);
    TraceOut { file, summary }
}

pub fn save_trace(report: &ScanReport, dir: &Path) -> Result<(PathBuf, Json), String> {
    let out = trace_report(report);
    let path = write_json(dir, "pixeltrace", report.frame, &out.file)?;
    Ok((path, out.summary))
}

// ---- dump_target ----

/// The image as 8-bit RGB (three bytes a pixel): `channel` is `rgb`, `r`,
/// `g`, `b` or `a`. Float values get `exposure` and x / (1 + x), then the
/// sRGB curve; normalized ones are shown as stored; integers are scaled by
/// the largest value found.
pub fn dump_rgb8(image: &crate::badpix::DumpImage, channel: &str, exposure: f32) -> Result<Vec<u8>, String> {
    let bpp = texel_bytes(image.format).ok_or_else(|| format!("{} cannot be decoded", format_name(image.format)))?;
    let (w, h, pitch) = (image.width as usize, image.height as usize, image.pitch as usize);
    if pitch < w * bpp || image.data.len() < pitch * (h.saturating_sub(1)) + w * bpp {
        return Err("the readback is smaller than the target".to_owned());
    }
    let pick = |t: &Texel| -> [f32; 3] {
        match channel {
            "r" => [t.ch[0]; 3],
            "g" => [t.ch[1]; 3],
            "b" => [t.ch[2]; 3],
            "a" => [t.ch[3]; 3],
            _ if t.n >= 3 => [t.ch[0], t.ch[1], t.ch[2]],
            _ => [t.ch[0]; 3],
        }
    };
    let first = decode_texel(image.format, &image.data).ok_or("cannot decode the first texel")?;
    let mut max_int = 1.0f32;
    if first.uint {
        for y in 0..h {
            for x in 0..w {
                let at = y * pitch + x * bpp;
                if let Some(t) = decode_texel(image.format, &image.data[at..at + bpp]) {
                    for v in pick(&t) {
                        if v.is_finite() {
                            max_int = max_int.max(v.abs());
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let at = y * pitch + x * bpp;
            let t = decode_texel(image.format, &image.data[at..at + bpp]).unwrap_or(first);
            for v in pick(&t) {
                let b = if t.float {
                    let e = if v.is_nan() { 0.0 } else { (v * exposure).max(0.0) };
                    linear_to_srgb8(if e.is_infinite() { 1.0 } else { e / (1.0 + e) })
                } else if t.uint {
                    ((v.abs() / max_int).clamp(0.0, 1.0) * 255.0 + 0.5) as u8
                } else {
                    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
                };
                out.push(b);
            }
        }
    }
    Ok(out)
}

/// Writes the dumped target as `target-<time>-f<frame>-cmd<N>.png`.
pub fn save_dump(report: &ScanReport, dir: &Path, channel: &str, exposure: f32) -> Result<(PathBuf, Json), String> {
    let image = report.dump.as_ref().ok_or("the capture never reached the command asked for")?;
    if u64::from(image.width) * u64::from(image.height) > MAX_DUMP_PIXELS {
        return Err("the target is too large to save".to_owned());
    }
    let rgb = dump_rgb8(image, channel, exposure)?;
    let png = encode_png(image.width, image.height, &rgb)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let stamp = timestamp();
    let path = dir.join(format!("target-{stamp}-f{}-cmd{}.png", report.frame, image.after_command));
    std::fs::write(&path, png).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let info = Json::obj([
        ("frame", Json::uint(report.frame)),
        ("after_command", Json::uint(image.after_command.into())),
        ("kind", Json::str(image.kind.as_str())),
        ("shaders", Json::Arr(image.shaders.iter().map(|(s, h)| Json::str(format!("{s} {h}"))).collect())),
        ("role", Json::str(image.role.as_str())),
        ("resource", Json::str(format!("{:#x}", image.resource))),
        ("format", Json::str(format_name(image.format))),
        ("subresource", Json::uint(image.subresource.into())),
        ("width", Json::uint(image.width.into())),
        ("height", Json::uint(image.height.into())),
        ("channel", Json::str(channel)),
        ("commands_in_frame", Json::uint(report.commands_seen.into())),
        ("notes", Json::Arr(common_notes(report))),
    ]);
    Ok((path, info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::badpix::{DumpImage, ScanParams};

    fn half(v: f32) -> [u8; 2] {
        // Enough for the exact values the tests use.
        let bits: u16 = match v {
            x if x == 0.0 => 0,
            x if x == 1.0 => 0x3c00,
            x if x == 0.5 => 0x3800,
            x if x == 2.0 => 0x4000,
            _ => panic!("half: add {v}"),
        };
        bits.to_le_bytes()
    }

    #[test]
    fn decodes_the_formats_the_scene_uses() {
        let t = decode_texel(28, &[255, 0, 128, 255]).unwrap();
        assert_eq!((t.n, t.srgb), (4, false));
        assert!((t.ch[0] - 1.0).abs() < 1e-6 && t.ch[1] == 0.0 && (t.ch[2] - 128.0 / 255.0).abs() < 1e-6);
        assert!(decode_texel(29, &[1, 2, 3, 4]).unwrap().srgb);
        // BGRA is read as r, g, b.
        let t = decode_texel(87, &[10, 20, 30, 255]).unwrap();
        assert!(t.ch[0] > t.ch[1] && t.ch[1] > t.ch[2]);
        // RGBA16F.
        let mut b = Vec::new();
        for v in [1.0, 0.5, 2.0, 0.0] {
            b.extend(half(v));
        }
        assert_eq!(decode_texel(10, &b).unwrap().ch, [1.0, 0.5, 2.0, 0.0]);
        assert!(decode_texel(10, &b).unwrap().float);
        // R32 float and uint.
        assert_eq!(decode_texel(41, &1.5f32.to_le_bytes()).unwrap().ch[0], 1.5);
        let u = decode_texel(42, &7u32.to_le_bytes()).unwrap();
        assert_eq!((u.ch[0], u.uint), (7.0, true));
        // R10G10B10A2: all ones.
        let t = decode_texel(24, &0xffff_ffffu32.to_le_bytes()).unwrap();
        assert_eq!(t.ch, [1.0; 4]);
        // R11G11B10F: 1.0 is exponent 15, mantissa 0 in each channel.
        let w: u32 = (15 << 6) | (15 << 17) | (15 << 27);
        let t = decode_texel(26, &w.to_le_bytes()).unwrap();
        assert_eq!(t.ch[..3], [1.0, 1.0, 1.0]);
        assert!(decode_texel(10, &[0, 1]).is_none(), "too few bytes");
        assert!(decode_texel(999, &[0; 16]).is_none());
    }

    #[test]
    fn sizes_of_texels() {
        assert_eq!(texel_bytes(10), Some(8));
        assert_eq!(texel_bytes(28), Some(4));
        assert_eq!(texel_bytes(41), Some(4));
        assert_eq!(texel_bytes(54), Some(2));
        assert_eq!(texel_bytes(61), Some(1));
        assert_eq!(texel_bytes(49), Some(2));
        assert_eq!(texel_bytes(2), Some(16));
        assert_eq!(texel_bytes(0), None);
    }

    fn target(resource: u64, format: u32, pre: u8, post: u8) -> TargetScan {
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        a[..4].copy_from_slice(&[pre, pre, pre, 255]);
        b[..4].copy_from_slice(&[post, post, post, 255]);
        TargetScan {
            role: "rt0".into(),
            resource,
            format,
            width: 100,
            height: 50,
            texel: Some((10, 5)),
            texel_pre: Some(a),
            texel_post: Some(b),
            ..Default::default()
        }
    }

    fn draw(index: u32, hash: &str, targets: Vec<TargetScan>) -> DrawScan {
        DrawScan { index, kind: "dispatch".into(), shaders: vec![("compute".into(), hash.into())], targets, counts: [4, 2, 1, 0], ..Default::default() }
    }

    #[test]
    fn changes_and_darkening_are_ranked_by_brightness_lost() {
        let report = ScanReport {
            frame: 3,
            params: ScanParams { pixel: Some((670, 620)), ..Default::default() },
            commands_seen: 5,
            draws: vec![
                draw(0, "aa", vec![target(1, 28, 100, 200)]),
                draw(1, "bb", vec![target(1, 28, 200, 100)]),
                draw(2, "cc", vec![target(1, 28, 100, 100)]),
                draw(3, "dd", vec![target(1, 28, 250, 10)]),
            ],
            ..Default::default()
        };
        let changes = pixel_changes(&report);
        assert_eq!(changes.len(), 3, "the unchanged command is not listed");
        let dark = darkening(&changes);
        assert_eq!(dark.len(), 2);
        assert_eq!(report.draws[dark[0].draw].index, 3, "the biggest drop first");
        assert_eq!(report.draws[dark[1].draw].index, 1);
        let out = trace_report(&report);
        assert_eq!(out.summary.get("commands_that_darkened_it").and_then(Json::as_i64), Some(2));
        assert_eq!(out.summary.get("commands_that_changed_the_texel").and_then(Json::as_i64), Some(3));
        assert!(out.file.get("all_changes_in_recording_order").is_some());
    }

    #[test]
    fn an_undecodable_format_still_counts_as_a_change() {
        let report = ScanReport { draws: vec![draw(0, "aa", vec![target(1, 999, 1, 2)])], ..Default::default() };
        let c = pixel_changes(&report);
        assert_eq!(c.len(), 1);
        assert!(darkening(&c).is_empty());
    }

    #[test]
    fn frame_commands_reports_which_filtered_hashes_ran() {
        let report = ScanReport {
            frame: 9,
            commands_seen: 3,
            params: ScanParams { shader_filter: vec!["1AA247".into(), "ffff".into()], ..Default::default() },
            draws: vec![draw(1, "1aa247ed3653b616", vec![]), draw(2, "1aa247ed3653b616", vec![])],
            ..Default::default()
        };
        let out = commands_report(&report);
        let hits = out.summary.get("shader_hits").unwrap();
        assert_eq!(hits.get("1AA247").and_then(|h| h.get("times")).and_then(Json::as_i64), Some(2));
        assert_eq!(hits.get("ffff").and_then(|h| h.get("ran_in_this_frame")), Some(&Json::Bool(false)));
        assert!(matches_filter(&report.draws[0], &["247ED3".to_owned()]));
        assert!(!matches_filter(&report.draws[0], &["zz".to_owned()]));
        assert!(matches_filter(&report.draws[0], &[]));
        let Some(Json::Arr(list)) = out.file.get("commands") else { panic!() };
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn a_dump_becomes_a_png_with_tone_mapped_floats() {
        // 2x1 R16G16B16A16_FLOAT: white-ish 2.0 and black, pitch 256.
        let mut data = vec![0u8; 256];
        for (i, v) in [2.0f32, 2.0, 2.0, 1.0, 0.0, 0.0, 0.0, 1.0].iter().enumerate() {
            data[i * 2..i * 2 + 2].copy_from_slice(&half(*v));
        }
        let image = DumpImage { format: 10, width: 2, height: 1, pitch: 256, data, ..Default::default() };
        let rgb = dump_rgb8(&image, "rgb", 1.0).unwrap();
        assert_eq!(rgb.len(), 6);
        assert!(rgb[0] > 150 && rgb[0] < 255, "2.0 maps to 2/3 then sRGB: {}", rgb[0]);
        assert_eq!(rgb[3..], [0, 0, 0]);
        let alpha = dump_rgb8(&image, "a", 1.0).unwrap();
        assert_eq!(alpha[0], alpha[1]);
        let short = DumpImage { format: 10, width: 2, height: 2, pitch: 16, data: vec![0; 8], ..Default::default() };
        assert!(dump_rgb8(&short, "rgb", 1.0).is_err());
        let dir = tempfile_dir();
        let report = ScanReport { frame: 4, dump: Some(DumpImage { after_command: 7, ..image }), ..Default::default() };
        let (path, info) = save_dump(&report, &dir, "rgb", 1.0).unwrap();
        assert!(path.file_name().unwrap().to_string_lossy().ends_with("-f4-cmd7.png"));
        assert!(std::fs::read(&path).unwrap().starts_with(&[0x89, b'P', b'N', b'G']));
        assert_eq!(info.get("after_command").and_then(Json::as_i64), Some(7));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(save_dump(&ScanReport::default(), &dir, "rgb", 1.0).is_err());
    }

    fn tempfile_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("victory-framecap-{}-{:?}", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
