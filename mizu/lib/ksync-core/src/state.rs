use core::{
    marker::PhantomData,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

use riscv::register::sstatus;

pub struct PreemptState {
    count: AtomicUsize,
    _non_send: PhantomData<*mut ()>,
}

pub struct PreemptStateGuard<'a> {
    state: &'a PreemptState,
}

impl PreemptState {
    pub const fn new() -> Self {
        PreemptState {
            count: AtomicUsize::new(0),
            _non_send: PhantomData,
        }
    }

    pub fn lock(&self) -> PreemptStateGuard {
        unsafe { self.disable() };
        PreemptStateGuard { state: self }
    }

    pub unsafe fn disable(&self) {
        if self.count.fetch_add(1, Relaxed) == 0 {
            unsafe { sstatus::clear_sie() }
        }
    }

    pub unsafe fn enable(&self) {
        if self.count.fetch_sub(1, Relaxed) == 1 {
            unsafe { sstatus::set_sie() }
        }
    }
}

impl Drop for PreemptStateGuard<'_> {
    fn drop(&mut self) {
        unsafe { self.state.enable() }
    }
}

impl const Default for PreemptState {
    fn default() -> Self {
        Self::new()
    }
}

#[thread_local]
pub static PREEMPT: PreemptState = PreemptState::default();

#[no_mangle]
unsafe extern "C" fn preempt_disable() {
    PREEMPT.disable()
}

#[no_mangle]
unsafe extern "C" fn preempt_enable() {
    PREEMPT.enable()
}
