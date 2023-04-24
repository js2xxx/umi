#![no_std]
#![feature(result_option_inspect)]

extern crate alloc;

pub mod dev;
mod intr;

pub use self::intr::*;
