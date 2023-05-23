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

#[macro_use]
extern crate klog;

extern crate alloc;

use alloc::{
    string::ToString,
    sync::{Arc, Weak},
    vec,
};
use core::pin::pin;

use futures_util::{stream, StreamExt};
use umifs::types::OpenOptions;

pub use self::rxx::executor;
use crate::task::InitTask;

async fn main(fdt: usize) {
    println!("Hello from UMI ^_^");

    // Init devices.
    unsafe { crate::dev::init(fdt as _).expect("failed to initialize devices") };
    // Init FS.
    fs::fs_init().await;

    mem::test_phys().await;

    let (fs, _) = fs::get("".as_ref()).unwrap();
    let rt = fs.root_dir().await.unwrap();

    let oo = OpenOptions::RDONLY;
    let perm = Default::default();

    let scripts = ["run-static.sh", "run-dynamic.sh"];

    let rt2 = rt.clone();
    let stream = stream::iter(scripts)
        .then(|sh| rt2.clone().open(sh.as_ref(), oo, perm))
        .flat_map(|res| {
            let (sh, _) = res.unwrap();
            let io = sh.to_io().unwrap();
            umio::lines(io).map(|res| res.unwrap())
        });
    let mut cmd = pin!(stream);

    let (runner, _) = rt.open("runtest".as_ref(), oo, perm).await.unwrap();
    let runner = Arc::new(mem::new_phys(runner.to_io().unwrap(), true));

    log::warn!("Start testing");
    while let Some(cmd) = cmd.next().await {
        log::info!("Executing cmd {cmd:?}");

        let init = InitTask::from_elf(
            Weak::new(),
            &runner,
            crate::mem::new_virt(),
            cmd.split(' ').map(|s| s.to_string()).collect(),
            vec![],
        )
        .await
        .unwrap();
        let task = init.spawn().unwrap();
        let code = task.wait().await;
        log::info!("cmd {cmd:?} returned with {code:?}\n");
    }

    log::warn!("Goodbye!");
}
