#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod mpmc;
mod mutex;
mod semaphore;

pub use event_listener as event;
pub use ksync_core::*;

pub use self::{mpmc::*, mutex::*, semaphore::*};
