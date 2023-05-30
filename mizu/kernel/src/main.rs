#![cfg_attr(not(feature = "test"), no_std)]
#![cfg_attr(not(feature = "test"), no_main)]
#![feature(alloc_layout_extra)]
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

#[macro_use]
extern crate klog;

extern crate alloc;

pub use self::rxx::executor;

async fn main(fdt: usize) {
    println!("Hello from UMI ^_^");

    // Init devices.
    unsafe { crate::dev::init(fdt as _).expect("failed to initialize devices") };
    // Init FS.
    fs::fs_init().await;

    mem::test_phys().await;

    let (fs, _) = fs::get("".as_ref()).unwrap();
    let rt = fs.root_dir().await.unwrap();

    self::test::comp2(rt).await;
}
