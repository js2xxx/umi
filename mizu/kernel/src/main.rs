#![cfg_attr(not(feature = "test"), no_std)]
#![cfg_attr(not(feature = "test"), no_main)]
#![feature(asm_const)]
#![feature(const_mut_refs)]
#![feature(const_trait_impl)]
#![feature(inline_const)]
#![feature(naked_functions)]
#![feature(result_option_inspect)]
#![feature(thread_local)]

mod dev;
mod rxx;
mod trap;

#[macro_use]
extern crate klog;

extern crate alloc;

pub use self::rxx::executor;

async fn main(fdt: usize) {
    println!("Hello from executor");

    unsafe { dev::init(fdt as _).expect("failed to initialize devices") };
}
