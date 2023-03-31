#![cfg_attr(not(feature = "test"), no_std)]
#![cfg_attr(not(feature = "test"), no_main)]
#![feature(asm_const)]
#![feature(const_mut_refs)]
#![feature(const_trait_impl)]
#![feature(inline_const)]
#![feature(naked_functions)]
#![feature(thread_local)]

mod rxx;

#[macro_use]
extern crate klog;

extern crate alloc;

use sbi_rt::{NoReason, Shutdown};

#[thread_local]
static mut X: i32 = 123;

fn main(hartid: usize, payload: usize) -> ! {
    unsafe { assert_eq!(X, 123) };

    println!("Hello world!");
    let vec = alloc::vec![1, 2, 3, 4, 5];
    log::debug!("{vec:?}");

    sbi_rt::system_reset(Shutdown, NoReason);
    loop {
        core::hint::spin_loop()
    }
}
