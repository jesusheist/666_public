//! Every `VICTORY_*` switch this build reads, one file per area under `known/`.
//!
//! Each file declares its switches with `tunable!` and lists them in a
//! `tunable_list!(pub(super) fn register: ...)`; add the file's module and its
//! `register()` call below. Every switch here is read by its own code from the
//! environment (this fork's readers are `std::env::var` calls, most of them
//! behind a `OnceLock`), so the registry only describes it: the saved value of
//! the live file is put into the environment at the start by
//! [`crate::tunables::export_env_read`], where the variable is not set, and the
//! readers see it as if it had been set. The default declared here is what the
//! reader does with the variable unset. A switch's value is never read through
//! `NAME.get()` unless the doc line of its group says so.
//!
//! Switches whose value is free text (folders, files, hash lists, addresses)
//! are not declared: the registry holds numbers, on/off and fixed choices.

mod menu;
mod overlay;
mod switches_1;
mod switches_2;
mod switches_3;
mod switches_4;
mod switches_5;
mod switches_6;

#[allow(unused_imports)]
pub use menu::*;
#[allow(unused_imports)]
pub use overlay::*;
#[allow(unused_imports)]
pub use switches_1::*;
#[allow(unused_imports)]
pub use switches_2::*;
#[allow(unused_imports)]
pub use switches_3::*;
#[allow(unused_imports)]
pub use switches_4::*;
#[allow(unused_imports)]
pub use switches_5::*;
#[allow(unused_imports)]
pub use switches_6::*;

/// Registers every area's tunables.
pub fn register_all() {
    menu::register();
    overlay::register();
    switches_1::register();
    switches_2::register();
    switches_3::register();
    switches_4::register();
    switches_5::register();
    switches_6::register();
}

/// The switches the runtime's own code reads from the environment: all of them in this fork.
pub fn env_read_slots() -> Vec<&'static crate::tunables::Slot> {
    crate::tunables::list_registered()
}
