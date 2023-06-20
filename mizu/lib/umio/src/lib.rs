#![cfg_attr(not(test), no_std)]
#![feature(async_fn_in_trait)]
#![allow(incomplete_features)]

mod event;
mod io;

use alloc::sync::Arc;
use core::{any::Any, mem, slice};

use arsc_rs::Arsc;

pub use self::{event::*, io::*};

extern crate alloc;

#[derive(Copy, PartialEq, Eq, Clone, Debug)]
pub enum SeekFrom {
    /// Sets the offset to the provided number of bytes.
    Start(usize),

    /// Sets the offset to the size of this object plus the specified number of
    /// bytes.
    ///
    /// It is possible to seek beyond the end of an object, but it's an error to
    /// seek before byte 0.
    End(isize),

    /// Sets the offset to the current position plus the specified number of
    /// bytes.
    ///
    /// It is possible to seek beyond the end of an object, but it's an error to
    /// seek before byte 0.
    Current(isize),
}

pub type IoSlice<'a> = &'a [u8];

pub type IoSliceMut<'a> = &'a mut [u8];

#[allow(clippy::len_without_is_empty)]
pub trait IoSliceExt {
    fn len(&self) -> usize;

    fn advance(&mut self, n: usize);
}

impl IoSliceExt for IoSlice<'_> {
    fn len(&self) -> usize {
        (**self).len()
    }

    fn advance(&mut self, n: usize) {
        if self.len() < n {
            panic!("advancing IoSlice beyond its length");
        }

        *self = &self[n..];
    }
}

impl IoSliceExt for IoSliceMut<'_> {
    fn len(&self) -> usize {
        (**self).len()
    }

    fn advance(&mut self, n: usize) {
        if self.len() < n {
            panic!("advancing IoSlice beyond its length");
        }

        *self = unsafe { slice::from_raw_parts_mut(self.as_mut_ptr().add(n), self.len() - n) };
    }
}

pub fn ioslice_len(bufs: &&mut [impl IoSliceExt]) -> usize {
    bufs.iter().fold(0, |sum, buf| sum + buf.len())
}

pub fn ioslice_is_empty(bufs: &&mut [impl IoSliceExt]) -> bool {
    bufs.iter().all(|b| b.len() == 0)
}

#[track_caller]
pub fn advance_slices(bufs: &mut &mut [impl IoSliceExt], n: usize) {
    // Number of buffers to remove.
    let mut remove = 0;
    // Total length of all the to be removed buffers.
    let mut accumulated_len = 0;
    for buf in bufs.iter() {
        if accumulated_len + buf.len() > n {
            break;
        } else {
            accumulated_len += buf.len();
            remove += 1;
        }
    }

    *bufs = &mut mem::take(bufs)[remove..];
    if bufs.is_empty() {
        assert!(
            n == accumulated_len,
            "advancing io slices beyond their length, {n} == {accumulated_len}"
        );
    } else {
        bufs[0].advance(n - accumulated_len)
    }
}

pub trait IntoAny: Any + Send + Sync {
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;

    fn into_any_arsc(self: Arsc<Self>) -> Arsc<dyn Any + Send + Sync>;
}

impl<T: Any + Send + Sync> IntoAny for T {
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self as _
    }

    fn into_any_arsc(self: Arsc<Self>) -> Arsc<dyn Any + Send + Sync> {
        self as _
    }
}

pub trait IntoAnyExt: IntoAny {
    fn downcast<T: Any + Send + Sync>(self: Arc<Self>) -> Option<Arc<T>> {
        self.into_any().downcast().ok()
    }

    fn downcast_arsc<T: Any + Send + Sync>(self: Arsc<Self>) -> Option<Arsc<T>> {
        self.into_any_arsc().downcast().ok()
    }
}

impl<T: IntoAny + ?Sized> IntoAnyExt for T {}
