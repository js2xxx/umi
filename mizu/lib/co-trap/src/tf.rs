use core::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use bevy_utils_proc_macros::all_tuples;
use ksc_core::{
    handler::{FromParam, Param},
    RawReg, Scn,
};
use num_traits::FromPrimitive;
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

impl Gpr {
    pub fn copy_to_x(&self, output: &mut [usize; 31]) {
        output[0..4].copy_from_slice(&[self.tx.ra, self.tx.sp, self.tx.gp, self.tx.tp]);
        output[4..7].copy_from_slice(&self.tx.t[..3]);
        output[7..9].copy_from_slice(&self.s[..2]);
        output[9..17].copy_from_slice(&self.tx.a);
        output[17..27].copy_from_slice(&self.s[2..]);
        output[27..31].copy_from_slice(&self.tx.t[3..]);
    }

    pub fn copy_from_x(&mut self, input: &[usize; 31]) {
        self.tx.ra = input[0];
        self.tx.sp = input[1];
        self.tx.gp = input[2];
        self.tx.tp = input[3];
        self.tx.t[..3].copy_from_slice(&input[4..7]);
        self.s[..2].copy_from_slice(&input[7..9]);
        self.tx.a.copy_from_slice(&input[9..17]);
        self.s[2..].copy_from_slice(&input[17..27]);
        self.tx.t[3..].copy_from_slice(&input[27..31]);
    }
}

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

    pub fn scn(&self) -> Result<Scn, usize> {
        let raw = self.syscall_arg::<7>();
        Scn::from_usize(raw).ok_or(raw)
    }

    pub fn set_syscall_ret(&mut self, ret: usize) {
        self.sepc += 4;
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

all_tuples!(impl_arg, 0, 6, P);

impl<A: 'static> Param for UserCx<'_, A> {
    type Item<'a> = UserCx<'a, A>;
}

impl<A: 'static> FromParam<&'_ mut TrapFrame> for UserCx<'_, A> {
    fn from_param<'a>(item: <&'_ mut TrapFrame as Param>::Item<'a>) -> Self::Item<'a> {
        UserCx::from(item)
    }
}
