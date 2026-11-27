//! Build-specific binding to the offline title's clock, and the shared "title
//! tick" hook: one patched call site on the title's game thread that runs the
//! story clock (when asked) and the queued cheats (when asked).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, ensure};
use victory_core::story_clock;
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{
    PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, VirtualProtect,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::LoadedImage;
use crate::console_insn::{jump, near};

const BUILD: &str = "b99f876b74d4a77f03af5f878223b27bd647b5c35a79d5decd9f716504239e93";
const UPDATE_CALL: usize = 0x0151_1fe4;
const UPDATE: usize = 0x0151_20b0;
const SET_TIME: usize = 0x01ab_9b20;
const CLOCK_FIELDS: usize = 0x0ab5_cdd0;
const CALL_BYTES: [u8; 10] = [0xe8, 0xc7, 0, 0, 0, 0xe8, 0x12, 0, 0, 0];
const SETTER_BYTES: [u8; 15] = [
    0x56, 0x57, 0x53, 0x48, 0x83, 0xec, 0x50, 0xc5, 0xfc, 0x10, 0x05, 0x91, 0x32, 0x0a, 0x09,
];

static IMAGE: OnceLock<usize> = OnceLock::new();
static CLOCK_ON: AtomicBool = AtomicBool::new(false);
/// The title's cheat functions, read from its table at install.
static CHEATS: OnceLock<[usize; crate::cheats::COUNT]> = OnceLock::new();

/// Install before any title code runs, while the call site is idle.
/// `clock`: run the story clock; `cheats`: run the queued title cheats.
/// Cheats that cannot be verified are left unavailable (a warning, not an error);
/// the clock keeps its old all-or-nothing behavior.
///
/// # Safety
/// The caller must keep the image loaded for the duration of title execution.
pub unsafe fn install(image: &LoadedImage, hash: &str, clock: bool, cheats: bool) -> Result<()> {
    ensure!(
        hash == BUILD,
        "story clock is unavailable for this title build"
    );
    let base = image.base();
    let call = base + UPDATE_CALL;
    if clock {
        ensure!(
            image.info().size_of_image as usize >= CLOCK_FIELDS + 12,
            "story clock fields are outside the title image"
        );
        // SAFETY: the supported image covers the checked byte range.
        let setter = unsafe { ((base + SET_TIME) as *const [u8; 15]).read() };
        ensure!(
            setter == SETTER_BYTES,
            "story clock instructions differ from the supported build"
        );
    }
    // SAFETY: the supported image covers the checked byte range.
    let site = unsafe { (call as *const [u8; 10]).read() };
    ensure!(
        site == CALL_BYTES,
        "title tick call site differs from the supported build"
    );
    let mut table = None;
    if cheats {
        match read_table(image) {
            Ok(t) => table = Some(t),
            Err(why) => tracing::warn!("title cheats are unavailable: {why}"),
        }
    }
    if !clock && table.is_none() {
        return Ok(());
    }

    let thunk = near(call).ok_or_else(|| anyhow::anyhow!("no space for the story clock thunk"))?;
    let mut patch =
        jump(call, thunk).ok_or_else(|| anyhow::anyhow!("story clock thunk is out of range"))?;
    patch[0] = 0xe8; // Preserve the call's return address and the original ABI.
    let mut body = [0; 14];
    body[..2].copy_from_slice(&[0xff, 0x25]); // jmp qword ptr [rip]
    body[6..].copy_from_slice(&(update_clock as *const () as usize).to_le_bytes());
    IMAGE
        .set(base)
        .map_err(|_| anyhow::anyhow!("story clock was already installed"))?;
    CLOCK_ON.store(clock, Ordering::Relaxed);
    if let Some(t) = table {
        let _ = CHEATS.set(t);
    }
    let mut protection = 0;
    // SAFETY: the thunk is private to this process and the title has not run.
    unsafe {
        std::ptr::copy_nonoverlapping(body.as_ptr(), thunk as *mut u8, body.len());
        ensure!(
            VirtualProtect(thunk as _, body.len(), PAGE_EXECUTE_READ, &mut protection) != 0,
            "could not protect the story clock thunk"
        );
        ensure!(
            FlushInstructionCache(GetCurrentProcess(), thunk as _, body.len()) != 0,
            "could not publish the story clock thunk"
        );
        ensure!(
            VirtualProtect(
                call as _,
                patch.len(),
                PAGE_EXECUTE_READWRITE,
                &mut protection
            ) != 0,
            "could not write the story clock call"
        );
        std::ptr::copy_nonoverlapping(patch.as_ptr(), call as *mut u8, patch.len());
        ensure!(
            VirtualProtect(call as _, patch.len(), protection, &mut protection) != 0,
            "could not restore title code protection"
        );
        ensure!(
            FlushInstructionCache(GetCurrentProcess(), call as _, patch.len()) != 0,
            "could not publish the story clock call"
        );
    }
    if table.is_some() {
        victory_core::cheats::set_ready(true);
    }
    tracing::info!(clock, cheats = table.is_some(), "title tick installed; F10 opens the menu once gameplay starts");
    Ok(())
}

/// The cheat table, checked: it lies in the image and every entry points into executable code.
fn read_table(image: &LoadedImage) -> Result<[usize; crate::cheats::COUNT]> {
    let size = image.info().size_of_image as usize;
    ensure!(
        size >= crate::cheats::TABLE + crate::cheats::COUNT * 8 && size > crate::cheats::PLAYER + 8,
        "the cheat table is outside the title image"
    );
    // SAFETY: the table lies inside the mapped, relocated image (checked above).
    let entries = unsafe { ((image.base() + crate::cheats::TABLE) as *const [usize; crate::cheats::COUNT]).read() };
    let exec = crate::cheats::executable_ranges(&image.info().sections);
    crate::cheats::validate(&entries, image.base(), &exec).map_err(|why| anyhow::anyhow!(why))?;
    Ok(entries)
}

// Read only on the title's clock thread. A separate atomic snapshot serves the menu.
unsafe fn read_clock(base: usize) -> Option<u32> {
    let fields = (base + CLOCK_FIELDS) as *const u32;
    // SAFETY: the supported image's three adjacent clock fields.
    let (hour, minute, second) = unsafe {
        (
            fields.read_volatile(),
            fields.add(1).read_volatile(),
            fields.add(2).read_volatile(),
        )
    };
    (hour < 24 && minute < 60 && second < 60).then(|| hour * 3600 + minute * 60 + second)
}

extern "system" fn update_clock() {
    let Some(&base) = IMAGE.get() else { return };
    // SAFETY: the expected call site targets a zero-argument clock update.
    let update: extern "system" fn() = unsafe { std::mem::transmute(base + UPDATE) };
    if CLOCK_ON.load(Ordering::Relaxed) {
        tick_clock(base, update);
    } else {
        update();
    }
    if let Some(table) = CHEATS.get() {
        run_cheats(base, table);
    }
}

fn run_cheats(base: usize, table: &[usize; crate::cheats::COUNT]) {
    // SAFETY: the player pointer global lies inside the checked image.
    let player = unsafe { ((base + crate::cheats::PLAYER) as *const usize).read_volatile() };
    if player == 0 {
        // No player in the world: a press now would run title code on nothing.
        let mut dropped = 0;
        victory_core::cheats::drain(|_| dropped += 1);
        if dropped > 0 {
            tracing::warn!(dropped, "title cheat dropped: no player yet");
        }
        return;
    }
    victory_core::cheats::drain(|n| {
        // SAFETY: each entry was verified to point into executable title code
        // and takes no arguments; this is the title's own game thread.
        let cheat: extern "system" fn() = unsafe { std::mem::transmute(table[usize::from(n)]) };
        cheat();
        tracing::info!(cheat = n, "title cheat run");
    });
}

fn tick_clock(base: usize, update: extern "system" fn()) {
    let before = unsafe { read_clock(base) };
    update();
    let after = unsafe { read_clock(base) };
    let Some(seconds) = after else { return };
    if before != after || story_clock::current().is_some() {
        story_clock::observe(seconds / 60);
    }
    let Some(minutes) = story_clock::take_request() else {
        return;
    };
    // SAFETY: this verified setter takes hour, minute and second in RCX, RDX
    // and R8. It runs on the same title thread as the update above.
    let set_time: extern "system" fn(u32, u32, u32) =
        unsafe { std::mem::transmute(base + SET_TIME) };
    set_time(minutes / 60, minutes % 60, 0);
    let result = unsafe { read_clock(base) };
    if result == Some(minutes * 60) {
        story_clock::observe(minutes);
        tracing::info!(
            previous_seconds = seconds,
            requested_minutes = minutes,
            "story clock set"
        );
    } else {
        tracing::warn!(
            requested_minutes = minutes,
            ?result,
            "story clock change was not confirmed"
        );
    }
}
