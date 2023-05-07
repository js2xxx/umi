use alloc::boxed::Box;
use core::{future::Future, ops::ControlFlow, pin::Pin};

use bevy_utils_proc_macros::all_tuples;

pub type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Param {
    type Item<'a>: Param + 'a;
}

impl<T: 'static> Param for &'_ mut T {
    type Item<'a> = &'a mut T;
}

impl<T: 'static> Param for &T {
    type Item<'a> = &'a T;
}

impl<T: 'static> FromParam<&'_ mut T> for &'_ mut T {
    fn from_param<'a>(item: <&'_ mut T as Param>::Item<'a>) -> Self::Item<'a> {
        item
    }
}

impl<T: 'static> FromParam<&'_ T> for &'_ T {
    fn from_param<'a>(item: <&'_ T as Param>::Item<'a>) -> Self::Item<'a> {
        item
    }
}

impl<T: Param> Param for Boxed<'_, T> {
    type Item<'a> = Boxed<'a, T::Item<'a>>;
}

pub trait FromParam<S: Param>: Param {
    fn from_param(item: <S as Param>::Item<'_>) -> Self::Item<'_>;
}

macro_rules! impl_param {
    ($(($source:ident, $param:ident)),*) => {
        impl<$($param: Param),*> Param for ($($param,)*) {
            type Item<'a> = ($(<$param as Param>::Item<'a>,)*);
        }

        #[allow(clippy::unused_unit)]
        #[allow(non_snake_case)]
        impl<$($source: Param, $param: FromParam<$source>),*> FromParam<($($source,)*)> for($($param,)*) {
            fn from_param(item: <($($source,)*) as Param>::Item<'_>) -> Self::Item<'_> {
                let ($($source,)*) = item;
                ($(<$param as FromParam<$source>>::from_param($source),)*)
            }
        }
    };
}

all_tuples!(impl_param, 0, 12, S, P);

macro_rules! impl_primitives {
    ($($type:ident),* $(,)?) => {
        $(
            impl Param for $type {
                type Item<'a> = $type;
            }
        )*
    };
}
impl_primitives!(bool, char, u8, u16, u32, u64, usize, i8, i16, i32, i64, isize);

impl<T: Param, E: Param> Param for Result<T, E> {
    type Item<'a> = Result<<T as Param>::Item<'a>, <E as Param>::Item<'a>>;
}

impl<T: Param> Param for Option<T> {
    type Item<'a> = Option<<T as Param>::Item<'a>>;
}

impl<B: Param, C: Param> Param for ControlFlow<B, C> {
    type Item<'a> = ControlFlow<<B as Param>::Item<'a>, <C as Param>::Item<'a>>;
}
