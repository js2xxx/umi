#![no_std]
#![feature(const_trait_impl)]
#![feature(thread_local)]

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
mod state;

/// Enter a critical section in a single core. Do not use it with multi-core
/// synchoronization. Intended to be used with mutexes.
///
/// Doesn't have any effect on test cases.
///
/// # Examples
///
/// ```rust
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
    let _preempt = state::PREEMPT.lock();
    f()
}
