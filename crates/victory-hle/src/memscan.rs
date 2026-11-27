//! Which memory is the title's, for the debug server's memory tools
//! (`mem_regions`, `mem_scan_*`, `mem_read`) and the vehicle flag of the
//! controls layer.
//!
//! The title's memory lies in our process among our own. The runtime tells
//! what it hands the title: every reservation `XMemVirtualAlloc` makes is
//! noted here ([`note_allocated`]) and every release dropped
//! ([`note_released`]), and the writable sections of the title's image are
//! read from its headers. The ranges the tools list are those places,
//! looked at with `VirtualQuery`: only what is committed and readable and
//! writable now (plus read-only mapped views, which the GPU side leaves
//! read-only for a while after it copies them). Nothing of ours is in it.
//!
//! Reads go through `victory_gpu::read_title_memory`, which never faults.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::Mutex;

use victory_mcp::{MemHooks, MemRegion};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_MAPPED, MEMORY_BASIC_INFORMATION, PAGE_GUARD, VirtualQuery,
};

/// What the title allocated: start to end, not merged, by start.
static ALLOCATED: Mutex<BTreeMap<usize, usize>> = Mutex::new(BTreeMap::new());

fn allocated() -> std::sync::MutexGuard<'static, BTreeMap<usize, usize>> {
    ALLOCATED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Notes `[at, at + size)` as the title's. Memory committed inside a
/// reservation already noted changes nothing.
fn insert(map: &mut BTreeMap<usize, usize>, at: usize, size: usize) {
    let end = at.saturating_add(size);
    if size == 0 {
        return;
    }
    if let Some((_, e)) = map.range_mut(..=at).next_back()
        && *e > at
    {
        *e = (*e).max(end);
        return;
    }
    let inside: Vec<(usize, usize)> = map.range(at..end).map(|(&k, &v)| (k, v)).collect();
    let mut last = end;
    for (k, v) in inside {
        map.remove(&k);
        last = last.max(v);
    }
    map.insert(at, last);
}

/// Drops `[at, at + size)` from the noted ranges; with `size` 0, the whole
/// range that starts at `at`.
fn remove(map: &mut BTreeMap<usize, usize>, at: usize, size: usize) {
    if size == 0 {
        map.remove(&at);
        return;
    }
    let end = at.saturating_add(size);
    let mut hit: Vec<(usize, usize)> = Vec::new();
    if let Some((&s, &e)) = map.range(..at).next_back()
        && e > at
    {
        hit.push((s, e));
    }
    hit.extend(map.range(at..end).map(|(&k, &v)| (k, v)));
    for (s, e) in hit {
        map.remove(&s);
        if s < at {
            map.insert(s, at);
        }
        if e > end {
            map.insert(end, e);
        }
    }
}

/// The runtime gave the title memory at `[at, at + size)`.
pub(crate) fn note_allocated(at: usize, size: usize) {
    insert(&mut allocated(), at, size);
}

/// The title released memory at `at` (size 0: the whole reservation).
pub(crate) fn note_released(at: usize, size: usize) {
    remove(&mut allocated(), at, size);
}

/// Whether pages with this protection (and type) belong in a scan: not
/// guard or no-access pages, and writable ones; read-only ones only when
/// mapped (a view the GPU side guarded).
fn scannable(protect: u32, kind: u32) -> bool {
    if protect & PAGE_GUARD != 0 {
        return false;
    }
    match protect & 0xff {
        0x04 | 0x08 | 0x40 | 0x80 => true,
        0x02 => kind == MEM_MAPPED,
        _ => false,
    }
}

/// The writable sections of a PE image from its first page: name, offset
/// from the image base and size.
fn writable_sections(headers: &[u8]) -> Vec<(String, usize, usize)> {
    const WRITE: u32 = 0x8000_0000;
    let u16_at = |o: usize| headers.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| headers.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let mut out = Vec::new();
    let Some(nt) = u32_at(0x3c).map(|v| v as usize) else { return out };
    if u32_at(nt) != Some(0x0000_4550) {
        return out;
    }
    let (Some(count), Some(optional)) = (u16_at(nt + 6), u16_at(nt + 20)) else { return out };
    let table = nt + 24 + optional as usize;
    for i in 0..count as usize {
        let at = table + i * 40;
        let (Some(name), Some(size), Some(rva), Some(flags)) = (headers.get(at..at + 8), u32_at(at + 8), u32_at(at + 12), u32_at(at + 36)) else {
            break;
        };
        if flags & WRITE != 0 && size != 0 {
            let name = String::from_utf8_lossy(name).trim_end_matches('\0').to_owned();
            out.push((name, rva as usize, size as usize));
        }
    }
    out
}

/// The committed, scannable parts of `[start, end)`, added to `out`.
fn walk(start: usize, end: usize, kind: &str, out: &mut Vec<MemRegion>) {
    let mut at = start;
    while at < end {
        // SAFETY: a plain query of the address space into a structure of ours.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let n = unsafe { VirtualQuery(at as *const c_void, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
        if n == 0 {
            break;
        }
        let from = info.BaseAddress as usize;
        let to = from.saturating_add(info.RegionSize);
        if info.State == MEM_COMMIT && scannable(info.Protect, info.Type) {
            let (s, e) = (at.max(from), end.min(to));
            match out.last_mut() {
                Some(last) if last.kind == kind && last.protect == info.Protect && last.base + last.size == s as u64 => {
                    last.size += (e - s) as u64;
                }
                _ => out.push(MemRegion { base: s as u64, size: (e - s) as u64, protect: info.Protect, kind: kind.to_owned() }),
            }
        }
        if to <= at {
            break;
        }
        at = to;
    }
}

/// The title's committed writable memory now, by address.
fn title_regions() -> Vec<MemRegion> {
    let mut places: Vec<(usize, usize, String)> = Vec::new();
    if let Some(base) = crate::runtime::main_image() {
        let mut headers = vec![0u8; 4096];
        if victory_gpu::read_title_memory(base, &mut headers) {
            for (name, rva, size) in writable_sections(&headers) {
                places.push((base + rva, base + rva + size, format!("image {name}")));
            }
        }
    }
    places.extend(allocated().iter().map(|(&s, &e)| (s, e, "xmem".to_owned())));
    places.sort_by_key(|p| p.0);
    let mut out = Vec::new();
    for (s, e, kind) in places {
        walk(s, e, &kind, &mut out);
    }
    out.sort_by_key(|r| r.base);
    out
}

fn read(addr: u64, out: &mut [u8]) -> bool {
    victory_gpu::read_title_memory(addr as usize, out)
}

fn image_base() -> u64 {
    crate::runtime::main_image().unwrap_or(0) as u64
}

/// Gives the debug server the memory tools' functions. Once is enough.
pub(crate) fn init() {
    victory_mcp::set_mem_hooks(MemHooks { regions: title_regions, read, image_base });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(map: &BTreeMap<usize, usize>) -> Vec<(usize, usize)> {
        map.iter().map(|(&s, &e)| (s, e)).collect()
    }

    #[test]
    fn allocations_are_kept_and_released_whole_or_in_part() {
        let mut m = BTreeMap::new();
        insert(&mut m, 0x1000, 0x4000);
        insert(&mut m, 0x10000, 0x1000);
        assert_eq!(ranges(&m), [(0x1000, 0x5000), (0x10000, 0x11000)]);
        // Committed inside a reservation: nothing new; past its end: it grows.
        insert(&mut m, 0x2000, 0x1000);
        assert_eq!(ranges(&m), [(0x1000, 0x5000), (0x10000, 0x11000)]);
        insert(&mut m, 0x4000, 0x3000);
        assert_eq!(ranges(&m), [(0x1000, 0x7000), (0x10000, 0x11000)]);
        // One that covers a later one takes it in.
        insert(&mut m, 0xf000, 0x3000);
        assert_eq!(ranges(&m), [(0x1000, 0x7000), (0xf000, 0x12000)]);
        // A part released splits the range; a whole release drops the start.
        remove(&mut m, 0x3000, 0x1000);
        assert_eq!(ranges(&m), [(0x1000, 0x3000), (0x4000, 0x7000), (0xf000, 0x12000)]);
        remove(&mut m, 0xf000, 0);
        assert_eq!(ranges(&m), [(0x1000, 0x3000), (0x4000, 0x7000)]);
        remove(&mut m, 0x0, 0x5000);
        assert_eq!(ranges(&m), [(0x5000, 0x7000)]);
        remove(&mut m, 0x6000, 0x10000);
        assert_eq!(ranges(&m), [(0x5000, 0x6000)]);
        insert(&mut m, 0x9000, 0);
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn writable_pages_are_scanned_and_guarded_ones_are_not() {
        assert!(scannable(0x04, 0x20000) && scannable(0x40, 0x20000) && scannable(0x08, 0x20000));
        assert!(!scannable(0x02, 0x20000), "read-only private memory is not");
        assert!(scannable(0x02, MEM_MAPPED), "a read-only mapped view is");
        assert!(!scannable(0x04 | PAGE_GUARD, 0x20000));
        assert!(!scannable(0x01, 0x20000) && !scannable(0x20, 0x20000) && !scannable(0, 0));
    }

    #[test]
    fn writable_sections_come_from_the_headers() {
        let mut h = vec![0u8; 0x400];
        let nt = 0x80usize;
        h[0x3c..0x40].copy_from_slice(&(nt as u32).to_le_bytes());
        h[nt..nt + 4].copy_from_slice(b"PE\0\0");
        h[nt + 6..nt + 8].copy_from_slice(&3u16.to_le_bytes());
        h[nt + 20..nt + 22].copy_from_slice(&0xf0u16.to_le_bytes());
        let table = nt + 24 + 0xf0;
        for (i, (name, rva, size, flags)) in [(".text", 0x1000u32, 0x5000u32, 0x6000_0020u32), (".data", 0x8000, 0x2000, 0xc000_0040), (".bss", 0xb000, 0, 0xc000_0080)].iter().enumerate() {
            let at = table + i * 40;
            h[at..at + name.len()].copy_from_slice(name.as_bytes());
            h[at + 8..at + 12].copy_from_slice(&size.to_le_bytes());
            h[at + 12..at + 16].copy_from_slice(&rva.to_le_bytes());
            h[at + 36..at + 40].copy_from_slice(&flags.to_le_bytes());
        }
        assert_eq!(writable_sections(&h), [(".data".to_owned(), 0x8000, 0x2000)]);
        assert!(writable_sections(&[0u8; 16]).is_empty());
        h[nt] = b'X';
        assert!(writable_sections(&h).is_empty());
    }

    #[test]
    fn our_own_memory_is_walked_only_where_asked() {
        // A page of ours, committed read-write: found when it is the range asked about.
        let page = vec![1u8; 4096 * 4];
        let start = page.as_ptr() as usize & !0xfff;
        let mut found = Vec::new();
        walk(start, start + 4096, "test", &mut found);
        assert_eq!(found.len(), 1);
        assert!(found[0].base as usize >= start && found[0].kind == "test");
        let mut none = Vec::new();
        walk(0x10, 0x20, "test", &mut none);
        assert!(none.is_empty());
    }
}
