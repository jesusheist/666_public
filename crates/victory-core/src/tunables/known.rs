//! Every migrated `VICTORY_*` switch, one file per area under `known/`, so
//! several people can migrate areas at once without touching the same file.
//!
//! To migrate one: add a `tunable!` line to the area file and its name to that
//! file's `tunable_list!(pub fn register: ...)`, then read it as
//! `victory_core::tunables::known::NAME.get()` where the switch was read
//! (victory-gpu and victory-hle reach it as `victory_gpu::tunables::known::NAME`).
//! Every area's names are re-exported here, so `known::NAME` works for all.

mod gpu_draw;
mod gpu_ports;
mod gpu_submit;
mod gpu_memory;
mod gpu_texture;
mod hle;
mod shader;
mod other;

#[allow(unused_imports)]
pub use gpu_draw::*;
#[allow(unused_imports)]
pub use gpu_ports::*;
#[allow(unused_imports)]
pub use gpu_submit::*;
#[allow(unused_imports)]
pub use gpu_memory::*;
#[allow(unused_imports)]
pub use gpu_texture::*;
#[allow(unused_imports)]
pub use hle::*;
#[allow(unused_imports)]
pub use shader::*;
#[allow(unused_imports)]
pub use other::*;

/// Registers every area's tunables.
pub fn register_all() {
    gpu_draw::register();
    gpu_ports::register();
    gpu_submit::register();
    gpu_memory::register();
    gpu_texture::register();
    hle::register();
    shader::register();
    other::register();
}
