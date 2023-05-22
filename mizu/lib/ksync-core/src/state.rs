use core::{arch::asm, marker::PhantomData};

pub struct PreemptState {
    flags: usize,
    _non_send: PhantomData<*mut ()>,
}

const SIE: usize = 1 << 1;

impl PreemptState {
    pub fn new() -> Self {
        let flags = unsafe { disable() };
        PreemptState {
            flags,
            _non_send: PhantomData,
        }
    }
}

impl Drop for PreemptState {
    fn drop(&mut self) {
        unsafe { enable(self.flags) }
    }
}

impl Default for PreemptState {
    fn default() -> Self {
        Self::new()
    }
}

pub unsafe fn disable() -> usize {
    let sstatus: usize;
    asm!("csrrc {}, sstatus, {}", out(reg) sstatus, in(reg) SIE);
    sstatus
}

pub unsafe fn enable(flags: usize) {
    asm!("csrs sstatus, {}", in(reg) (flags & SIE));
}
