//! Enable the S mode context to run as coroutines.
//!
//! The S mode code can thus be written linearly, instead of in the form of
//! `interrupt_handlers` and so on.
//!
//! Furthermore, task scheduling in the S mode can be more smooth and simple in
//! 2 ways:
//!
//! 1. No need of manual context-switching, and thus...
//! 2. No need of one kernel stack per task.
//!
//! # Examples
//!
//! Task scheduling by some async runtime.
//! ```rust,no_run
//! use co_trap::{yield_to_user, TrapFrame};
//! use riscv::register::scause;
//!
//! fn init_context() {
//!     unsafe { co_trap::init(reent_handler) };
//!     // Enable interrupts
//! }
//!
//! async fn init_task() -> TrapFrame {
//!     todo!("init_task")
//! }
//!
//! async fn run_task() {
//!     let mut frame = init_task().await;
//!     loop {
//!         unsafe { yield_to_user(&mut frame) };
//!         let cause = scause::read();
//!         // `await` indicates possible task scheduling.
//!         handle_resume(&mut frame, cause).await;
//!     }
//! }
//!
//! async fn handle_resume(_frame: &mut TrapFrame, _cause: scause::Scause) {
//!     todo!("handle_resume");
//! }
//!
//! // Process exceptions or interrupts occurred in S-mode.
//! extern "C" fn reent_handler(_frame: &mut TrapFrame) {
//!     // Example:
//!     // * Panic or reset if an S-mode exception occurred.
//!     // * Wake wakers of tasks waiting for it if an interrupt occurred.
//! }
//! ```
#![cfg_attr(not(test), no_std)]

use core::sync::atomic;

use static_assertions::const_assert;

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
mod imp;

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct TrapFrame {
    /// Note that `x{i}` is `x[i - 1]`.
    pub x: [usize; 31], // 0..248
    /// the "a0" in `x` (i.e. x10) is actually `sscratch`.
    pub a0: usize, // 248
    pub sepc: usize,    // 256
    pub sstatus: usize, // 264

    rt_stack: usize, // 272
    rt_ra: usize,    // 280
}
const_assert!(core::mem::size_of::<TrapFrame>() == 288);

#[no_mangle]
static mut REENT_HANDLER: usize = 0;

pub type ReentHandler = extern "C" fn(frame: &mut TrapFrame);

/// Initialize the environment for [trap coroutines].
///
/// Note: when using the functionality, interrupts (i.e. `sie`) must be enabled
/// to ensure the proper resumption to the context.
///
/// # Safety
///
/// * This function must be called only once during initialization.
///
/// * The environment occupies the interrupt entry vector (i.e. `stvec`), so it
///   must not be changed after initialization.
///
/// [trap coroutines]: yield_to_user
pub unsafe fn init(reent_handler: ReentHandler) {
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    unsafe {
        use riscv::register::{stvec, utvec::TrapMode};
        extern "C" {
            fn _intr_entry();
        }
        stvec::write(_intr_entry as usize, TrapMode::Direct)
    }
    atomic::fence(atomic::Ordering::Release);
    unsafe { REENT_HANDLER = reent_handler as usize }
    atomic::fence(atomic::Ordering::Acquire);
}

/// Yield to the calling context (i.e. the user context).
///
/// The calling context resume back here by raising interrupts or exceptions.
///
/// # Safety
///
/// `frame` must points to a valid user task context, and the environment must
/// be [initialized] before any call to this function.
///
/// [initialized]: init
#[inline]
pub unsafe fn yield_to_user(frame: &mut TrapFrame) {
    unsafe {
        debug_assert_ne!(REENT_HANDLER, 0, "Yield before initialization");
    }

    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    unsafe {
        extern "C" {
            fn _return_to_user(frame: *mut TrapFrame);
        }
        _return_to_user(frame);
    }
    #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
    let _ = frame;
}
