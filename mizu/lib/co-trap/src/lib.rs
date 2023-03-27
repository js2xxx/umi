#![cfg_attr(target_arch = "riscv64", no_std)]
#![feature(const_trait_impl)]

use riscv::register::{
    scause::Scause,
    stvec::{self, Stvec, TrapMode},
};
use static_assertions::const_assert_eq;

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!("imp.S"));

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Tx {
    pub ra: usize,     // 12
    pub sp: usize,     // 13
    pub gp: usize,     // 14
    pub tp: usize,     // 15
    pub a: [usize; 8], // 16..24
    pub t: [usize; 7], // 24..31
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Gpr {
    pub s: [usize; 12], // 0..12
    pub tx: Tx,         // 12..31
}
const_assert_eq!(
    core::mem::size_of::<Gpr>(),
    core::mem::size_of::<usize>() * 31
);

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct TrapFrame {
    pub gpr: Gpr,       // 0..31
    pub sepc: usize,    // 31
    pub sstatus: usize, // 32
    pub stval: usize,   // 33
    pub scause: usize,  // 34
}

pub fn user_entry() -> usize {
    extern "C" {
        fn _user_entry();
    }
    _user_entry as _
}

/// A temporary write to `stvec` register.
pub struct StvecTemp(Stvec);

impl StvecTemp {
    /// Creates a new [`StvecGuard`].
    ///
    /// # Safety
    ///
    /// - Interrupts **MUST BE DISABLED** on the current CPU during the whole
    ///   lifetime of this struct.
    /// - `entry` and `mode` must be valid.
    pub unsafe fn new(entry: usize, mode: TrapMode) -> Self {
        let old = stvec::read();
        unsafe { stvec::write(entry, mode) };
        StvecTemp(old)
    }
}

impl Drop for StvecTemp {
    fn drop(&mut self) {
        // SAFETY: The caller is aware of that safety notice of `Self::new`.
        unsafe { stvec::write(self.0.address(), self.0.trap_mode().unwrap()) };
    }
}

pub fn yield_to_user(frame: &mut TrapFrame) -> (Scause, usize) {
    extern "C" {
        fn _return_to_user(frame: *mut TrapFrame) -> usize;
    }

    ksync_core::critical(|| unsafe {
        let _stvec = StvecTemp::new(user_entry(), TrapMode::Direct);
        let status = _return_to_user(frame);
        (core::mem::transmute(frame.scause), status)
    })
}

#[doc(hidden)]
#[repr(C)]
pub struct FastRet {
    pub cx: &'static mut TrapFrame,
    pub status: usize,
}

/// Set the fast path function (conventional trap handler).
///
/// ```
/// fn some_fast_func(_cx: &mut co_trap::TrapFrame) -> usize {
///     1 // 0 means returning directly to the user thread, while others will
///       // be passed to the normal coroutine context.
/// }
/// co_trap::fast_func!(some_fast_func);
/// ```
///
/// If a fast path is not desired, just use `co_trap::fast_func!()` instead.
///
/// The interrupt **MUST BE DISABLED** during the execution of the function.
#[macro_export]
macro_rules! fast_func {
    ($func:ident) => {
        #[no_mangle]
        extern "C" fn _fast_func(cx: &'static mut $crate::TrapFrame) -> $crate::FastRet {
            let status = ($func)(cx);
            $crate::FastRet { cx, status }
        }
    };
    () => {
        #[no_mangle]
        extern "C" fn _fast_func(cx: &'static mut $crate::TrapFrame) -> $crate::FastRet {
            $crate::FastRet { cx, status: 1 }
        }
    };
}
