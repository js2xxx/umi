//! Asynchorous RunTime.
#![cfg_attr(not(feature = "test"), no_std)]

extern crate alloc;

pub mod queue;
mod rand;
mod sched;

pub use self::sched::Executor;
