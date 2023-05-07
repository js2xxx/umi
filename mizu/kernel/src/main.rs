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
pub mod fs;
pub mod mem;
mod rxx;
pub mod syscall;
pub mod task;
mod trap;

#[macro_use]
extern crate klog;

extern crate alloc;

use umifs::types::FileType;

pub use self::rxx::executor;

async fn main(fdt: usize) {
    println!("Hello from executor");

    unsafe { dev::init(fdt as _).expect("failed to initialize devices") };
    fs::fs_init().await;

    let (fs, path) = fs::get("chdir".as_ref()).unwrap();
    let rt = fs.root_dir().await.unwrap();
    rt.open(
        path,
        Some(FileType::FILE),
        Default::default(),
        Default::default(),
    )
    .await
    .unwrap();
}
