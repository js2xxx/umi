#![no_std]
#![feature(pointer_byte_offsets)]
#![feature(slice_ptr_get)]

extern crate alloc;

pub mod dev;
mod intr;

pub use self::intr::*;
