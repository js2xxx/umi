use alloc::boxed::Box;
use core::{future::Future, pin::Pin};

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

impl Param for usize {
    type Item<'a> = usize;
}

impl Param for bool {
    type Item<'a> = bool;
}

impl<T: Param> Param for crate::Result<T> {
    type Item<'a> = crate::Result<<T as Param>::Item<'a>>;
}
