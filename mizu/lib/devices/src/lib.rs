#![no_std]
#![feature(pointer_byte_offsets)]

extern crate alloc;

pub mod dev;
mod intr;

pub use self::intr::*;
