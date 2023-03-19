#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod mpmc;
mod mutex;

pub use ksync_core::*;

pub use self::{mpmc::*, mutex::*};
