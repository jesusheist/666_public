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

#[allow(unused_imports)]
pub use menu::*;

/// Registers every area's tunables.
pub fn register_all() {
    menu::register();
}

/// The switches the runtime's own code reads from the environment: all of them in this fork.
pub fn env_read_slots() -> Vec<&'static crate::tunables::Slot> {
    crate::tunables::list_registered()
}
