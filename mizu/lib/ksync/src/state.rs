use core::{
    marker::PhantomData,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

use riscv::interrupt;

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
        if self.count.fetch_add(1, Relaxed) == 0 {
            unsafe { interrupt::disable() }
        }
        PreemptStateGuard { state: self }
    }
}

impl Drop for PreemptStateGuard<'_> {
    fn drop(&mut self) {
        if self.state.count.fetch_sub(1, Relaxed) == 1 {
            unsafe { interrupt::enable() }
        }
    }
}

impl const Default for PreemptState {
    fn default() -> Self {
        Self::new()
    }
}

#[thread_local]
pub static PREEMPT: PreemptState = PreemptState::default();
