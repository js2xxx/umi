use core::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use bevy_utils_proc_macros::all_tuples;
use ksc_core::RawReg;
use static_assertions::const_assert_eq;

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

impl TrapFrame {
    pub const fn syscall_arg<const N: usize>(&self) -> usize {
        self.gpr.tx.a[N]
    }

    pub fn set_syscall_ret(&mut self, ret: usize) {
        self.gpr.tx.a[0] = ret;
    }
}

/// A wrapper around `TrapFrame` to make it easier to access the arguments and
/// return values from user's syscalls.
///
/// Pass a function prototype to the generic parameter to utilize its max
/// functionality:
///
/// ```
/// use ksc::UserCx;
///
/// let mut tf = Default::default();
///
/// let user: UserCx<'_, fn(u32, *const u8) -> usize> =
///     UserCx::from(&mut tf);
///
/// let (a, b): (u32, *const u8) = user.args();
/// user.ret(a as usize + b as usize);
/// ```
pub struct UserCx<'a, A> {
    tf: &'a mut TrapFrame,
    _marker: PhantomData<A>,
}

impl<'a, A> From<&'a mut TrapFrame> for UserCx<'a, A> {
    fn from(tf: &'a mut TrapFrame) -> Self {
        UserCx {
            tf,
            _marker: PhantomData,
        }
    }
}

impl<'a, A> Deref for UserCx<'a, A> {
    type Target = TrapFrame;

    fn deref(&self) -> &Self::Target {
        self.tf
    }
}

impl<'a, A> DerefMut for UserCx<'a, A> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.tf
    }
}

impl<'a, A> UserCx<'a, A> {
    /// Get the underlying `TrapFrame`, consuming `self`.
    pub fn into_inner(self) -> &'a mut TrapFrame {
        self.tf
    }
}

macro_rules! impl_arg {
    ($($arg:ident),*) => {
        impl<'a, $($arg: RawReg,)* T: RawReg> UserCx<'a, fn($($arg),*) -> T> {
            #[allow(clippy::unused_unit)]
            #[allow(non_snake_case)]
            #[allow(unused_parens)]
            /// Get the arguments with the same prototype as the parameters in the function prototype.
            pub fn args(&self) -> ($($arg),*) {
                $(
                    let $arg = self.tf.syscall_arg::<${index()}>();
                )*
                ($(RawReg::from_raw($arg)),*)
            }

            /// Gives the return value to the user context, consuming `self`.
            pub fn ret(self, value: T) {
                self.tf.set_syscall_ret(RawReg::into_raw(value))
            }
        }
    };
}

all_tuples!(impl_arg, 0, 7, P);
