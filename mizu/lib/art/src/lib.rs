//! Asynchorous RunTime.
#![cfg_attr(not(feature = "test"), no_std)]

extern crate alloc;

mod queue;
mod sched;
mod task;

pub use self::{queue::*, sched::SCHED, task::SchedInfo};

static mut NR_HARTS: usize = 0;
/// Initialize the `ART` module.
///
/// # Safety
///
/// This function must be called only once during initialization.
pub unsafe fn init(nr_harts: usize) {
    unsafe { NR_HARTS = nr_harts }
}
