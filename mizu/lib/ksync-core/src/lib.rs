#![no_std]

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
mod state;

/// Enter a critical section in a single core. Do not use it with multi-core
/// synchoronization. Intended to be used with mutexes.
///
/// Doesn't have any effect on test cases.
///
/// # Examples
///
/// ```rust,ignore
/// use spin::Mutex;
///
/// let mutex = Mutex::new(0);
/// ksync::critical(|| {
///     let value = mutex.lock();
///     assert_eq!(*value, 0);
/// })
/// ```
#[inline]
pub fn critical<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    let _preempt = state::PreemptState::new();
    f()
}

/// Disable interrupts manually.
///
/// # Safety
///
/// The caller must care about the potential risks of functions that have sth to
/// do with interrupts.
pub unsafe fn disable() -> usize {
    #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
    return 0;
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    state::disable()
}

/// Enable interrupts manually.
///
/// # Safety
///
/// The caller must care about the potential risks of functions that have sth to
/// do with interrupts.
pub unsafe fn enable(_flags: usize) {
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    state::enable(_flags)
}

pub fn is_enabled() -> bool {
    riscv::register::sstatus::read().sie()
}
