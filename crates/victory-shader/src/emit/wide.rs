//! Wave-wide branches in a thread group of several console waves.
//!
//! The console takes a branch on a lane mask (`s_cbranch_execz`, `vccnz`, ...) with the whole
//! wave at once. A console wave run as two host waves of 32 lanes exchanges that decision and
//! meets (a group barrier) inside the branch. A group barrier needs every thread of the *group*,
//! and a group of several console waves has waves that decide differently: the ones that skip
//! would never reach a barrier inside, and the others would wait for them. So the decision is
//! made for the group: the body runs if any console wave of the group runs it. A console wave
//! that would have skipped runs the body as a "ghost": every vector effect there is guarded by
//! the wave's own decision (and, in a loop, by a flag that tells the wave has left it), so such
//! a wave changes nothing the console would not have changed. What this module decides is where
//! that is sound; [`plan`] says why not otherwise.

use std::collections::{HashMap, HashSet};

use crate::gfx10::Fields;
use crate::ir::{EXEC, Inst, Program, Reg, Value};
use crate::structure::{LaneExit, Structured};

pub(super) type Why = &'static str;

/// Where the console wave's own decisions guard the code of a group-wide branch.
#[derive(Debug, Default)]
pub(super) struct Plan {
    /// Per wide branch (its block): whether the body side, the one a wave that does not take the
    /// branch past the body would run, is its `taken` side. The other side joins the program
    /// again (a forward branch) or leaves the loop (a loop's back edge is the body side).
    pub body_taken: HashMap<usize, bool>,
    /// Per IR block: the forward wide branches (their blocks) whose body holds the block.
    pub region: Vec<Vec<usize>>,
    /// Per IR block: the headers of the loops, with a wide branch inside, that hold the block.
    pub looped: Vec<Vec<usize>>,
    /// The headers of those loops, in block order.
    pub headers: Vec<usize>,
    /// Per IR block: whether it is on a side that only the waves that took it run, on their own
    /// after the body, no group barrier possible in it (`alone_exits`).
    pub alone: Vec<bool>,
    /// For the edges that leave the body of such a branch: the branch (its block) and the block
    /// its other side starts with, where the waves that took that side go after the body.
    pub alone_exits: HashMap<(usize, usize), (usize, usize)>,
}

/// Instructions that do nothing a wave could miss.
const WAITS: [&str; 6] = ["s_nop", "s_waitcnt", "s_waitcnt_vscnt", "s_waitcnt_vmcnt", "s_waitcnt_lgkmcnt", "s_waitcnt_expcnt"];

/// Whether an instruction needs the lanes of the console wave beyond the host wave's own (the
/// translation meets in a group barrier for these), or the group.
fn crosses_lanes(st: &Structured, i: &Inst) -> bool {
    let n = i.name();
    matches!(i.fields, Fields::Ds { .. })
        || matches!(&i.fields, Fields::Valu(v) if v.dpp.is_some())
        || n == "s_barrier"
        || n.starts_with("v_mbcnt")
        || n.starts_with("v_permlane")
        || n.contains("permute")
        || n.contains("swizzle")
        || n.starts_with("v_readlane")
        || n.starts_with("v_readfirstlane")
        || n.starts_with("v_writelane")
        || n.starts_with("s_bcnt")
        || n.starts_with("s_ff")
        || n.starts_with("s_fl")
        || n.starts_with("s_wqm")
        || super::lower::reads_mask_as_data(st, i)
}

/// Successor blocks of each block, and the post-dominator set of each (a virtual exit after
/// every block that ends the program), as bitsets. None with an indirect exit.
pub(super) fn post_dominators(st: &Structured) -> Option<(Vec<Vec<usize>>, Vec<Vec<u64>>)> {
    let n = st.exits.len();
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (b, e) in st.exits.iter().enumerate() {
        match *e {
            LaneExit::Jump(t) => succ[b].push(t),
            LaneExit::Branch { taken, not_taken, .. } => {
                succ[b].push(taken);
                succ[b].push(not_taken);
            }
            LaneExit::End => {}
            LaneExit::Indirect => return None,
        }
    }
    // post-dominators (bitsets; a virtual exit after every End)
    let words = n.div_ceil(64);
    let full = vec![u64::MAX; words];
    let mut pdom: Vec<Vec<u64>> = (0..n).map(|b| if succ[b].is_empty() { let mut v = vec![0; words]; v[b / 64] |= 1 << (b % 64); v } else { full.clone() }).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for b in (0..n).rev() {
            if succ[b].is_empty() {
                continue;
            }
            let mut v = full.clone();
            for &t in &succ[b] {
                for w in 0..words {
                    v[w] &= pdom[t][w];
                }
            }
            v[b / 64] |= 1 << (b % 64);
            if v != pdom[b] {
                pdom[b] = v;
                changed = true;
            }
        }
    }
    Some((succ, pdom))
}

fn has(set: &[u64], b: usize) -> bool {
    set[b / 64] >> (b % 64) & 1 == 1
}

/// Whether register `reg`'s value is a scalar number or bit the structure does not track as a
/// lane mask.
fn is_scalar(st: &Structured, reg: Reg, value: Value) -> bool {
    match reg {
        Reg::Scc => true,
        Reg::S(c) => {
            c != EXEC && c != EXEC + 1 && !(st.masks.contains_key(&value) || st.highs.contains_key(&value))
        }
        Reg::V(_) => false,
    }
}

/// How the wide branches of `st` (`st.wide`) are taken for the group, or why they cannot be.
pub(super) fn plan(p: &Program, st: &Structured) -> Result<Plan, Why> {
    let n = p.blocks.len();
    let order = crate::ir::reverse_postorder(&p.blocks);
    let mut place = vec![usize::MAX; n];
    for (k, &b) in order.iter().enumerate() {
        place[b] = k;
    }
    let live = |b: usize| place[b] != usize::MAX;
    let (succ, pdom) = post_dominators(st).ok_or("an indirect exit")?;
    let idom = crate::ir::idoms(&p.blocks, &order, &place);
    let dominates = |a: usize, mut b: usize| loop {
        if a == b {
            return true;
        }
        if idom[b] == b || idom[b] == usize::MAX {
            return false;
        }
        b = idom[b];
    };
    // natural loops: the blocks that reach a latch without passing the header
    let mut loops: Vec<(usize, Vec<bool>)> = Vec::new();
    for &l in &order {
        for &h in &succ[l] {
            if live(h) && place[h] <= place[l] {
                if !dominates(h, l) {
                    return Err("an irreducible loop");
                }
                let at = match loops.iter().position(|(x, _)| *x == h) {
                    Some(k) => k,
                    None => {
                        let mut set = vec![false; n];
                        set[h] = true;
                        loops.push((h, set));
                        loops.len() - 1
                    }
                };
                let mut stack = vec![l];
                while let Some(b) = stack.pop() {
                    if loops[at].1[b] {
                        continue;
                    }
                    loops[at].1[b] = true;
                    stack.extend(p.blocks[b].preds.iter().copied().filter(|&q| live(q)));
                }
            }
        }
    }
    let mut plan = Plan {
        region: vec![Vec::new(); n],
        looped: vec![Vec::new(); n],
        alone: vec![false; n],
        ..Plan::default()
    };
    // the bodies, each a set of blocks, and the blocks the group leaves them for
    let mut bodies: Vec<(Vec<bool>, bool)> = Vec::new();
    let mut wide: Vec<usize> = st.wide.iter().copied().filter(|&b| live(b)).collect();
    wide.sort_unstable();
    for &q in &wide {
        let LaneExit::Branch { taken, not_taken, .. } = st.exits[q] else {
            return Err("a wave-wide branch that is not a branch");
        };
        let back = |s: usize| place[s] <= place[q];
        match (back(taken), back(not_taken)) {
            (true, true) => return Err("a wave-wide branch with both sides going back"),
            (true, false) | (false, true) => {
                // the closing branch of a loop: the back edge continues it, the group leaves it
                // when no wave wants another round
                let (head, exit) = if back(taken) { (taken, not_taken) } else { (not_taken, taken) };
                let held = loops.iter().find(|(h, _)| *h == head).map(|(_, set)| set[q] && !set[exit]);
                if held != Some(true) {
                    return Err("a wave-wide loop branch whose other side stays in the loop");
                }
                plan.body_taken.insert(q, back(taken));
            }
            (false, false) => {
                // The side the console takes past the body: it has nothing of its own before the
                // two paths meet again (the blocks it passes before a block every path from the
                // branch passes do nothing but wait), so a wave that goes the other way misses
                // nothing by it.
                let bare = |s: usize| {
                    let (mut seen, mut stack) = (HashSet::new(), vec![s]);
                    while let Some(b) = stack.pop() {
                        if has(&pdom[q], b) || !seen.insert(b) {
                            continue;
                        }
                        let block = &p.blocks[b];
                        if !block.phis.is_empty() || !block.insts.iter().all(|i| WAITS.contains(&i.name())) {
                            return false;
                        }
                        stack.extend(succ[b].iter().copied().filter(|&t| place[t] > place[b]));
                    }
                    true
                };
                // Both sides do something: the group runs the body side for every wave, and the
                // waves that took the other side run it on their own after it, where no group
                // barrier can be (code a wave can run alone, entered by this branch alone, and
                // both sides ending at blocks every path passes).
                let forward = |from: usize| {
                    let (mut set, mut stack) = (vec![false; n], vec![from]);
                    while let Some(b) = stack.pop() {
                        if set[b] || has(&pdom[q], b) {
                            continue;
                        }
                        set[b] = true;
                        stack.extend(succ[b].iter().copied().filter(|&t| place[t] > place[b]));
                    }
                    set
                };
                let exits_of = |set: &[bool]| -> Option<Vec<(usize, usize)>> {
                    let mut out = Vec::new();
                    for u in (0..n).filter(|&u| set[u]) {
                        if matches!(st.exits[u], LaneExit::End | LaneExit::Indirect) {
                            return None;
                        }
                        for &x in succ[u].iter().filter(|&&x| !set[x]) {
                            if !has(&pdom[q], x) || place[x] <= place[u] {
                                return None;
                            }
                            out.push((u, x));
                        }
                    }
                    Some(out)
                };
                let alone = |s: usize| -> Option<(usize, Vec<bool>, Vec<(usize, usize)>)> {
                    let body = if s == taken { not_taken } else { taken };
                    if has(&pdom[q], s) || p.blocks[s].preds.iter().filter(|&&x| live(x)).count() != 1 {
                        return None;
                    }
                    let (mine, theirs) = (forward(s), forward(body));
                    if (0..n).any(|x| mine[x] && theirs[x]) {
                        return None;
                    }
                    let fine = (0..n).filter(|&x| mine[x]).all(|x| {
                        !st.wide.contains(&x) && !p.blocks[x].insts.iter().any(|i| crosses_lanes(st, i))
                    });
                    if !fine {
                        return None;
                    }
                    exits_of(&mine)?;
                    Some((s, mine, exits_of(&theirs)?))
                };
                let (skip, alone) = match (bare(taken), bare(not_taken)) {
                    (true, false) => (taken, None),
                    (false, true) => (not_taken, None),
                    (true, true) => (not_taken, None),
                    _ => match alone(not_taken).or_else(|| alone(taken)) {
                        Some((s, mine, exits)) => (s, Some((mine, exits))),
                        None => {
                            if std::env::var_os("VICTORY_WAVE_WHY").is_some() {
                                eprintln!("WAVE64-WIDE branch at {:#x} (to {:#x} and {:#x}): neither side is bare, and neither can be run alone", p.blocks[q].offset, p.blocks[taken].offset, p.blocks[not_taken].offset);
                            }
                            return Err("a wave-wide branch whose both sides do something");
                        }
                    },
                };
                let body = if skip == taken { not_taken } else { taken };
                plan.body_taken.insert(q, body == taken);
                if let Some((mine, exits)) = &alone {
                    for x in (0..n).filter(|&x| mine[x]) {
                        plan.alone[x] = true;
                    }
                    plan.alone_exits.extend(exits.iter().map(|&e| (e, (q, skip))));
                }
                // the body: reached from its side without passing the join, going forward only
                let set = forward(body);
                for b in (0..n).filter(|&b| set[b]) {
                    if !dominates(q, b) {
                        return Err("a wave-wide branch whose body has another way in");
                    }
                    plan.region[b].push(q);
                }
                // (what the body leaves in scalar registers is what the other side's waves
                // do not read: they go on with their own)
                if alone.is_none() {
                    bodies.push((set, true));
                }
            }
        }
    }
    // loops with a wide branch inside carry a flag that tells a wave has left
    for (h, set) in &loops {
        if wide.iter().any(|&q| set[q]) {
            plan.headers.push(*h);
            for b in (0..n).filter(|&b| set[b]) {
                plan.looped[b].push(*h);
            }
            bodies.push((set.clone(), false));
        }
    }
    plan.headers.sort_unstable();
    // What a wave that ran a body as a ghost leaves behind is what the guards leave: vector
    // registers keep their values, memory is not written. A scalar number carried out of a
    // body would be the ghost's own, not the one the console wave that skipped had.
    let mut defs: HashMap<Value, usize> = HashMap::new();
    for (b, block) in p.blocks.iter().enumerate().filter(|(b, _)| live(*b)) {
        for phi in &block.phis {
            defs.insert(phi.value, b);
        }
        for inst in &block.insts {
            for &(_, v) in &inst.writes {
                defs.insert(v, b);
            }
        }
    }
    for (set, forward) in &bodies {
        for (e, block) in p.blocks.iter().enumerate().filter(|(e, _)| live(*e) && !set[*e]) {
            for phi in &block.phis {
                if !is_scalar(st, phi.reg, phi.value) {
                    continue;
                }
                let mut seen: Option<Value> = None;
                let mut inside = false;
                let mut differ = false;
                for (k, &q) in block.preds.iter().enumerate().filter(|(_, q)| live(**q)) {
                    // (a body inside a loop goes round by a back edge: the loop's own check)
                    if *forward && place[e] <= place[q] {
                        continue;
                    }
                    inside |= set[q];
                    match seen {
                        None => seen = Some(phi.args[k]),
                        Some(v) if v != phi.args[k] => differ = true,
                        _ => {}
                    }
                }
                if inside && differ {
                    return Err("a scalar value carried out of a wave-wide branch");
                }
            }
            for inst in &block.insts {
                for &(reg, v) in &inst.reads {
                    if is_scalar(st, reg, v) && defs.get(&v).is_some_and(|&d| set[d]) {
                        return Err("a scalar value read after a wave-wide branch");
                    }
                }
            }
        }
    }
    Ok(plan)
}
