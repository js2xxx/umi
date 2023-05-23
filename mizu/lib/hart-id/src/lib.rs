#![cfg_attr(not(test), no_std)]
#![feature(thread_local)]

use core::sync::atomic::{
    AtomicUsize,
    Ordering::{Relaxed, Release},
};

static BSP_ID: AtomicUsize = AtomicUsize::new(0);
static COUNT: AtomicUsize = AtomicUsize::new(1);
static HIDS: AtomicUsize = AtomicUsize::new(0);

#[thread_local]
static mut HART_ID: usize = 0;

pub fn bsp_id() -> usize {
    BSP_ID.load(Relaxed)
}

pub fn hart_id() -> usize {
    unsafe { HART_ID }
}

pub fn is_bsp() -> bool {
    hart_id() == bsp_id()
}

/// # Safety
///
/// This function must be called only once during initialization ofr each CPU
/// core.
pub unsafe fn init_hart_id(id: usize) {
    HART_ID = id;
    COUNT.fetch_add(1, Release);
    HIDS.fetch_or(1 << id, Release);
}

pub fn init_bsp_id(id: usize) {
    BSP_ID.store(id, Release);
}

pub fn count() -> usize {
    COUNT.load(Relaxed)
}

pub fn hart_ids() -> usize {
    HIDS.load(Relaxed)
}
