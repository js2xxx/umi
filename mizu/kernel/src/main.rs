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

mod dev;
pub mod fs;
pub mod mem;
mod rxx;
mod syscall;
pub mod task;
mod trap;

#[macro_use]
extern crate klog;

extern crate alloc;

use alloc::sync::{Arc, Weak};

use afat32::{FatDir, NullTimeProvider};
use kmem::Phys;
use umifs::traits::IntoAnyExt;

pub use self::rxx::executor;
use crate::task::InitTask;

async fn main(fdt: usize) {
    println!("Hello from UMI ^_^");

    unsafe { dev::init(fdt as _).expect("failed to initialize devices") };
    fs::fs_init().await;

    let (fs, _) = fs::get("".as_ref()).unwrap();
    let rt = fs.root_dir().await.unwrap();

    let rt = rt.downcast::<FatDir<NullTimeProvider>>().unwrap();

    let spec = [
        "brk",
        "read",
        "write",
        "open",
        "openat",
        "dup",
        "dup2",
        "getdents",
        "mkdir_",
        "unlink",
        "getcwd",
        "chdir",
        "close",
        "fstat",
        "getpid",
        "getppid",
        "gettimeofday",
        "sleep",
        "times",
        "mmap",
        "munmap",
        "clone",
        "exit",
        "fork",
        "wait",
        "waitpid",
        "yield",
        "pipe",
        "uname",
    ];

    sbi_rt::set_timer(0);

    for case in spec {
        let file = rt.open_file(case.as_ref()).await.unwrap();
        log::info!("Found test case {case:?}");

        let task = InitTask::from_elf(
            Weak::new(),
            Phys::new(Arc::new(file), 0, true),
            Default::default(),
        )
        .await
        .unwrap();
        let task = task.spawn().unwrap();
        let code = task.wait().await;
        log::info!("test case {case:?} returned with {code}\n");
    }

    log::info!("Goodbye!");
}
