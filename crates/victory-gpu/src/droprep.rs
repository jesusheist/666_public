//! The dropped-draw reporter (`VICTORY_DROP_REPORT=1`, default off).
//!
//! Every place the host skips a draw, mesh dispatch, dispatch or indirect
//! execution says why ([`why`]) and then counts the drop ([`drop_draw`]).
//! The counts are kept per reason and per pipeline (its shader hashes, with
//! the kind of command) for a window of 5 s. When the window ends the top
//! 20 rows and the totals per reason go to `draws.log` (target
//! `victory::draws`, one event per window, routed ahead of the word rules in
//! `victory_core::logging`) and are kept for the `get_dropped_draws` MCP tool.
//!
//! Off, every entry point is one relaxed load of a cached flag.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use victory_mcp::Json;

/// Why a command was not run on the host. `code()` is the name in the logs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Reason {
    /// `VICTORY_SKIP_PIPELINES` names one of its shaders.
    SkippedShader,
    /// The title drew without setting a pipeline.
    NoPipelineSet,
    /// Its host pipeline is not made yet (a worker is still on it).
    PipelineMaking,
    /// Its shaders were refused (not translated) or the host refused the pipeline.
    PipelineRefused,
    /// Left out on purpose (`left_out`: shaders the runtime does not take).
    PipelineLeftOut,
    /// A compute pipeline in a draw, a graphics one in a dispatch, mesh and plain mixed.
    WrongPipelineKind,
    /// A stage of the ray-tracing structure builder while the builder is off.
    BuilderOff,
    /// A descriptor table found no free dynamic slots (`DYNAMIC_REFUSED`).
    TablesNoSlots,
    /// A shader reads title memory by address and no view heap holds its views.
    MemoryNoViewHeap,
    /// A root UAV with no buffer to bind.
    RootUavNothing,
    /// A root UAV with no bound view heap.
    RootUavNoHeap,
    /// A root SRV with no bound view heap.
    RootSrvNoHeap,
    /// A root view needed a dynamic slot and none was free.
    RootDynamicSlotRefused,
    /// Indexed draw, and no host copy of the index buffer.
    IndexBufferMissing,
    /// Rectangles of the draw could not be expanded.
    RectanglesNotExpanded,
    /// Indexed rectangles with a pipeline that cannot complete them.
    IndexedRectanglesLost,
    /// Mesh draws switched off (`VICTORY_MESH_TEST`).
    MeshOff,
    /// The host list cannot take mesh dispatches.
    MeshNoList6,
    /// `bind` said no, and nothing said why.
    BindFailed,
    /// Indirect: the pipeline kind does not match the command kind.
    IndirectKindMismatch,
    /// Indirect: the argument buffer is not a title buffer.
    IndirectArgsOutside,
    /// Indirect: the count buffer is not a title buffer.
    IndirectCountOutside,
    /// Indirect: no host command signature.
    IndirectNoSignature,
    /// Indirect: no argument clamp (or arguments not 4-byte aligned).
    IndirectNotBounded,
    /// Indirect: drop with no more specific reason.
    IndirectOther,
    /// A drop nothing named.
    Unknown,
}

impl Reason {
    const ALL: [Reason; 26] = [
        Reason::SkippedShader,
        Reason::NoPipelineSet,
        Reason::PipelineMaking,
        Reason::PipelineRefused,
        Reason::PipelineLeftOut,
        Reason::WrongPipelineKind,
        Reason::BuilderOff,
        Reason::TablesNoSlots,
        Reason::MemoryNoViewHeap,
        Reason::RootUavNothing,
        Reason::RootUavNoHeap,
        Reason::RootSrvNoHeap,
        Reason::RootDynamicSlotRefused,
        Reason::IndexBufferMissing,
        Reason::RectanglesNotExpanded,
        Reason::IndexedRectanglesLost,
        Reason::MeshOff,
        Reason::MeshNoList6,
        Reason::BindFailed,
        Reason::IndirectKindMismatch,
        Reason::IndirectArgsOutside,
        Reason::IndirectCountOutside,
        Reason::IndirectNoSignature,
        Reason::IndirectNotBounded,
        Reason::IndirectOther,
        Reason::Unknown,
    ];

    pub(crate) fn code(self) -> &'static str {
        match self {
            Reason::SkippedShader => "skipped_shader",
            Reason::NoPipelineSet => "no_pipeline_set",
            Reason::PipelineMaking => "pipeline_making",
            Reason::PipelineRefused => "pipeline_refused",
            Reason::PipelineLeftOut => "pipeline_left_out",
            Reason::WrongPipelineKind => "wrong_pipeline_kind",
            Reason::BuilderOff => "builder_off",
            Reason::TablesNoSlots => "tables_no_slots",
            Reason::MemoryNoViewHeap => "memory_no_view_heap",
            Reason::RootUavNothing => "root_uav_nothing",
            Reason::RootUavNoHeap => "root_uav_no_heap",
            Reason::RootSrvNoHeap => "root_srv_no_heap",
            Reason::RootDynamicSlotRefused => "root_dynamic_slot_refused",
            Reason::IndexBufferMissing => "index_buffer_missing",
            Reason::RectanglesNotExpanded => "rectangles_not_expanded",
            Reason::IndexedRectanglesLost => "indexed_rectangles_lost",
            Reason::MeshOff => "mesh_off",
            Reason::MeshNoList6 => "mesh_no_list6",
            Reason::BindFailed => "bind_failed",
            Reason::IndirectKindMismatch => "indirect_kind_mismatch",
            Reason::IndirectArgsOutside => "indirect_args_outside",
            Reason::IndirectCountOutside => "indirect_count_outside",
            Reason::IndirectNoSignature => "indirect_no_signature",
            Reason::IndirectNotBounded => "indirect_not_bounded",
            Reason::IndirectOther => "indirect_other",
            Reason::Unknown => "unknown",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|r| *r == self).unwrap_or(Self::ALL.len() - 1)
    }
}

/// One row of a report: a reason, a pipeline and the drops of it in the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub reason: Reason,
    /// The kind of command and the shader hashes (`draw vs:ab+ps:cd`).
    pub key: String,
    pub count: u64,
    /// The first frame of the window this was seen in.
    pub first_frame: u64,
}

/// A finished window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Report {
    /// Drops in the window.
    pub total: u64,
    /// The top rows, most drops first.
    pub rows: Vec<Row>,
    /// Drops per reason in the window (only reasons seen).
    pub by_reason: Vec<(Reason, u64)>,
    /// Drops per reason since the start (only reasons seen).
    pub since_start: Vec<(Reason, u64)>,
    /// Distinct (reason, pipeline) pairs in the window, rows or not.
    pub distinct: usize,
}

/// The counting, apart from the clock and the statics (so it can be tested).
#[derive(Default)]
pub(crate) struct Table {
    window: HashMap<(Reason, String), (u64, u64)>,
    window_by_reason: [u64; Reason::ALL.len()],
    total_by_reason: [u64; Reason::ALL.len()],
}

impl Table {
    pub(crate) fn note(&mut self, reason: Reason, key: &str, frame: u64) {
        let slot = self.window.entry((reason, key.to_owned())).or_insert((0, frame));
        slot.0 += 1;
        self.window_by_reason[reason.index()] += 1;
        self.total_by_reason[reason.index()] += 1;
    }

    /// Ends the window: the top `top` rows and the totals; the window starts empty.
    pub(crate) fn take(&mut self, top: usize) -> Report {
        let mut rows: Vec<Row> = self
            .window
            .drain()
            .map(|((reason, key), (count, first_frame))| Row { reason, key, count, first_frame })
            .collect();
        let distinct = rows.len();
        // Ties by reason code and key, so the order never depends on the hash map.
        rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.reason.code().cmp(b.reason.code())).then_with(|| a.key.cmp(&b.key)));
        rows.truncate(top);
        let seen = |counts: &[u64; Reason::ALL.len()]| {
            let mut v: Vec<(Reason, u64)> = Reason::ALL.iter().map(|r| (*r, counts[r.index()])).filter(|(_, n)| *n > 0).collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.code().cmp(b.0.code())));
            v
        };
        let report = Report {
            total: self.window_by_reason.iter().sum(),
            rows,
            by_reason: seen(&self.window_by_reason),
            since_start: seen(&self.total_by_reason),
            distinct,
        };
        self.window_by_reason = [0; Reason::ALL.len()];
        report
    }
}

impl Report {
    /// The text of one `draws.log` entry (one event, a few lines): the window
    /// totals with one count per reason, the top rows, the totals since the start.
    pub(crate) fn text(&self, secs: f64, dynamic_refused: u64) -> String {
        let list = |v: &[(Reason, u64)]| {
            if v.is_empty() {
                "none".to_owned()
            } else {
                v.iter().map(|(r, n)| format!("{}={n}", r.code())).collect::<Vec<_>>().join(" ")
            }
        };
        let mut out = format!(
            "drop report {secs:.1} s: total {} pairs {} dynamic_refused {dynamic_refused} | {}",
            self.total,
            self.distinct,
            list(&self.by_reason)
        );
        for row in &self.rows {
            out.push_str(&format!("\n  {} {} count {} first_frame {}", row.reason.code(), row.key, row.count, row.first_frame));
        }
        out.push_str(&format!("\n  since start | {}", list(&self.since_start)));
        out
    }

    pub(crate) fn json(&self, secs: f64, dynamic_refused: u64) -> Json {
        let pairs = |v: &[(Reason, u64)]| Json::Arr(v.iter().map(|(r, n)| Json::obj([("reason", Json::str(r.code())), ("count", Json::Int(*n as i64))])).collect());
        Json::obj([
            ("window_secs", Json::Num(secs)),
            ("total", Json::Int(self.total as i64)),
            ("distinct_pairs", Json::Int(self.distinct as i64)),
            ("dynamic_slot_requests_refused_in_window", Json::Int(dynamic_refused as i64)),
            ("by_reason", pairs(&self.by_reason)),
            ("by_reason_since_start", pairs(&self.since_start)),
            (
                "top",
                Json::Arr(
                    self.rows
                        .iter()
                        .map(|row| {
                            Json::obj([
                                ("reason", Json::str(row.reason.code())),
                                ("pipeline", Json::str(row.key.clone())),
                                ("count", Json::Int(row.count as i64)),
                                ("first_frame", Json::Int(row.first_frame as i64)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

const WINDOW: Duration = Duration::from_secs(5);
const TOP: usize = 20;

/// 0 unread, 1 off, 2 on.
static SWITCH: AtomicU8 = AtomicU8::new(0);

/// Whether `VICTORY_DROP_REPORT` is `1` (read once).
#[inline]
pub(crate) fn on() -> bool {
    match SWITCH.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => {
            let on = std::env::var("VICTORY_DROP_REPORT").is_ok_and(|v| matches!(v.trim(), "1" | "on" | "true"));
            SWITCH.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
    }
}

#[derive(Default)]
struct State {
    table: Table,
    since: Option<Instant>,
    last: Option<(Report, f64, u64)>,
    refused_seen: u64,
}

static STATE: std::sync::LazyLock<Mutex<State>> = std::sync::LazyLock::new(|| Mutex::new(State::default()));

thread_local! {
    static WHY: Cell<Option<Reason>> = const { Cell::new(None) };
}

/// Forgets a reason left by a command that did not end in a drop.
#[inline]
pub(crate) fn begin() {
    if on() {
        WHY.with(|w| w.set(None));
    }
}

/// The reason the command being prepared on this thread will be dropped for.
/// The first one said wins (the inner site knows best).
#[inline]
pub(crate) fn why(reason: Reason) {
    if on() {
        WHY.with(|w| {
            if w.get().is_none() {
                w.set(Some(reason));
            }
        });
    }
}

/// Counts a dropped command of `op` (`draw`, `mesh`, `dispatch`, `indirect`)
/// with the pipeline `this`; `fallback` if no site said why.
#[inline]
pub(crate) fn drop_draw(pipeline: usize, op: &str, fallback: impl FnOnce() -> Reason) {
    if !on() {
        return;
    }
    let reason = WHY.with(Cell::take).unwrap_or_else(fallback);
    let key = format!("{op} {}", crate::pipeline::drop_key(pipeline));
    let frame = crate::frame::current();
    STATE.lock().unwrap_or_else(|e| e.into_inner()).table.note(reason, &key, frame);
}

/// Once a second, from the frame meter: ends the window when 5 s have passed.
pub(crate) fn tick() {
    if !on() {
        return;
    }
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| victory_mcp::set_drops_hook(mcp_json));
    let refused = crate::descriptor::host::DYNAMIC_REFUSED.load(Ordering::Relaxed);
    let text = {
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let since = *s.since.get_or_insert(now);
        if now.duration_since(since) < WINDOW {
            return;
        }
        let secs = now.duration_since(since).as_secs_f64();
        let report = s.table.take(TOP);
        let refused_now = refused.saturating_sub(s.refused_seen);
        s.refused_seen = refused;
        s.since = Some(now);
        let text = report.text(secs, refused_now);
        s.last = Some((report, secs, refused_now));
        text
    };
    tracing::info!(target: "victory::draws", "{text}");
}

/// `get_dropped_draws`: the last finished window, as the log has it.
pub(crate) fn mcp_json() -> Json {
    let s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    match &s.last {
        Some((report, secs, refused)) => report.json(*secs, *refused),
        None => Json::obj([("note", Json::str("no window finished yet (a window is 5 s)"))]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_per_reason_and_pipeline_and_keeps_the_first_frame() {
        let mut t = Table::default();
        t.note(Reason::PipelineMaking, "draw vs:aa+ps:bb", 10);
        t.note(Reason::PipelineMaking, "draw vs:aa+ps:bb", 11);
        t.note(Reason::PipelineMaking, "draw vs:cc+ps:dd", 12);
        t.note(Reason::TablesNoSlots, "draw vs:aa+ps:bb", 13);
        let r = t.take(20);
        assert_eq!(r.total, 4);
        assert_eq!(r.distinct, 3);
        assert_eq!(r.rows[0], Row { reason: Reason::PipelineMaking, key: "draw vs:aa+ps:bb".into(), count: 2, first_frame: 10 });
        assert_eq!(r.by_reason, vec![(Reason::PipelineMaking, 3), (Reason::TablesNoSlots, 1)]);
    }

    #[test]
    fn top_rows_are_cut_and_ties_have_a_stable_order() {
        let mut t = Table::default();
        for i in 0..30u64 {
            t.note(Reason::BindFailed, &format!("draw k{i:02}"), i);
        }
        for _ in 0..5 {
            t.note(Reason::MeshOff, "mesh m", 99);
        }
        let r = t.take(20);
        assert_eq!(r.rows.len(), 20);
        assert_eq!(r.distinct, 31);
        assert_eq!(r.total, 35);
        assert_eq!(r.rows[0].reason, Reason::MeshOff);
        // Equal counts: by reason code then key.
        assert_eq!(r.rows[1].key, "draw k00");
        assert_eq!(r.rows[2].key, "draw k01");
    }

    #[test]
    fn a_window_starts_empty_but_the_totals_since_start_stay() {
        let mut t = Table::default();
        t.note(Reason::NoPipelineSet, "draw ", 1);
        let first = t.take(20);
        assert_eq!(first.since_start, vec![(Reason::NoPipelineSet, 1)]);
        t.note(Reason::NoPipelineSet, "draw ", 2);
        t.note(Reason::BuilderOff, "dispatch cs:ee", 3);
        let second = t.take(20);
        assert_eq!(second.total, 2);
        assert_eq!(second.rows.iter().find(|r| r.reason == Reason::NoPipelineSet).unwrap().first_frame, 2);
        assert_eq!(second.since_start, vec![(Reason::NoPipelineSet, 2), (Reason::BuilderOff, 1)]);
        let empty = t.take(20);
        assert_eq!(empty, Report { since_start: empty.since_start.clone(), ..Report::default() });
        assert_eq!(empty.since_start.len(), 2);
    }

    #[test]
    fn every_reason_has_a_unique_code_and_index() {
        let mut codes: Vec<&str> = Reason::ALL.iter().map(|r| r.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), Reason::ALL.len());
        for (i, r) in Reason::ALL.iter().enumerate() {
            assert_eq!(r.index(), i);
        }
    }

    #[test]
    fn text_and_json_carry_the_same_table() {
        let mut t = Table::default();
        t.note(Reason::PipelineRefused, "draw vs:a+ps:b", 7);
        let r = t.take(20);
        let text = r.text(5.0, 3);
        assert_eq!(
            text,
            "drop report 5.0 s: total 1 pairs 1 dynamic_refused 3 | pipeline_refused=1\n  pipeline_refused draw vs:a+ps:b count 1 first_frame 7\n  since start | pipeline_refused=1"
        );
        let j = r.json(5.0, 3);
        assert_eq!(j.get("total"), Some(&Json::Int(1)));
        let top = j.get("top").unwrap();
        let Json::Arr(rows) = top else { panic!("top is an array") };
        assert_eq!(rows[0].get("first_frame"), Some(&Json::Int(7)));
    }
}
