#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod mpmc;
mod mutex;

pub use event_listener as event;
pub use ksync_core::*;

pub use self::{mpmc::*, mutex::*};
