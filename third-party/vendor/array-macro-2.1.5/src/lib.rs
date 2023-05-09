//! Array multiple elements constructor syntax.
//!
//! While Rust does provide those, they require copy, and you cannot obtain the
//! index that will be created. This crate provides syntax that fixes both of
//! those issues.
//!
//! # Examples
//!
//! ```
//! # #[macro_use]
//! # extern crate array_macro;
//! # fn main() {
//! assert_eq!(array![String::from("x"); 2], [String::from("x"), String::from("x")]);
//! assert_eq!(array![x => x; 3], [0, 1, 2]);
//! # }
//! ```

#![no_std]
#![deny(missing_docs)]

#[doc(hidden)]
pub extern crate core as __core;

/// Creates an array containing the arguments.
///
/// This macro provides a way to repeat the same macro element multiple times
/// without requiring `Copy` implementation as array expressions require.
///
/// There are two forms of this macro.
///
/// - Create an array from a given element and size. This will `Clone` the element.
///
///   ```
///   use array_macro::array;
///   assert_eq!(array![vec![1, 2, 3]; 2], [[1, 2, 3], [1, 2, 3]]);
///   ```
///
///   Unlike array expressions this syntax supports all elements which implement
///   `Clone`.
///
/// - Create an array from a given expression that is based on index and size.
///   This doesn't require the element to implement `Clone`.
///
///   ```
///   use array_macro::array;
///   assert_eq!(array![x => x * 2; 3], [0, 2, 4]);
///   ```
///
///   This form can be used for declaring `const` variables.
///
///   ```
///   use array_macro::array;
///   const ARRAY: [String; 3] = array![_ => String::new(); 3];
///   assert_eq!(ARRAY, ["", "", ""]);
///   ```
///
/// # Limitations
///
/// When using a form with provided index it's not possible to use `break`
/// or `continue` without providing a label. This won't compile.
///
/// ```compile_fail
/// use array_macro::array;
/// loop {
///     array![_ => break; 1];
/// }
/// ```
///
/// To work-around this issue you can provide a label.
///
/// ```
/// use array_macro::array;
/// 'label: loop {
///     array![_ => break 'label; 1];
/// }
/// ```
#[macro_export]
macro_rules! array {
    [$expr:expr; $count:expr] => {{
        let value = $expr;
        $crate::array![_ => $crate::__core::clone::Clone::clone(&value); $count]
    }};
    [$i:pat => $e:expr; $count:expr] => {
        $crate::__array![$i => $e; $count]
    };
}

use core::mem::{ManuallyDrop, MaybeUninit};
use core::ptr;

#[doc(hidden)]
#[repr(transparent)]
pub struct __ArrayVec<T, const N: usize>(pub __ArrayVecInner<T, N>);

impl<T, const N: usize> Drop for __ArrayVec<T, N> {
    fn drop(&mut self) {
        // This is safe as arr[..len] is initialized due to
        // __ArrayVecInner's type invariant.
        let initialized = &mut self.0.arr[..self.0.len] as *mut _ as *mut [T];
        unsafe { ptr::drop_in_place(initialized) };
    }
}

// Type invariant: arr[..len] must be initialized
#[doc(hidden)]
#[non_exhaustive]
pub struct __ArrayVecInner<T, const N: usize> {
    pub arr: [MaybeUninit<T>; N],
    pub len: usize,
    // This field exists so that array! macro could retrieve the value of N.
    // The method to retrieve N cannot be directly on __ArrayVecInner as
    // borrowing it could cause a reference to interior mutable data to
    // be created which is not allowed in `const fn`.
    //
    // Because this field doesn't actually store anything it's not possible
    // to replace it in an already existing instance of __ArrayVecInner.
    pub capacity: __Capacity<N>,
}

impl<T, const N: usize> __ArrayVecInner<T, N> {
    #[doc(hidden)]
    pub const unsafe fn new(arr: [MaybeUninit<T>; N]) -> Self {
        Self {
            arr,
            len: 0,
            capacity: __Capacity,
        }
    }
}

#[doc(hidden)]
pub struct __Capacity<const N: usize>;

impl<const N: usize> __Capacity<N> {
    #[doc(hidden)]
    pub const fn get(&self) -> usize {
        N
    }
}
#[doc(hidden)]
#[repr(C)]
pub union __Transmuter<T, const N: usize> {
    pub init_uninit_array: ManuallyDrop<MaybeUninit<[T; N]>>,
    pub uninit_array: ManuallyDrop<[MaybeUninit<T>; N]>,
    pub out: ManuallyDrop<[T; N]>,
}

#[doc(hidden)]
#[repr(C)]
pub union __ArrayVecTransmuter<T, const N: usize> {
    pub vec: ManuallyDrop<__ArrayVec<T, N>>,
    pub inner: ManuallyDrop<__ArrayVecInner<T, N>>,
}

#[doc(hidden)]
#[macro_export]
macro_rules! __array {
    [$i:pat => $e:expr; $count:expr] => {{
        let mut vec = $crate::__ArrayVec::<_, {$count}>(unsafe { $crate::__ArrayVecInner::new(
            // An uninitialized `[MaybeUninit<_>; LEN]` is valid.
            $crate::__core::mem::ManuallyDrop::into_inner(unsafe {
                $crate::__Transmuter {
                    init_uninit_array: $crate::__core::mem::ManuallyDrop::new($crate::__core::mem::MaybeUninit::uninit()),
                }
                .uninit_array
            }),
        )});
        while vec.0.len < (&vec.0.capacity).get() {
            let $i = vec.0.len;
            let _please_do_not_use_continue_without_label;
            let value;
            struct __PleaseDoNotUseBreakWithoutLabel;
            loop {
                _please_do_not_use_continue_without_label = ();
                value = $e;
                break __PleaseDoNotUseBreakWithoutLabel;
            };
            // This writes an initialized element.
            vec.0.arr[vec.0.len] = $crate::__core::mem::MaybeUninit::new(value);
            // We just wrote a valid element, so we can add 1 to len, it's valid.
            vec.0.len += 1;
        }
        // When leaving this loop, vec.0.len must equal to $count due
        // to loop condition. It cannot be more as len is increased by 1
        // every time loop is iterated on, and $count never changes.

        // __ArrayVec is representation compatible with __ArrayVecInner
        // due to #[repr(transparent)] in __ArrayVec.
        let inner = $crate::__core::mem::ManuallyDrop::into_inner(unsafe {
            $crate::__ArrayVecTransmuter {
                vec: $crate::__core::mem::ManuallyDrop::new(vec),
            }
            .inner
        });
        // At this point the array is fully initialized, as vec.0.len == $count,
        // so converting an array of potentially uninitialized elements into fully
        // initialized array is safe.
        $crate::__core::mem::ManuallyDrop::into_inner(unsafe {
            $crate::__Transmuter {
                uninit_array: $crate::__core::mem::ManuallyDrop::new(inner.arr),
            }
            .out
        })
    }}
}
