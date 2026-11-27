//! A Cheat-Engine-style scanner over the title's memory, for the `mem_*`
//! tools. It only ever reads title memory (through the runtime's safe read,
//! which reports a missing page instead of faulting) and keeps its own copies.
//!
//! A scan starts as a snapshot of every committed, writable range the title
//! owns ([`MemHooks::regions`]) and a bitmap with one bit per aligned slot
//! (all set: every aligned address is a candidate). Each filter reads the
//! memory again, compares it with the snapshot slot by slot, clears the bits
//! that fail and refreshes the snapshot of the survivors. Once the survivors
//! are few enough that a list of (address, value) pairs is smaller than the
//! snapshot, the snapshot is dropped and the survivors are kept as that list.
//!
//! Memory: a dense scan holds one byte per scanned byte plus one bit per slot
//! (1.125 bytes per byte at width 1, 1.03 at width 4), and one 1 MiB chunk of
//! scratch. A sparse one holds 16 bytes per survivor.

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use crate::json::Json;

/// The kernel's page: the unit a failed read is retried in.
const PAGE: usize = 4096;
/// Bytes read at a time (a multiple of 64 slots at every width).
const CHUNK: usize = 1 << 20;
/// Bytes one survivor takes once the snapshot is dropped.
const SPARSE_ENTRY: usize = 16;
/// Largest snapshot taken unless `max_mb` says otherwise.
pub const DEFAULT_MAX_MB: u64 = 16 * 1024;

/// A committed, writable range of title memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemRegion {
    pub base: u64,
    pub size: u64,
    /// The Windows protection constant it had when listed.
    pub protect: u32,
    /// Where it came from: `xmem` (a reservation the title made) or
    /// `image <section>` (a writable section of the title's image).
    pub kind: String,
}

impl MemRegion {
    fn is_image(&self) -> bool {
        self.kind.starts_with("image")
    }
}

/// What the runtime gives the memory tools (the HLE layer registers it once
/// the title image is mapped).
#[derive(Clone, Copy)]
pub struct MemHooks {
    /// The title's committed, readable and writable ranges now, by address.
    pub regions: fn() -> Vec<MemRegion>,
    /// Reads title memory; false if a page of the range cannot be read.
    pub read: fn(u64, &mut [u8]) -> bool,
    /// Where the title's image is mapped (0 if it is not).
    pub image_base: fn() -> u64,
}

static MEM_HOOKS: OnceLock<MemHooks> = OnceLock::new();

/// Registers the memory tools' functions. The first call wins.
pub fn set_mem_hooks(hooks: MemHooks) {
    let _ = MEM_HOOKS.set(hooks);
}

pub(crate) fn mem_hooks() -> Option<&'static MemHooks> {
    MEM_HOOKS.get()
}

/// Where bytes come from (title memory, or a fake one in tests).
pub trait Source {
    fn read(&self, addr: u64, out: &mut [u8]) -> bool;
}

/// The registered read function as a [`Source`].
pub(crate) struct HookSource(pub fn(u64, &mut [u8]) -> bool);

impl Source for HookSource {
    fn read(&self, addr: u64, out: &mut [u8]) -> bool {
        (self.0)(addr, out)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Changed,
    Unchanged,
    Increased,
    Decreased,
    Equals,
    NotEquals,
}

impl Mode {
    pub const NAMES: [&'static str; 6] = ["changed", "unchanged", "increased", "decreased", "equals", "not_equals"];

    pub fn parse(text: &str) -> Option<Mode> {
        Some(match text {
            "changed" => Mode::Changed,
            "unchanged" => Mode::Unchanged,
            "increased" => Mode::Increased,
            "decreased" => Mode::Decreased,
            "equals" => Mode::Equals,
            "not_equals" => Mode::NotEquals,
            _ => return None,
        })
    }

    pub fn needs_value(self) -> bool {
        matches!(self, Mode::Equals | Mode::NotEquals)
    }
}

/// A number as the tools take it: `0x` hex, else decimal.
pub fn parse_u64(text: &str) -> Option<u64> {
    let t = text.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => t.parse().ok(),
    }
}

/// The little-endian number in `bytes` (at most 8).
fn le(bytes: &[u8]) -> u64 {
    let mut v = [0u8; 8];
    v[..bytes.len()].copy_from_slice(bytes);
    u64::from_le_bytes(v)
}

fn as_signed(v: u64, width: usize) -> i64 {
    let shift = 64 - 8 * width as u32;
    ((v << shift) as i64) >> shift
}

/// The bits a value of `width` bytes has.
pub fn mask(width: usize) -> u64 {
    if width >= 8 { u64::MAX } else { (1u64 << (8 * width)) - 1 }
}

/// Whether a slot that held `old` and holds `new` stays a candidate.
/// `value` is already masked to the width.
pub fn keeps(mode: Mode, old: u64, new: u64, value: u64, width: usize, signed: bool) -> bool {
    match mode {
        Mode::Changed => old != new,
        Mode::Unchanged => old == new,
        Mode::Equals => new == value,
        Mode::NotEquals => new != value,
        Mode::Increased if signed => as_signed(new, width) > as_signed(old, width),
        Mode::Decreased if signed => as_signed(new, width) < as_signed(old, width),
        Mode::Increased => new > old,
        Mode::Decreased => new < old,
    }
}

/// Reads `out` from `addr`; pieces (of the kernel's page) that cannot be
/// read are zeroed and told to `bad` as offsets into `out`.
fn read_chunk(src: &dyn Source, addr: u64, out: &mut [u8], bad: &mut dyn FnMut(usize, usize)) {
    if src.read(addr, out) {
        return;
    }
    let mut at = 0;
    while at < out.len() {
        let a = addr + at as u64;
        let n = (PAGE - (a as usize % PAGE)).min(out.len() - at);
        if !src.read(a, &mut out[at..at + n]) {
            out[at..at + n].fill(0);
            bad(at, at + n);
        }
        at += n;
    }
}

fn clear_slots(bits: &mut [u64], from: usize, to: usize) {
    for slot in from..to.min(bits.len() * 64) {
        bits[slot / 64] &= !(1u64 << (slot % 64));
    }
}

fn popcount(bits: &[u64]) -> u64 {
    bits.iter().map(|w| u64::from(w.count_ones())).sum()
}

struct Dense {
    info: MemRegion,
    snap: Vec<u8>,
    bits: Vec<u64>,
    live: u64,
}

enum Store {
    Empty,
    Dense(Vec<Dense>),
    Sparse { regions: Vec<MemRegion>, hits: Vec<(u64, u64)> },
}

/// One candidate, for listing.
pub struct Hit {
    pub address: u64,
    /// What memory holds now (the snapshot's value if it cannot be read).
    pub value: u64,
    pub region: MemRegion,
}

#[derive(Debug)]
pub struct StartInfo {
    pub regions: usize,
    pub bytes: u64,
    pub candidates: u64,
    pub held_bytes: u64,
    pub ms: f64,
}

#[derive(Debug)]
pub struct FilterInfo {
    pub before: u64,
    pub survivors: u64,
    /// Candidates dropped because their page could not be read.
    pub unreadable: u64,
    pub sparse: bool,
    pub held_bytes: u64,
    pub ms: f64,
}

pub struct Scanner {
    width: usize,
    store: Store,
}

static SCAN: Mutex<Scanner> = Mutex::new(Scanner::new());

/// The one scan state; held for a whole tool call.
pub fn scanner() -> MutexGuard<'static, Scanner> {
    SCAN.lock().unwrap_or_else(|e| e.into_inner())
}

impl Scanner {
    pub const fn new() -> Scanner {
        Scanner { width: 1, store: Store::Empty }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn active(&self) -> bool {
        !matches!(self.store, Store::Empty)
    }

    pub fn reset(&mut self) {
        self.store = Store::Empty;
    }

    pub fn candidates(&self) -> u64 {
        match &self.store {
            Store::Empty => 0,
            Store::Dense(rs) => rs.iter().map(|r| r.live).sum(),
            Store::Sparse { hits, .. } => hits.len() as u64,
        }
    }

    pub fn held_bytes(&self) -> u64 {
        match &self.store {
            Store::Empty => 0,
            Store::Dense(rs) => rs.iter().map(|r| (r.snap.len() + r.bits.len() * 8) as u64).sum(),
            Store::Sparse { hits, .. } => (hits.len() * SPARSE_ENTRY) as u64,
        }
    }

    /// Takes the snapshot of `regions` and makes every aligned address a
    /// candidate. A scan already held is dropped first.
    pub fn start(&mut self, src: &dyn Source, mut regions: Vec<MemRegion>, width: usize, max_bytes: u64) -> Result<StartInfo, String> {
        let t = Instant::now();
        self.store = Store::Empty;
        if !matches!(width, 1 | 2 | 4) {
            return Err("width must be 1, 2 or 4".into());
        }
        regions.sort_by_key(|r| r.base);
        for r in &mut regions {
            r.size -= r.size % width as u64;
        }
        regions.retain(|r| r.size > 0);
        let total: u64 = regions.iter().map(|r| r.size).sum();
        if total == 0 {
            return Err("the title has no committed writable memory to scan (is the title running?)".into());
        }
        if total > max_bytes {
            return Err(format!(
                "the title's writable memory is {} MB, above the limit of {} MB: nothing was read; pass a larger max_mb to scan it \
                 (a scan holds about {:.2} bytes per scanned byte)",
                total >> 20,
                max_bytes >> 20,
                1.0 + 1.0 / (8.0 * width as f64),
            ));
        }
        let mut dense = Vec::with_capacity(regions.len());
        for info in regions {
            let size = info.size as usize;
            let slots = size / width;
            let words = slots.div_ceil(64);
            let (mut snap, mut bits) = (Vec::new(), Vec::new());
            if snap.try_reserve_exact(size).is_err() || bits.try_reserve_exact(words).is_err() {
                return Err(format!("out of memory taking the snapshot ({} MB wanted in all); try a smaller max_mb", total >> 20));
            }
            snap.resize(size, 0);
            bits.resize(words, u64::MAX);
            if slots % 64 != 0 {
                bits[words - 1] = (1u64 << (slots % 64)) - 1;
            }
            let mut off = 0;
            while off < size {
                let n = CHUNK.min(size - off);
                read_chunk(src, info.base + off as u64, &mut snap[off..off + n], &mut |a, b| {
                    clear_slots(&mut bits, (off + a) / width, (off + b).div_ceil(width));
                });
                off += n;
            }
            let live = popcount(&bits);
            dense.push(Dense { info, snap, bits, live });
        }
        self.width = width;
        self.store = Store::Dense(dense);
        let candidates = self.candidates();
        Ok(StartInfo { regions: self.region_count(), bytes: total, candidates, held_bytes: self.held_bytes(), ms: ms_since(t) })
    }

    fn region_count(&self) -> usize {
        match &self.store {
            Store::Empty => 0,
            Store::Dense(rs) => rs.len(),
            Store::Sparse { regions, .. } => regions.len(),
        }
    }

    /// Compares memory now with the snapshot and keeps the candidates that
    /// match `mode`; the survivors' snapshot values are refreshed.
    pub fn filter(&mut self, src: &dyn Source, mode: Mode, value: Option<u64>, signed: bool) -> Result<FilterInfo, String> {
        let t = Instant::now();
        let width = self.width;
        let value = match (mode.needs_value(), value) {
            (true, None) => return Err("this mode needs a value".into()),
            (_, v) => v.unwrap_or(0) & mask(width),
        };
        let before = self.candidates();
        let mut unreadable = 0u64;
        match &mut self.store {
            Store::Empty => return Err("no scan is running: call mem_scan_start first".into()),
            Store::Dense(regions) => {
                let mut cur = Vec::new();
                if cur.try_reserve_exact(CHUNK).is_err() {
                    return Err("out of memory for the read buffer".into());
                }
                cur.resize(CHUNK, 0u8);
                for r in regions.iter_mut() {
                    if r.live == 0 {
                        continue;
                    }
                    let size = r.snap.len();
                    let mut off = 0;
                    while off < size {
                        let n = CHUNK.min(size - off);
                        let mut bad: Vec<(usize, usize)> = Vec::new();
                        read_chunk(src, r.info.base + off as u64, &mut cur[..n], &mut |a, b| bad.push((a, b)));
                        for (a, b) in bad {
                            let (from, to) = ((off + a) / width, (off + b).div_ceil(width));
                            unreadable += popcount_range(&r.bits, from, to);
                            clear_slots(&mut r.bits, from, to);
                        }
                        filter_chunk(&mut r.bits, &mut r.snap, &cur[..n], off, width, mode, value, signed);
                        off += n;
                    }
                    r.live = popcount(&r.bits);
                }
            }
            Store::Sparse { hits, .. } => {
                let mut cache = PageCache { base: 0, valid: false, ok: false, buf: vec![0u8; PAGE] };
                hits.retain_mut(|h| match read_value(src, &mut cache, h.0, width) {
                    None => {
                        unreadable += 1;
                        false
                    }
                    Some(new) => {
                        let keep = keeps(mode, h.1, new, value, width, signed);
                        if keep {
                            h.1 = new;
                        }
                        keep
                    }
                });
            }
        }
        self.compact();
        Ok(FilterInfo {
            before,
            survivors: self.candidates(),
            unreadable,
            sparse: matches!(self.store, Store::Sparse { .. }),
            held_bytes: self.held_bytes(),
            ms: ms_since(t),
        })
    }

    /// Drops the snapshot for a list of survivors once the list is smaller.
    fn compact(&mut self) {
        let Store::Dense(regions) = &mut self.store else { return };
        let live: u64 = regions.iter().map(|r| r.live).sum();
        let held: usize = regions.iter().map(|r| r.snap.len()).sum();
        if (live as usize).saturating_mul(SPARSE_ENTRY) >= held {
            return;
        }
        let width = self.width;
        let mut hits: Vec<(u64, u64)> = Vec::new();
        if hits.try_reserve_exact(live as usize).is_err() {
            return;
        }
        for r in regions.iter() {
            for (wi, &word) in r.bits.iter().enumerate() {
                let mut m = word;
                while m != 0 {
                    let slot = wi * 64 + m.trailing_zeros() as usize;
                    m &= m - 1;
                    let o = slot * width;
                    hits.push((r.info.base + o as u64, le(&r.snap[o..o + width])));
                }
            }
        }
        let infos = regions.iter().map(|r| r.info.clone()).collect();
        self.store = Store::Sparse { regions: infos, hits };
    }

    /// The first `limit` candidates by address, with their value now.
    pub fn list(&self, src: &dyn Source, limit: usize) -> Vec<Hit> {
        let width = self.width;
        let now = |addr: u64, was: u64| {
            let mut b = [0u8; 8];
            if src.read(addr, &mut b[..width]) { le(&b[..width]) } else { was }
        };
        let mut out = Vec::new();
        match &self.store {
            Store::Empty => {}
            Store::Dense(regions) => {
                'all: for r in regions {
                    for (wi, &word) in r.bits.iter().enumerate() {
                        let mut m = word;
                        while m != 0 {
                            if out.len() >= limit {
                                break 'all;
                            }
                            let slot = wi * 64 + m.trailing_zeros() as usize;
                            m &= m - 1;
                            let o = slot * width;
                            let addr = r.info.base + o as u64;
                            out.push(Hit { address: addr, value: now(addr, le(&r.snap[o..o + width])), region: r.info.clone() });
                        }
                    }
                }
            }
            Store::Sparse { regions, hits } => {
                for &(addr, was) in hits.iter().take(limit) {
                    let i = regions.partition_point(|r| r.base <= addr).saturating_sub(1);
                    out.push(Hit { address: addr, value: now(addr, was), region: regions[i].clone() });
                }
            }
        }
        out
    }
}

impl Default for Scanner {
    fn default() -> Self {
        Scanner::new()
    }
}

fn ms_since(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn popcount_range(bits: &[u64], from: usize, to: usize) -> u64 {
    (from..to.min(bits.len() * 64)).filter(|s| bits[s / 64] >> (s % 64) & 1 != 0).count() as u64
}

/// One chunk of a dense filter: `cur` is the memory now at `off` of the
/// region, `snap` the region's whole snapshot.
#[allow(clippy::too_many_arguments)]
fn filter_chunk(bits: &mut [u64], snap: &mut [u8], cur: &[u8], off: usize, width: usize, mode: Mode, value: u64, signed: bool) {
    let n = cur.len();
    let (s0, s1) = (off / width, (off + n) / width);
    for wi in s0 / 64..s1.div_ceil(64) {
        let mut word = bits[wi];
        if word == 0 {
            continue;
        }
        let first = wi * 64;
        // A whole word of candidates that is unchanged as a block.
        if word == u64::MAX && first + 64 <= s1 && matches!(mode, Mode::Unchanged | Mode::Changed) {
            let a = first * width - off;
            let b = a + 64 * width;
            if cur[a..b] == snap[off + a..off + b] {
                if mode == Mode::Changed {
                    bits[wi] = 0;
                }
                continue;
            }
        }
        let mut m = word;
        while m != 0 {
            let bit = m.trailing_zeros() as usize;
            m &= m - 1;
            let o = (first + bit) * width - off;
            let old = le(&snap[off + o..off + o + width]);
            let new = le(&cur[o..o + width]);
            if keeps(mode, old, new, value, width, signed) {
                if new != old {
                    snap[off + o..off + o + width].copy_from_slice(&cur[o..o + width]);
                }
            } else {
                word &= !(1u64 << bit);
            }
        }
        bits[wi] = word;
    }
}

struct PageCache {
    base: u64,
    valid: bool,
    ok: bool,
    buf: Vec<u8>,
}

/// The value of `width` bytes at `addr`, reading the page once for the
/// candidates that follow in it.
fn read_value(src: &dyn Source, cache: &mut PageCache, addr: u64, width: usize) -> Option<u64> {
    let page = addr & !(PAGE as u64 - 1);
    if !cache.valid || cache.base != page {
        cache.valid = true;
        cache.base = page;
        cache.ok = src.read(page, &mut cache.buf);
    }
    let o = (addr - page) as usize;
    if cache.ok && o + width <= PAGE {
        return Some(le(&cache.buf[o..o + width]));
    }
    let mut b = [0u8; 8];
    src.read(addr, &mut b[..width]).then(|| le(&b[..width]))
}

/// Reads up to `len` bytes at `addr`, as far as pages can be read (the
/// bytes before the first page that cannot).
pub fn read_prefix(src: &dyn Source, addr: u64, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let mut at = 0;
    while at < len {
        let a = addr + at as u64;
        let n = (PAGE - (a as usize % PAGE)).min(len - at);
        if !src.read(a, &mut out[at..at + n]) {
            break;
        }
        at += n;
    }
    out.truncate(at);
    out
}

/// A classic hex dump, 16 bytes a line.
pub fn hex_dump(base: u64, bytes: &[u8]) -> String {
    let mut s = String::new();
    for (i, line) in bytes.chunks(16).enumerate() {
        s.push_str(&format!("{:#018x}  ", base + (i * 16) as u64));
        for j in 0..16 {
            match line.get(j) {
                Some(b) => s.push_str(&format!("{b:02x} ")),
                None => s.push_str("   "),
            }
        }
        s.push(' ');
        s.extend(line.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }));
        s.push('\n');
    }
    s
}

/// Windows protection constant as letters (R read, W write, X execute,
/// C copy-on-write).
pub fn protect_name(p: u32) -> String {
    match p & 0xff {
        0x02 => "R",
        0x04 => "RW",
        0x08 => "RWC",
        0x10 => "X",
        0x20 => "RX",
        0x40 => "RWX",
        0x80 => "RWXC",
        _ => return format!("{p:#x}"),
    }
    .to_owned()
}

pub fn hex(v: u64) -> Json {
    Json::str(format!("{v:#x}"))
}

pub fn region_json(r: &MemRegion) -> Json {
    Json::obj([
        ("base", hex(r.base)),
        ("end", hex(r.base + r.size)),
        ("size", Json::uint(r.size)),
        ("protect", Json::str(protect_name(r.protect))),
        ("kind", Json::str(r.kind.as_str())),
    ])
}

/// A candidate with where it lies: its region and offset in it, and for an
/// address in the title's image the offset from the image base (what
/// `VICTORY_VEHICLE_FLAG` takes as `img+<offset>`).
pub fn hit_json(h: &Hit, width: usize, image_base: u64) -> Json {
    let offset = h.address - h.region.base;
    let static_address = h.region.is_image() && image_base != 0 && h.address >= image_base;
    Json::obj([
        ("address", hex(h.address)),
        ("value", Json::uint(h.value)),
        ("value_hex", Json::str(format!("{:#0w$x}", h.value, w = 2 + 2 * width))),
        (
            "region",
            Json::obj([("base", hex(h.region.base)), ("size", Json::uint(h.region.size)), ("kind", Json::str(h.region.kind.as_str())), ("offset", hex(offset))]),
        ),
        ("static", Json::Bool(static_address)),
        ("image_offset", if static_address { Json::str(format!("img+{:x}", h.address - image_base)) } else { Json::Null }),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Fake memory: segments by base address; a read that is not wholly
    /// inside one segment fails.
    struct Fake(RefCell<Vec<(u64, Vec<u8>)>>);

    impl Source for Fake {
        fn read(&self, addr: u64, out: &mut [u8]) -> bool {
            let segs = self.0.borrow();
            for (base, data) in segs.iter() {
                if addr >= *base && addr + out.len() as u64 <= base + data.len() as u64 {
                    let o = (addr - base) as usize;
                    out.copy_from_slice(&data[o..o + out.len()]);
                    return true;
                }
            }
            false
        }
    }

    impl Fake {
        fn one(base: u64, data: Vec<u8>) -> Fake {
            Fake(RefCell::new(vec![(base, data)]))
        }
        fn poke(&self, addr: u64, bytes: &[u8]) {
            for (base, data) in self.0.borrow_mut().iter_mut() {
                if addr >= *base && addr < *base + data.len() as u64 {
                    let o = (addr - *base) as usize;
                    data[o..o + bytes.len()].copy_from_slice(bytes);
                }
            }
        }
    }

    fn region(base: u64, size: u64) -> MemRegion {
        MemRegion { base, size, protect: 4, kind: "xmem".into() }
    }

    const BASE: u64 = 0x1_0000_0000;

    fn addrs(s: &Scanner, src: &dyn Source) -> Vec<u64> {
        s.list(src, usize::MAX).iter().map(|h| h.address).collect()
    }

    #[test]
    fn modes_compare_old_and_new() {
        assert!(keeps(Mode::Changed, 1, 2, 0, 1, false) && !keeps(Mode::Changed, 2, 2, 0, 1, false));
        assert!(keeps(Mode::Unchanged, 2, 2, 0, 1, false) && !keeps(Mode::Unchanged, 1, 2, 0, 1, false));
        assert!(keeps(Mode::Increased, 1, 2, 0, 1, false) && !keeps(Mode::Increased, 2, 2, 0, 1, false));
        assert!(keeps(Mode::Decreased, 3, 2, 0, 1, false) && !keeps(Mode::Decreased, 2, 3, 0, 1, false));
        assert!(keeps(Mode::Equals, 9, 7, 7, 1, false) && !keeps(Mode::Equals, 7, 8, 7, 1, false));
        assert!(keeps(Mode::NotEquals, 7, 8, 7, 1, false) && !keeps(Mode::NotEquals, 8, 7, 7, 1, false));
        // 0xff is 255 unsigned and -1 signed.
        assert!(keeps(Mode::Increased, 1, 0xff, 0, 1, false));
        assert!(!keeps(Mode::Increased, 1, 0xff, 0, 1, true));
        assert!(keeps(Mode::Decreased, 1, 0xffff_ffff, 0, 4, true));
        assert_eq!(mask(1), 0xff);
        assert_eq!(mask(4), 0xffff_ffff);
    }

    #[test]
    fn numbers_parse_as_hex_or_decimal() {
        assert_eq!(parse_u64("0x10"), Some(16));
        assert_eq!(parse_u64(" 10 "), Some(10));
        assert_eq!(parse_u64("0Xff"), Some(255));
        assert_eq!(parse_u64("zz"), None);
        assert_eq!(parse_u64(""), None);
    }

    #[test]
    fn unchanged_then_changed_then_equals_narrow_to_the_flag() {
        // 200 bytes of zeros; byte 77 is the "flag".
        let mem = Fake::one(BASE, vec![0; 200]);
        let mut s = Scanner::new();
        let info = s.start(&mem, vec![region(BASE, 200)], 1, 1 << 20).unwrap();
        assert_eq!((info.regions, info.bytes, info.candidates), (1, 200, 200));
        // Nothing changed: all stay.
        let f = s.filter(&mem, Mode::Unchanged, None, false).unwrap();
        assert_eq!((f.before, f.survivors), (200, 200));
        // The flag and byte 150 change.
        mem.poke(BASE + 77, &[1]);
        mem.poke(BASE + 150, &[9]);
        let f = s.filter(&mem, Mode::Changed, None, false).unwrap();
        assert_eq!(f.survivors, 2);
        assert_eq!(addrs(&s, &mem), vec![BASE + 77, BASE + 150]);
        // The flag goes back to 0 (a decrease); 150 stays 9.
        mem.poke(BASE + 77, &[0]);
        let f = s.filter(&mem, Mode::Decreased, None, false).unwrap();
        assert_eq!(addrs(&s, &mem), vec![BASE + 77]);
        assert_eq!(f.survivors, 1);
        let f = s.filter(&mem, Mode::Equals, Some(0), false).unwrap();
        assert_eq!(f.survivors, 1);
        let hits = s.list(&mem, 10);
        assert_eq!((hits[0].address, hits[0].value), (BASE + 77, 0));
        assert!(s.filter(&mem, Mode::Equals, None, false).is_err());
    }

    #[test]
    fn a_big_first_pass_becomes_a_list_and_keeps_working() {
        let mem = Fake::one(BASE, vec![0; 100_000]);
        let mut s = Scanner::new();
        s.start(&mem, vec![region(BASE, 100_000)], 1, 1 << 20).unwrap();
        let dense = s.held_bytes();
        assert!(dense >= 100_000);
        mem.poke(BASE + 5, &[1]);
        mem.poke(BASE + 99_999, &[2]);
        let f = s.filter(&mem, Mode::Changed, None, false).unwrap();
        assert!(f.sparse && f.survivors == 2);
        assert_eq!(f.held_bytes, 2 * SPARSE_ENTRY as u64);
        // The list keeps filtering against refreshed values.
        mem.poke(BASE + 5, &[3]);
        let f = s.filter(&mem, Mode::Increased, None, false).unwrap();
        assert_eq!(f.survivors, 1);
        assert_eq!(addrs(&s, &mem), vec![BASE + 5]);
        let f = s.filter(&mem, Mode::Unchanged, None, false).unwrap();
        assert_eq!(f.survivors, 1);
        s.reset();
        assert!(!s.active());
        assert!(s.filter(&mem, Mode::Changed, None, false).is_err());
    }

    #[test]
    fn width_four_scans_aligned_dwords_only() {
        let mut data = vec![0u8; 64];
        data[8..12].copy_from_slice(&7u32.to_le_bytes());
        let mem = Fake::one(BASE, data);
        let mut s = Scanner::new();
        let info = s.start(&mem, vec![region(BASE, 64)], 4, 1 << 20).unwrap();
        assert_eq!(info.candidates, 16);
        let f = s.filter(&mem, Mode::Equals, Some(7), false).unwrap();
        assert_eq!(f.survivors, 1);
        let hits = s.list(&mem, 5);
        assert_eq!((hits[0].address, hits[0].value), (BASE + 8, 7));
        mem.poke(BASE + 8, &9u32.to_le_bytes());
        let f = s.filter(&mem, Mode::Increased, None, false).unwrap();
        assert_eq!(f.survivors, 1);
        // A size that is no multiple of the width loses the tail.
        let mut t = Scanner::new();
        let info = t.start(&mem, vec![region(BASE, 62)], 4, 1 << 20).unwrap();
        assert_eq!((info.bytes, info.candidates), (60, 15));
    }

    #[test]
    fn unreadable_pages_are_skipped_at_the_start_and_dropped_later() {
        // Two pages, then a hole, then one page.
        let mem = Fake(RefCell::new(vec![(BASE, vec![0; 2 * PAGE]), (BASE + 3 * PAGE as u64, vec![0; PAGE])]));
        let mut s = Scanner::new();
        let info = s.start(&mem, vec![region(BASE, 4 * PAGE as u64)], 1, 1 << 20).unwrap();
        assert_eq!(info.candidates, 3 * PAGE as u64);
        // The last page goes away.
        mem.0.borrow_mut().pop();
        let f = s.filter(&mem, Mode::Unchanged, None, false).unwrap();
        assert_eq!(f.survivors, 2 * PAGE as u64);
        assert_eq!(f.unreadable, PAGE as u64);
    }

    #[test]
    fn a_scan_above_the_limit_is_refused_before_reading() {
        let mem = Fake::one(BASE, vec![0; 3 << 20]);
        let mut s = Scanner::new();
        let err = s.start(&mem, vec![region(BASE, 3 << 20)], 1, 2 << 20).unwrap_err();
        assert!(err.contains("3 MB") && err.contains("2 MB") && err.contains("max_mb"), "{err}");
        assert!(!s.active());
        assert!(s.start(&mem, vec![], 1, 1 << 20).is_err());
        assert!(s.start(&mem, vec![region(BASE, 16)], 3, 1 << 20).is_err());
    }

    #[test]
    fn regions_across_chunks_and_a_list_tell_where_each_hit_is() {
        let size = CHUNK + 4096;
        let mem = Fake::one(BASE, vec![0; size]);
        let image = MemRegion { base: BASE, size: size as u64, protect: 4, kind: "image .data".into() };
        let mut s = Scanner::new();
        s.start(&mem, vec![image], 1, 64 << 20).unwrap();
        mem.poke(BASE + CHUNK as u64 + 10, &[1]);
        s.filter(&mem, Mode::Changed, None, false).unwrap();
        let hits = s.list(&mem, 5);
        assert_eq!(hits.len(), 1);
        let j = hit_json(&hits[0], 1, BASE - 0x1000);
        assert_eq!(j.get("image_offset").and_then(Json::as_str), Some(format!("img+{:x}", CHUNK + 10 + 0x1000).as_str()));
        assert_eq!(j.get("static"), Some(&Json::Bool(true)));
        let heap = Hit { address: BASE + 4, value: 1, region: region(BASE, 16) };
        let j = hit_json(&heap, 1, BASE - 0x1000);
        assert_eq!(j.get("image_offset"), Some(&Json::Null));
        assert_eq!(j.get("region").and_then(|r| r.get("offset")).and_then(Json::as_str), Some("0x4"));
    }

    #[test]
    fn reads_stop_at_the_first_missing_page_and_dump_as_hex() {
        let mem = Fake::one(BASE, (0..=255u8).cycle().take(PAGE + 8).collect());
        let got = read_prefix(&mem, BASE + 4090, 100);
        // The first piece (to the page end) reads; the next one runs past the end of the fake.
        assert_eq!(got.len(), 6);
        let got = read_prefix(&mem, BASE, 20);
        assert_eq!(got.len(), 20);
        assert!(read_prefix(&mem, BASE - 8, 4).is_empty());
        let text = hex_dump(BASE, &got);
        assert!(text.starts_with("0x0000000100000000  00 01 02 03"));
        assert_eq!(text.lines().count(), 2);
    }
}
