#![cfg_attr(not(test), no_std)]

mod timer;

pub use ktime_core::*;

pub use self::timer::{notify_timer, Timer};
