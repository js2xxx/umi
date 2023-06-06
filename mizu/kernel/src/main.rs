#![cfg_attr(not(feature = "test"), no_std)]
#![cfg_attr(not(feature = "test"), no_main)]
#![feature(alloc_layout_extra)]
#![feature(array_methods)]
#![feature(asm_const)]
#![feature(const_mut_refs)]
#![feature(const_trait_impl)]
#![feature(inline_const)]
#![feature(maybe_uninit_as_bytes)]
#![feature(naked_functions)]
#![feature(pointer_is_aligned)]
#![feature(result_option_inspect)]
#![feature(thread_local)]

mod cpu;
mod dev;
pub mod fs;
mod mem;
mod rxx;
mod syscall;
pub mod task;
mod trap;

mod test;

extern crate alloc;

pub use self::rxx::executor;

async fn main(fdt: usize) {
    // Init devices.
    unsafe { crate::dev::init(fdt as _).expect("failed to initialize devices") };
    // Init FS.
    fs::fs_init().await;

    println!("Hello from UMI ^_^");

    mem::test_phys().await;
    fs::test_file().await;

    self::test::busybox_debug(true).await;
}
