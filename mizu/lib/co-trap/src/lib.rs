#![cfg_attr(target_arch = "riscv64", no_std)]
#![feature(const_trait_impl)]
#![feature(macro_metavar_expr)]

mod tf;

use core::sync::atomic::{compiler_fence, Ordering::SeqCst};

use enum_primitive_derive::Primitive;
use num_traits::FromPrimitive;
use riscv::register::{
    scause::Scause,
    stvec::{self, Stvec, TrapMode},
};

pub use self::tf::*;

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!("imp.S"));

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
        compiler_fence(SeqCst);
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

pub fn yield_to_user(frame: &mut TrapFrame) -> (Scause, FastResult) {
    extern "C" {
        fn _return_to_user(frame: *mut TrapFrame) -> usize;
    }

    ksync_core::critical(|| unsafe {
        let _stvec = StvecTemp::new(user_entry(), TrapMode::Direct);
        let res = _return_to_user(frame);
        (
            core::mem::transmute(frame.scause),
            FastResult::from_usize(res).unwrap(),
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Primitive)]
#[repr(usize)]
pub enum FastResult {
    /// Finished fast-path execution without error, and directly yield to user.
    Yield = 0,
    /// The fast-path execution cannot be done, should do the ordinary path in
    /// the async context.
    Continue = 1,
    /// Finished fast-path execution without error, but the current task has
    /// some pending events (signals).
    Pending = 2,
    /// Should directly exit the current task.
    Break = 3,
}

#[doc(hidden)]
#[repr(C)]
pub struct FastRet {
    pub cx: &'static mut TrapFrame,
    pub res: FastResult,
}

/// Set the fast path function (conventional trap handler).
///
/// ```
/// use co_trap::{TrapFrame, FastResult, fast_func};
///
/// fn some_fast_func(_cx: &mut TrapFrame) -> FastResult {
///     FastResult::Continue
/// }
/// fast_func!(some_fast_func);
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
            let res = ($func)(cx);
            $crate::FastRet { cx, res }
        }
    };
    () => {
        #[no_mangle]
        extern "C" fn _fast_func(cx: &'static mut $crate::TrapFrame) -> $crate::FastRet {
            $crate::FastRet {
                cx,
                res: $crate::FastResult::Continue,
            }
        }
    };
}
