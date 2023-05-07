#![cfg_attr(not(test), no_std)]
#![feature(once_cell)]
#![feature(thread_local)]

extern crate alloc;

pub mod epoch;
mod mpmc;
mod mutex;
mod rcu;
mod rw_lock;
mod semaphore;

pub use event_listener as event;
pub use ksync_core::*;

pub use self::{mpmc::*, mutex::*, rcu::*, rw_lock::*, semaphore::*};
