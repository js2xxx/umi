#![cfg_attr(not(feature = "test"), no_std)]

extern crate alloc;

mod timer;

use core::time::Duration;

pub use ktime_core::*;

pub use self::timer::Timer;

pub fn timer_tick() {
    timer::TIMER_QUEUE.tick();
}

pub async fn sleep(duration: Duration) {
    Timer::after(duration).await;
}
