#![no_std]
#![feature(slice_ptr_get)]

pub mod dev;
mod intr;

extern crate alloc;

pub use self::intr::IntrManager;
