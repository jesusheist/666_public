//! Named threads. Every thread the runtime starts goes through here (or a
//! `std::thread::Builder` with a name), so a profiler or the `thread_cpu`
//! debug tool can tell the pools apart. On Windows, std puts the name in the
//! thread's description.
//!
//! Names are `victory-<pool>` for a single thread and `victory-<pool>-<n>`
//! for a pool member; the number is dropped when threads are grouped.

use std::io;
use std::thread::{Builder, JoinHandle, Scope, ScopedJoinHandle};

/// [`std::thread::spawn`], with a name.
///
/// # Panics
/// When the thread cannot be started, as `std::thread::spawn` does.
pub fn spawn<F, T>(name: &str, work: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    try_spawn(name, work).unwrap_or_else(|e| panic!("cannot start thread {name}: {e}"))
}

/// As [`spawn`], but says when no thread could be started.
pub fn try_spawn<F, T>(name: &str, work: F) -> io::Result<JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Builder::new().name(name.to_owned()).spawn(work)
}

/// `scope.spawn`, with a name.
///
/// # Panics
/// When the thread cannot be started, as `Scope::spawn` does.
pub fn scoped<'scope, 'env, F, T>(scope: &'scope Scope<'scope, 'env>, name: &str, work: F) -> ScopedJoinHandle<'scope, T>
where
    F: FnOnce() -> T + Send + 'scope,
    T: Send + 'scope,
{
    Builder::new()
        .name(name.to_owned())
        .spawn_scoped(scope, work)
        .unwrap_or_else(|e| panic!("cannot start thread {name}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawned_thread_has_its_name() {
        let name = spawn("victory-test-a", || std::thread::current().name().map(str::to_owned)).join().unwrap();
        assert_eq!(name.as_deref(), Some("victory-test-a"));
    }

    #[test]
    fn a_scoped_thread_has_its_name_and_borrows() {
        let local = 5;
        let name = std::thread::scope(|s| {
            scoped(s, "victory-test-b", || (local, std::thread::current().name().map(str::to_owned))).join().unwrap()
        });
        assert_eq!(name, (5, Some("victory-test-b".to_owned())));
    }
}
