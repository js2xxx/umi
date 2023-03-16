#![cfg_attr(not(feature = "test"), no_std)]

extern crate alloc;

mod timer;

pub use ktime_core::*;

pub use self::timer::Timer;

pub fn timer_tick() {
    timer::TIMER_QUEUE.tick();
}
