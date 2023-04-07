use core::cell::LazyCell;

use crossbeam_epoch::{Collector, Guard, LocalHandle};
use spin::Lazy;

fn collector() -> &'static Collector {
    /// The global data for the default garbage collector.
    static COLLECTOR: Lazy<Collector> = Lazy::new(Collector::new);
    &COLLECTOR
}

#[thread_local]
/// The per-thread participant for the default garbage collector.
static HANDLE: LazyCell<LocalHandle> = LazyCell::new(|| collector().register());

/// Pins the current thread.
#[inline]
pub fn pin() -> Guard {
    with_handle(|handle| handle.pin())
}

/// Returns `true` if the current thread is pinned.
#[inline]
pub fn is_pinned() -> bool {
    with_handle(|handle| handle.is_pinned())
}

/// Returns the default global collector.
pub fn default_collector() -> &'static Collector {
    collector()
}

#[inline]
fn with_handle<F, R>(mut f: F) -> R
where
    F: FnMut(&LocalHandle) -> R,
{
    f(&HANDLE)
}
