#![cfg_attr(not(test), no_std)]
#![cfg_attr(test, feature(once_cell))]
extern crate alloc;

mod handler;

pub use ksc_core::*;

pub use self::handler::*;
