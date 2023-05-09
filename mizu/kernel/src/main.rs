#![cfg_attr(not(feature = "test"), no_std)]
#![cfg_attr(not(feature = "test"), no_main)]
#![feature(asm_const)]
#![feature(const_mut_refs)]
#![feature(const_trait_impl)]
#![feature(inline_const)]
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

use alloc::sync::Arc;
use core::pin::pin;

use afat32::{FatDir, NullTimeProvider};
use futures_util::StreamExt;
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

    let skips = ["mmap"];

    let mut iter = pin!(rt.iter(true));
    while let Some(entry) = iter.next().await {
        let (case, file) = match entry {
            Ok(e) if e.is_file() && e.file_name().find('.').is_none() => (
                e.file_name(),
                match e.to_file().await {
                    Ok(file) => file,
                    _ => continue,
                },
            ),
            _ => continue,
        };
        log::info!("Found test case {case:?}");
        if skips.contains(&&*case) {
            log::info!("Skipping");
            continue;
        }

        let task = InitTask::from_elf(Phys::new(Arc::new(file), 0, true), Default::default())
            .await
            .unwrap();
        let task = task.spawn().unwrap();
        let code = task.wait().await;
        log::info!("test case {case:?} returned with {code}\n");
    }

    log::info!("Goodbye!");
}
