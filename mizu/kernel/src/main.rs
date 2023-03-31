#![cfg_attr(not(feature = "test"), no_std)]
#![cfg_attr(not(feature = "test"), no_main)]
#![feature(asm_const)]
#![feature(const_mut_refs)]
#![feature(const_trait_impl)]
#![feature(inline_const)]
#![feature(naked_functions)]
#![feature(thread_local)]

mod rxx;
mod trap;

#[macro_use]
extern crate klog;

extern crate alloc;

use alloc::boxed::Box;
use core::hint;

use arsc_rs::Arsc;
use art::Executor;
use sbi_rt::{NoReason, Shutdown};

fn main(payload: usize) -> ! {
    run_art(payload);

    if rxx::is_bsp() {
        sbi_rt::system_reset(Shutdown, NoReason);
    }
    loop {
        core::hint::spin_loop()
    }
}

fn run_art(payload: usize) {
    type Payload = *mut Box<dyn FnOnce() + Send>;
    if rxx::is_bsp() {
        log::debug!("Starting ART");
        let mut runners = Executor::start(config::MAX_HARTS, init);
        let me = runners.next().unwrap();
        for (id, runner) in config::HART_RANGE
            .filter(|&id| id != rxx::bsp_id())
            .zip(runners)
        {
            log::debug!("Starting #{id}");

            let payload: Payload = Box::into_raw(Box::new(Box::new(runner)));

            let ret = sbi_rt::hart_start(id, config::KERNEL_START_PHYS, payload as usize);

            if let Some(err) = ret.err() {
                log::error!("failed to start hart {id} due to error {err:?}");
            }
        }
        me();
    } else {
        log::debug!("Running ART from #{}", rxx::hart_id());

        let runner = payload as Payload;
        // SAFETY: The payload must come from the BSP.
        unsafe { Box::from_raw(runner)() };
    }
}

async fn init(executor: Arsc<Executor>) {
    println!("Hello from executor");
    let i = ktime::Instant::now();
    while i.elapsed().as_secs() < 1 {
        hint::spin_loop()
    }
    executor.shutdown()
}
