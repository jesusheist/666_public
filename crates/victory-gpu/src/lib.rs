//! D3D12.X on host D3D12.
//!
//! D3D12.X records commands from code inlined into the title, which writes
//! packets into the command list while its put pointer is below a limit and
//! calls the list's method otherwise. Our lists keep the limits at zero, so
//! every command arrives here as a call, to be run on host D3D12. The
//! title's GPU addresses are CPU addresses (console memory is unified).
//!
//! [`device`] is the device object titles create, [`dxgi`] its adapter side,
//! [`resource`] heaps and resources, [`fence`] fences, [`queue`] command
//! queues, [`list`] command allocators and lists, [`descriptor`]
//! descriptor heaps, [`root`] root signatures, [`query`] query heaps,
//! [`signature`] command signatures, [`pipeline`] pipeline states,
//! [`frame`] frame pacing, [`object`] what all console graphics objects
//! share, [`console`] which console model the title sees. Planned:
//! `present` (swap chain).

#[cfg(windows)]
mod ambient;
#[cfg(all(windows, feature = "diagnostics"))]
mod capture;
#[cfg(windows)]
mod compose;
pub mod console;
#[cfg(all(windows, feature = "diagnostics"))]
mod copystat;
#[cfg(windows)]
mod logstats;
#[cfg(windows)]
mod droprep;
#[cfg(all(windows, feature = "diagnostics"))]
mod faultbook;
#[cfg(windows)]
mod cores;
pub mod descriptor;
pub mod device;
#[cfg(windows)]
mod draw;
pub mod dxgi;
pub mod fence;
pub mod frame;
#[cfg(all(windows, feature = "diagnostics"))]
mod book;
#[cfg(windows)]
mod heap_worker;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod port;
#[cfg(windows)]
mod port_cbv;
#[cfg(windows)]
mod port_journal;
#[cfg(windows)]
mod indirect;
#[cfg(windows)]
mod badpix;
#[cfg(windows)]
mod mcp_shot;
#[cfg(windows)]
mod inplace;
#[cfg(windows)]
mod land;
pub mod list;
pub mod object;
pub mod pipeline;
#[cfg(windows)]
mod picked;
#[cfg(windows)]
mod pool;
#[cfg(all(windows, feature = "diagnostics"))]
mod popwatch;
#[cfg(windows)]
mod primitive;
pub mod query;
pub mod queue;
#[cfg(all(windows, feature = "diagnostics"))]
pub mod replay;
#[cfg(windows)]
mod residency;
pub mod resource;
pub mod root;
#[cfg(windows)]
mod safecopy;
#[cfg(windows)]
mod sampler;
#[cfg(windows)]
pub mod shader_cache;
#[cfg(windows)]
mod memory;
#[cfg(windows)]
mod vram;
#[cfg(windows)]
mod menu;
#[cfg(windows)]
mod shadow;
#[cfg(windows)]
mod wcback;
#[cfg(windows)]
mod tiles;
pub mod signature;
mod switch;
#[cfg(all(windows, feature = "diagnostics"))]
mod ab;
#[cfg(windows)]
mod tdr;
#[cfg(windows)]
mod texture;
pub mod timeline;
#[cfg(windows)]
pub mod titlewrite;
#[cfg(windows)]
mod upload;
#[cfg(windows)]
mod watch;
#[cfg(windows)]
mod texback;

/// The live tunables (`victory_core::tunables`), for the crates above this one.
pub use victory_core::tunables;

#[cfg(windows)]
pub use host::on_failure;
#[cfg(windows)]
mod window_mode;
#[cfg(windows)]
pub use window_mode::window_mode_message;

/// Whether `switch` is on now, if it is one of those `VICTORY_AB` turns on and
/// off while the title runs (`ab`).
#[cfg(all(windows, feature = "diagnostics"))]
pub fn switched(switch: &str) -> Option<bool> {
    ab::pick(switch)
}

/// What the frame is at (`copystat::phase`), for the sampling profiler.
#[cfg(all(windows, feature = "diagnostics"))]
pub fn frame_phase() -> u32 {
    copystat::phase()
}
#[cfg(windows)]
pub use safecopy::read_title as read_title_memory;
#[cfg(windows)]
pub use wcback::title_memory_reserved;
#[cfg(windows)]
pub use inplace::{title_memory_freeing, title_memory_section, title_memory_section_gone, title_memory_view};
#[cfg(windows)]
pub use watch::{
    kernel_writing, title_memory_allocated, title_memory_freed, title_memory_reading, title_memory_remapped,
    title_memory_remapping, title_memory_splitting, title_memory_unmapping, title_memory_written,
    writing as kernel_will_write,
};

/// Whether the in-game menu ([`menu`]) takes the keyboard key `virtual_key`
/// now (it is open, or the key was held when it was closed and has not been
/// let go since): the title must not see it.
#[cfg(windows)]
pub fn menu_takes_key(virtual_key: u8) -> bool {
    menu::takes_key(virtual_key)
}

/// Whether the in-game menu is open now: the mouse cursor is the user's
/// (free and visible) while it is.
#[cfg(windows)]
pub fn menu_open() -> bool {
    menu::is_open()
}
