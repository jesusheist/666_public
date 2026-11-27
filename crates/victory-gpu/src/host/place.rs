//! The title's buffer copies, done on the card at their place in the list
//! (`VICTORY_COPIES_IN_PLACE`, on by default; `0`: all at submission, on the
//! CPU, as before).
//!
//! A buffer copy was done in title memory when its list was submitted, before
//! any draw of the submission ran, and the host copies refreshed after it.
//! That is another order than the list's: draws ahead of the copy read what
//! it put; of two copies to the same bytes with a draw behind each, both
//! draws read the second; a source that a dispatch of the same submission
//! writes first was read as it was before it; and a destination the
//! submissions in the air still read changed under them (where its host copy
//! has one side only, `crate::shadow::card_all`).
//!
//! Now the list copies, in the destination's host copy: out of the source's
//! host copy where shaders write that one, else out of upload memory of the
//! list that takes the source's bytes when the list is submitted (a source
//! shaders never wrote is the title's CPU's, or a copy's put there earlier in
//! the submission). What is written there is a shader's write from then on:
//! the bytes are declared, and what the card made of them is put into title
//! memory before the title is told of the submission.
//!
//! Title memory takes the copy at submission as well, as it did: fills of
//! upload memory, textures and host copies that lie over the same bytes read
//! it there. The reference of the destination's host copy takes the same
//! bytes ([`Shadow::placed_copy`]), so what the landing finds is what the card
//! made of them afterwards (a counter copied to zero and counted up; a source
//! that a shader wrote in between), and nothing is sent up over it. Where
//! flights in the air may write the source or the destination this is left to
//! the landing: what they wrote is not in title memory yet, and what they
//! will put there must not stand over this copy's bytes. If a step of the
//! submission reads the destination on the CPU, it waits for them instead
//! ([`Placed::waits`]).
//!
//! Copies to memory that has no host copy yet are done as before: nothing on
//! the card has read or written it. So are those of more than 8 MiB out of
//! memory shaders do not write (streaming).

use super::*;
use crate::shadow::Shadow;

pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_COPIES_IN_PLACE").is_none_or(|v| v != "0"))
}

/// The most bytes copied out of memory shaders do not write.
const MOST: usize = 8 << 20;

/// Since the last frame-rate line: copies at their place, their bytes, those
/// out of a host copy, and texture copies out of one.
pub(super) static STATS: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];

/// What the CPU does at submission for a copy at its place in a list.
pub(super) struct Placed {
    pub(super) dst: usize,
    pub(super) src: usize,
    pub(super) len: usize,
    /// The destination's host copy.
    pub(super) shadow: std::sync::Arc<Shadow>,
    /// Upload memory the list copies from, to be given the source's bytes.
    from: Option<usize>,
}

impl Placed {
    pub(super) fn gpu_sourced(&self) -> bool {
        self.from.is_none()
    }

    /// Whether flights that write `written` are waited for.
    pub(super) fn waits(&self, written: &[(usize, usize)]) -> Waits {
        let source = touches(written, self.src, self.src + self.len);
        if source && self.from.is_some() {
            return Waits::Yes;
        }
        match source || touches(written, self.dst, self.dst + self.len) {
            true => Waits::ForReaders,
            false => Waits::No,
        }
    }

    /// At submission, in the order of the steps: the list's upload memory
    /// takes the source's bytes and, if `free` (no flight in the air may write
    /// source or destination), title memory at the destination does. Gives
    /// the bytes if it took them.
    pub(super) fn prepare(&self, free: bool) -> Option<Vec<u8>> {
        if !free && self.from.is_none() {
            return None;
        }
        let mut bytes = vec![0u8; self.len];
        // (What is not there reads as zeros, as any copy's source.)
        copy_mapped(bytes.as_mut_ptr(), self.src as *const u8, self.len, false);
        if let Some(to) = self.from {
            // SAFETY: the list's own upload memory of that many bytes.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), to as *mut u8, self.len) };
        }
        if !free {
            return None;
        }
        copy_mapped(self.dst as *mut u8, bytes.as_ptr(), self.len, true);
        crate::watch::wrote(self.dst, self.len);
        // Title memory that is not all there was left alone.
        let mut there = vec![0u8; self.len];
        (read_into(self.dst as *const u8, &mut there) && there == bytes).then_some(bytes)
    }
}

/// Whether a copy waits for flights before its submission is prepared: not;
/// yes, as it takes a source on the CPU that they may write; or only if
/// another step of the submission reads its destination on the CPU.
pub(super) enum Waits {
    No,
    ForReaders,
    Yes,
}

/// The first step that reads a recorded copy's destination on the CPU.
pub(super) fn cpu_reader(
    runs: &[&HostList],
    written: &[(usize, usize)],
    mut ranges: Vec<(usize, usize)>,
    taken: impl Fn(&HostList, usize) -> bool,
) -> Option<&'static str> {
    join_ranges(&mut ranges, []);
    let ordered = crate::toggled!("VICTORY_ORDER_PLACED_READS");
    let mut earlier = Vec::new();
    #[cfg(feature = "diagnostics")]
    let mut upload = None;
    let found = runs.iter().enumerate().find_map(|(_list_at, list)| {
        list.deferred().copies.iter().enumerate().find_map(|(i, step)| {
            if taken(list, i) {
                return None;
            }
            match step {
                &Step::Copy(_, src, len) => touches(&ranges, src, src + len).then_some("a CPU buffer copy reads a recorded copy's destination"),
                Step::Fill(_, src, len, _target) if touches(if ordered { &earlier } else { &ranges }, *src, *src + *len) => {
                    #[cfg(feature = "diagnostics")]
                    if crate::asked_for!("VICTORY_PLACED_UPLOAD_TRACE") {
                        let target = match _target {
                            Some(card_steps::FillTo::Slot(..)) => "constant descriptor",
                            Some(card_steps::FillTo::Buffer(_)) => "staging buffer",
                            None => "CPU-only upload",
                        };
                        upload = Some(((_list_at, i), *src, *len, target));
                    }
                    Some("a CPU upload reads a recorded copy's destination")
                }
                Step::Texture(staging) => {
                    let (at, len) = staging.memory();
                    touches(&ranges, at, at + len).then_some("a texture upload reads a recorded copy's destination")
                }
                &Step::Plane((at, len), _) =>
                    touches(&ranges, at, at.saturating_add(len)).then_some("a movie plane reads a recorded copy's destination"),
                Step::Host(_, (from, to)) => touches(&ranges, *from, *to).then_some("a CPU callback reads a recorded copy's destination"),
                Step::Placed(p) => {
                    if ordered && matches!(p.waits(written), Waits::ForReaders) {
                        join_ranges(&mut earlier, [(p.dst, p.dst + p.len)]);
                    }
                    (p.from.is_some() && touches(&ranges, p.src, p.src + p.len))
                        .then_some("a recorded copy stages the destination of another copy on the CPU")
                }
                _ => None,
            }
        })
    });
    #[cfg(feature = "diagnostics")]
    if let Some((at, src, len, target)) = upload {
        trace_upload(runs, written, at, src, len, target);
    }
    found
}

#[cfg(feature = "diagnostics")]
fn trace_upload(
    runs: &[&HostList],
    written: &[(usize, usize)],
    reader: (usize, usize),
    src: usize,
    len: usize,
    target: &'static str,
) {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    // Resample each shape occasionally so startup and warm gameplay can be
    // distinguished without logging every submission.
    type Shape = (bool, bool, bool, usize, usize, &'static str);
    static LOGGED: Mutex<Vec<(Shape, Instant)>> = Mutex::new(Vec::new());
    for (list_at, list) in runs.iter().enumerate() {
        for (i, step) in list.deferred().copies.iter().enumerate() {
            let Step::Placed(copy) = step else { continue };
            if src >= copy.dst + copy.len || copy.dst >= src + len || !matches!(copy.waits(written), Waits::ForReaders) {
                continue;
            }
            let after = reader > (list_at, i);
            let same_list = reader.0 == list_at;
            let cpu_source = copy.from.is_some();
            let covers_upload = copy.dst <= src && src + len <= copy.dst + copy.len;
            let shape = (after, same_list, cpu_source, len, copy.len, target);
            let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let room = logged.len() < 16;
            match logged.iter_mut().find(|(known, _)| *known == shape) {
                Some((_, last)) if now.duration_since(*last) >= Duration::from_secs(10) => *last = now,
                Some(_) => continue,
                None if room => logged.push((shape, now)),
                None => continue,
            }
            tracing::info!(after, same_list, cpu_source, target, covers_upload, bytes = len, copy_bytes = copy.len, "placed copy upload dependency");
        }
    }
}

/// Everything recorded so far is done, and seen, before what follows.
///
/// # Safety
/// An open list.
unsafe fn wait_for_all(host: &Host, list: &ID3D12GraphicsCommandList) {
    static ENHANCED: OnceLock<bool> = OnceLock::new();
    let enhanced = *ENHANCED.get_or_init(|| {
        let mut o = D3D12_FEATURE_DATA_D3D12_OPTIONS12::default();
        // SAFETY: a feature structure of its size.
        let known = unsafe {
            host.device.CheckFeatureSupport(
                D3D12_FEATURE_D3D12_OPTIONS12,
                (&raw mut o).cast(),
                size_of::<D3D12_FEATURE_DATA_D3D12_OPTIONS12>() as u32,
            )
        };
        known.is_ok() && o.EnhancedBarriersSupported.as_bool()
    });
    // SAFETY: the caller's.
    unsafe {
        if enhanced && let Ok(list) = list.cast::<ID3D12GraphicsCommandList7>() {
            let all = [D3D12_GLOBAL_BARRIER {
                SyncBefore: D3D12_BARRIER_SYNC_ALL,
                SyncAfter: D3D12_BARRIER_SYNC_ALL,
                AccessBefore: D3D12_BARRIER_ACCESS_COMMON,
                AccessAfter: D3D12_BARRIER_ACCESS_COMMON,
            }];
            list.Barrier(&[D3D12_BARRIER_GROUP {
                Type: D3D12_BARRIER_TYPE_GLOBAL,
                NumBarriers: 1,
                Anonymous: D3D12_BARRIER_GROUP_0 {
                    pGlobalBarriers: all.as_ptr(),
                },
            }]);
            return;
        }
        let alias = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_ALIASING,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Aliasing: std::mem::ManuallyDrop::new(D3D12_RESOURCE_ALIASING_BARRIER {
                    pResourceBefore: std::mem::ManuallyDrop::new(None),
                    pResourceAfter: std::mem::ManuallyDrop::new(None),
                }),
            },
        };
        let uav = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                    pResource: std::mem::ManuallyDrop::new(None),
                }),
            },
        };
        list.ResourceBarrier(&[alias, uav]);
    }
}

/// The host copy of `[src, src + len)` if shaders write it: what it holds at
/// a place in a list is not what title memory holds at submission.
fn shaders_own(src: usize, len: usize) -> Option<std::sync::Arc<Shadow>> {
    crate::shadow::peek(src as u64, len as u64).filter(|s| s.written() && !s.in_place())
}

impl HostList {
    /// Copies `len` bytes of title memory at `src` to `dst`, at this place of
    /// the list (see the file). `false` if it is to be done at submission.
    pub(crate) fn place_copy(&self, dst: usize, src: usize, len: usize) -> bool {
        if !on() {
            return false;
        }
        let Some(host) = host() else {
            return false;
        };
        let Some(to) = crate::shadow::peek(dst as u64, len as u64).filter(|s| !s.in_place()) else {
            return false;
        };
        let own = shaders_own(src, len);
        let scratch = match &own {
            Some(_) => None,
            None if len <= MOST => match self.scratch_at(len as u64) {
                Some(scratch) => Some(scratch),
                None => return false,
            },
            None => return false,
        };
        let at = to.offset(dst as u64);
        // What the title's CPU wrote around and under it so far goes first.
        to.refresh(self, dst as u64, len as u64);
        // SAFETY (both): an open list; live buffers (the list holds the
        // copies and its scratch memory), ranges inside them.
        unsafe { wait_for_all(host, &self.list) };
        match (&own, &scratch) {
            (Some(from), _) => {
                from.refresh(self, src as u64, len as u64);
                let offset = from.offset(src as u64);
                if from.buffer.as_raw() == to.buffer.as_raw() {
                    let Some(between) = self.between(host, len as u64) else {
                        return false;
                    };
                    unsafe {
                        self.list.CopyBufferRegion(&between, 0, &from.buffer, offset, len as u64);
                        self.list.ResourceBarrier(&[transition(&between, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
                        wait_for_all(host, &self.list);
                        self.list.CopyBufferRegion(&to.buffer, at, &between, 0, len as u64);
                    }
                } else {
                    unsafe { self.list.CopyBufferRegion(&to.buffer, at, &from.buffer, offset, len as u64) };
                }
                STATS[2].fetch_add(1, Ordering::Relaxed);
            }
            (None, Some((from, offset, _))) => unsafe {
                self.list.CopyBufferRegion(&to.buffer, at, from, *offset, len as u64);
            },
            (None, None) => return false,
        }
        unsafe { wait_for_all(host, &self.list) };
        STATS[0].fetch_add(1, Ordering::Relaxed);
        STATS[1].fetch_add(len as u64, Ordering::Relaxed);
        to.gpu_writes(self, dst as u64, len as u64);
        let mut deferred = self.deferred();
        deferred.copies.push(Step::Placed(Placed {
            dst,
            src,
            len,
            shadow: to,
            from: scratch.map(|s| s.2 as usize),
        }));
        deferred.title_written();
        deferred.last_placed = Some(deferred.copies.len() - 1);
        self.changed_something();
        true
    }

    /// Snapshots a constant buffer immediately after the GPU copy that
    /// produced it, in the same recording. Other constants keep the normal
    /// upload path. Only the exact range of the last title-memory copy is
    /// used, so a later CPU copy cannot be hidden by this snapshot.
    pub(crate) fn placed_constant(&self, address: u64, size: usize) -> Option<ID3D12Resource> {
        if size == 0 || size > 64 << 10 || !crate::toggled!("VICTORY_PLACED_CONSTANTS") {
            return None;
        }
        let src = usize::try_from(address).ok()?;
        let end = src.checked_add(size)?;
        let from = {
            let deferred = self.deferred();
            let at = deferred.last_placed?;
            let Step::Placed(copy) = &deferred.copies[at] else {
                return None;
            };
            if copy.dst != src || copy.len != size || copy.from.is_some() {
                return None;
            }
            if deferred.copies[at + 1..].iter().any(|step| {
                matches!(step, Step::GpuWrite(shadow, from, len)
                    if *from < end && src < from.saturating_add(*len)
                        && !std::sync::Arc::ptr_eq(shadow, &copy.shadow))
            }) {
                return None;
            }
            copy.shadow.clone()
        };
        let host = host()?;
        let buffer = self.between(host, size as u64)?;
        // SAFETY: the recording is open, the list keeps both buffers, and
        // the matching copy proved the source range lies in its shadow.
        unsafe {
            wait_for_all(host, &self.list);
            self.list.CopyBufferRegion(&buffer, 0, &from.buffer, from.offset(address), size as u64);
            self.list.ResourceBarrier(&[transition(
                &buffer,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
            )]);
        }
        #[cfg(feature = "diagnostics")]
        if crate::asked_for!("VICTORY_PLACED_UPLOAD_TRACE") {
            static RECORDED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if RECORDED.fetch_add(1, Ordering::Relaxed) < 8 {
                tracing::info!(bytes = size, "constant upload snapshot recorded after the placed GPU copy");
            }
        }
        Some(buffer)
    }

    /// For a copy of `[src, src + len)` into a texture: a buffer (readable by
    /// copies) that holds what shaders wrote there, at this place of the list.
    /// `None` where shaders do not write it.
    pub(crate) fn place_source(&self, src: usize, len: usize) -> Option<ID3D12Resource> {
        if !on() {
            return None;
        }
        let host = host()?;
        let from = shaders_own(src, len)?;
        let between = self.between(host, len as u64)?;
        from.refresh(self, src as u64, len as u64);
        STATS[3].fetch_add(1, Ordering::Relaxed);
        // SAFETY: an open list; live buffers, ranges inside them.
        unsafe {
            wait_for_all(host, &self.list);
            self.list.CopyBufferRegion(&between, 0, &from.buffer, from.offset(src as u64), len as u64);
            self.list.ResourceBarrier(&[transition(&between, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            wait_for_all(host, &self.list);
        }
        Some(between)
    }

    /// A buffer on the card for this recording alone.
    fn between(&self, host: &Host, len: u64) -> Option<ID3D12Resource> {
        let buffer = host.uav_buffer(len.next_multiple_of(1 << 16))?;
        name(&buffer, || "bytes between copies".into());
        self.keep(&buffer);
        Some(buffer)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_later_constant_reads_the_placed_copy_in_the_same_list() {
        use super::*;
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};

        let host = host().expect("this test needs a D3D12 device");
        const HEAP: usize = 64 << 10;
        const SIZE: usize = 560;
        // SAFETY: the allocation belongs to this test, and the registered
        // shadow keeps the range until the end of the test process.
        let title = unsafe { VirtualAlloc(std::ptr::null(), HEAP, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) } as usize;
        assert_ne!(title, 0);
        let (src, dst) = (title, title + 4096);
        let expected: [u8; SIZE] = std::array::from_fn(|i| i.wrapping_mul(7) as u8);
        crate::shadow::register(0x4244, title as u64, HEAP as u64);
        let shadow = crate::shadow::find(title as u64, HEAP as u64).unwrap();
        let list = host.new_list().unwrap();
        assert!(list.placed_constant(dst as u64, SIZE).is_none());
        let (upload, bytes) = host.shared_buffer(SIZE as u64).unwrap();
        // SAFETY: the upload is mapped and retained through this submission.
        unsafe { std::ptr::copy_nonoverlapping(expected.as_ptr(), bytes, SIZE) };
        shadow.refresh(&list, src as u64, SIZE as u64);
        shadow.gpu_writes(&list, src as u64, SIZE as u64);
        // SAFETY: both buffers are kept live below, and the recording is open.
        unsafe {
            list.list.CopyBufferRegion(&shadow.buffer, shadow.offset(src as u64), &upload, 0, SIZE as u64);
            wait_for_all(host, &list.list);
        }
        assert!(list.place_copy(dst, src, SIZE));
        assert!(list.placed_constant(dst as u64, SIZE - 16).is_none());
        let snapshot = list.placed_constant(dst as u64, SIZE).expect("the exact range has a GPU snapshot");
        let (result, mapped) = host.shared_buffer(SIZE as u64).unwrap();
        // SAFETY: the open list retains the snapshot, and the result stays
        // alive and mapped until this synchronous submission completes.
        unsafe {
            list.list.ResourceBarrier(&[transition(
                &snapshot,
                D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
            list.list.CopyBufferRegion(&result, 0, &snapshot, 0, SIZE as u64);
        }
        list.defer_copy(dst as *mut u8, src as *const u8, SIZE);
        assert!(list.placed_constant(dst as u64, SIZE).is_none());
        list.close();
        host.execute_sync_internal(&list);
        // SAFETY: synchronous execution finished the copy and made it visible.
        assert_eq!(unsafe { std::slice::from_raw_parts(mapped, SIZE) }, &expected);
        assert_eq!(host.log_messages(), None);
    }

    /// What copies with two sides cost to make (ignored: a measurement for
    /// `VICTORY_CARD_ALL`): buffers of 64 KiB on the card and in the PC's
    /// memory.
    #[test]
    #[ignore]
    fn small_buffers_cost() {
        let Some(host) = super::host() else {
            return;
        };
        const N: usize = 1000;
        let used = || host.video_memory().map_or(0, |m| m[1] >> 20);
        let before = used();
        let t = std::time::Instant::now();
        let card: Vec<_> = (0..N).map(|_| host.uav_buffer(1 << 16)).collect();
        let card_us = t.elapsed().as_micros() as f64 / N as f64;
        let after = used();
        let t = std::time::Instant::now();
        let pc: Vec<_> = (0..N).map(|_| host.shared_buffer(1 << 16)).collect();
        let pc_us = t.elapsed().as_micros() as f64 / N as f64;
        println!(
            "{N} buffers of 64 KiB: on the card {card_us:.0} us each, {} MB of its memory; in the PC's memory {pc_us:.0} us each",
            after.saturating_sub(before)
        );
        drop((card, pc));
    }
}
