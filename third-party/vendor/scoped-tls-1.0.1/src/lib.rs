// Copyright 2014-2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Scoped thread-local storage
//!
//! This module provides the ability to generate *scoped* thread-local
//! variables. In this sense, scoped indicates that thread local storage
//! actually stores a reference to a value, and this reference is only placed
//! in storage for a scoped amount of time.
//!
//! There are no restrictions on what types can be placed into a scoped
//! variable, but all scoped variables are initialized to the equivalent of
//! null. Scoped thread local storage is useful when a value is present for a known
//! period of time and it is not required to relinquish ownership of the
//! contents.
//!
//! # Examples
//!
//! ```
//! #[macro_use]
//! extern crate scoped_tls;
//!
//! scoped_thread_local!(static FOO: u32);
//!
//! # fn main() {
//! // Initially each scoped slot is empty.
//! assert!(!FOO.is_set());
//!
//! // When inserting a value, the value is only in place for the duration
//! // of the closure specified.
//! FOO.set(&1, || {
//!     FOO.with(|slot| {
//!         assert_eq!(*slot, 1);
//!     });
//! });
//! # }
//! ```

#![deny(missing_docs, warnings)]
#![no_std]
#![feature(allow_internal_unstable)]
#![feature(thread_local)]

#[cfg(test)]
extern crate std;

use core::{cell::Cell, marker};

#[doc(hidden)]
pub use core::cell::Cell as CoreCell;
#[doc(hidden)]
pub use core::mem::needs_drop;
#[doc(hidden)]
pub use core::ptr::null as ptr_null;

/// The macro. See the module level documentation for the description and examples.
#[macro_export]
#[allow_internal_unstable(thread_local)]
macro_rules! scoped_thread_local {
    ($(#[$attrs:meta])* $vis:vis static $name:ident: $ty:ty) => (
        $(#[$attrs])*
        $vis static $name: $crate::ScopedKey<$ty> = {
            unsafe fn __getit() -> &'static $crate::CoreCell<*const ()> {
                static _ASSERT_DOESNT_NEED_DROP: [(); 0] = [
                    ();
                    0 - $crate::needs_drop::<$crate::CoreCell<*const ()>>() as usize
                ];
                // This is safe because `Cell<*const ()>` doesn't need destructors asserted above,
                // and thus we can get rid of some `registor_dtor` which is implemented in libc.
                #[thread_local]
                static mut FOO: $crate::CoreCell<*const ()> =
                    $crate::CoreCell::new($crate::ptr_null());
                &FOO
            }

            // Safety: nothing else can access FOO since it's hidden in its own scope
            unsafe { $crate::ScopedKey::new(__getit) }
        };
    )
}

/// Type representing a thread local storage key corresponding to a reference
/// to the type parameter `T`.
///
/// Keys are statically allocated and can contain a reference to an instance of
/// type `T` scoped to a particular lifetime. Keys provides two methods, `set`
/// and `with`, both of which currently use closures to control the scope of
/// their contents.
pub struct ScopedKey<T> {
    inner: unsafe fn() -> &'static Cell<*const ()>,
    _marker: marker::PhantomData<T>,
}

unsafe impl<T> Sync for ScopedKey<T> {}

impl<T> ScopedKey<T> {
    #[doc(hidden)]
    /// # Safety
    /// `inner` must only be accessed through `ScopedKey`'s API
    pub const unsafe fn new(inner: unsafe fn() -> &'static Cell<*const ()>) -> Self {
        Self {
            inner,
            _marker: marker::PhantomData,
        }
    }

    fn with_inner<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&Cell<*const ()>) -> R,
    {
        unsafe { f((self.inner)()) }
    }

    /// Inserts a value into this scoped thread local storage slot for a
    /// duration of a closure.
    ///
    /// While `f` is running, the value `t` will be returned by `get` unless
    /// this function is called recursively inside of `f`.
    ///
    /// Upon return, this function will restore the previous value, if any
    /// was available.
    ///
    /// # Examples
    ///
    /// ```
    /// #[macro_use]
    /// extern crate scoped_tls;
    ///
    /// scoped_thread_local!(static FOO: u32);
    ///
    /// # fn main() {
    /// FOO.set(&100, || {
    ///     let val = FOO.with(|v| *v);
    ///     assert_eq!(val, 100);
    ///
    ///     // set can be called recursively
    ///     FOO.set(&101, || {
    ///         // ...
    ///     });
    ///
    ///     // Recursive calls restore the previous value.
    ///     let val = FOO.with(|v| *v);
    ///     assert_eq!(val, 100);
    /// });
    /// # }
    /// ```
    pub fn set<F, R>(&'static self, t: &T, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        struct Reset {
            key: unsafe fn() -> &'static Cell<*const ()>,
            val: *const (),
        }
        impl Drop for Reset {
            fn drop(&mut self) {
                unsafe { (self.key)() }.set(self.val);
            }
        }
        let prev = self.with_inner(|c| {
            let prev = c.get();
            c.set(t as *const T as *const ());
            prev
        });
        let _reset = Reset {
            key: self.inner,
            val: prev,
        };
        f()
    }

    /// Gets a value out of this scoped variable.
    ///
    /// This function takes a closure which receives the value of this
    /// variable.
    ///
    /// # Panics
    ///
    /// This function will panic if `set` has not previously been called.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// #[macro_use]
    /// extern crate scoped_tls;
    ///
    /// scoped_thread_local!(static FOO: u32);
    ///
    /// # fn main() {
    /// FOO.with(|slot| {
    ///     // work with `slot`
    /// # drop(slot);
    /// });
    /// # }
    /// ```
    pub fn with<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let val = self.with_inner(|c| c.get());
        assert!(
            !val.is_null(),
            "cannot access a scoped thread local variable without calling `set` first"
        );
        unsafe { f(&*(val as *const T)) }
    }

    /// Gets a value out of this scoped variable, if any.
    ///
    /// This function takes a closure which receives the value of this
    /// variable.
    ///
    /// # Errors
    ///
    /// This function will return `None` if `set` has not previously been called.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// #[macro_use]
    /// extern crate scoped_tls;
    ///
    /// scoped_thread_local!(static FOO: u32);
    ///
    /// # fn main() {
    /// FOO.try_with(|slot| {
    ///     // work with `slot`
    /// # drop(slot);
    /// });
    /// # }
    /// ```
    pub fn try_with<F, R>(&'static self, f: F) -> Option<R>
    where
        F: FnOnce(&T) -> R,
    {
        let val = self.with_inner(|c| c.get());
        if val.is_null() {
            return None;
        }
        Some(unsafe { f(&*(val as *const T)) })
    }

    /// Test whether this TLS key has been `set` for the current thread.
    pub fn is_set(&'static self) -> bool {
        self.with_inner(|c| !c.get().is_null())
    }

    /// Return the raw pointer stored in the variable.
    pub fn as_ptr(&'static self) -> *const T {
        self.with_inner(|c| c.get()).cast()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        sync::mpsc::{channel, Sender},
        thread,
    };

    scoped_thread_local!(static FOO: u32);

    #[test]
    fn smoke() {
        scoped_thread_local!(static BAR: u32);

        assert!(!BAR.is_set());
        BAR.set(&1, || {
            assert!(BAR.is_set());
            BAR.with(|slot| {
                assert_eq!(*slot, 1);
            });
        });
        assert!(!BAR.is_set());
    }

    #[test]
    fn cell_allowed() {
        scoped_thread_local!(static BAR: Cell<u32>);

        BAR.set(&Cell::new(1), || {
            BAR.with(|slot| {
                assert_eq!(slot.get(), 1);
            });
        });
    }

    #[test]
    fn scope_item_allowed() {
        assert!(!FOO.is_set());
        FOO.set(&1, || {
            assert!(FOO.is_set());
            FOO.with(|slot| {
                assert_eq!(*slot, 1);
            });
        });
        assert!(!FOO.is_set());
    }

    #[test]
    fn panic_resets() {
        struct Check(Sender<u32>);
        impl Drop for Check {
            fn drop(&mut self) {
                FOO.with(|r| {
                    self.0.send(*r).unwrap();
                })
            }
        }

        let (tx, rx) = channel();
        let t = thread::spawn(|| {
            FOO.set(&1, || {
                let _r = Check(tx);

                FOO.set(&2, || panic!());
            });
        });

        assert_eq!(rx.recv().unwrap(), 1);
        assert!(t.join().is_err());
    }

    #[test]
    fn attrs_allowed() {
        scoped_thread_local!(
            /// Docs
            static BAZ: u32
        );

        scoped_thread_local!(
            #[allow(non_upper_case_globals)]
            static quux: u32
        );

        let _ = BAZ;
        let _ = quux;
    }
}
