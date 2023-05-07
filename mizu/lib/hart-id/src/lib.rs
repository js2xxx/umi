#![cfg_attr(not(test), no_std)]
#![feature(thread_local)]

use alloc::vec::Vec;
use core::sync::atomic::{
    AtomicUsize,
    Ordering::{Relaxed, Release},
};

use spin::lock_api::Mutex;

extern crate alloc;

static BSP_ID: AtomicUsize = AtomicUsize::new(0);
static COUNT: AtomicUsize = AtomicUsize::new(1);
static HIDS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

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
    ksync_core::critical(|| HIDS.lock().push(id));
}

pub fn init_bsp_id(id: usize) {
    BSP_ID.store(id, Release);
    ksync_core::critical(|| HIDS.lock().push(id));
}

pub fn count() -> usize {
    COUNT.load(Relaxed)
}

pub fn hart_ids() -> Vec<usize> {
    ksync_core::critical(|| HIDS.lock().clone())
}
