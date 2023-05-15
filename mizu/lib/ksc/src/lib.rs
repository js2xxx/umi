#![cfg_attr(not(feature = "test"), no_std)]
#![cfg_attr(feature = "test", feature(once_cell))]

extern crate alloc;

mod handler;

pub use ksc_core::*;
pub use ksc_macros::*;

pub use self::handler::*;
