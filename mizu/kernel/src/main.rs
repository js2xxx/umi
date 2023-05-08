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

use kmem::Phys;
use umifs::types::FileType;

pub use self::rxx::executor;
use crate::task::InitTask;

async fn main(fdt: usize) {
    println!("Hello from executor");

    unsafe { dev::init(fdt as _).expect("failed to initialize devices") };
    fs::fs_init().await;

    let (fs, path) = fs::get("write".as_ref()).unwrap();
    let rt = fs.root_dir().await.unwrap();
    let (entry, _) = rt
        .open(
            path,
            Some(FileType::FILE),
            Default::default(),
            Default::default(),
        )
        .await
        .unwrap();

    let io = entry.to_io().unwrap();
    let phys = Phys::new(io, 0, true);

    let init = InitTask::from_elf(phys, Default::default()).await.unwrap();

    sbi_rt::set_timer(0);
    let task = init.spawn().unwrap();
    let event = task.event();
    loop {
        if let Ok(task::TaskEvent::Exited(code)) = event.recv().await {
            log::info!("returned with {code}");
            break;
        }
    }

    log::info!("Goodbye!");
}
