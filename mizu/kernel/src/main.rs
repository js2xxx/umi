#![cfg_attr(not(feature = "test"), no_std)]
#![cfg_attr(not(feature = "test"), no_main)]
#![feature(alloc_layout_extra)]
#![feature(array_methods)]
#![feature(asm_const)]
#![feature(const_mut_refs)]
#![feature(const_trait_impl)]
#![feature(inline_const)]
#![feature(link_llvm_intrinsics)]
#![feature(maybe_uninit_as_bytes)]
#![feature(naked_functions)]
#![feature(pointer_byte_offsets)]
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

async fn main(payload: usize) {
    println!("Hello from UMI ^_^");

    // Init devices.
    unsafe {
        let device_tree = config::device_tree(payload);
        crate::dev::init(device_tree).expect("failed to initialize devices")
    }
    // Init FS.
    fs::fs_init().await;

    mem::test_phys().await;
    fs::test_file().await;

    // self::test::test_all().await;

    // self::test::run("/bin/ssh-keygen -t rsa -f ssh_host_rsa_key -N \"\"").await;
    // self::test::run("busybox chmod 0600 ssh_host_rsa_key").await;
    // self::test::run("busybox chmod 0700 /var/empty").await;
    // self::test::run("/bin/sshd -ddd").await;

    self::test::busybox_interact().await;
    println!("Goodbye!");
    // loop {
    //     crate::task::yield_now().await;
    // }
}
