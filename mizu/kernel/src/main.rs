#![cfg_attr(not(feature = "test"), no_std)]
#![cfg_attr(not(feature = "test"), no_main)]
#![feature(asm_const)]
#![feature(const_mut_refs)]
#![feature(const_trait_impl)]
#![feature(inline_const)]
#![feature(naked_functions)]
#![feature(result_option_inspect)]
#![feature(thread_local)]

pub mod dev;
pub mod mem;
mod rxx;
pub mod syscall;
pub mod task;
mod trap;

#[macro_use]
extern crate klog;

extern crate alloc;

use alloc::sync::Arc;
use core::pin::pin;

use futures_util::StreamExt;
use kmem::Phys;

pub use self::rxx::executor;

async fn main(fdt: usize) {
    println!("Hello from executor");

    unsafe { dev::init(fdt as _).expect("failed to initialize devices") };

    let block = dev::block(0).unwrap();
    let block_shift = block.block_shift();
    let phys = Phys::new(block.to_backend(), 0);

    let fs = afat32::FatFileSystem::new(Arc::new(phys), block_shift, afat32::NullTimeProvider)
        .await
        .unwrap();

    log::debug!("{:?}", fs.stats().await.unwrap());

    let root = fs.root_dir().await.unwrap();
    let mut iter = pin!(root.iter(true));
    while let Some(entry) = iter.next().await {
        let entry = entry.unwrap();
        log::debug!("{}", entry.file_name());
    }
}
