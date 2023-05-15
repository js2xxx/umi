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
        unsafe { self.disable(true) };
        PreemptStateGuard { state: self }
    }

    pub unsafe fn disable(&self, set_sstatus: bool) {
        if self.count.fetch_add(1, Relaxed) == 0 && set_sstatus {
            unsafe { sstatus::clear_sie() }
        }
    }

    pub unsafe fn enable(&self, set_sstatus: bool) {
        if self.count.fetch_sub(1, Relaxed) == 1 && set_sstatus {
            unsafe { sstatus::set_sie() }
        }
    }
}

impl Drop for PreemptStateGuard<'_> {
    fn drop(&mut self) {
        unsafe { self.state.enable(true) }
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
unsafe extern "C" fn preempt_disable(set_sstatus: bool) {
    PREEMPT.disable(set_sstatus)
}

#[no_mangle]
unsafe extern "C" fn preempt_enable(set_sstatus: bool) {
    PREEMPT.enable(set_sstatus)
}
