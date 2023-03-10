#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![feature(naked_functions)]
#![feature(thread_local)]

mod rxx;

use sbi_rt::{NoReason, Shutdown};

#[thread_local]
static mut X: i32 = 123;

fn main(_hartid: usize) -> ! {
    unsafe { assert_eq!(X, 123) };

    sbi_rt::system_reset(Shutdown, NoReason);
    loop {
        core::hint::spin_loop()
    }
}
