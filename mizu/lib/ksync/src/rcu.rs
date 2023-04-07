use alloc::boxed::Box;
use core::{
    fmt,
    mem::{self, MaybeUninit},
    ops::Deref,
    sync::atomic::Ordering::*,
};

use crossbeam_epoch::{Atomic, Guard, Owned, Pointable, Shared};

use crate::epoch::pin;

/// A slot that either stores a value or nothing, just like an atomic
/// `Option<T>`.
///
/// The reason why `T` is bounded with `Send` even if the slot is not `Send` is
/// that the destruction of this slot will transfer the possible data to the
/// global collector, which might result in the destructor of `T` being run in
/// another thread.
pub struct RcuSlot<T: Pointable + Send> {
    inner: Atomic<T>,
}

impl<T: Pointable + Send> fmt::Debug for RcuSlot<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RcuSlot").finish_non_exhaustive()
    }
}

unsafe impl<T: Pointable + Send + Sync> Send for RcuSlot<T> {}
unsafe impl<T: Pointable + Send + Sync> Sync for RcuSlot<T> {}

impl<T: Pointable + Send> RcuSlot<T> {
    pub const fn none() -> Self {
        RcuSlot {
            inner: Atomic::null(),
        }
    }

    pub fn new(data: T) -> Self {
        RcuSlot {
            inner: Atomic::new(data),
        }
    }

    pub fn read<'a>(&self, guard: &'a Guard) -> Option<RcuReadGuard<'a, T>> {
        let shared = self.inner.load_consume(guard);
        (!shared.is_null()).then_some(RcuReadGuard { inner: shared })
    }

    pub fn write(&self, data: T, guard: &Guard) {
        self.replace(data, guard);
    }

    pub fn replace<'a>(&self, data: T, guard: &'a Guard) -> Option<RcuDropGuard<'a, T>> {
        let old = self
            .inner
            .swap(Owned::new(data).into_shared(guard), AcqRel, guard);
        (!old.is_null()).then_some(RcuDropGuard { inner: old, guard })
    }

    pub fn take<'a>(&self, guard: &'a Guard) -> Option<RcuDropGuard<'a, T>> {
        let old = self.inner.swap(Shared::null(), AcqRel, guard);
        (!old.is_null()).then_some(RcuDropGuard { inner: old, guard })
    }

    /// # Arguments
    ///
    /// - `update` - A closure that takes the slot's current reference and a
    ///   possible data constructed on last fail trial, and returns a possible
    ///   new data.
    pub fn update<'a, F>(&self, guard: &'a Guard, mut update: F) -> Option<RcuDropGuard<'a, T>>
    where
        F: FnMut(&'a T, Option<T>) -> Option<T>,
    {
        // A temp slot of memory to avoid repeating allocation.
        let mut temp: Option<Box<MaybeUninit<T>>> = None;
        let mut current = self.inner.load_consume(guard);
        loop {
            // Hand out the ownership of the data in `temp` if any.
            let new = unsafe {
                update(
                    current.deref(),
                    temp.as_ref().map(|ptr| ptr.assume_init_read()),
                )
            }?;
            // After that: `temp <- None | Some(Box::new(MaybeUninit::uninit()))`

            // Store the ownership of the new data in `temp`.
            let new = temp
                .get_or_insert_with(|| Box::new(MaybeUninit::uninit()))
                .write(new);
            // After that: `temp <- Some(Box::new(MaybeUninit::new(new)))`

            match self.inner.compare_exchange_weak(
                current,
                Shared::from(new as *const _),
                AcqRel,
                Acquire,
                guard,
            ) {
                Ok(old) => {
                    // Transferred the ownership of the data with its heap memory to the slot, so
                    // forget `temp`.
                    mem::forget(temp);
                    break (!old.is_null()).then_some(RcuDropGuard { inner: old, guard });
                }
                Err(err) => current = err.current,
            }
            // If errored, `temp` remains unchanged.
            // After that: `temp <- Some(Box::new(MaybeUninit::new(new)))`
        }
    }
}

impl<T: Pointable + Send> Default for RcuSlot<T> {
    fn default() -> Self {
        Self::none()
    }
}

impl<T: Pointable + Send> Drop for RcuSlot<T> {
    fn drop(&mut self) {
        self.take(&pin());
    }
}

pub struct RcuReadGuard<'a, T: Pointable> {
    inner: Shared<'a, T>,
}

impl<T: Pointable> RcuReadGuard<'_, T> {
    pub fn as_ptr(&self) -> *const T {
        self.inner.as_raw()
    }
}

impl<T: Pointable + fmt::Debug> fmt::Debug for RcuReadGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RcuReadGuard")
            .field("data", &**self)
            .finish()
    }
}

impl<T: Pointable> Deref for RcuReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.inner.deref() }
    }
}

pub struct RcuDropGuard<'a, T: Pointable + Send> {
    inner: Shared<'a, T>,
    guard: &'a Guard,
}

impl<T: Pointable + Send> RcuDropGuard<'_, T> {
    pub fn as_ptr(&self) -> *const T {
        self.inner.as_raw()
    }
}

impl<T: Pointable + Send> Deref for RcuDropGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.inner.deref() }
    }
}

impl<T: Pointable + Send + fmt::Debug> fmt::Debug for RcuDropGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RcuDropGuard")
            .field("data", &**self)
            .finish()
    }
}

impl<T: Pointable + Send> Drop for RcuDropGuard<'_, T> {
    fn drop(&mut self) {
        // SAFETY: The `Atomic` it pointed to has no more access to this pointer, thus
        // no new reference will be created.
        unsafe { self.guard.defer_destroy(self.inner) }
    }
}
